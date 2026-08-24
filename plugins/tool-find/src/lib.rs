use artist_plugin_sdk::{
    artist::plugin::{host_resources, types::FindRequest},
    schemars::JsonSchema,
    serde::Deserialize,
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FindArgs {
    uri: String,
    glob: Option<String>,
    max_depth: Option<u64>,
    cursor: Option<String>,
    limit: Option<u64>,
}

struct FindTool;

fn invoke(args: FindArgs) -> Result<String, String> {
    host_resources::find(&FindRequest {
        uri: args.uri,
        glob: args.glob,
        max_depth: args.max_depth,
        cursor: args.cursor,
        limit: Some(args.limit.unwrap_or(50)),
    })
}

artist_plugin_sdk::tool_component!(
    FindTool,
    "artist.tool.find",
    FindArgs,
    "find",
    "Find descendants of a resource URI. With no glob and depth one, lists immediate children.",
    vec![artist_plugin_sdk::artist::plugin::types::ToolEffect::Observe],
    invoke
);
