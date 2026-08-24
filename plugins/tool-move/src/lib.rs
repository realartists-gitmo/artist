use artist_plugin_sdk::{artist::plugin::types::*, schemars::JsonSchema, serde::Deserialize};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveArgs {
    from: String,
    to: Option<String>,
}

struct MoveTool;

fn invoke(args: MoveArgs) -> Result<String, String> {
    artist_plugin_sdk::call_resource(ResourceRequest::Move(MoveRequest {
        source: args.from,
        to: args.to,
    }))
}

artist_plugin_sdk::tool_component!(
    MoveTool,
    "artist.tool.move",
    MoveArgs,
    "move",
    "Move a resource, or remove it when `to` is null.",
    vec![artist_plugin_sdk::artist::plugin::types::ToolEffect::Mutate],
    invoke
);
