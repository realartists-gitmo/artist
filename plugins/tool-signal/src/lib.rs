use artist_plugin_sdk::{artist::plugin::types::*, schemars::JsonSchema, serde::Deserialize};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SignalArgs {
    uri: String,
    name: String,
    payload: Option<String>,
}

struct SignalTool;

fn invoke(args: SignalArgs) -> Result<String, String> {
    artist_plugin_sdk::call_resource(ResourceRequest::Signal(SignalRequest {
        uri: args.uri,
        name: args.name,
        payload: args.payload,
    }))
}

artist_plugin_sdk::tool_component!(
    SignalTool,
    "artist.tool.signal",
    SignalArgs,
    "signal",
    "Send a named signal to a resource URI.",
    vec![artist_plugin_sdk::artist::plugin::types::ToolEffect::Mutate],
    invoke
);
