//! Provider-neutral model-context state.
//!
//! This module deliberately contains no provider message types. It owns the
//! stable contribution identity, deterministic ordering, and replayable
//! context events. Provider adapters project a snapshot elsewhere.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use crate::EventLog;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Contribution {
    pub id: String,
    pub source: Option<String>,
    pub slot: String,
    pub order: i64,
    pub revision: u64,
    pub content: String,
}

impl Contribution {
    pub fn new(id: impl Into<String>, slot: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            source: None,
            slot: slot.into(),
            order: 0,
            revision: 1,
            content: content.into(),
        }
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn with_order(mut self, order: i64) -> Self {
        self.order = order;
        self
    }

    pub fn with_revision(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Snapshot {
    pub contributions: Vec<Contribution>,
}

impl Snapshot {
    pub fn new(contributions: impl IntoIterator<Item = Contribution>) -> Self {
        let mut contributions: Vec<_> = contributions.into_iter().collect();
        sort_contributions(&mut contributions);
        Self { contributions }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContextEvent {
    Replace { contribution: Contribution },
    Remove { id: String },
    Append { contribution: Contribution },
}

impl ContextEvent {
    pub fn source(&self) -> Option<&str> {
        match self {
            Self::Replace { contribution } | Self::Append { contribution } => {
                contribution.source.as_deref()
            }
            Self::Remove { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContextState {
    contributions: BTreeMap<String, Contribution>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextError {
    EmptyContributionId,
    EmptySlot,
    DuplicateContribution(String),
    MissingContribution(String),
    Log(String),
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyContributionId => f.write_str("contribution id cannot be empty"),
            Self::EmptySlot => f.write_str("contribution slot cannot be empty"),
            Self::DuplicateContribution(id) => write!(f, "contribution already exists: {id}"),
            Self::MissingContribution(id) => write!(f, "contribution does not exist: {id}"),
            Self::Log(error) => write!(f, "could not persist context event: {error}"),
        }
    }
}

impl std::error::Error for ContextError {}

impl ContextState {
    pub fn from_snapshot(snapshot: Snapshot) -> Result<Self, ContextError> {
        let mut state = Self::default();
        for contribution in snapshot.contributions {
            state.insert_initial(contribution)?;
        }
        Ok(state)
    }

    pub fn apply(&mut self, event: ContextEvent) -> Result<(), ContextError> {
        match event {
            ContextEvent::Replace { contribution } => {
                validate(&contribution)?;
                if !self.contributions.contains_key(&contribution.id) {
                    return Err(ContextError::MissingContribution(contribution.id));
                }
                self.contributions
                    .insert(contribution.id.clone(), contribution);
            }
            ContextEvent::Remove { id } => {
                if self.contributions.remove(&id).is_none() {
                    return Err(ContextError::MissingContribution(id));
                }
            }
            ContextEvent::Append { contribution } => {
                validate(&contribution)?;
                if self.contributions.contains_key(&contribution.id) {
                    return Err(ContextError::DuplicateContribution(contribution.id));
                }
                self.contributions
                    .insert(contribution.id.clone(), contribution);
            }
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot::new(self.contributions.values().cloned())
    }

    pub fn get(&self, id: &str) -> Option<&Contribution> {
        self.contributions.get(id)
    }

    fn insert_initial(&mut self, contribution: Contribution) -> Result<(), ContextError> {
        validate(&contribution)?;
        if self
            .contributions
            .insert(contribution.id.clone(), contribution)
            .is_some()
        {
            return Err(ContextError::DuplicateContribution(
                "duplicate snapshot id".into(),
            ));
        }
        Ok(())
    }
}

/// Canonical session context state backed by the durable session log.
pub struct ContextController {
    state: Mutex<ContextState>,
    log: Arc<EventLog>,
}

impl ContextController {
    pub fn new(snapshot: Snapshot, log: Arc<EventLog>) -> Result<Self, ContextError> {
        Ok(Self {
            state: Mutex::new(ContextState::from_snapshot(snapshot)?),
            log,
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap().snapshot()
    }

    pub fn apply(&self, event: ContextEvent) -> Result<(), ContextError> {
        let mut next = self.state.lock().unwrap().clone();
        next.apply(event.clone())?;
        let event_type = match event {
            ContextEvent::Replace { .. } => "context.replace",
            ContextEvent::Remove { .. } => "context.remove",
            ContextEvent::Append { .. } => "context.append",
        };
        let (target, revision, content) = match &event {
            ContextEvent::Replace { contribution } | ContextEvent::Append { contribution } => (
                Some(contribution.id.clone()),
                Some(contribution.revision),
                Some(contribution.content.clone()),
            ),
            ContextEvent::Remove { id } => (Some(id.clone()), None, None),
        };
        let payload = serde_json::json!({
            "kind": event_type,
            "source": event.source(),
            "target": target,
            "revision": revision,
            "content": content,
            "event": event,
        });
        self.log
            .append(event_type, payload)
            .map_err(|error| ContextError::Log(error.to_string()))?;
        *self.state.lock().unwrap() = next;
        Ok(())
    }

    pub fn log(&self) -> &Arc<EventLog> {
        &self.log
    }
}

fn validate(contribution: &Contribution) -> Result<(), ContextError> {
    if contribution.id.trim().is_empty() {
        return Err(ContextError::EmptyContributionId);
    }
    if contribution.slot.trim().is_empty() {
        return Err(ContextError::EmptySlot);
    }
    Ok(())
}

fn sort_contributions(contributions: &mut [Contribution]) {
    contributions.sort_by(|left, right| {
        slot_rank(&left.slot)
            .cmp(&slot_rank(&right.slot))
            .then(left.order.cmp(&right.order))
            .then(left.id.cmp(&right.id))
    });
}

fn slot_rank(slot: &str) -> (u8, &str) {
    let rank = match slot {
        "system" => 0,
        "agent_instructions" => 1,
        "tools" => 2,
        "profile" => 3,
        "identity" => 4,
        _ => 5,
    };
    (rank, slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_are_deterministic_and_slot_ordered() {
        let snapshot = Snapshot::new([
            Contribution::new("identity", "identity", "A"),
            Contribution::new("system", "system", "S"),
            Contribution::new("profile", "profile", "P"),
        ]);
        assert_eq!(
            snapshot
                .contributions
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["system", "profile", "identity"]
        );
    }

    #[test]
    fn replacement_and_removal_replay_without_rebuilding_the_snapshot() {
        let mut state = ContextState::from_snapshot(Snapshot::new([Contribution::new(
            "profile", "profile", "old",
        )
        .with_revision(1)]))
        .unwrap();
        state
            .apply(ContextEvent::Replace {
                contribution: Contribution::new("profile", "profile", "new").with_revision(2),
            })
            .unwrap();
        assert_eq!(state.get("profile").unwrap().revision, 2);
        state
            .apply(ContextEvent::Remove {
                id: "profile".into(),
            })
            .unwrap();
        assert!(state.get("profile").is_none());
    }

    #[test]
    fn controller_persists_context_events_in_the_canonical_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(EventLog::open(dir.path().join("session.jsonl"), "session").unwrap());
        let controller = ContextController::new(Snapshot::new([]), log.clone()).unwrap();
        controller
            .apply(ContextEvent::Append {
                contribution: Contribution::new("identity", "identity", "You are A."),
            })
            .unwrap();
        assert_eq!(controller.snapshot().contributions.len(), 1);
        assert_eq!(log.records().unwrap()[0].event_type, "context.append");
    }
}
