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

use artist::plugin::types::{PollOutcome, ResourceReply, ResourceRequest, ToolDefinition};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

pub fn definition<T: JsonSchema>(
    name: &str,
    description: &str,
    effects: Vec<artist::plugin::types::ToolEffect>,
) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema: serde_json::to_string(&schemars::schema_for!(T))
            .expect("tool schema is serializable"),
        effects,
    }
}

pub fn parse_arguments<T: for<'de> Deserialize<'de>>(value: &str) -> Result<T, String> {
    serde_json::from_str(value).map_err(|error| format!("invalid tool arguments: {error}"))
}

pub fn resource_reply(reply: ResourceReply) -> String {
    match reply {
        ResourceReply::Text(text) => json!({"text": text}),
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

pub fn provider_state_get(key: &str) -> Result<Option<String>, String> {
    artist::plugin::provider_state::get(key)
}

pub fn provider_state_set(key: &str, value: &str) -> Result<(), String> {
    artist::plugin::provider_state::set(key, value)
}

pub fn provider_state_delete(key: &str) -> Result<(), String> {
    artist::plugin::provider_state::delete(key)
}

/// Atomically replace a state value only when it still equals `expected`.
/// `None` represents an absent key and may also be used to delete it.
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

            fn invoke(_: String, _: String) -> Result<String, String> {
                Err("tool socket is not advertised".into())
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

#[macro_export]
macro_rules! prompt_component {
    ($plugin:ident, $id:literal, $priority:expr, $compose:path) => {
        impl $crate::exports::artist::plugin::lifecycle::Guest for $plugin {
            $crate::lifecycle_stubs!($plugin, $id, $priority, $crate::artist::plugin::types::Capability::Prompt);
            fn compose_prompt(fragments: Vec<$crate::artist::plugin::types::ContextFragment>) -> Result<Vec<$crate::artist::plugin::types::ContextFragment>, String> { $compose(fragments) }
            fn transform_context(_: Vec<$crate::artist::plugin::types::Message>) -> Result<Vec<$crate::artist::plugin::types::Message>, String> { Err("context socket is not advertised".into()) }
            fn observe_hook(_: $crate::artist::plugin::types::HookEvent) -> Result<$crate::artist::plugin::types::HookDecision, String> { Err("hook socket is not advertised".into()) }
            fn configure_model(_: $crate::artist::plugin::types::ModelConfig) -> Result<$crate::artist::plugin::types::ModelConfig, String> { Err("model socket is not advertised".into()) }
            fn observe_event(_: String) -> Result<(), String> { Err("event socket is not advertised".into()) }
        }
        $crate::unadvertised_tools!($plugin);
        $crate::unadvertised_resources!($plugin);
        $crate::unadvertised_slash_commands!($plugin);
        $crate::export!($plugin with_types_in $crate);
    };
}

#[macro_export]
macro_rules! context_component {
    ($plugin:ident, $id:literal, $priority:expr, $transform:path) => {
        impl $crate::exports::artist::plugin::lifecycle::Guest for $plugin {
            $crate::lifecycle_stubs!($plugin, $id, $priority, $crate::artist::plugin::types::Capability::Context);
            fn compose_prompt(_: Vec<$crate::artist::plugin::types::ContextFragment>) -> Result<Vec<$crate::artist::plugin::types::ContextFragment>, String> { Err("prompt socket is not advertised".into()) }
            fn transform_context(messages: Vec<$crate::artist::plugin::types::Message>) -> Result<Vec<$crate::artist::plugin::types::Message>, String> { $transform(messages) }
            fn observe_hook(_: $crate::artist::plugin::types::HookEvent) -> Result<$crate::artist::plugin::types::HookDecision, String> { Err("hook socket is not advertised".into()) }
            fn configure_model(_: $crate::artist::plugin::types::ModelConfig) -> Result<$crate::artist::plugin::types::ModelConfig, String> { Err("model socket is not advertised".into()) }
            fn observe_event(_: String) -> Result<(), String> { Err("event socket is not advertised".into()) }
        }
        $crate::unadvertised_tools!($plugin);
        $crate::unadvertised_resources!($plugin);
        $crate::unadvertised_slash_commands!($plugin);
        $crate::export!($plugin with_types_in $crate);
    };
}

#[macro_export]
macro_rules! hooks_component {
    ($plugin:ident, $id:literal, $priority:expr, $observe:path) => {
        impl $crate::exports::artist::plugin::lifecycle::Guest for $plugin {
            $crate::lifecycle_stubs!($plugin, $id, $priority, $crate::artist::plugin::types::Capability::Hooks);
            fn compose_prompt(_: Vec<$crate::artist::plugin::types::ContextFragment>) -> Result<Vec<$crate::artist::plugin::types::ContextFragment>, String> { Err("prompt socket is not advertised".into()) }
            fn transform_context(_: Vec<$crate::artist::plugin::types::Message>) -> Result<Vec<$crate::artist::plugin::types::Message>, String> { Err("context socket is not advertised".into()) }
            fn observe_hook(event: $crate::artist::plugin::types::HookEvent) -> Result<$crate::artist::plugin::types::HookDecision, String> { $observe(event) }
            fn configure_model(_: $crate::artist::plugin::types::ModelConfig) -> Result<$crate::artist::plugin::types::ModelConfig, String> { Err("model socket is not advertised".into()) }
            fn observe_event(_: String) -> Result<(), String> { Err("event socket is not advertised".into()) }
        }
        $crate::unadvertised_tools!($plugin);
        $crate::unadvertised_resources!($plugin);
        $crate::unadvertised_slash_commands!($plugin);
        $crate::export!($plugin with_types_in $crate);
    };
}

#[macro_export]
macro_rules! model_component {
    ($plugin:ident, $id:literal, $priority:expr, $configure:path) => {
        impl $crate::exports::artist::plugin::lifecycle::Guest for $plugin {
            $crate::lifecycle_stubs!($plugin, $id, $priority, $crate::artist::plugin::types::Capability::Model);
            fn compose_prompt(_: Vec<$crate::artist::plugin::types::ContextFragment>) -> Result<Vec<$crate::artist::plugin::types::ContextFragment>, String> { Err("prompt socket is not advertised".into()) }
            fn transform_context(_: Vec<$crate::artist::plugin::types::Message>) -> Result<Vec<$crate::artist::plugin::types::Message>, String> { Err("context socket is not advertised".into()) }
            fn observe_hook(_: $crate::artist::plugin::types::HookEvent) -> Result<$crate::artist::plugin::types::HookDecision, String> { Err("hook socket is not advertised".into()) }
            fn configure_model(config: $crate::artist::plugin::types::ModelConfig) -> Result<$crate::artist::plugin::types::ModelConfig, String> { $configure(config) }
            fn observe_event(_: String) -> Result<(), String> { Err("event socket is not advertised".into()) }
        }
        $crate::unadvertised_tools!($plugin);
        $crate::unadvertised_resources!($plugin);
        $crate::unadvertised_slash_commands!($plugin);
        $crate::export!($plugin with_types_in $crate);
    };
}

#[macro_export]
macro_rules! events_component {
    ($plugin:ident, $id:literal, $priority:expr, $observe:path) => {
        impl $crate::exports::artist::plugin::lifecycle::Guest for $plugin {
            $crate::lifecycle_stubs!($plugin, $id, $priority, $crate::artist::plugin::types::Capability::Events);
            fn compose_prompt(_: Vec<$crate::artist::plugin::types::ContextFragment>) -> Result<Vec<$crate::artist::plugin::types::ContextFragment>, String> { Err("prompt socket is not advertised".into()) }
            fn transform_context(_: Vec<$crate::artist::plugin::types::Message>) -> Result<Vec<$crate::artist::plugin::types::Message>, String> { Err("context socket is not advertised".into()) }
            fn observe_hook(_: $crate::artist::plugin::types::HookEvent) -> Result<$crate::artist::plugin::types::HookDecision, String> { Err("hook socket is not advertised".into()) }
            fn configure_model(_: $crate::artist::plugin::types::ModelConfig) -> Result<$crate::artist::plugin::types::ModelConfig, String> { Err("model socket is not advertised".into()) }
            fn observe_event(event: String) -> Result<(), String> { $observe(event) }
        }
        $crate::unadvertised_tools!($plugin);
        $crate::unadvertised_resources!($plugin);
        $crate::unadvertised_slash_commands!($plugin);
        $crate::export!($plugin with_types_in $crate);
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! unadvertised_lifecycle {
    ($plugin:ident, $id:literal, $capability:expr) => {
        impl $crate::exports::artist::plugin::lifecycle::Guest for $plugin {
            $crate::lifecycle_stubs!($plugin, $id, 0, $capability);
            fn compose_prompt(
                _: Vec<$crate::artist::plugin::types::ContextFragment>,
            ) -> Result<Vec<$crate::artist::plugin::types::ContextFragment>, String> {
                Err("prompt socket is not advertised".into())
            }
            fn transform_context(
                _: Vec<$crate::artist::plugin::types::Message>,
            ) -> Result<Vec<$crate::artist::plugin::types::Message>, String> {
                Err("context socket is not advertised".into())
            }
            fn observe_hook(
                _: $crate::artist::plugin::types::HookEvent,
            ) -> Result<$crate::artist::plugin::types::HookDecision, String> {
                Err("hook socket is not advertised".into())
            }
            fn configure_model(
                _: $crate::artist::plugin::types::ModelConfig,
            ) -> Result<$crate::artist::plugin::types::ModelConfig, String> {
                Err("model socket is not advertised".into())
            }
            fn observe_event(_: String) -> Result<(), String> {
                Err("event socket is not advertised".into())
            }
        }
    };
}

#[macro_export]
macro_rules! tool_component {
    ($plugin:ident, $id:literal, $args:ty, $name:literal, $description:literal, $effects:expr, $invoke:path) => {
        $crate::unadvertised_lifecycle!($plugin, $id, $crate::artist::plugin::types::Capability::Tools);
        impl $crate::exports::artist::plugin::tool_provider::Guest for $plugin {
            fn definitions() -> Result<Vec<$crate::artist::plugin::types::ToolDefinition>, String> {
                Ok(vec![$crate::definition::<$args>($name, $description, $effects)])
            }
            fn invoke(name: String, arguments: String) -> Result<String, String> {
                if name != $name { return Err(format!("unknown tool: {name}")); }
                $invoke($crate::parse_arguments::<$args>(&arguments)?)
            }
        }
        $crate::unadvertised_resources!($plugin);
        $crate::unadvertised_slash_commands!($plugin);
        $crate::export!($plugin with_types_in $crate);
    };
}

#[macro_export]
macro_rules! resource_component {
    ($plugin:ident, $id:literal, $routes:expr, $handle:path) => {
        $crate::unadvertised_lifecycle!($plugin, $id, $crate::artist::plugin::types::Capability::Resources);
        $crate::unadvertised_tools!($plugin);
        $crate::unadvertised_slash_commands!($plugin);
        impl $crate::exports::artist::plugin::resource_provider::Guest for $plugin {
            fn routes() -> Result<Vec<$crate::artist::plugin::types::ResourceRoute>, String> {
                Ok($routes)
            }
            fn handle(request: $crate::artist::plugin::types::ResourceRequest) -> Result<$crate::artist::plugin::types::ResourceReply, $crate::artist::plugin::types::ResourceError> {
                $handle(request)
            }
        }
        $crate::export!($plugin with_types_in $crate);
    };
}

#[macro_export]
macro_rules! slash_command_component {
    ($plugin:ident, $id:literal, $definitions:path, $invoke:path) => {
        $crate::unadvertised_lifecycle!($plugin, $id, $crate::artist::plugin::types::Capability::Commands);
        $crate::unadvertised_tools!($plugin);
        $crate::unadvertised_resources!($plugin);
        impl $crate::exports::artist::plugin::slash_command_provider::Guest for $plugin {
            fn definitions() -> Result<Vec<$crate::artist::plugin::types::SlashCommandDefinition>, String> {
                $definitions()
            }
            fn invoke(name: String, arguments: String) -> Result<$crate::artist::plugin::types::SlashCommandResult, String> {
                $invoke(name, arguments)
            }
        }
        $crate::export!($plugin with_types_in $crate);
    };
}
