use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use tokio::sync::broadcast;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub kind: String,
    pub payload: Value,
}

/// A bounded replay buffer plus a live broadcast channel. Events are normalized
/// through serde before storage so components always receive valid JSON blocks.
#[derive(Clone, Debug)]
pub struct EventBus {
    capacity: usize,
    recent: std::sync::Arc<std::sync::Mutex<VecDeque<String>>>,
    live: broadcast::Sender<String>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (live, _) = broadcast::channel(capacity.max(1));
        Self {
            capacity,
            recent: Default::default(),
            live,
        }
    }

    pub fn publish(&self, event: &Event) -> Result<(), serde_json::Error> {
        let json = serde_json::to_string(event)?;
        let mut recent = self.recent.lock().expect("event buffer lock poisoned");
        if self.capacity > 0 {
            if recent.len() == self.capacity {
                recent.pop_front();
            }
            recent.push_back(json.clone());
        }
        drop(recent);
        let _ = self.live.send(json);
        Ok(())
    }

    pub fn recent(&self) -> Vec<String> {
        self.recent
            .lock()
            .expect("event buffer lock poisoned")
            .iter()
            .cloned()
            .collect()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.live.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_is_bounded_and_normalized() {
        let bus = EventBus::new(2);
        for value in 0..3 {
            bus.publish(&Event {
                kind: "turn".into(),
                payload: value.into(),
            })
            .unwrap();
        }
        let recent = bus.recent();
        assert_eq!(recent.len(), 2);
        assert_eq!(
            serde_json::from_str::<Value>(&recent[0]).unwrap()["payload"],
            1
        );
    }
}
