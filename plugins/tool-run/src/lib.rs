use artist_plugin_sdk::{artist::plugin::types::*, schemars::JsonSchema, serde::Deserialize};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EnvironmentArg {
    name: String,
    value: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunArgs {
    target: String,
    input: String,
    cwd: Option<String>,
    #[serde(default)]
    env: Vec<EnvironmentArg>,
    #[schemars(range(min = 1))]
    timeout_ms: Option<u64>,
}

struct RunTool;

fn invoke(args: RunArgs) -> Result<String, String> {
    if args.timeout_ms == Some(0) {
        return Err("invalid tool arguments: timeout_ms must be positive".into());
    }
    artist_plugin_sdk::call_resource(ResourceRequest::Run(RunRequest {
        target: args.target,
        input: args.input,
        cwd: args.cwd,
        env: args
            .env
            .into_iter()
            .map(|entry| EnvironmentEntry {
                name: entry.name,
                value: entry.value,
            })
            .collect(),
        timeout_ms: args.timeout_ms,
    }))
}

artist_plugin_sdk::tool_component!(
    RunTool,
    "artist.tool.run",
    RunArgs,
    "run",
    "Start work at a resource URI.",
    vec![artist_plugin_sdk::artist::plugin::types::ToolEffect::Execute],
    invoke
);
