use artist_plugin_sdk::{
    artist::plugin::{host_resources, types::GrepRequest},
    schemars::JsonSchema,
    serde::Deserialize,
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GrepArgs {
    uri: String,
    regex: String,
    include_glob: Option<String>,
    context: Option<u64>,
    cursor: Option<String>,
    limit: Option<u64>,
}

struct GrepTool;

fn invoke(args: GrepArgs) -> Result<String, String> {
    host_resources::grep(&GrepRequest {
        uri: args.uri,
        regex: args.regex,
        include_glob: args.include_glob,
        context: args.context,
        cursor: args.cursor,
        limit: Some(args.limit.unwrap_or(100)),
    })
}

artist_plugin_sdk::tool_component!(
    GrepTool,
    "artist.tool.grep",
    GrepArgs,
    "grep",
    "Search text resources with a regular expression.",
    vec![artist_plugin_sdk::artist::plugin::types::ToolEffect::Observe],
    invoke
);
