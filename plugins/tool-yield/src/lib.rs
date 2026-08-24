use artist_plugin_sdk::artist::plugin::{host_control, types::*};

struct YieldTool;

impl artist_plugin_sdk::exports::artist::plugin::tool_provider::Guest for YieldTool {
    fn definitions() -> Result<Vec<ToolDefinition>, String> {
        Ok(vec![ToolDefinition {
            name: "yield".into(),
            description: "Return structured work state and end the response.".into(),
            input_schema: artist_plugin_sdk::default_yield_schema().to_string(),
            effects: vec![ToolEffect::SessionControl],
        }])
    }

    fn invoke(name: String, arguments: String) -> Result<String, String> {
        if name != "yield" {
            return Err(format!("unknown tool: {name}"));
        }
        let payload: artist_plugin_sdk::serde_json::Value =
            artist_plugin_sdk::serde_json::from_str(&arguments)
                .map_err(|error| format!("invalid tool arguments: {error}"))?;
        host_control::yield_(&payload.to_string())?;
        Ok(payload.to_string())
    }
}

artist_plugin_sdk::unadvertised_lifecycle!(
    YieldTool,
    "artist.tool.yield",
    artist_plugin_sdk::artist::plugin::types::Capability::Tools
);
artist_plugin_sdk::unadvertised_resources!(YieldTool);
artist_plugin_sdk::unadvertised_slash_commands!(YieldTool);
artist_plugin_sdk::export!(YieldTool with_types_in artist_plugin_sdk);
