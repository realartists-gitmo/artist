use artist_plugin_sdk::{
    artist::plugin::{host_control, types::ToolEffect},
    schemars::JsonSchema,
    serde::Deserialize,
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HandoffArgs {
    profile: String,
    brief: String,
}

struct HandoffTool;

fn invoke(args: HandoffArgs) -> Result<String, String> {
    host_control::handoff(&args.profile, &args.brief)?;
    Ok(artist_plugin_sdk::serde_json::json!({
        "profile": args.profile,
        "brief": args.brief,
    })
    .to_string())
}

artist_plugin_sdk::tool_component!(
    HandoffTool,
    "artist.tool.handoff",
    HandoffArgs,
    "handoff",
    "Transfer work to another profile.",
    vec![ToolEffect::SessionControl],
    invoke
);
