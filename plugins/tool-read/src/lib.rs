use artist_plugin_sdk::{artist::plugin::types::*, schemars::JsonSchema, serde::Deserialize};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    uri: String,
    #[schemars(range(min = 1))]
    start_line: Option<u64>,
    #[schemars(range(min = 1))]
    line_count: Option<u64>,
}

struct ReadTool;

fn invoke(args: ReadArgs) -> Result<String, String> {
    if args.start_line == Some(0) || args.line_count == Some(0) {
        return Err("invalid tool arguments: start_line and line_count must be positive".into());
    }
    let reply = artist_plugin_sdk::artist::plugin::host_resources::read_anchored(&ReadRequest {
        uri: args.uri,
        start_line: args.start_line,
        line_count: args.line_count,
    })
    .map_err(artist_plugin_sdk::resource_error)?;
    let mut text = String::new();
    let mut truncated = false;
    for line in reply.lines {
        let rendered = format!("{}: {}\n", line.anchor, line.text);
        if text.len() + rendered.len() > artist_plugin_sdk::MODEL_OUTPUT_BUDGET / 3 {
            truncated = true;
            break;
        }
        text.push_str(&rendered);
    }
    Ok(artist_plugin_sdk::serde_json::json!({
        "revision": reply.revision,
        "total_lines": reply.total_lines,
        "text": text,
        "truncated": truncated,
    })
    .to_string())
}

artist_plugin_sdk::tool_component!(
    ReadTool,
    "artist.tool.read",
    ReadArgs,
    "read",
    "Read UTF-8 text from a resource URI.",
    vec![artist_plugin_sdk::artist::plugin::types::ToolEffect::Observe],
    invoke
);
