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
    Reset { snapshot: Snapshot },
}

impl ContextEvent {
    pub fn source(&self) -> Option<&str> {
        match self {
            Self::Replace { contribution } | Self::Append { contribution } => {
                contribution.source.as_deref()
            }
            Self::Remove { .. } | Self::Reset { .. } => None,
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
    ConflictingContribution(String),
    MissingContribution(String),
    Log(String),
    Replay(String),
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyContributionId => f.write_str("contribution id cannot be empty"),
            Self::EmptySlot => f.write_str("contribution slot cannot be empty"),
            Self::DuplicateContribution(id) => write!(f, "contribution already exists: {id}"),
            Self::ConflictingContribution(id) => {
                write!(f, "contribution update conflicts with existing id: {id}")
            }
            Self::MissingContribution(id) => write!(f, "contribution does not exist: {id}"),
            Self::Log(error) => write!(f, "could not persist context event: {error}"),
            Self::Replay(error) => write!(f, "could not replay context event: {error}"),
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
                match self.contributions.get(&contribution.id) {
                    Some(current) if current == &contribution => {}
                    Some(_) => {
                        self.contributions
                            .insert(contribution.id.clone(), contribution);
                    }
                    None => return Err(ContextError::MissingContribution(contribution.id)),
                }
            }
            ContextEvent::Remove { id } => {
                // Removal is idempotent so a duplicated composition event
                // cannot turn a valid replay into a failure.
                self.contributions.remove(&id);
            }
            ContextEvent::Append { contribution } => {
                validate(&contribution)?;
                match self.contributions.get(&contribution.id) {
                    Some(current) if current == &contribution => {}
                    Some(_) => {
                        return Err(ContextError::ConflictingContribution(contribution.id));
                    }
                    None => {
                        self.contributions
                            .insert(contribution.id.clone(), contribution);
                    }
                }
            }
            ContextEvent::Reset { snapshot } => {
                let replacement = ContextState::from_snapshot(snapshot)?;
                self.contributions = replacement.contributions;
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

    /// Restore the durable context projection. The first snapshot is itself
    /// recorded so reopening a session never has to guess which initial
    /// composition was used. Later mutations are replayed from their
    /// self-contained event payloads.
    pub fn restore(snapshot: Snapshot, log: Arc<EventLog>) -> Result<Self, ContextError> {
        let records = log
            .records()
            .map_err(|error| ContextError::Log(error.to_string()))?;
        let initial = records
            .iter()
            .find(|record| record.event_type == "context.initial")
            .map(|record| {
                serde_json::from_value::<Snapshot>(
                    record.payload.get("snapshot").cloned().ok_or_else(|| {
                        ContextError::Replay("context.initial lacks snapshot".into())
                    })?,
                )
                .map_err(|error| ContextError::Replay(error.to_string()))
            })
            .transpose()?
            .unwrap_or(snapshot);
        let state = ContextState::from_snapshot(initial.clone())?;
        let controller = Self {
            state: Mutex::new(state),
            log: Arc::clone(&log),
        };

        if !records
            .iter()
            .any(|record| record.event_type == "context.initial")
            && !log.is_closed()
        {
            log.append("context.initial", serde_json::json!({"snapshot": initial}))
                .map_err(|error| ContextError::Log(error.to_string()))?;
        }

        for record in records {
            let Some(event) = (match record.event_type.as_str() {
                "context.replace" | "context.remove" | "context.append" | "context.reset" => record
                    .payload
                    .get("event")
                    .cloned()
                    .map(|value| {
                        serde_json::from_value::<ContextEvent>(value)
                            .map_err(|error| ContextError::Replay(error.to_string()))
                    })
                    .transpose()?,
                _ => None,
            }) else {
                continue;
            };
            controller.apply_replayed(event)?;
        }
        Ok(controller)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap().snapshot()
    }

    pub fn apply(&self, event: ContextEvent) -> Result<(), ContextError> {
        let mut state = self.state.lock().unwrap();
        let mut next = state.clone();
        next.apply(event.clone())?;
        if *state == next {
            return Ok(());
        }
        let event_type = match event {
            ContextEvent::Replace { .. } => "context.replace",
            ContextEvent::Remove { .. } => "context.remove",
            ContextEvent::Append { .. } => "context.append",
            ContextEvent::Reset { .. } => "context.reset",
        };
        let (target, revision, content) = match &event {
            ContextEvent::Replace { contribution } | ContextEvent::Append { contribution } => (
                Some(contribution.id.clone()),
                Some(contribution.revision),
                Some(contribution.content.clone()),
            ),
            ContextEvent::Remove { id } => (Some(id.clone()), None, None),
            ContextEvent::Reset { .. } => (None, None, None),
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
        *state = next;
        Ok(())
    }

    /// Replace the active composed context while retaining the durable
    /// transcript. Handoff uses this boundary to reset provider-facing
    /// context without erasing historical events.
    pub fn reset(&self, snapshot: Snapshot) -> Result<(), ContextError> {
        self.apply(ContextEvent::Reset { snapshot })
    }

    fn apply_replayed(&self, event: ContextEvent) -> Result<(), ContextError> {
        let mut state = self.state.lock().unwrap();
        state.apply(event)
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

    #[test]
    fn restore_replays_replacements_removals_and_reset_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let log = Arc::new(EventLog::open(&path, "session").unwrap());
        let initial = Snapshot::new([
            Contribution::new("system", "system", "old"),
            Contribution::new("stale", "profile", "remove me"),
        ]);
        let controller = ContextController::restore(initial.clone(), Arc::clone(&log)).unwrap();
        controller
            .apply(ContextEvent::Replace {
                contribution: Contribution::new("system", "system", "new").with_revision(2),
            })
            .unwrap();
        controller
            .apply(ContextEvent::Remove { id: "stale".into() })
            .unwrap();
        controller
            .apply(ContextEvent::Append {
                contribution: Contribution::new("extra", "profile", "extra"),
            })
            .unwrap();
        let expected = controller.snapshot();
        drop(controller);

        let reopened = ContextController::restore(initial, Arc::clone(&log)).unwrap();
        assert_eq!(reopened.snapshot(), expected);
        reopened
            .apply(ContextEvent::Replace {
                contribution: Contribution::new("system", "system", "new").with_revision(2),
            })
            .unwrap();
        assert_eq!(reopened.snapshot(), expected);

        let reset = Snapshot::new([Contribution::new("handoff", "profile", "brief")]);
        reopened.reset(reset.clone()).unwrap();
        drop(reopened);
        let restored = ContextController::restore(expected, log).unwrap();
        assert_eq!(restored.snapshot(), reset);
    }

    #[test]
    fn closed_logs_restore_read_only_context_and_reject_mutations() {
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(EventLog::open(dir.path().join("session.jsonl"), "session").unwrap());
        let initial = Snapshot::new([Contribution::new("system", "system", "stable")]);
        log.close().unwrap();

        let controller = ContextController::restore(initial.clone(), Arc::clone(&log)).unwrap();
        assert_eq!(controller.snapshot(), initial);
        assert!(log.records().unwrap().is_empty());

        let error = controller
            .apply(ContextEvent::Append {
                contribution: Contribution::new("new", "profile", "must not persist"),
            })
            .unwrap_err();
        assert!(matches!(error, ContextError::Log(message) if message.contains("closed")));
        assert_eq!(controller.snapshot(), initial);
    }
}
