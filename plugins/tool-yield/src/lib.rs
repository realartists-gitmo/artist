use artist_plugin_sdk::artist::plugin::{host_control, types::*};

struct YieldTool;

impl artist_plugin_sdk::exports::artist::plugin::tool_provider::Guest for YieldTool {
    fn definitions() -> Result<Vec<ToolDefinition>, String> {
        Ok(vec![ToolDefinition {
            name: "yield".into(),
            description: "Return structured work state and end the response.".into(),
            category: "session-control".into(),
            input_schema: artist_plugin_sdk::default_yield_schema().to_string(),
            output_schema: "{}".into(),
            effects: vec![ToolEffect::SessionControl],
            annotations: ToolAnnotations {
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: false,
            },
        }])
    }

    fn invoke(name: String, arguments: String) -> Result<ToolSuccess, ToolFailure> {
        if name != "yield" {
            return Err(artist_plugin_sdk::failure(
                "unknown-tool",
                format!("unknown tool: {name}"),
            ));
        }
        let payload: artist_plugin_sdk::serde_json::Value =
            artist_plugin_sdk::serde_json::from_str(&arguments).map_err(|error| {
                artist_plugin_sdk::failure("invalid-arguments", error.to_string())
            })?;
        host_control::yield_(&payload.to_string())
            .map_err(|error| artist_plugin_sdk::failure("control-failed", error))?;
        Ok(artist_plugin_sdk::success(payload.to_string()))
    }
}

artist_plugin_sdk::unadvertised_lifecycle!(
    YieldTool,
    "artist.tool.yield",
    artist_plugin_sdk::artist::plugin::types::Capability::Tools
);
artist_plugin_sdk::unadvertised_model_provider!(YieldTool);
artist_plugin_sdk::unadvertised_resources!(YieldTool);
artist_plugin_sdk::unadvertised_slash_commands!(YieldTool);
artist_plugin_sdk::export!(YieldTool with_types_in artist_plugin_sdk);
