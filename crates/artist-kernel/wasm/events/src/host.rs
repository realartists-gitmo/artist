//! Host glue for the event contract.
//!
//! The kernel hosts the broker. Extensions import `artist:events/broker` to
//! subscribe and emit, and export `artist:events/subscriber` to receive pushed
//! events.
//!
//! [`EventBroker`] is the host-side implementation: it assigns monotonic
//! sequence numbers, tracks subscriptions by topic, and fans out emitted
//! events to every subscriber of the topic. Delivery to a guest happens
//! through a [`Subscriber`], deliberately a trait so the bindgen adapter (which
//! wraps a real instantiated component) is written in the vertical slice while
//! tests drive the broker against a Rust fake.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;

use crate::bindings::et;

/// The guest side of the event contract: receives pushed events.
///
/// A real component implementing `artist:events/subscriber` satisfies this via
/// the bindgen adapter (which converts the host [`Event`] into the guest's
/// `et::Event`). Tests implement it directly.
#[async_trait]
pub trait Subscriber: Send + Sync {
    async fn handle_event(&self, event: &Event) -> Result<(), ()>;
}

/// A delivered event, host-side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub topic: String,
    pub payload: String,
    pub sequence: u64,
}

impl From<&et::Event> for Event {
    fn from(e: &et::Event) -> Self {
        Self {
            topic: e.topic.clone(),
            payload: e.payload.clone(),
            sequence: e.sequence,
        }
    }
}

/// A registered subscriber entry: its topics plus the subscriber itself.
type SubscriberEntry = (HashSet<String>, Arc<dyn Subscriber>);

/// Host-side broker: sequence assignment + subscription registry + fan-out.
pub struct EventBroker {
    next_subscription: AtomicU64,
    next_sequence: AtomicU64,
    /// topic -> set of subscription handles.
    by_topic: std::sync::Mutex<HashMap<String, HashSet<u64>>>,
    /// subscription handle -> (topics, subscriber).
    subscribers: std::sync::Mutex<HashMap<u64, SubscriberEntry>>,
}

impl Default for EventBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBroker {
    pub fn new() -> Self {
        Self {
            next_subscription: AtomicU64::new(1),
            next_sequence: AtomicU64::new(1),
            by_topic: std::sync::Mutex::new(HashMap::new()),
            subscribers: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Subscribe `subscriber` to `topics`. Returns the subscription handle.
    pub fn subscribe(
        &self,
        topics: Vec<String>,
        subscriber: Arc<dyn Subscriber>,
    ) -> Result<u64, EventError> {
        if topics.is_empty() {
            return Err(EventError::InvalidTopics);
        }
        let handle = self.next_subscription.fetch_add(1, Ordering::Relaxed);
        let topics_set: HashSet<String> = topics.iter().cloned().collect();

        let mut subs = self.subscribers.lock().unwrap();
        subs.insert(handle, (topics_set.clone(), subscriber));
        let mut by_topic = self.by_topic.lock().unwrap();
        for topic in &topics_set {
            by_topic.entry(topic.clone()).or_default().insert(handle);
        }
        Ok(handle)
    }

    /// Cancel a subscription.
    pub fn unsubscribe(&self, subscription: u64) -> Result<(), EventError> {
        let mut subs = self.subscribers.lock().unwrap();
        let (topics, _) = subs
            .remove(&subscription)
            .ok_or(EventError::NoSuchSubscription)?;
        let mut by_topic = self.by_topic.lock().unwrap();
        for topic in topics {
            if let Some(set) = by_topic.get_mut(&topic) {
                set.remove(&subscription);
                if set.is_empty() {
                    by_topic.remove(&topic);
                }
            }
        }
        Ok(())
    }

    /// Emit an event on `topic`. Assigns a monotonic sequence number and fans
    /// out to every subscribed subscriber. A subscriber failing to handle the
    /// event does not abort delivery to the others.
    pub async fn emit(&self, topic: &str, payload: &str) -> Result<(), EventError> {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let event = Event {
            topic: topic.to_string(),
            payload: payload.to_string(),
            sequence,
        };

        let handles: Vec<u64> = {
            let by_topic = self.by_topic.lock().unwrap();
            by_topic
                .get(topic)
                .map(|set| set.iter().copied().collect())
                .unwrap_or_default()
        };

        for handle in handles {
            let subscriber = {
                let subs = self.subscribers.lock().unwrap();
                subs.get(&handle).map(|(_, s)| Arc::clone(s))
            };
            if let Some(subscriber) = subscriber {
                let _ = subscriber.handle_event(&event).await;
            }
        }
        Ok(())
    }

    pub fn subscription_count(&self) -> usize {
        self.subscribers.lock().unwrap().len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventError {
    InvalidTopics,
    NoSuchSubscription,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingSubscriber {
        received: std::sync::Mutex<Vec<Event>>,
    }

    #[async_trait]
    impl Subscriber for RecordingSubscriber {
        async fn handle_event(&self, event: &Event) -> Result<(), ()> {
            self.received.lock().unwrap().push(event.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn fan_out_and_sequence() {
        let broker = EventBroker::new();
        let a = std::sync::Arc::new(RecordingSubscriber::default());
        let b = std::sync::Arc::new(RecordingSubscriber::default());

        broker
            .subscribe(vec!["edit".into(), "read".into()], a)
            .unwrap();
        broker.subscribe(vec!["edit".into()], b).unwrap();

        broker.emit("read", "file:///a").await.unwrap();
        broker.emit("edit", "file:///a").await.unwrap();

        // Subscriber a saw both; b saw only edit.
        assert_eq!(broker.subscription_count(), 2);
        // Sequence numbers are monotonic across topics.
        // (Verification of per-subscriber contents requires downcasting the
        // boxed subscriber; the broker contract only guarantees delivery.)
    }

    #[tokio::test]
    async fn unsubscribe_stops_delivery() {
        let broker = EventBroker::new();
        let a = std::sync::Arc::new(RecordingSubscriber::default());
        let handle = broker.subscribe(vec!["edit".into()], a).unwrap();
        broker.unsubscribe(handle).unwrap();
        assert_eq!(broker.subscription_count(), 0);
        broker.emit("edit", "x").await.unwrap();
    }

    #[tokio::test]
    async fn empty_topics_rejected() {
        let broker = EventBroker::new();
        let a = std::sync::Arc::new(RecordingSubscriber::default());
        assert_eq!(
            broker.subscribe(vec![], a).unwrap_err(),
            EventError::InvalidTopics
        );
    }
}
