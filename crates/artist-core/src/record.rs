use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::{
    CallId, ContentPart, EventId, InterruptionCause, MessageId, RunId, RunOutcome, SessionId,
    Source,
};

pub const RECORD_VERSION: u32 = 3;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContextFragment {
    pub source: String,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InitialContext {
    pub fragments: Vec<ContextFragment>,
}

/// A validated canonical session record.
///
/// Fields are private so deserialization and mutation cannot bypass the
/// transcript reducer. Serialization emits v3; deserialization validates v3
/// and migrates v1/v2 snapshots.
#[derive(Clone, Debug)]
pub struct SessionRecord {
    version: u32,
    session_id: SessionId,
    initial_context: InitialContext,
    entries: Vec<TranscriptEntry>,
    state: RecordState,
}

impl PartialEq for SessionRecord {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
            && self.session_id == other.session_id
            && self.initial_context == other.initial_context
            && self.entries == other.entries
    }
}

impl SessionRecord {
    pub fn new(session_id: SessionId, initial_context: InitialContext) -> Self {
        Self {
            version: RECORD_VERSION,
            session_id,
            initial_context,
            entries: Vec::new(),
            state: RecordState::default(),
        }
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn initial_context(&self) -> &InitialContext {
        &self.initial_context
    }

    pub fn entries(&self) -> &[TranscriptEntry] {
        &self.entries
    }

    pub fn next_sequence(&self) -> u64 {
        self.state.next_sequence
    }

    pub fn active_run(&self) -> Option<&RunId> {
        self.state.active.as_ref().map(|active| &active.id)
    }

    pub fn active_message_id(&self) -> Option<&MessageId> {
        self.state
            .active
            .as_ref()
            .and_then(|active| active.message_id.as_ref())
    }

    pub fn entry(&self, kind: TranscriptEntryKind) -> TranscriptEntry {
        self.entry_at(self.next_sequence(), kind)
    }

    pub fn entries_for(
        &self,
        kinds: impl IntoIterator<Item = TranscriptEntryKind>,
    ) -> Vec<TranscriptEntry> {
        kinds
            .into_iter()
            .enumerate()
            .map(|(offset, kind)| self.entry_at(self.next_sequence() + offset as u64, kind))
            .collect()
    }

    fn entry_at(&self, sequence: u64, kind: TranscriptEntryKind) -> TranscriptEntry {
        TranscriptEntry {
            event_id: event_id(&self.session_id, sequence),
            sequence,
            kind,
        }
    }

    pub fn append(&mut self, entry: TranscriptEntry) -> Result<(), RecordError> {
        self.append_batch(std::slice::from_ref(&entry))
    }

    /// Apply one logical batch. Successful work is proportional to the batch.
    /// A failed batch rebuilds only to roll back the exceptional path.
    pub fn append_batch(&mut self, entries: &[TranscriptEntry]) -> Result<(), RecordError> {
        let original_len = self.entries.len();
        for entry in entries {
            if let Err(error) = self.state.apply(&self.session_id, entry) {
                self.entries.truncate(original_len);
                self.state = RecordState::replay(&self.session_id, &self.entries)
                    .expect("the pre-batch transcript was already validated");
                return Err(error);
            }
            self.entries.push(entry.clone());
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), RecordError> {
        if self.version != RECORD_VERSION {
            return Err(RecordError::Version(self.version));
        }
        RecordState::replay(&self.session_id, &self.entries).map(|_| ())
    }

    pub fn from_current_parts(
        session_id: SessionId,
        initial_context: InitialContext,
        entries: Vec<TranscriptEntry>,
    ) -> Result<Self, RecordError> {
        let state = RecordState::replay(&session_id, &entries)?;
        Ok(Self {
            version: RECORD_VERSION,
            session_id,
            initial_context,
            entries,
            state,
        })
    }

    #[cfg(test)]
    fn validation_steps(&self) -> u64 {
        self.state.validation_steps
    }

    fn migrate_v1(snapshot: SnapshotV1) -> Result<Self, RecordError> {
        if snapshot.version != 1 {
            return Err(RecordError::Version(snapshot.version));
        }
        let session_id = snapshot.session_id;
        let mut record = Self::new(session_id.clone(), snapshot.initial_context);
        let mut old_to_new = Vec::<u64>::with_capacity(snapshot.entries.len());
        let mut messages = HashMap::<RunId, MessageId>::new();
        for (expected_sequence, legacy) in snapshot.entries.into_iter().enumerate() {
            let expected_sequence = expected_sequence as u64;
            if legacy.sequence != expected_sequence {
                return Err(RecordError::Sequence {
                    expected: expected_sequence,
                    actual: legacy.sequence,
                });
            }
            let expected_id = event_id(&session_id, expected_sequence);
            if legacy.event_id != expected_id {
                return Err(RecordError::EventId {
                    expected: expected_id,
                    actual: legacy.event_id,
                });
            }
            let mut kinds = Vec::new();
            match legacy.kind {
                LegacyEntryKind::Input {
                    message_id,
                    source,
                    content,
                } => kinds.push(TranscriptEntryKind::Input {
                    message_id,
                    source,
                    content,
                }),
                LegacyEntryKind::SteeringQueued {
                    message_id,
                    source,
                    content,
                } => kinds.push(TranscriptEntryKind::SteeringQueued {
                    message_id,
                    source,
                    content,
                }),
                LegacyEntryKind::SteeringDelivered {
                    run_id,
                    message_ids,
                } => kinds.push(TranscriptEntryKind::SteeringDelivered {
                    run_id,
                    message_ids,
                }),
                LegacyEntryKind::RunStarted { run_id, input_id } => {
                    kinds.push(TranscriptEntryKind::RunStarted { run_id, input_id });
                }
                LegacyEntryKind::AssistantMessage {
                    message_id,
                    run_id,
                    content,
                    complete,
                } => {
                    messages.insert(run_id.clone(), message_id.clone());
                    kinds.push(TranscriptEntryKind::AssistantMessage {
                        message_id: message_id.clone(),
                        run_id: run_id.clone(),
                        content: vec![ContentPart::text(content)],
                    });
                    if complete {
                        kinds.push(TranscriptEntryKind::RunFinished {
                            run_id,
                            outcome: RunOutcome::Completed { message_id },
                        });
                    }
                }
                LegacyEntryKind::Interrupted { run_id, cause } => {
                    let message_id = messages
                        .get(&run_id)
                        .cloned()
                        .ok_or(RecordError::MissingAssistantMessage)?;
                    kinds.push(TranscriptEntryKind::RunFinished {
                        run_id,
                        outcome: RunOutcome::Interrupted { message_id, cause },
                    });
                }
                LegacyEntryKind::RunFailed { run_id, error } => {
                    let message_id = messages.get(&run_id).cloned();
                    kinds.push(TranscriptEntryKind::RunFinished {
                        run_id,
                        outcome: RunOutcome::Failed { message_id, error },
                    });
                }
                LegacyEntryKind::ToolCall {
                    run_id,
                    call_id,
                    name,
                    arguments,
                } => kinds.push(TranscriptEntryKind::ToolCall {
                    run_id,
                    call_id,
                    name,
                    arguments,
                }),
                LegacyEntryKind::ToolResult {
                    run_id,
                    call_id,
                    result,
                } => kinds.push(TranscriptEntryKind::ToolResult {
                    run_id,
                    call_id,
                    content: vec![ContentPart::text(result)],
                }),
                LegacyEntryKind::Compaction {
                    through_sequence,
                    artifact,
                } => {
                    let through_sequence = old_to_new
                        .get(through_sequence as usize)
                        .copied()
                        .ok_or(RecordError::InvalidCompaction)?;
                    kinds.push(TranscriptEntryKind::Compaction {
                        through_sequence,
                        artifact,
                    });
                }
            }
            let entries = record.entries_for(kinds);
            record.append_batch(&entries)?;
            old_to_new.push(record.next_sequence() - 1);
        }
        Ok(record)
    }

    /// Migrate entries from the v2 string-result representation. This is also
    /// used by the hash-chained physical-frame decoder after it validates the
    /// original v2 bytes.
    pub fn from_v2_parts(
        session_id: SessionId,
        initial_context: InitialContext,
        entries: Vec<V2TranscriptEntry>,
    ) -> Result<Self, RecordError> {
        let entries = entries
            .into_iter()
            .map(V2TranscriptEntry::migrate)
            .collect();
        Self::from_current_parts(session_id, initial_context, entries)
    }
}

impl Serialize for SessionRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SnapshotV3 {
            version: self.version,
            session_id: self.session_id.clone(),
            initial_context: self.initial_context.clone(),
            entries: self.entries.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SessionRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| serde::de::Error::custom("session snapshot has no numeric version"))?;
        match version {
            3 => {
                let snapshot: SnapshotV3 =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                Self::from_current_parts(
                    snapshot.session_id,
                    snapshot.initial_context,
                    snapshot.entries,
                )
                .map_err(serde::de::Error::custom)
            }
            2 => {
                let snapshot: SnapshotV2 =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                Self::from_v2_parts(
                    snapshot.session_id,
                    snapshot.initial_context,
                    snapshot.entries,
                )
                .map_err(serde::de::Error::custom)
            }
            1 => Self::migrate_v1(serde_json::from_value(value).map_err(serde::de::Error::custom)?)
                .map_err(serde::de::Error::custom),
            other => Err(serde::de::Error::custom(RecordError::Version(other as u32))),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct SnapshotV3 {
    version: u32,
    session_id: SessionId,
    initial_context: InitialContext,
    entries: Vec<TranscriptEntry>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct SnapshotV2 {
    version: u32,
    session_id: SessionId,
    initial_context: InitialContext,
    entries: Vec<V2TranscriptEntry>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TranscriptEntry {
    pub event_id: EventId,
    pub sequence: u64,
    pub kind: TranscriptEntryKind,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub enum TranscriptEntryKind {
    Input {
        message_id: MessageId,
        source: Source,
        content: String,
    },
    SteeringQueued {
        message_id: MessageId,
        source: Source,
        content: String,
    },
    SteeringDelivered {
        run_id: RunId,
        message_ids: Vec<MessageId>,
    },
    RunStarted {
        run_id: RunId,
        input_id: MessageId,
    },
    AssistantMessage {
        message_id: MessageId,
        run_id: RunId,
        content: Vec<ContentPart>,
    },
    RunFinished {
        run_id: RunId,
        outcome: RunOutcome,
    },
    ToolCall {
        run_id: RunId,
        call_id: CallId,
        name: String,
        arguments: String,
    },
    ToolResult {
        run_id: RunId,
        call_id: CallId,
        content: Vec<ContentPart>,
    },
    Compaction {
        through_sequence: u64,
        artifact: String,
    },
}

/// Exact v2 entry representation retained solely for migration and physical
/// hash verification. New code must use [`TranscriptEntry`].
#[doc(hidden)]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct V2TranscriptEntry {
    pub event_id: EventId,
    pub sequence: u64,
    pub kind: V2TranscriptEntryKind,
}

#[doc(hidden)]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub enum V2TranscriptEntryKind {
    Input {
        message_id: MessageId,
        source: Source,
        content: String,
    },
    SteeringQueued {
        message_id: MessageId,
        source: Source,
        content: String,
    },
    SteeringDelivered {
        run_id: RunId,
        message_ids: Vec<MessageId>,
    },
    RunStarted {
        run_id: RunId,
        input_id: MessageId,
    },
    AssistantMessage {
        message_id: MessageId,
        run_id: RunId,
        content: String,
    },
    RunFinished {
        run_id: RunId,
        outcome: RunOutcome,
    },
    ToolCall {
        run_id: RunId,
        call_id: CallId,
        name: String,
        arguments: String,
    },
    ToolResult {
        run_id: RunId,
        call_id: CallId,
        result: String,
    },
    Compaction {
        through_sequence: u64,
        artifact: String,
    },
}

impl V2TranscriptEntry {
    fn migrate(self) -> TranscriptEntry {
        let kind = match self.kind {
            V2TranscriptEntryKind::Input {
                message_id,
                source,
                content,
            } => TranscriptEntryKind::Input {
                message_id,
                source,
                content,
            },
            V2TranscriptEntryKind::SteeringQueued {
                message_id,
                source,
                content,
            } => TranscriptEntryKind::SteeringQueued {
                message_id,
                source,
                content,
            },
            V2TranscriptEntryKind::SteeringDelivered {
                run_id,
                message_ids,
            } => TranscriptEntryKind::SteeringDelivered {
                run_id,
                message_ids,
            },
            V2TranscriptEntryKind::RunStarted { run_id, input_id } => {
                TranscriptEntryKind::RunStarted { run_id, input_id }
            }
            V2TranscriptEntryKind::AssistantMessage {
                message_id,
                run_id,
                content,
            } => TranscriptEntryKind::AssistantMessage {
                message_id,
                run_id,
                content: vec![ContentPart::text(content)],
            },
            V2TranscriptEntryKind::RunFinished { run_id, outcome } => {
                TranscriptEntryKind::RunFinished { run_id, outcome }
            }
            V2TranscriptEntryKind::ToolCall {
                run_id,
                call_id,
                name,
                arguments,
            } => TranscriptEntryKind::ToolCall {
                run_id,
                call_id,
                name,
                arguments,
            },
            V2TranscriptEntryKind::ToolResult {
                run_id,
                call_id,
                result,
            } => TranscriptEntryKind::ToolResult {
                run_id,
                call_id,
                content: vec![ContentPart::text(result)],
            },
            V2TranscriptEntryKind::Compaction {
                through_sequence,
                artifact,
            } => TranscriptEntryKind::Compaction {
                through_sequence,
                artifact,
            },
        };
        TranscriptEntry {
            event_id: self.event_id,
            sequence: self.sequence,
            kind,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct RecordState {
    next_sequence: u64,
    active: Option<ActiveRun>,
    messages: HashSet<MessageId>,
    inputs: HashSet<MessageId>,
    started_inputs: HashSet<MessageId>,
    steering: HashSet<MessageId>,
    delivered_steering: HashSet<MessageId>,
    runs: HashSet<RunId>,
    calls: HashMap<CallId, ToolCallState>,
    validation_steps: u64,
}

#[derive(Clone, Debug)]
struct ActiveRun {
    id: RunId,
    message_id: Option<MessageId>,
    open_tools: usize,
}

#[derive(Clone, Debug)]
struct ToolCallState {
    run_id: RunId,
    resolved: bool,
}

impl RecordState {
    fn replay(session_id: &SessionId, entries: &[TranscriptEntry]) -> Result<Self, RecordError> {
        let mut state = Self::default();
        for entry in entries {
            state.apply(session_id, entry)?;
        }
        Ok(state)
    }

    fn apply(
        &mut self,
        session_id: &SessionId,
        entry: &TranscriptEntry,
    ) -> Result<(), RecordError> {
        if entry.sequence != self.next_sequence {
            return Err(RecordError::Sequence {
                expected: self.next_sequence,
                actual: entry.sequence,
            });
        }
        let expected_id = event_id(session_id, entry.sequence);
        if entry.event_id != expected_id {
            return Err(RecordError::EventId {
                expected: expected_id,
                actual: entry.event_id.clone(),
            });
        }
        match &entry.kind {
            TranscriptEntryKind::Input { message_id, .. } => {
                self.require_new_message(message_id)?;
                self.messages.insert(message_id.clone());
                self.inputs.insert(message_id.clone());
            }
            TranscriptEntryKind::SteeringQueued { message_id, .. } => {
                self.require_new_message(message_id)?;
                self.messages.insert(message_id.clone());
                self.steering.insert(message_id.clone());
            }
            TranscriptEntryKind::SteeringDelivered {
                run_id,
                message_ids,
            } => {
                self.require_run(run_id)?;
                let mut in_entry = HashSet::new();
                for message_id in message_ids {
                    if !self.steering.contains(message_id) {
                        return Err(RecordError::UnknownSteering);
                    }
                    if self.delivered_steering.contains(message_id) || !in_entry.insert(message_id)
                    {
                        return Err(RecordError::SteeringAlreadyDelivered);
                    }
                }
                self.delivered_steering.extend(message_ids.iter().cloned());
            }
            TranscriptEntryKind::RunStarted { run_id, input_id } => {
                if !self.inputs.contains(input_id) {
                    return Err(RecordError::UnknownInput);
                }
                if self.started_inputs.contains(input_id) {
                    return Err(RecordError::InputAlreadyStarted);
                }
                if self.active.is_some() {
                    return Err(RecordError::RunAlreadyActive);
                }
                if self.runs.contains(run_id) {
                    return Err(RecordError::DuplicateRun);
                }
                self.started_inputs.insert(input_id.clone());
                self.runs.insert(run_id.clone());
                self.active = Some(ActiveRun {
                    id: run_id.clone(),
                    message_id: None,
                    open_tools: 0,
                });
            }
            TranscriptEntryKind::AssistantMessage {
                message_id, run_id, ..
            } => {
                self.require_run(run_id)?;
                self.require_new_message(message_id)?;
                if self.active.as_ref().unwrap().message_id.is_some() {
                    return Err(RecordError::MultipleAssistantMessages);
                }
                self.messages.insert(message_id.clone());
                self.active.as_mut().unwrap().message_id = Some(message_id.clone());
            }
            TranscriptEntryKind::RunFinished { run_id, outcome } => {
                self.require_run(run_id)?;
                let active = self.active.as_ref().unwrap();
                match outcome {
                    RunOutcome::Completed { message_id } => {
                        if active.message_id.as_ref() != Some(message_id) {
                            return Err(RecordError::WrongAssistantMessage);
                        }
                        if active.open_tools != 0 {
                            return Err(RecordError::UnresolvedToolCalls);
                        }
                    }
                    RunOutcome::Interrupted { message_id, .. } => {
                        if active.message_id.as_ref() != Some(message_id) {
                            return Err(RecordError::WrongAssistantMessage);
                        }
                    }
                    RunOutcome::Failed { message_id, .. } => {
                        if active.message_id.as_ref() != message_id.as_ref() {
                            return Err(RecordError::WrongAssistantMessage);
                        }
                    }
                }
                self.active = None;
            }
            TranscriptEntryKind::ToolCall {
                run_id, call_id, ..
            } => {
                self.require_run(run_id)?;
                if self.calls.contains_key(call_id) {
                    return Err(RecordError::DuplicateToolCall);
                }
                self.calls.insert(
                    call_id.clone(),
                    ToolCallState {
                        run_id: run_id.clone(),
                        resolved: false,
                    },
                );
                self.active.as_mut().unwrap().open_tools += 1;
            }
            TranscriptEntryKind::ToolResult {
                run_id, call_id, ..
            } => {
                self.require_run(run_id)?;
                let call = self
                    .calls
                    .get_mut(call_id)
                    .ok_or(RecordError::UnknownToolCall)?;
                if &call.run_id != run_id {
                    return Err(RecordError::WrongRun);
                }
                if call.resolved {
                    return Err(RecordError::DuplicateToolResult);
                }
                call.resolved = true;
                self.active.as_mut().unwrap().open_tools -= 1;
            }
            TranscriptEntryKind::Compaction {
                through_sequence, ..
            } => {
                if *through_sequence >= entry.sequence {
                    return Err(RecordError::InvalidCompaction);
                }
            }
        }
        self.next_sequence += 1;
        self.validation_steps += 1;
        Ok(())
    }

    fn require_new_message(&self, message_id: &MessageId) -> Result<(), RecordError> {
        if self.messages.contains(message_id) {
            Err(RecordError::DuplicateMessage)
        } else {
            Ok(())
        }
    }

    fn require_run(&self, run_id: &RunId) -> Result<(), RecordError> {
        match &self.active {
            Some(active) if &active.id == run_id => Ok(()),
            Some(_) => Err(RecordError::WrongRun),
            None => Err(RecordError::NoActiveRun),
        }
    }
}

fn event_id(session_id: &SessionId, sequence: u64) -> EventId {
    EventId::new(format!("{session_id}:event:{sequence}"))
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RecordError {
    #[error("unsupported record version {0}")]
    Version(u32),
    #[error("expected transcript sequence {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("expected event ID {expected}, got {actual}")]
    EventId { expected: EventId, actual: EventId },
    #[error("a run is already active")]
    RunAlreadyActive,
    #[error("no run is active")]
    NoActiveRun,
    #[error("entry refers to a different active run")]
    WrongRun,
    #[error("run refers to an input that is not in the transcript")]
    UnknownInput,
    #[error("an input cannot start more than one run")]
    InputAlreadyStarted,
    #[error("a run ID is reused")]
    DuplicateRun,
    #[error("a message ID is reused")]
    DuplicateMessage,
    #[error("a run can persist at most one assistant message")]
    MultipleAssistantMessages,
    #[error("run outcome refers to the wrong assistant message")]
    WrongAssistantMessage,
    #[error("run outcome requires an assistant message")]
    MissingAssistantMessage,
    #[error("delivery refers to steering that is not in the transcript")]
    UnknownSteering,
    #[error("a steering notification cannot be delivered more than once")]
    SteeringAlreadyDelivered,
    #[error("a tool-call ID is reused")]
    DuplicateToolCall,
    #[error("tool result has no matching call in the active run")]
    UnknownToolCall,
    #[error("a tool call cannot have more than one result")]
    DuplicateToolResult,
    #[error("a completed run cannot contain unresolved tool calls")]
    UnresolvedToolCalls,
    #[error("compaction must refer to an earlier transcript sequence")]
    InvalidCompaction,
}

#[derive(Deserialize)]
struct SnapshotV1 {
    version: u32,
    session_id: SessionId,
    initial_context: InitialContext,
    entries: Vec<LegacyTranscriptEntry>,
}

#[derive(Deserialize)]
struct LegacyTranscriptEntry {
    event_id: EventId,
    sequence: u64,
    kind: LegacyEntryKind,
}

#[derive(Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
enum LegacyEntryKind {
    Input {
        message_id: MessageId,
        source: Source,
        content: String,
    },
    SteeringQueued {
        message_id: MessageId,
        source: Source,
        content: String,
    },
    SteeringDelivered {
        run_id: RunId,
        message_ids: Vec<MessageId>,
    },
    RunStarted {
        run_id: RunId,
        input_id: MessageId,
    },
    AssistantMessage {
        message_id: MessageId,
        run_id: RunId,
        content: String,
        complete: bool,
    },
    Interrupted {
        run_id: RunId,
        cause: InterruptionCause,
    },
    RunFailed {
        run_id: RunId,
        error: String,
    },
    ToolCall {
        run_id: RunId,
        call_id: CallId,
        name: String,
        arguments: String,
    },
    ToolResult {
        run_id: RunId,
        call_id: CallId,
        result: String,
    },
    Compaction {
        through_sequence: u64,
        artifact: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record() -> SessionRecord {
        SessionRecord::new(
            SessionId::from("session"),
            InitialContext { fragments: vec![] },
        )
    }

    #[test]
    fn v2_string_tool_results_migrate_to_one_text_part() {
        let snapshot = json!({
            "version": 2,
            "session_id": "session",
            "initial_context": { "fragments": [] },
            "entries": [
                {"event_id":"session:event:0","sequence":0,"kind":{"entry":"input","message_id":"input","source":"user","content":"hello"}},
                {"event_id":"session:event:1","sequence":1,"kind":{"entry":"run_started","run_id":"run","input_id":"input"}},
                {"event_id":"session:event:2","sequence":2,"kind":{"entry":"tool_call","run_id":"run","call_id":"call","name":"read","arguments":"{}"}},
                {"event_id":"session:event:3","sequence":3,"kind":{"entry":"tool_result","run_id":"run","call_id":"call","result":"body"}}
            ]
        });
        let migrated: SessionRecord = serde_json::from_value(snapshot).unwrap();
        assert_eq!(migrated.version(), 3);
        assert!(matches!(
            &migrated.entries()[3].kind,
            TranscriptEntryKind::ToolResult { content, .. }
                if content == &vec![ContentPart::text("body")]
        ));
    }

    fn append(record: &mut SessionRecord, kind: TranscriptEntryKind) {
        record.append(record.entry(kind)).unwrap();
    }

    fn start(record: &mut SessionRecord, run: &str, input: &str) {
        append(
            record,
            TranscriptEntryKind::Input {
                message_id: MessageId::from(input),
                source: Source::User,
                content: "hello".into(),
            },
        );
        append(
            record,
            TranscriptEntryKind::RunStarted {
                run_id: RunId::from(run),
                input_id: MessageId::from(input),
            },
        );
    }

    #[test]
    fn all_terminal_outcomes_are_explicit_and_round_trip() {
        let mut record = record();
        start(&mut record, "complete", "input-1");
        append(
            &mut record,
            TranscriptEntryKind::AssistantMessage {
                message_id: MessageId::from("answer-1"),
                run_id: RunId::from("complete"),
                content: vec![ContentPart::text("done")],
            },
        );
        append(
            &mut record,
            TranscriptEntryKind::RunFinished {
                run_id: RunId::from("complete"),
                outcome: RunOutcome::Completed {
                    message_id: MessageId::from("answer-1"),
                },
            },
        );
        start(&mut record, "interrupted", "input-2");
        append(
            &mut record,
            TranscriptEntryKind::AssistantMessage {
                message_id: MessageId::from("answer-2"),
                run_id: RunId::from("interrupted"),
                content: vec![ContentPart::text("half")],
            },
        );
        append(
            &mut record,
            TranscriptEntryKind::RunFinished {
                run_id: RunId::from("interrupted"),
                outcome: RunOutcome::Interrupted {
                    message_id: MessageId::from("answer-2"),
                    cause: InterruptionCause::User,
                },
            },
        );
        start(&mut record, "failed", "input-3");
        append(
            &mut record,
            TranscriptEntryKind::RunFinished {
                run_id: RunId::from("failed"),
                outcome: RunOutcome::Failed {
                    message_id: None,
                    error: "provider".into(),
                },
            },
        );
        let encoded = serde_json::to_string(&record).unwrap();
        assert_eq!(
            serde_json::from_str::<SessionRecord>(&encoded).unwrap(),
            record
        );
    }

    #[test]
    fn rejects_duplicate_ids_delivery_results_and_unresolved_completion() {
        let mut record = record();
        start(&mut record, "run", "input");
        let duplicate_input = record.entry(TranscriptEntryKind::Input {
            message_id: MessageId::from("input"),
            source: Source::User,
            content: "duplicate".into(),
        });
        assert_eq!(
            record.append(duplicate_input),
            Err(RecordError::DuplicateMessage)
        );
        append(
            &mut record,
            TranscriptEntryKind::SteeringQueued {
                message_id: MessageId::from("steer"),
                source: Source::Harness,
                content: "notice".into(),
            },
        );
        append(
            &mut record,
            TranscriptEntryKind::SteeringDelivered {
                run_id: RunId::from("run"),
                message_ids: vec![MessageId::from("steer")],
            },
        );
        let repeated = record.entry(TranscriptEntryKind::SteeringDelivered {
            run_id: RunId::from("run"),
            message_ids: vec![MessageId::from("steer")],
        });
        assert_eq!(
            record.append(repeated),
            Err(RecordError::SteeringAlreadyDelivered)
        );
        append(
            &mut record,
            TranscriptEntryKind::ToolCall {
                run_id: RunId::from("run"),
                call_id: CallId::from("call"),
                name: "read".into(),
                arguments: "{}".into(),
            },
        );
        append(
            &mut record,
            TranscriptEntryKind::ToolResult {
                run_id: RunId::from("run"),
                call_id: CallId::from("call"),
                content: vec![ContentPart::text("ok")],
            },
        );
        let duplicate = record.entry(TranscriptEntryKind::ToolResult {
            run_id: RunId::from("run"),
            call_id: CallId::from("call"),
            content: vec![ContentPart::text("again")],
        });
        assert_eq!(
            record.append(duplicate),
            Err(RecordError::DuplicateToolResult)
        );
        append(
            &mut record,
            TranscriptEntryKind::ToolCall {
                run_id: RunId::from("run"),
                call_id: CallId::from("open"),
                name: "poll".into(),
                arguments: "{}".into(),
            },
        );
        append(
            &mut record,
            TranscriptEntryKind::AssistantMessage {
                message_id: MessageId::from("answer"),
                run_id: RunId::from("run"),
                content: vec![ContentPart::text("done")],
            },
        );
        let finish = record.entry(TranscriptEntryKind::RunFinished {
            run_id: RunId::from("run"),
            outcome: RunOutcome::Completed {
                message_id: MessageId::from("answer"),
            },
        });
        assert_eq!(record.append(finish), Err(RecordError::UnresolvedToolCalls));
    }

    #[test]
    fn rejects_reused_inputs_runs_messages_and_wrong_terminal_references() {
        let mut record = record();
        start(&mut record, "run", "input");
        append(
            &mut record,
            TranscriptEntryKind::AssistantMessage {
                message_id: MessageId::from("answer"),
                run_id: RunId::from("run"),
                content: vec![ContentPart::text("done")],
            },
        );
        let second_message = record.entry(TranscriptEntryKind::AssistantMessage {
            message_id: MessageId::from("another"),
            run_id: RunId::from("run"),
            content: vec![ContentPart::text("duplicate")],
        });
        assert_eq!(
            record.append(second_message),
            Err(RecordError::MultipleAssistantMessages)
        );
        let wrong_finish = record.entry(TranscriptEntryKind::RunFinished {
            run_id: RunId::from("run"),
            outcome: RunOutcome::Completed {
                message_id: MessageId::from("wrong"),
            },
        });
        assert_eq!(
            record.append(wrong_finish),
            Err(RecordError::WrongAssistantMessage)
        );
        append(
            &mut record,
            TranscriptEntryKind::RunFinished {
                run_id: RunId::from("run"),
                outcome: RunOutcome::Completed {
                    message_id: MessageId::from("answer"),
                },
            },
        );
        let reused_input = record.entry(TranscriptEntryKind::RunStarted {
            run_id: RunId::from("new-run"),
            input_id: MessageId::from("input"),
        });
        assert_eq!(
            record.append(reused_input),
            Err(RecordError::InputAlreadyStarted)
        );
        append(
            &mut record,
            TranscriptEntryKind::Input {
                message_id: MessageId::from("new-input"),
                source: Source::User,
                content: "again".into(),
            },
        );
        let reused_run = record.entry(TranscriptEntryKind::RunStarted {
            run_id: RunId::from("run"),
            input_id: MessageId::from("new-input"),
        });
        assert_eq!(record.append(reused_run), Err(RecordError::DuplicateRun));

        let mut bad_id = record.entry(TranscriptEntryKind::Input {
            message_id: MessageId::from("bad-event"),
            source: Source::User,
            content: String::new(),
        });
        bad_id.event_id = EventId::from("forged");
        assert!(matches!(
            record.append(bad_id),
            Err(RecordError::EventId { .. })
        ));
    }

    #[test]
    fn a_failed_batch_rolls_back_every_entry() {
        let mut record = record();
        let batch = record.entries_for([
            TranscriptEntryKind::Input {
                message_id: MessageId::from("same"),
                source: Source::User,
                content: "first".into(),
            },
            TranscriptEntryKind::Input {
                message_id: MessageId::from("same"),
                source: Source::User,
                content: "second".into(),
            },
        ]);
        assert_eq!(
            record.append_batch(&batch),
            Err(RecordError::DuplicateMessage)
        );
        assert_eq!(record.next_sequence(), 0);
        assert!(record.entries().is_empty());
    }

    #[test]
    fn successful_incremental_append_does_not_replay_the_prefix() {
        let mut record = record();
        for index in 0..10_000 {
            append(
                &mut record,
                TranscriptEntryKind::Input {
                    message_id: MessageId::new(format!("input-{index}")),
                    source: Source::User,
                    content: String::new(),
                },
            );
        }
        assert_eq!(record.validation_steps(), 10_000);
    }

    #[test]
    fn migrates_v1_and_rejects_unknown_versions() {
        let legacy = json!({
            "version": 1, "session_id": "legacy", "initial_context": {"fragments": []},
            "entries": [
                {"event_id":"legacy:event:0","sequence":0,"kind":{"entry":"input","message_id":"input","source":"user","content":"hello"}},
                {"event_id":"legacy:event:1","sequence":1,"kind":{"entry":"run_started","run_id":"run","input_id":"input"}},
                {"event_id":"legacy:event:2","sequence":2,"kind":{"entry":"assistant_message","message_id":"answer","run_id":"run","content":"done","complete":true}}
            ]
        });
        let migrated: SessionRecord = serde_json::from_value(legacy).unwrap();
        assert_eq!(migrated.version(), RECORD_VERSION);
        assert!(matches!(
            migrated.entries().last().unwrap().kind,
            TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Completed { .. },
                ..
            }
        ));
        let unknown = json!({"version":99,"session_id":"future","initial_context":{"fragments":[]},"entries":[]});
        assert!(serde_json::from_value::<SessionRecord>(unknown).is_err());
        let unknown_entry = json!({
            "version": 2,
            "session_id": "future-entry",
            "initial_context": {"fragments": []},
            "entries": [{
                "event_id": "future-entry:event:0",
                "sequence": 0,
                "kind": {"entry": "future_semantics"}
            }]
        });
        assert!(serde_json::from_value::<SessionRecord>(unknown_entry).is_err());

        let corrupt_legacy = json!({
            "version": 1,
            "session_id": "legacy",
            "initial_context": {"fragments": []},
            "entries": [{
                "event_id": "forged",
                "sequence": 7,
                "kind": {"entry": "input", "message_id": "input", "source": "user", "content": "hello"}
            }]
        });
        assert!(serde_json::from_value::<SessionRecord>(corrupt_legacy).is_err());
    }
}
