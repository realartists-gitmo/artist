//! `rig-tap` projection for Artist's public stream protocol.

use std::collections::HashMap;

use artist_core::{CallId, RunId, StreamEvent, StreamEventKind, TokenUsage};
use rig_tap::{ErrorClass, EventKind, emit_kind};
use tokio::sync::broadcast;

pub struct TapObserver {
    model: String,
    tools: HashMap<CallId, String>,
    usage: HashMap<RunId, TokenUsage>,
}

impl TapObserver {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            tools: HashMap::new(),
            usage: HashMap::new(),
        }
    }

    pub fn observe(&mut self, event: &StreamEvent) {
        let kind = match &event.kind {
            StreamEventKind::RunStarted { messages_in } => Some(EventKind::PromptStarted {
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
                Some(EventKind::PromptCompleted {
                    model: self.model.clone(),
                    tokens_in: usage.and_then(|value| nonzero(value.input)),
                    tokens_out: usage.and_then(|value| nonzero(value.output)),
                    cached_tokens_in: usage.and_then(|value| nonzero(value.cached_input)),
                    reasoning_tokens: usage.and_then(|value| nonzero(value.reasoning)),
                    cost_usd: None,
                    finish_reason: Some("stop".into()),
                    response_id: None,
                    previous_response_id: None,
                    time_to_first_token_ms: *time_to_first_token_ms,
                    duration_ms: Some(*duration_ms),
                })
            }
            StreamEventKind::Usage(usage) => {
                if let Some(run_id) = &event.run_id {
                    self.usage.insert(run_id.clone(), *usage);
                }
                None
            }
            StreamEventKind::Interrupted { cause } => Some(EventKind::PromptFailed {
                model: self.model.clone(),
                error_class: ErrorClass::Cancelled,
                message: format!("{cause:?}"),
                retriable: true,
                provider_error_code: None,
                http_status: None,
            }),
            StreamEventKind::Failed { error } => Some(EventKind::PromptFailed {
                model: self.model.clone(),
                error_class: ErrorClass::Unknown,
                message: error.clone(),
                retriable: false,
                provider_error_code: None,
                http_status: None,
            }),
            StreamEventKind::ToolCall {
                call_id,
                name,
                arguments,
            } => {
                self.tools.insert(call_id.clone(), name.clone());
                Some(EventKind::ToolInvoked {
                    tool_name: name.clone(),
                    provider_call_id: None,
                    call_id: call_id.to_string(),
                    args_json: arguments.clone(),
                    truncated: false,
                })
            }
            StreamEventKind::ToolResult { call_id, result } => Some(EventKind::ToolCompleted {
                tool_name: self
                    .tools
                    .remove(call_id)
                    .unwrap_or_else(|| "unknown".into()),
                provider_call_id: None,
                call_id: call_id.to_string(),
                result: result.clone(),
                truncated: false,
                duration_ms: None,
            }),
            StreamEventKind::ContextCompacted {
                evicted_count,
                evicted_bytes,
                summary_bytes,
            } => Some(EventKind::ContextCompacted {
                evicted_count: *evicted_count,
                evicted_bytes: *evicted_bytes,
                carry_over: true,
                summary_bytes: *summary_bytes,
            }),
            StreamEventKind::InputQueued { .. }
            | StreamEventKind::SteeringQueued { .. }
            | StreamEventKind::SteeringDelivered { .. }
            | StreamEventKind::TextDelta { .. }
            | StreamEventKind::TextReset
            | StreamEventKind::ToolCallDelta { .. } => None,
        };
        if let Some(kind) = kind {
            emit_kind(event.session_id.to_string(), kind);
        }
    }

    pub async fn forward(mut self, mut events: broadcast::Receiver<StreamEvent>) {
        loop {
            match events.recv().await {
                Ok(event) => self.observe(&event),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

fn nonzero(value: u64) -> Option<u64> {
    (value != 0).then_some(value)
}
