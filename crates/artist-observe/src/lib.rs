//! Backend-neutral observations projected from Artist's public stream.
//!
//! Observations are never required to replay or resume a session. A
//! model-emitted tool request and a Rig-confirmed execution are distinct facts.

use std::collections::HashMap;

use artist_core::{
    CallId, CompletionCallMetadata, ContentPart, InterruptionCause, ModelFailure, RunId, SessionId,
    StreamEvent, StreamEventKind, TokenUsage,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub session_id: SessionId,
    pub run_id: Option<RunId>,
    pub kind: ObservationKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "observation", rename_all = "snake_case")]
pub enum ObservationKind {
    PromptStarted {
        model: String,
        messages_in: usize,
    },
    PromptCompleted {
        model: String,
        usage: Option<TokenUsage>,
        terminal: Option<CompletionCallMetadata>,
        time_to_first_token_ms: Option<u64>,
        duration_ms: u64,
    },
    PromptInterrupted {
        model: String,
        cause: InterruptionCause,
    },
    PromptFailed {
        model: String,
        failure: ModelFailure,
    },
    ToolRequested {
        call_id: CallId,
        name: String,
        arguments: String,
    },
    ToolExecutionCommitted {
        call_id: CallId,
        name: String,
        arguments: String,
    },
    ToolCompleted {
        call_id: CallId,
        name: String,
        content: Vec<ContentPart>,
    },
    ContextCompacted {
        evicted_count: usize,
        evicted_bytes: usize,
        summary_bytes: usize,
    },
}

pub trait ObservationSink: Send + 'static {
    fn emit(&mut self, observation: Observation);
}

impl ObservationSink for mpsc::UnboundedSender<Observation> {
    fn emit(&mut self, observation: Observation) {
        let _ = self.send(observation);
    }
}

pub struct Observer<S> {
    model: String,
    sink: S,
    executed_tools: HashMap<CallId, String>,
    usage: HashMap<RunId, TokenUsage>,
    completion_calls: HashMap<RunId, Vec<CompletionCallMetadata>>,
}

impl<S: ObservationSink> Observer<S> {
    pub fn new(model: impl Into<String>, sink: S) -> Self {
        Self {
            model: model.into(),
            sink,
            executed_tools: HashMap::new(),
            usage: HashMap::new(),
            completion_calls: HashMap::new(),
        }
    }

    pub fn observe(&mut self, event: &StreamEvent) {
        let kind = match &event.kind {
            StreamEventKind::RunStarted { messages_in } => Some(ObservationKind::PromptStarted {
                model: self.model.clone(),
                messages_in: *messages_in,
            }),
            StreamEventKind::Completed {
                duration_ms,
                time_to_first_token_ms,
                ..
            } => {
                let usage = event
                    .run_id
                    .as_ref()
                    .and_then(|run_id| self.usage.remove(run_id));
                let terminal = event.run_id.as_ref().and_then(|run_id| {
                    self.completion_calls
                        .remove(run_id)
                        .and_then(|calls| calls.last().cloned())
                });
                Some(ObservationKind::PromptCompleted {
                    model: self.model.clone(),
                    usage,
                    terminal,
                    time_to_first_token_ms: *time_to_first_token_ms,
                    duration_ms: *duration_ms,
                })
            }
            StreamEventKind::Usage(usage) => {
                if let Some(run_id) = &event.run_id {
                    self.usage.insert(run_id.clone(), *usage);
                }
                None
            }
            StreamEventKind::CompletionMetadata { calls } => {
                if let Some(run_id) = &event.run_id {
                    self.completion_calls.insert(run_id.clone(), calls.clone());
                }
                None
            }
            StreamEventKind::Interrupted { cause } => {
                self.clear_run(event.run_id.as_ref());
                Some(ObservationKind::PromptInterrupted {
                    model: self.model.clone(),
                    cause: cause.clone(),
                })
            }
            StreamEventKind::Failed { failure } => {
                self.clear_run(event.run_id.as_ref());
                Some(ObservationKind::PromptFailed {
                    model: self.model.clone(),
                    failure: failure.clone(),
                })
            }
            StreamEventKind::ToolCall {
                call_id,
                name,
                arguments,
            } => Some(ObservationKind::ToolRequested {
                call_id: call_id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
            }),
            StreamEventKind::ToolExecutionCommitted {
                call_id,
                name,
                arguments,
            } => {
                self.executed_tools.insert(call_id.clone(), name.clone());
                Some(ObservationKind::ToolExecutionCommitted {
                    call_id: call_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                })
            }
            StreamEventKind::ToolResult { call_id, content } => self
                .executed_tools
                .remove(call_id)
                .map(|name| ObservationKind::ToolCompleted {
                    call_id: call_id.clone(),
                    name,
                    content: content.clone(),
                }),
            StreamEventKind::ContextCompacted {
                evicted_count,
                evicted_bytes,
                summary_bytes,
            } => Some(ObservationKind::ContextCompacted {
                evicted_count: *evicted_count,
                evicted_bytes: *evicted_bytes,
                summary_bytes: *summary_bytes,
            }),
            StreamEventKind::InputQueued { .. }
            | StreamEventKind::SteeringQueued { .. }
            | StreamEventKind::SteeringDelivered { .. }
            | StreamEventKind::TextDelta { .. }
            | StreamEventKind::TextReset
            | StreamEventKind::Content { .. }
            | StreamEventKind::ToolCallDelta { .. } => None,
        };
        if let Some(kind) = kind {
            self.sink.emit(Observation {
                session_id: event.session_id.clone(),
                run_id: event.run_id.clone(),
                kind,
            });
        }
    }

    pub async fn forward(mut self, mut events: broadcast::Receiver<StreamEvent>) {
        loop {
            match events.recv().await {
                Ok(event) => self.observe(&event),
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    self.executed_tools.clear();
                    self.usage.clear();
                    self.completion_calls.clear();
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }

    fn clear_run(&mut self, run_id: Option<&RunId>) {
        if let Some(run_id) = run_id {
            self.usage.remove(run_id);
            self.completion_calls.remove(run_id);
        }
        self.executed_tools.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_core::{EventId, FailureClass, FinishReason, MessageId};

    fn event(run: &str, kind: StreamEventKind) -> StreamEvent {
        StreamEvent {
            event_id: EventId::from("event"),
            session_id: SessionId::from("session"),
            run_id: Some(RunId::from(run)),
            sequence: 0,
            kind,
        }
    }

    #[test]
    fn requested_and_executed_tools_are_distinct() {
        let (sender, mut observations) = mpsc::unbounded_channel();
        let mut observer = Observer::new("model", sender);
        observer.observe(&event(
            "run",
            StreamEventKind::ToolCall {
                call_id: CallId::from("skipped"),
                name: "write".into(),
                arguments: "{}".into(),
            },
        ));
        observer.observe(&event(
            "run",
            StreamEventKind::ToolResult {
                call_id: CallId::from("skipped"),
                content: vec![ContentPart::text("denied")],
            },
        ));
        assert!(matches!(
            observations.try_recv().unwrap().kind,
            ObservationKind::ToolRequested { .. }
        ));
        assert!(observations.try_recv().is_err());

        observer.observe(&event(
            "run",
            StreamEventKind::ToolExecutionCommitted {
                call_id: CallId::from("executed"),
                name: "read".into(),
                arguments: "{}".into(),
            },
        ));
        observer.observe(&event(
            "run",
            StreamEventKind::ToolResult {
                call_id: CallId::from("executed"),
                content: vec![ContentPart::text("ok")],
            },
        ));
        assert!(matches!(
            observations.try_recv().unwrap().kind,
            ObservationKind::ToolExecutionCommitted { .. }
        ));
        assert!(matches!(
            observations.try_recv().unwrap().kind,
            ObservationKind::ToolCompleted { .. }
        ));
    }

    #[test]
    fn terminal_metadata_and_structured_failures_are_preserved() {
        let (sender, mut observations) = mpsc::unbounded_channel();
        let mut observer = Observer::new("model", sender);
        let terminal = CompletionCallMetadata {
            call_index: 1,
            finish_reason: Some(FinishReason::Other("provider_stop".into())),
            message_id: Some("message".into()),
            response_id: Some("response".into()),
            provider_request_id: Some("request".into()),
        };
        observer.observe(&event(
            "success",
            StreamEventKind::CompletionMetadata {
                calls: vec![terminal.clone()],
            },
        ));
        observer.observe(&event(
            "success",
            StreamEventKind::Completed {
                message_id: MessageId::from("answer"),
                duration_ms: 10,
                time_to_first_token_ms: Some(2),
            },
        ));
        assert!(matches!(
            observations.try_recv().unwrap().kind,
            ObservationKind::PromptCompleted { terminal: Some(found), .. } if found == terminal
        ));

        let failure = ModelFailure {
            message: "rate limited".into(),
            class: FailureClass::Provider,
            retriable: true,
            provider_code: Some("rate_limit".into()),
            http_status: Some(429),
            provider_request_id: Some("request".into()),
        };
        observer.observe(&event(
            "failed",
            StreamEventKind::Failed {
                failure: failure.clone(),
            },
        ));
        assert!(matches!(
            observations.try_recv().unwrap().kind,
            ObservationKind::PromptFailed { failure: found, .. } if found == failure
        ));
    }
}
