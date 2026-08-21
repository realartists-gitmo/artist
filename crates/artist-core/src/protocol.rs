use serde::{Deserialize, Serialize};

use crate::{CallId, EventId, MessageId, RunId, SessionId};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    User,
    Harness,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Input { source: Source, content: String },
    Steer { source: Source, content: String },
    Abort { cause: InterruptionCause },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "cause", rename_all = "snake_case")]
pub enum InterruptionCause {
    User,
    Harness { reason: String },
    Provider { reason: String },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StreamEvent {
    pub event_id: EventId,
    pub session_id: SessionId,
    pub run_id: Option<RunId>,
    pub sequence: u64,
    pub kind: StreamEventKind,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StreamEventKind {
    InputQueued {
        message_id: MessageId,
    },
    SteeringQueued {
        message_id: MessageId,
    },
    SteeringDelivered {
        message_ids: Vec<MessageId>,
    },
    RunStarted {
        messages_in: usize,
    },
    TextDelta {
        delta: String,
    },
    TextReset,
    ToolCallDelta {
        call_id: CallId,
        delta: String,
    },
    ToolCall {
        call_id: CallId,
        name: String,
        arguments: String,
    },
    ToolResult {
        call_id: CallId,
        result: String,
    },
    ContextCompacted {
        evicted_count: usize,
        evicted_bytes: usize,
        summary_bytes: usize,
    },
    Usage(TokenUsage),
    Completed {
        message_id: MessageId,
        duration_ms: u64,
        time_to_first_token_ms: Option<u64>,
    },
    Interrupted {
        cause: InterruptionCause,
    },
    Failed {
        error: String,
    },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cached_input: u64,
    pub reasoning: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginDescriptor {
    pub id: crate::PluginId,
    pub version: String,
    pub capabilities: Vec<PluginCapability>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    Prompt,
    Tools,
    Resources,
    Context,
    Hooks,
    Model,
    Events,
}
