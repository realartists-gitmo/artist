use serde::{Deserialize, Serialize};

use crate::{CallId, EventId, MessageId, ProfileSnapshot, RunId, SessionId, SlashCommandId};

/// Backend-neutral content preserved across live events and durable history.
/// Unknown provider-native content is retained as an opaque tagged JSON value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    Json {
        value: serde_json::Value,
    },
    Image {
        value: serde_json::Value,
    },
    Reasoning {
        value: serde_json::Value,
    },
    Opaque {
        kind: String,
        value: serde_json::Value,
    },
}

impl ContentPart {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

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
    Slash { name: String, arguments: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SlashCommandDefinition {
    /// Globally unique command name, without the leading slash.
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SlashCommandAction {
    Input {
        content: String,
    },
    Steer {
        content: String,
    },
    Abort {
        reason: String,
    },
    ActivateProfile {
        profile: String,
        brief: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SlashCommandResult {
    /// Harness-facing text. This is never projected into model context.
    pub output: Option<String>,
    /// Typed kernel actions applied in order after invocation succeeds.
    pub actions: Vec<SlashCommandAction>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "cause", rename_all = "snake_case")]
pub enum InterruptionCause {
    User,
    Harness { reason: String },
    Provider { reason: String },
}

/// Terminal state of one run, independent of transport or model-provider
/// representations.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RunOutcome {
    Completed {
        message_id: MessageId,
    },
    Failed {
        message_id: Option<MessageId>,
        error: String,
    },
    Interrupted {
        message_id: MessageId,
        cause: InterruptionCause,
    },
    Yielded {
        call_id: CallId,
        payload: serde_json::Value,
    },
    HandedOff {
        call_id: CallId,
        profile: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Other(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletionCallMetadata {
    pub call_index: usize,
    pub finish_reason: Option<FinishReason>,
    pub message_id: Option<String>,
    pub response_id: Option<String>,
    pub provider_request_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Transport,
    Provider,
    InvalidRequest,
    InvalidResponse,
    Memory,
    Tool,
    Cancelled,
    Limit,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelFailure {
    pub message: String,
    pub class: FailureClass,
    pub retriable: bool,
    pub provider_code: Option<String>,
    pub http_status: Option<u16>,
    pub provider_request_id: Option<String>,
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
    ProfileActivated {
        profile: ProfileSnapshot,
    },
    InputQueued {
        message_id: MessageId,
    },
    SteeringQueued {
        message_id: MessageId,
    },
    SteeringDelivered {
        message_ids: Vec<MessageId>,
    },
    SlashCommandCompleted {
        command_id: SlashCommandId,
        name: String,
        output: Option<String>,
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
    ToolExecutionCommitted {
        call_id: CallId,
        name: String,
        arguments: String,
    },
    ToolResult {
        call_id: CallId,
        content: Vec<ContentPart>,
    },
    Content {
        part: ContentPart,
    },
    ContextCompacted {
        evicted_count: usize,
        evicted_bytes: usize,
        summary_bytes: usize,
    },
    Usage(TokenUsage),
    CompletionMetadata {
        calls: Vec<CompletionCallMetadata>,
    },
    Completed {
        message_id: MessageId,
        duration_ms: u64,
        time_to_first_token_ms: Option<u64>,
    },
    Interrupted {
        cause: InterruptionCause,
    },
    Failed {
        failure: ModelFailure,
    },
    Yielded {
        call_id: CallId,
        payload: serde_json::Value,
    },
    HandedOff {
        call_id: CallId,
        profile: String,
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
    /// Lifecycle composition order: lower values run first, then plugin id.
    pub priority: i32,
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
    Commands,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PluginId;
    use serde::{Serialize, de::DeserializeOwned};

    fn round_trip<T>(value: &T)
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).unwrap();
        assert_eq!(serde_json::from_str::<T>(&json).unwrap(), *value);
    }

    #[test]
    fn public_commands_round_trip() {
        for command in [
            Command::Input {
                source: Source::User,
                content: "hello".into(),
            },
            Command::Steer {
                source: Source::Harness,
                content: "notice".into(),
            },
            Command::Abort {
                cause: InterruptionCause::Provider {
                    reason: "connection closed".into(),
                },
            },
            Command::Slash {
                name: "status".into(),
                arguments: "verbose".into(),
            },
        ] {
            round_trip(&command);
        }
        round_trip(&SlashCommandResult {
            output: Some("switched".into()),
            actions: vec![SlashCommandAction::ActivateProfile {
                profile: "worker".into(),
                brief: Some("continue".into()),
            }],
        });
    }

    #[test]
    fn streamed_events_and_plugin_descriptors_round_trip() {
        let event = StreamEvent {
            event_id: EventId::from("event"),
            session_id: SessionId::from("session"),
            run_id: Some(RunId::from("run")),
            sequence: 7,
            kind: StreamEventKind::Interrupted {
                cause: InterruptionCause::User,
            },
        };
        round_trip(&event);
        round_trip(&StreamEvent {
            event_id: EventId::from("slash-event"),
            session_id: SessionId::from("session"),
            run_id: None,
            sequence: 8,
            kind: StreamEventKind::SlashCommandCompleted {
                command_id: SlashCommandId::from("command"),
                name: "status".into(),
                output: Some("ready".into()),
            },
        });

        let descriptor = PluginDescriptor {
            id: PluginId::from("artist.prompt"),
            version: "0.3.0".into(),
            priority: 0,
            capabilities: vec![PluginCapability::Prompt],
        };
        round_trip(&descriptor);

        for outcome in [
            RunOutcome::Completed {
                message_id: MessageId::from("answer"),
            },
            RunOutcome::Failed {
                message_id: None,
                error: "provider failed".into(),
            },
            RunOutcome::Interrupted {
                message_id: MessageId::from("partial"),
                cause: InterruptionCause::User,
            },
        ] {
            round_trip(&outcome);
        }
    }
}
