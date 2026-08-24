use artist_plugin_sdk::{artist::plugin::types::*, schemars::JsonSchema, serde::Deserialize};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    uri: String,
    text: String,
}

struct WriteTool;

fn invoke(args: WriteArgs) -> Result<String, String> {
    artist_plugin_sdk::call_resource(ResourceRequest::Write(WriteRequest {
        uri: args.uri,
        text: args.text,
    }))
}

artist_plugin_sdk::tool_component!(
    WriteTool,
    "artist.tool.write",
    WriteArgs,
    "write",
    "Write UTF-8 text to a resource URI.",
    vec![artist_plugin_sdk::artist::plugin::types::ToolEffect::Mutate],
    invoke
);
