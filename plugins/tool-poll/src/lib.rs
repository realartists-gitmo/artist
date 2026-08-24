use artist_plugin_sdk::{artist::plugin::types::*, schemars::JsonSchema, serde::Deserialize};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PollArgs {
    uri: String,
    #[serde(rename = "match")]
    #[schemars(rename = "match")]
    pattern: Option<String>,
    timeout_ms: Option<u64>,
    cursor: Option<String>,
}

struct PollTool;

fn invoke(args: PollArgs) -> Result<String, String> {
    artist_plugin_sdk::call_resource(ResourceRequest::Poll(PollRequest {
        uri: args.uri,
        match_: args.pattern,
        timeout_ms: args.timeout_ms,
        cursor: args.cursor,
    }))
}

artist_plugin_sdk::tool_component!(
    PollTool,
    "artist.tool.poll",
    PollArgs,
    "poll",
    "Wait for appended text, a match, close, or timeout. Pass next_cursor from the previous poll to receive output losslessly.",
    vec![artist_plugin_sdk::artist::plugin::types::ToolEffect::Observe],
    invoke
);
