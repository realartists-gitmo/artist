use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

use crate::{CallId, EventId, InterruptionCause, MessageId, RunId, SessionId, Source};

pub const RECORD_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContextFragment {
    pub source: String,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InitialContext {
    pub fragments: Vec<ContextFragment>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SessionRecord {
    pub version: u32,
    pub session_id: SessionId,
    pub initial_context: InitialContext,
    pub entries: Vec<TranscriptEntry>,
}

impl SessionRecord {
    pub fn new(session_id: SessionId, initial_context: InitialContext) -> Self {
        Self {
            version: RECORD_VERSION,
            session_id,
            initial_context,
            entries: Vec::new(),
        }
    }

    pub fn next_sequence(&self) -> u64 {
        self.entries.len() as u64
    }

    pub fn entry(&self, kind: TranscriptEntryKind) -> TranscriptEntry {
        let sequence = self.next_sequence();
        TranscriptEntry {
            event_id: EventId::new(format!("{}:event:{sequence}", self.session_id)),
            sequence,
            kind,
        }
    }

    pub fn append(&mut self, entry: TranscriptEntry) -> Result<(), RecordError> {
        if entry.sequence != self.next_sequence() {
            return Err(RecordError::Sequence {
                expected: self.next_sequence(),
                actual: entry.sequence,
            });
        }
        self.entries.push(entry);
        if let Err(error) = self.validate() {
            self.entries.pop();
            return Err(error);
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), RecordError> {
        if self.version != RECORD_VERSION {
            return Err(RecordError::Version(self.version));
        }

        let mut active: Option<&RunId> = None;
        let mut inputs = HashSet::new();
        let mut steering = HashSet::new();
        let mut calls = HashMap::new();
        for (expected, entry) in self.entries.iter().enumerate() {
            if entry.sequence != expected as u64 {
                return Err(RecordError::Sequence {
                    expected: expected as u64,
                    actual: entry.sequence,
                });
            }

            match &entry.kind {
                TranscriptEntryKind::Input { message_id, .. } => {
                    inputs.insert(message_id);
                }
                TranscriptEntryKind::SteeringQueued { message_id, .. } => {
                    steering.insert(message_id);
                }
                TranscriptEntryKind::RunStarted { run_id, input_id } => {
                    if !inputs.contains(input_id) {
                        return Err(RecordError::UnknownInput);
                    }
                    if active.replace(run_id).is_some() {
                        return Err(RecordError::RunAlreadyActive);
                    }
                }
                TranscriptEntryKind::AssistantMessage {
                    run_id, complete, ..
                } => {
                    require_run(active, run_id)?;
                    if *complete {
                        finish_run(&mut active, run_id)?;
                    }
                }
                TranscriptEntryKind::Interrupted { run_id, .. } => {
                    let previous = expected.checked_sub(1).and_then(|i| self.entries.get(i));
                    if !matches!(
                        previous.map(|entry| &entry.kind),
                        Some(TranscriptEntryKind::AssistantMessage {
                            run_id: partial_run,
                            complete: false,
                            ..
                        }) if partial_run == run_id
                    ) {
                        return Err(RecordError::MissingPartialMessage);
                    }
                    finish_run(&mut active, run_id)?;
                }
                TranscriptEntryKind::RunFailed { run_id, .. } => finish_run(&mut active, run_id)?,
                TranscriptEntryKind::SteeringDelivered {
                    run_id,
                    message_ids,
                } => {
                    require_run(active, run_id)?;
                    if message_ids.iter().any(|id| !steering.contains(id)) {
                        return Err(RecordError::UnknownSteering);
                    }
                }
                TranscriptEntryKind::ToolCall {
                    run_id, call_id, ..
                } => {
                    require_run(active, run_id)?;
                    if calls.insert(call_id, run_id).is_some() {
                        return Err(RecordError::DuplicateToolCall);
                    }
                }
                TranscriptEntryKind::ToolResult {
                    run_id, call_id, ..
                } => {
                    require_run(active, run_id)?;
                    if calls.get(call_id) != Some(&run_id) {
                        return Err(RecordError::UnknownToolCall);
                    }
                }
                TranscriptEntryKind::Compaction {
                    through_sequence, ..
                } if *through_sequence >= entry.sequence => {
                    return Err(RecordError::InvalidCompaction);
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn require_run(active: Option<&RunId>, run_id: &RunId) -> Result<(), RecordError> {
    match active {
        Some(current) if current == run_id => Ok(()),
        Some(_) => Err(RecordError::WrongRun),
        None => Err(RecordError::NoActiveRun),
    }
}

fn finish_run<'a>(active: &mut Option<&'a RunId>, run_id: &'a RunId) -> Result<(), RecordError> {
    match active.take() {
        Some(current) if current == run_id => Ok(()),
        Some(current) => {
            *active = Some(current);
            Err(RecordError::WrongRun)
        }
        None => Err(RecordError::NoActiveRun),
    }
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

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RecordError {
    #[error("unsupported record version {0}")]
    Version(u32),
    #[error("expected transcript sequence {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("a run is already active")]
    RunAlreadyActive,
    #[error("no run is active")]
    NoActiveRun,
    #[error("entry refers to a different active run")]
    WrongRun,
    #[error("an interruption must immediately follow its partial assistant message")]
    MissingPartialMessage,
    #[error("run refers to an input that is not in the transcript")]
    UnknownInput,
    #[error("delivery refers to steering that is not in the transcript")]
    UnknownSteering,
    #[error("a tool-call ID is reused")]
    DuplicateToolCall,
    #[error("tool result has no matching call in the active run")]
    UnknownToolCall,
    #[error("compaction must refer to an earlier transcript sequence")]
    InvalidCompaction,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> SessionRecord {
        SessionRecord::new(
            SessionId::from("session"),
            InitialContext { fragments: vec![] },
        )
    }

    #[test]
    fn interruption_requires_partial_content() {
        let mut record = record();
        let run_id = RunId::from("run");
        record
            .append(record.entry(TranscriptEntryKind::Input {
                message_id: MessageId::from("input"),
                source: Source::User,
                content: "hello".into(),
            }))
            .unwrap();
        record
            .append(record.entry(TranscriptEntryKind::RunStarted {
                run_id: run_id.clone(),
                input_id: MessageId::from("input"),
            }))
            .unwrap();

        let error = record
            .append(record.entry(TranscriptEntryKind::Interrupted {
                run_id,
                cause: InterruptionCause::User,
            }))
            .unwrap_err();

        assert_eq!(error, RecordError::MissingPartialMessage);
        assert_eq!(record.entries.len(), 2);
    }

    #[test]
    fn interrupted_run_is_valid_and_round_trips() {
        let mut record = record();
        let run_id = RunId::from("run");
        for kind in [
            TranscriptEntryKind::Input {
                message_id: MessageId::from("input"),
                source: Source::User,
                content: "hello".into(),
            },
            TranscriptEntryKind::RunStarted {
                run_id: run_id.clone(),
                input_id: MessageId::from("input"),
            },
            TranscriptEntryKind::AssistantMessage {
                message_id: MessageId::from("partial"),
                run_id: run_id.clone(),
                content: "half".into(),
                complete: false,
            },
            TranscriptEntryKind::Interrupted {
                run_id,
                cause: InterruptionCause::User,
            },
        ] {
            record.append(record.entry(kind)).unwrap();
        }

        record.validate().unwrap();
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(
            serde_json::from_str::<SessionRecord>(&json).unwrap(),
            record
        );
    }

    #[test]
    fn rejects_references_to_missing_canonical_entries() {
        let mut record = record();
        let error = record
            .append(record.entry(TranscriptEntryKind::RunStarted {
                run_id: RunId::from("run"),
                input_id: MessageId::from("missing"),
            }))
            .unwrap_err();
        assert_eq!(error, RecordError::UnknownInput);

        let error = record
            .append(record.entry(TranscriptEntryKind::Compaction {
                through_sequence: 0,
                artifact: "future".into(),
            }))
            .unwrap_err();
        assert_eq!(error, RecordError::InvalidCompaction);
    }
}
