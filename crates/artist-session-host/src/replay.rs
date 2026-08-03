use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sequenced<T> {
    pub seq: u64,
    pub payload: T,
}

/// Bounded live-event replay. Persisted envelopes are separately reconstructed from the log.
#[derive(Clone, Debug)]
pub struct EventJournal<T> {
    next_seq: u64,
    capacity: usize,
    events: VecDeque<Sequenced<T>>,
}

impl<T: Clone> EventJournal<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            next_seq: 1,
            capacity,
            events: VecDeque::new(),
        }
    }
    pub fn push(&mut self, payload: T) -> Sequenced<T> {
        let event = Sequenced {
            seq: self.next_seq,
            payload,
        };
        self.next_seq += 1;
        if self.capacity > 0 {
            self.events.push_back(event.clone());
            while self.events.len() > self.capacity {
                self.events.pop_front();
            }
        }
        event
    }
    pub fn after(&self, seq: u64) -> Option<Vec<Sequenced<T>>> {
        let oldest = self.events.front().map_or(self.next_seq, |event| event.seq);
        if seq.saturating_add(1) < oldest {
            return None;
        }
        Some(
            self.events
                .iter()
                .filter(|event| event.seq > seq)
                .cloned()
                .collect(),
        )
    }
    pub fn latest_seq(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }
}
