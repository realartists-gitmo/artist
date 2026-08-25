use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    CallId, CorrelationId, EventId, EventSchemaId, MessageId, PluginId, ProfileSnapshot, RunId,
    SessionId, SlashCommandId,
};

pub const MAX_CONTENT_PARTS: usize = 4_096;
pub const MAX_PLUGIN_EVENT_BYTES: usize = 1_048_576;
pub const MAX_PLUGIN_EVENT_DEPTH: usize = 64;
pub const MAX_EVENT_SCHEMA_BYTES: usize = 262_144;

/// Immutable reference to host-addressed binary content. The canonical digest
/// is over the exact stored bytes; provider upload handles never belong here.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlobRef {
    pub algorithm: String,
    pub digest: String,
    pub byte_length: u64,
    pub media_type: String,
    pub logical_name: Option<String>,
}

impl BlobRef {
    pub fn sha256(
        digest: impl Into<String>,
        byte_length: u64,
        media_type: impl Into<String>,
    ) -> Self {
        Self {
            algorithm: "sha256".into(),
            digest: digest.into(),
            byte_length,
            media_type: media_type.into(),
            logical_name: None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.algorithm != "sha256"
            || self.digest.len() != 64
            || !self.digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("blob references require a 64-character SHA-256 digest".into());
        }
        if self.media_type.trim().is_empty() || self.media_type.len() > 255 {
            return Err("blob media type must contain 1..=255 bytes".into());
        }
        if self
            .logical_name
            .as_ref()
            .is_some_and(|name| name.is_empty() || name.len() > 4_096)
        {
            return Err("blob logical name must contain 1..=4096 bytes".into());
        }
        Ok(())
    }
}

/// A semantic binary attachment. `role` is namespaced vocabulary owned by the
/// producer (for example `artist.input`), not a closed kernel media enum.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub blob: BlobRef,
    pub role: String,
    pub alternate_text: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

/// Backend-neutral ordered content preserved across live events and durable
/// history. New modalities use `Attachment` or namespaced `Opaque` content;
/// they do not require a kernel enum variant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    Json {
        value: serde_json::Value,
    },
    Attachment {
        attachment: Attachment,
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

    pub fn attachment(blob: BlobRef, role: impl Into<String>) -> Self {
        Self::Attachment {
            attachment: Attachment {
                blob,
                role: role.into(),
                alternate_text: None,
                metadata: BTreeMap::new(),
            },
        }
    }
}

pub fn validate_content(content: &[ContentPart]) -> Result<(), String> {
    if content.len() > MAX_CONTENT_PARTS {
        return Err(format!(
            "content must contain at most {MAX_CONTENT_PARTS} parts"
        ));
    }
    for part in content {
        match part {
            ContentPart::Text { text } if text.len() > MAX_PLUGIN_EVENT_BYTES => {
                return Err("text content exceeds 1 MiB".into());
            }
            ContentPart::Attachment { attachment } => {
                attachment.blob.validate()?;
                if attachment.role.is_empty() || attachment.role.len() > 255 {
                    return Err("attachment role must contain 1..=255 bytes".into());
                }
            }
            ContentPart::Opaque { kind, .. } if kind.is_empty() || kind.len() > 255 => {
                return Err("opaque content kind must contain 1..=255 bytes".into());
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationScope {
    pub session_id: SessionId,
    pub run_id: Option<RunId>,
    pub call_id: Option<CallId>,
    pub correlation_id: CorrelationId,
    pub parent_correlation_id: Option<CorrelationId>,
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
    Input {
        source: Source,
        content: Vec<ContentPart>,
    },
    Steer {
        source: Source,
        content: Vec<ContentPart>,
    },
    Abort {
        cause: InterruptionCause,
    },
    Slash {
        name: String,
        arguments: String,
    },
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
        content: Vec<ContentPart>,
    },
    Steer {
        content: Vec<ContentPart>,
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
#[serde(deny_unknown_fields)]
pub struct ContentDerivation {
    pub source_blobs: Vec<BlobRef>,
    pub transformation: String,
    pub transformation_version: String,
    pub output_blob: BlobRef,
    pub provider_serialization: Option<String>,
    pub earliest_changed_sequence: u64,
    pub projection_scope: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionArtifact {
    pub content: Vec<ContentPart>,
    #[serde(default)]
    pub derivations: Vec<ContentDerivation>,
    pub projection_digest: String,
    pub earliest_changed_sequence: u64,
}

impl ProjectionArtifact {
    pub fn text(text: impl Into<String>, through_sequence: u64) -> Self {
        let text = text.into();
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Self {
            content: vec![ContentPart::text(text)],
            derivations: Vec::new(),
            projection_digest: digest,
            earliest_changed_sequence: through_sequence,
        }
    }
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginEventSchema {
    pub schema_id: EventSchemaId,
    pub plugin_id: PluginId,
    pub event_type: String,
    pub version: String,
    pub payload_schema: serde_json::Value,
    pub presentation_schema: serde_json::Value,
    pub schema_digest: String,
    #[serde(default)]
    pub presentation: serde_json::Value,
}

impl PluginEventSchema {
    pub fn canonical_digest(&self) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(&(
            &self.plugin_id,
            &self.event_type,
            &self.version,
            &self.payload_schema,
            &self.presentation_schema,
            &self.presentation,
        ))
        .map_err(|error| error.to_string())?;
        Ok(Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.event_type.starts_with(&format!("{}.", self.plugin_id))
            || self.event_type.len() > 255
            || self.version.is_empty()
            || self.version.len() > 64
        {
            return Err(
                "plugin event type must be namespaced by plugin ID and have a version".into(),
            );
        }
        if serde_json::to_vec(&self.payload_schema)
            .map_err(|error| error.to_string())?
            .len()
            > MAX_EVENT_SCHEMA_BYTES
            || serde_json::to_vec(&self.presentation_schema)
                .map_err(|error| error.to_string())?
                .len()
                > MAX_EVENT_SCHEMA_BYTES
        {
            return Err("plugin event schema exceeds 256 KiB".into());
        }
        jsonschema::validator_for(&self.payload_schema).map_err(|error| error.to_string())?;
        let presentation = jsonschema::validator_for(&self.presentation_schema)
            .map_err(|error| error.to_string())?;
        if !presentation.is_valid(&self.presentation) {
            return Err("default plugin event presentation does not match its schema".into());
        }
        Self::validate_presentation_namespacing(&self.presentation)?;
        if self.canonical_digest()? != self.schema_digest {
            return Err("plugin event schema digest mismatch".into());
        }
        Ok(())
    }

    /// Presentation metadata keys are namespaced so clients that do not
    /// understand a namespace can ignore it safely instead of guessing. Every
    /// top-level key must carry an explicit `<namespace>.<key>` form; the
    /// reserved `render` hint is the only bare key permitted.
    fn validate_presentation_namespacing(presentation: &serde_json::Value) -> Result<(), String> {
        const RESERVED: [&str; 1] = ["render"];
        let Some(object) = presentation.as_object() else {
            return Ok(());
        };
        for key in object.keys() {
            let namespaced = key.contains('.') && !key.starts_with('.') && !key.ends_with('.');
            if !namespaced && !RESERVED.contains(&key.as_str()) {
                return Err(format!(
                    "presentation metadata key `{key}` must be namespaced (`namespace.key`)"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginEvent {
    pub plugin_id: PluginId,
    pub schema_id: EventSchemaId,
    pub event_type: String,
    pub schema_version: String,
    pub schema_digest: String,
    pub scope: InvocationScope,
    pub payload: serde_json::Value,
    #[serde(default)]
    pub presentation: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "fact", rename_all = "snake_case")]
pub enum PluginFact {
    SchemaRegistered { schema: PluginEventSchema },
    Event { event: PluginEvent },
}

impl PluginEvent {
    pub fn validate_against(&self, schema: &PluginEventSchema) -> Result<(), String> {
        if self.plugin_id != schema.plugin_id
            || self.schema_id != schema.schema_id
            || self.event_type != schema.event_type
            || self.schema_version != schema.version
            || self.schema_digest != schema.schema_digest
        {
            return Err("plugin event does not match its registered schema identity".into());
        }
        let bytes = serde_json::to_vec(&(&self.payload, &self.presentation))
            .map_err(|error| error.to_string())?;
        if bytes.len() > MAX_PLUGIN_EVENT_BYTES {
            return Err("plugin event exceeds 1 MiB".into());
        }
        if json_depth(&self.payload) > MAX_PLUGIN_EVENT_DEPTH
            || json_depth(&self.presentation) > MAX_PLUGIN_EVENT_DEPTH
        {
            return Err("plugin event exceeds maximum JSON depth".into());
        }
        let payload =
            jsonschema::validator_for(&schema.payload_schema).map_err(|error| error.to_string())?;
        if !payload.is_valid(&self.payload) {
            return Err("plugin event payload does not match its schema".into());
        }
        let presentation = jsonschema::validator_for(&schema.presentation_schema)
            .map_err(|error| error.to_string())?;
        if !presentation.is_valid(&self.presentation) {
            return Err("plugin event presentation does not match its schema".into());
        }
        PluginEventSchema::validate_presentation_namespacing(&self.presentation)?;
        Ok(())
    }
}

fn json_depth(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(values) => {
            1 + values.iter().map(json_depth).max().unwrap_or_default()
        }
        serde_json::Value::Object(values) => {
            1 + values.values().map(json_depth).max().unwrap_or_default()
        }
        _ => 1,
    }
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
    PluginEventSchemaRegistered {
        schema: PluginEventSchema,
    },
    PluginEvent {
        plugin_event: PluginEvent,
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
    ToolProgress {
        progress: ToolProgress,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolProgress {
    pub scope: InvocationScope,
    pub sequence: u64,
    pub fraction: Option<f64>,
    pub message: Option<String>,
    #[serde(default)]
    pub detail: serde_json::Value,
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
    ModelConfig,
    ModelProvider,
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
                content: vec![ContentPart::text("hello")],
            },
            Command::Steer {
                source: Source::Harness,
                content: vec![ContentPart::text("notice")],
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
