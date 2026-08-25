wit_bindgen::generate!({
    path: "../../wit",
    world: "artist-plugin",
    pub_export_macro: true,
});

pub use schemars;
pub use serde;
pub use serde_json;

pub const MODEL_OUTPUT_BUDGET: usize = 64 * 1024;

pub fn default_yield_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["completed"],
        "properties": {
            "completed": {"type": "boolean"},
            "remainder": {"type": ["string", "null"]}
        }
    })
}

use artist::plugin::types::{
    ContentPart, MoveRequest, PollOutcome, ReadRequest, ResourceReply, ResourceRequest,
    ToolAnnotations, ToolDefinition, ToolEffect, ToolFailure, ToolSuccess, WriteRequest,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

pub fn definition<T: JsonSchema>(
    name: &str,
    description: &str,
    effects: Vec<ToolEffect>,
) -> ToolDefinition {
    let mutating = effects.iter().any(|effect| {
        matches!(
            effect,
            ToolEffect::Mutate
                | ToolEffect::Execute
                | ToolEffect::SessionControl
                | ToolEffect::Unknown
        )
    });
    let category = match name {
        "read" | "find" | "grep" => "resource-observation",
        "write" | "edit" | "move" => "resource-mutation",
        "run" | "signal" | "poll" => "resource-lifecycle",
        "yield" | "handoff" => "session-control",
        _ => "general",
    };
    let idempotent = matches!(name, "read" | "find" | "grep" | "write");
    let destructive = matches!(name, "write" | "edit" | "move");
    let open_world = matches!(name, "run" | "signal");
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        category: category.into(),
        input_schema: serde_json::to_string(&schemars::schema_for!(T))
            .expect("tool schema is serializable"),
        output_schema: output_schema(name).to_string(),
        effects,
        annotations: ToolAnnotations {
            read_only: !mutating,
            destructive,
            idempotent,
            open_world,
        },
    }
}

fn output_schema(name: &str) -> Value {
    match name {
        "read" => json!({
            "type": "object",
            "required": ["revision", "total_lines", "text", "truncated"],
            "properties": {
                "revision": {"type": "string"},
                "total_lines": {"type": "integer", "minimum": 0},
                "text": {"type": "string"},
                "truncated": {"type": "boolean"}
            }
        }),
        "write" => {
            json!({"type": "object", "required": ["written"], "properties": {"written": {"const": true}}})
        }
        "move" => {
            json!({"type": "object", "required": ["moved"], "properties": {"moved": {"const": true}}})
        }
        "signal" => {
            json!({"type": "object", "required": ["signaled"], "properties": {"signaled": {"const": true}}})
        }
        "run" => {
            json!({"type": "object", "required": ["uri"], "properties": {"uri": {"type": "string"}}})
        }
        "poll" => json!({
            "type": "object",
            "required": ["text", "next_cursor", "outcome"],
            "properties": {
                "text": {"type": "string"},
                "next_cursor": {"type": "string"},
                "outcome": {"enum": ["matched", "closed", "timed-out"]}
            }
        }),
        _ => json!({}),
    }
}

pub fn parse_arguments<T: for<'de> Deserialize<'de>>(value: &str) -> Result<T, String> {
    serde_json::from_str(value).map_err(|error| format!("invalid tool arguments: {error}"))
}

pub fn failure(code: &str, message: impl Into<String>) -> ToolFailure {
    ToolFailure {
        code: code.into(),
        message: message.into(),
        retriable: false,
        details: "null".into(),
        violations: Vec::new(),
        next_actions: Vec::new(),
    }
}

pub fn success(value: String) -> ToolSuccess {
    let parsed = serde_json::from_str::<Value>(&value).unwrap_or(Value::String(value));
    let encoded = parsed.to_string();
    ToolSuccess {
        value: encoded.clone(),
        content: vec![ContentPart::Json(encoded)],
        next_actions: Vec::new(),
    }
}

fn content_json(part: ContentPart) -> Value {
    match part {
        ContentPart::Text(text) => json!({"type": "text", "text": text}),
        ContentPart::Json(value) => json!({
            "type": "json",
            "value": serde_json::from_str::<Value>(&value).unwrap_or(Value::String(value)),
        }),
        ContentPart::Attachment(attachment) => json!({
            "type": "attachment",
            "blob": {
                "algorithm": attachment.blob.algorithm,
                "digest": attachment.blob.digest,
                "byte_length": attachment.blob.byte_length,
                "media_type": attachment.blob.media_type,
                "logical_name": attachment.blob.logical_name,
            },
            "role": attachment.role,
            "alternate_text": attachment.alternate_text,
            "metadata": attachment.metadata.into_iter().map(|entry| (entry.key, entry.value)).collect::<std::collections::BTreeMap<_, _>>(),
        }),
        ContentPart::Reasoning(value) => json!({"type": "reasoning", "value": value}),
        ContentPart::Opaque(value) => {
            json!({"type": "opaque", "kind": value.kind, "value": value.value})
        }
    }
}

pub fn resource_reply(reply: ResourceReply) -> String {
    match reply {
        ResourceReply::Text(text) => json!({"text": text}),
        ResourceReply::Content(content) => json!({
            "content": content.into_iter().map(content_json).collect::<Vec<_>>()
        }),
        ResourceReply::Children(children) => json!({"children": children}),
        ResourceReply::Written => json!({"written": true}),
        ResourceReply::Edited(reply) => json!({"revision": reply.revision}),
        ResourceReply::Moved => json!({"moved": true}),
        ResourceReply::Started(uri) => json!({"uri": uri}),
        ResourceReply::Signaled => json!({"signaled": true}),
        ResourceReply::Poll(reply) => json!({
            "text": reply.text,
            "next_cursor": reply.next_cursor,
            "outcome": match reply.outcome {
                PollOutcome::Matched => "matched",
                PollOutcome::Closed => "closed",
                PollOutcome::TimedOut => "timed-out",
            }
        }),
    }
    .to_string()
}

pub fn call_resource(request: ResourceRequest) -> Result<String, String> {
    artist::plugin::host_resources::handle(&request)
        .map(resource_reply)
        .map_err(resource_error)
}

/// Read a durable scoped document. `scope` is a URI path such as `global`,
/// `account/acme`, or `session/s-1`; the full URI is `store:///<scope>/<key>`.
/// Missing documents read as `None`; every other error is fatal.
pub fn durable_get(scope: &str, key: &str) -> Result<Option<String>, String> {
    match call_resource(ResourceRequest::Read(ReadRequest {
        uri: durable_uri(scope, key),
        start_line: None,
        line_count: None,
    })) {
        Ok(reply) => {
            let value: crate::serde_json::Value = parse_reply(&reply)?;
            Ok(Some(
                value
                    .get("text")
                    .and_then(|text| text.as_str())
                    .unwrap_or_default()
                    .to_string(),
            ))
        }
        Err(error) if error.contains("not found") => Ok(None),
        Err(error) => Err(error),
    }
}

/// Write a durable scoped document (unconditional; revisions are available
/// through raw resource requests with expected hashes).
pub fn durable_put(scope: &str, key: &str, value: &str) -> Result<(), String> {
    call_resource(ResourceRequest::Write(WriteRequest {
        uri: durable_uri(scope, key),
        text: value.to_string(),
    }))
    .map(|_| ())
}

/// Delete a durable scoped document.
pub fn durable_delete(scope: &str, key: &str) -> Result<(), String> {
    call_resource(ResourceRequest::Move(MoveRequest {
        source: durable_uri(scope, key),
        to: None,
    }))
    .map(|_| ())
}

/// Specification for spawning a related session through the host service.
pub struct ChildSpec {
    pub request_id: String,
    pub session_id: String,
    pub profile: String,
    /// Attached children are cancelled when the parent stops; detached ones
    /// keep running independently. There is no default.
    pub attached: bool,
    /// `remain-interrupted`, `resume-queued-work`, or `plugin-resolved`.
    pub recovery: String,
    pub relationship: String,
    pub input: String,
}

/// Create a related session, deliver one input, and await its terminal
/// outcome. Returns the outcome JSON emitted by the host.
pub fn spawn_child(spec: ChildSpec) -> Result<String, String> {
    let session_id =
        artist::plugin::host_sessions::create(&artist::plugin::host_sessions::CreateRequest {
            request_id: spec.request_id,
            session_id: spec.session_id,
            profile: spec.profile,
            content: vec![artist::plugin::types::ContentPart::Text(spec.input)],
            attachment: if spec.attached {
                artist::plugin::host_sessions::Attachment::Attached
            } else {
                artist::plugin::host_sessions::Attachment::Detached
            },
            recovery: match spec.recovery.as_str() {
                "resume-queued-work" => {
                    artist::plugin::host_sessions::RecoveryPolicy::ResumeQueuedWork
                }
                "plugin-resolved" => artist::plugin::host_sessions::RecoveryPolicy::PluginResolved,
                _ => artist::plugin::host_sessions::RecoveryPolicy::RemainInterrupted,
            },
            relationship: spec.relationship,
        })?;
    artist::plugin::host_sessions::send(
        &session_id,
        &[artist::plugin::types::ContentPart::Text("begin".into())],
    )?;
    let outcome = artist::plugin::host_sessions::await_terminal(&session_id)?;
    Ok(serde_json::json!({"child": session_id, "outcome": outcome}).to_string())
}

fn durable_uri(scope: &str, key: &str) -> String {
    format!("store:///{}/{key}", scope.trim_matches('/'))
}

fn parse_reply(reply: &str) -> Result<crate::serde_json::Value, String> {
    crate::serde_json::from_str(reply).map_err(|error| format!("invalid resource reply: {error}"))
}

/// Process/provider-lifetime scratch state. It is intentionally not durable
/// across host restart; durable domain state belongs in scoped resources.
pub fn register_event_schema(
    schema_id: &str,
    event_type: &str,
    version: &str,
    payload_schema: Value,
    presentation_schema: Value,
    presentation: Value,
) -> Result<String, String> {
    artist::plugin::host_events::register_schema(&artist::plugin::host_events::EventSchema {
        schema_id: schema_id.into(),
        event_type: event_type.into(),
        version: version.into(),
        payload_schema: payload_schema.to_string(),
        presentation_schema: presentation_schema.to_string(),
        presentation: presentation.to_string(),
    })
}

pub struct EventEmission {
    pub schema_id: String,
    pub event_type: String,
    pub schema_version: String,
    pub schema_digest: String,
    pub scope: artist::plugin::types::InvocationScope,
    pub payload: Value,
    pub presentation: Value,
    pub durable: bool,
}

pub fn emit_event(emission: EventEmission) -> Result<(), String> {
    artist::plugin::host_events::emit(&artist::plugin::host_events::EventRequest {
        schema_id: emission.schema_id,
        event_type: emission.event_type,
        schema_version: emission.schema_version,
        schema_digest: emission.schema_digest,
        scope: emission.scope,
        payload: emission.payload.to_string(),
        presentation: emission.presentation.to_string(),
        durable: emission.durable,
    })
}

pub fn emit_progress(
    scope: artist::plugin::types::InvocationScope,
    sequence: u64,
    fraction: Option<f64>,
    message: Option<String>,
    detail: Value,
) -> Result<(), String> {
    artist::plugin::host_progress::emit(&artist::plugin::host_progress::Progress {
        scope,
        sequence,
        fraction,
        message,
        detail: detail.to_string(),
    })
}

pub fn provider_state_get(key: &str) -> Result<Option<String>, String> {
    artist::plugin::provider_state::get(key)
}
pub fn provider_state_set(key: &str, value: &str) -> Result<(), String> {
    artist::plugin::provider_state::set(key, value)
}
pub fn provider_state_delete(key: &str) -> Result<(), String> {
    artist::plugin::provider_state::delete(key)
}
pub fn provider_state_compare_and_swap(
    key: &str,
    expected: Option<&str>,
    value: Option<&str>,
) -> Result<bool, String> {
    artist::plugin::provider_state::compare_and_swap(key, expected, value)
}

pub fn resource_error(error: artist::plugin::types::ResourceError) -> String {
    use artist::plugin::types::ResourceError;
    match error {
        ResourceError::NotFound(error) => {
            format!("no {:?} resource route for {}", error.operation, error.uri)
        }
        ResourceError::Unsupported(error) => {
            format!("{:?} is unsupported on {}", error.operation, error.uri)
        }
        ResourceError::Conflict(error) => format!(
            "resource conflict on {}; current revision is {}",
            error.uri, error.current_revision
        ),
        ResourceError::Invalid(message) => format!("invalid resource request: {message}"),
        ResourceError::Provider(message) => format!("resource provider failed: {message}"),
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! unadvertised_tools {
    ($plugin:ident) => {
        impl $crate::exports::artist::plugin::tool_provider::Guest for $plugin {
            fn definitions() -> Result<Vec<$crate::artist::plugin::types::ToolDefinition>, String> {
                Err("tool socket is not advertised".into())
            }
            fn invoke(
                _: String,
                _: String,
            ) -> Result<
                $crate::artist::plugin::types::ToolSuccess,
                $crate::artist::plugin::types::ToolFailure,
            > {
                Err($crate::failure(
                    "not-advertised",
                    "tool socket is not advertised",
                ))
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! unadvertised_model_provider {
    ($plugin:ident) => {
        impl $crate::exports::artist::plugin::model_provider::Guest for $plugin {
            fn descriptors() -> Result<
                Vec<$crate::exports::artist::plugin::model_provider::ProviderDescriptor>,
                String,
            > {
                Err("model-provider socket is not advertised".into())
            }
            fn start(_: String) -> Result<String, $crate::artist::plugin::types::ToolFailure> {
                Err($crate::failure(
                    "not-advertised",
                    "model-provider socket is not advertised",
                ))
            }
            fn poll(
                _: String,
            ) -> Result<
                $crate::exports::artist::plugin::model_provider::StreamItem,
                $crate::artist::plugin::types::ToolFailure,
            > {
                Err($crate::failure(
                    "not-advertised",
                    "model-provider socket is not advertised",
                ))
            }
            fn cancel(_: String) -> Result<(), $crate::artist::plugin::types::ToolFailure> {
                Err($crate::failure(
                    "not-advertised",
                    "model-provider socket is not advertised",
                ))
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! unadvertised_resources {
    ($plugin:ident) => {
        impl $crate::exports::artist::plugin::resource_provider::Guest for $plugin {
            fn routes() -> Result<Vec<$crate::artist::plugin::types::ResourceRoute>, String> {
                Err("resource socket is not advertised".into())
            }
            fn handle(
                _: $crate::artist::plugin::types::ResourceRequest,
            ) -> Result<
                $crate::artist::plugin::types::ResourceReply,
                $crate::artist::plugin::types::ResourceError,
            > {
                Err($crate::artist::plugin::types::ResourceError::Unsupported(
                    $crate::artist::plugin::types::RouteError {
                        uri: "unknown:/".into(),
                        operation: $crate::artist::plugin::types::ResourceOperation::Read,
                    },
                ))
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! unadvertised_slash_commands {
    ($plugin:ident) => {
        impl $crate::exports::artist::plugin::slash_command_provider::Guest for $plugin {
            fn definitions()
            -> Result<Vec<$crate::artist::plugin::types::SlashCommandDefinition>, String> {
                Err("slash-command socket is not advertised".into())
            }
            fn invoke(
                _: String,
                _: String,
            ) -> Result<$crate::artist::plugin::types::SlashCommandResult, String> {
                Err("slash-command socket is not advertised".into())
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! lifecycle_stubs {
    ($plugin:ident, $id:literal, $priority:expr, $capability:expr) => {
        fn descriptor() -> $crate::artist::plugin::types::PluginDescriptor {
            $crate::artist::plugin::types::PluginDescriptor {
                id: $id.into(),
                version: env!("CARGO_PKG_VERSION").into(),
                priority: $priority,
                capabilities: vec![$capability],
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! lifecycle_unadvertised_methods {
    () => {
        fn compose_prompt(
            _: Vec<$crate::artist::plugin::types::ContextFragment>,
        ) -> Result<Vec<$crate::artist::plugin::types::ContextFragment>, String> {
            Err("prompt socket is not advertised".into())
        }
        fn transform_context(
            _: $crate::artist::plugin::types::ModelContext,
        ) -> Result<$crate::artist::plugin::types::ModelContext, String> {
            Err("context socket is not advertised".into())
        }
        fn compact_context(
            _: $crate::artist::plugin::types::ModelContext,
        ) -> Result<Option<$crate::artist::plugin::types::CompactionArtifact>, String> {
            Ok(None)
        }
        fn observe_hook(
            _: $crate::artist::plugin::types::HookEvent,
        ) -> Result<$crate::artist::plugin::types::HookDecision, String> {
            Err("hook socket is not advertised".into())
        }
        fn configure_model(
            _: $crate::artist::plugin::types::ModelConfig,
        ) -> Result<$crate::artist::plugin::types::ModelConfig, String> {
            Err("model-config socket is not advertised".into())
        }
        fn observe_event(_: String) -> Result<(), String> {
            Err("event socket is not advertised".into())
        }
    };
}

#[macro_export]
macro_rules! prompt_component {
    ($plugin:ident, $id:literal, $priority:expr, $compose:path) => {
        impl $crate::exports::artist::plugin::lifecycle::Guest for $plugin {
            $crate::lifecycle_stubs!($plugin, $id, $priority, $crate::artist::plugin::types::Capability::Prompt);
            fn compose_prompt(fragments: Vec<$crate::artist::plugin::types::ContextFragment>) -> Result<Vec<$crate::artist::plugin::types::ContextFragment>, String> { $compose(fragments) }
            fn transform_context(_: $crate::artist::plugin::types::ModelContext) -> Result<$crate::artist::plugin::types::ModelContext, String> { Err("context socket is not advertised".into()) }
            fn compact_context(_: $crate::artist::plugin::types::ModelContext) -> Result<Option<$crate::artist::plugin::types::CompactionArtifact>, String> { Ok(None) }
            fn observe_hook(_: $crate::artist::plugin::types::HookEvent) -> Result<$crate::artist::plugin::types::HookDecision, String> { Err("hook socket is not advertised".into()) }
            fn configure_model(_: $crate::artist::plugin::types::ModelConfig) -> Result<$crate::artist::plugin::types::ModelConfig, String> { Err("model-config socket is not advertised".into()) }
            fn observe_event(_: String) -> Result<(), String> { Err("event socket is not advertised".into()) }
        }
        $crate::unadvertised_model_provider!($plugin);
        $crate::unadvertised_tools!($plugin); $crate::unadvertised_resources!($plugin); $crate::unadvertised_slash_commands!($plugin);
        $crate::export!($plugin with_types_in $crate);
    };
}

#[macro_export]
macro_rules! context_component {
    ($plugin:ident, $id:literal, $priority:expr, $transform:path) => {
        impl $crate::exports::artist::plugin::lifecycle::Guest for $plugin {
            $crate::lifecycle_stubs!($plugin, $id, $priority, $crate::artist::plugin::types::Capability::Context);
            fn compose_prompt(_: Vec<$crate::artist::plugin::types::ContextFragment>) -> Result<Vec<$crate::artist::plugin::types::ContextFragment>, String> { Err("prompt socket is not advertised".into()) }
            fn transform_context(context: $crate::artist::plugin::types::ModelContext) -> Result<$crate::artist::plugin::types::ModelContext, String> { $transform(context) }
            fn compact_context(_: $crate::artist::plugin::types::ModelContext) -> Result<Option<$crate::artist::plugin::types::CompactionArtifact>, String> { Ok(None) }
            fn observe_hook(_: $crate::artist::plugin::types::HookEvent) -> Result<$crate::artist::plugin::types::HookDecision, String> { Err("hook socket is not advertised".into()) }
            fn configure_model(_: $crate::artist::plugin::types::ModelConfig) -> Result<$crate::artist::plugin::types::ModelConfig, String> { Err("model-config socket is not advertised".into()) }
            fn observe_event(_: String) -> Result<(), String> { Err("event socket is not advertised".into()) }
        }
        $crate::unadvertised_model_provider!($plugin);
        $crate::unadvertised_tools!($plugin); $crate::unadvertised_resources!($plugin); $crate::unadvertised_slash_commands!($plugin);
        $crate::export!($plugin with_types_in $crate);
    };
}

#[macro_export]
macro_rules! hooks_component {
    ($plugin:ident, $id:literal, $priority:expr, $observe:path) => {
        impl $crate::exports::artist::plugin::lifecycle::Guest for $plugin {
            $crate::lifecycle_stubs!($plugin, $id, $priority, $crate::artist::plugin::types::Capability::Hooks);
            fn compose_prompt(_: Vec<$crate::artist::plugin::types::ContextFragment>) -> Result<Vec<$crate::artist::plugin::types::ContextFragment>, String> { Err("prompt socket is not advertised".into()) }
            fn transform_context(_: $crate::artist::plugin::types::ModelContext) -> Result<$crate::artist::plugin::types::ModelContext, String> { Err("context socket is not advertised".into()) }
            fn compact_context(_: $crate::artist::plugin::types::ModelContext) -> Result<Option<$crate::artist::plugin::types::CompactionArtifact>, String> { Ok(None) }
            fn observe_hook(event: $crate::artist::plugin::types::HookEvent) -> Result<$crate::artist::plugin::types::HookDecision, String> { $observe(event) }
            fn configure_model(_: $crate::artist::plugin::types::ModelConfig) -> Result<$crate::artist::plugin::types::ModelConfig, String> { Err("model-config socket is not advertised".into()) }
            fn observe_event(_: String) -> Result<(), String> { Err("event socket is not advertised".into()) }
        }
        $crate::unadvertised_model_provider!($plugin);
        $crate::unadvertised_tools!($plugin); $crate::unadvertised_resources!($plugin); $crate::unadvertised_slash_commands!($plugin);
        $crate::export!($plugin with_types_in $crate);
    };
}

#[macro_export]
macro_rules! model_component {
    ($plugin:ident, $id:literal, $priority:expr, $configure:path) => {
        impl $crate::exports::artist::plugin::lifecycle::Guest for $plugin {
            $crate::lifecycle_stubs!($plugin, $id, $priority, $crate::artist::plugin::types::Capability::ModelConfig);
            fn compose_prompt(_: Vec<$crate::artist::plugin::types::ContextFragment>) -> Result<Vec<$crate::artist::plugin::types::ContextFragment>, String> { Err("prompt socket is not advertised".into()) }
            fn transform_context(_: $crate::artist::plugin::types::ModelContext) -> Result<$crate::artist::plugin::types::ModelContext, String> { Err("context socket is not advertised".into()) }
            fn compact_context(_: $crate::artist::plugin::types::ModelContext) -> Result<Option<$crate::artist::plugin::types::CompactionArtifact>, String> { Ok(None) }
            fn observe_hook(_: $crate::artist::plugin::types::HookEvent) -> Result<$crate::artist::plugin::types::HookDecision, String> { Err("hook socket is not advertised".into()) }
            fn configure_model(config: $crate::artist::plugin::types::ModelConfig) -> Result<$crate::artist::plugin::types::ModelConfig, String> { $configure(config) }
            fn observe_event(_: String) -> Result<(), String> { Err("event socket is not advertised".into()) }
        }
        $crate::unadvertised_model_provider!($plugin);
        $crate::unadvertised_tools!($plugin); $crate::unadvertised_resources!($plugin); $crate::unadvertised_slash_commands!($plugin);
        $crate::export!($plugin with_types_in $crate);
    };
}

#[macro_export]
macro_rules! events_component {
    ($plugin:ident, $id:literal, $priority:expr, $observe:path) => {
        impl $crate::exports::artist::plugin::lifecycle::Guest for $plugin {
            $crate::lifecycle_stubs!($plugin, $id, $priority, $crate::artist::plugin::types::Capability::Events);
            fn compose_prompt(_: Vec<$crate::artist::plugin::types::ContextFragment>) -> Result<Vec<$crate::artist::plugin::types::ContextFragment>, String> { Err("prompt socket is not advertised".into()) }
            fn transform_context(_: $crate::artist::plugin::types::ModelContext) -> Result<$crate::artist::plugin::types::ModelContext, String> { Err("context socket is not advertised".into()) }
            fn compact_context(_: $crate::artist::plugin::types::ModelContext) -> Result<Option<$crate::artist::plugin::types::CompactionArtifact>, String> { Ok(None) }
            fn observe_hook(_: $crate::artist::plugin::types::HookEvent) -> Result<$crate::artist::plugin::types::HookDecision, String> { Err("hook socket is not advertised".into()) }
            fn configure_model(_: $crate::artist::plugin::types::ModelConfig) -> Result<$crate::artist::plugin::types::ModelConfig, String> { Err("model-config socket is not advertised".into()) }
            fn observe_event(event: String) -> Result<(), String> { $observe(event) }
        }
        $crate::unadvertised_model_provider!($plugin);
        $crate::unadvertised_tools!($plugin); $crate::unadvertised_resources!($plugin); $crate::unadvertised_slash_commands!($plugin);
        $crate::export!($plugin with_types_in $crate);
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! unadvertised_lifecycle {
    ($plugin:ident, $id:literal, $capability:expr) => {
        impl $crate::exports::artist::plugin::lifecycle::Guest for $plugin {
            $crate::lifecycle_stubs!($plugin, $id, 0, $capability);
            $crate::lifecycle_unadvertised_methods!();
        }
    };
}

#[macro_export]
macro_rules! tool_component {
    ($plugin:ident, $id:literal, $args:ty, $name:literal, $description:literal, $effects:expr, $invoke:path) => {
        $crate::unadvertised_lifecycle!($plugin, $id, $crate::artist::plugin::types::Capability::Tools);
        $crate::unadvertised_model_provider!($plugin);
        impl $crate::exports::artist::plugin::tool_provider::Guest for $plugin {
            fn definitions() -> Result<Vec<$crate::artist::plugin::types::ToolDefinition>, String> {
                Ok(vec![$crate::definition::<$args>($name, $description, $effects)])
            }
            fn invoke(name: String, arguments: String) -> Result<$crate::artist::plugin::types::ToolSuccess, $crate::artist::plugin::types::ToolFailure> {
                if name != $name { return Err($crate::failure("unknown-tool", format!("unknown tool: {name}"))); }
                let args = $crate::parse_arguments::<$args>(&arguments).map_err(|message| $crate::failure("invalid-arguments", message))?;
                $invoke(args).map($crate::success).map_err(|message| $crate::failure("tool-failed", message))
            }
        }
        $crate::unadvertised_resources!($plugin); $crate::unadvertised_slash_commands!($plugin);
        $crate::export!($plugin with_types_in $crate);
    };
}

#[macro_export]
macro_rules! resource_component {
    ($plugin:ident, $id:literal, $routes:expr, $handle:path) => {
        $crate::unadvertised_lifecycle!($plugin, $id, $crate::artist::plugin::types::Capability::Resources);
        $crate::unadvertised_model_provider!($plugin); $crate::unadvertised_tools!($plugin); $crate::unadvertised_slash_commands!($plugin);
        impl $crate::exports::artist::plugin::resource_provider::Guest for $plugin {
            fn routes() -> Result<Vec<$crate::artist::plugin::types::ResourceRoute>, String> { Ok($routes) }
            fn handle(request: $crate::artist::plugin::types::ResourceRequest) -> Result<$crate::artist::plugin::types::ResourceReply, $crate::artist::plugin::types::ResourceError> { $handle(request) }
        }
        $crate::export!($plugin with_types_in $crate);
    };
}

#[macro_export]
macro_rules! slash_command_component {
    ($plugin:ident, $id:literal, $definitions:path, $invoke:path) => {
        $crate::unadvertised_lifecycle!($plugin, $id, $crate::artist::plugin::types::Capability::Commands);
        $crate::unadvertised_model_provider!($plugin); $crate::unadvertised_tools!($plugin); $crate::unadvertised_resources!($plugin);
        impl $crate::exports::artist::plugin::slash_command_provider::Guest for $plugin {
            fn definitions() -> Result<Vec<$crate::artist::plugin::types::SlashCommandDefinition>, String> { $definitions() }
            fn invoke(name: String, arguments: String) -> Result<$crate::artist::plugin::types::SlashCommandResult, String> { $invoke(name, arguments) }
        }
        $crate::export!($plugin with_types_in $crate);
    };
}
