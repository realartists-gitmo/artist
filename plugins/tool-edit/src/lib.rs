use artist_plugin_sdk::{artist::plugin::types::*, schemars::JsonSchema, serde::Deserialize};

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum EditOperationArg {
    Replace {
        start: String,
        end: Option<String>,
        content: String,
    },
    Delete {
        start: String,
        end: Option<String>,
    },
    InsertBefore {
        anchor: String,
        content: String,
    },
    InsertAfter {
        anchor: String,
        content: String,
    },
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    uri: String,
    operations: Vec<EditOperationArg>,
}

struct EditTool;

fn invoke(args: EditArgs) -> Result<String, String> {
    if args.operations.is_empty() {
        return Err("invalid tool arguments: operations must not be empty".into());
    }
    let operations = args
        .operations
        .into_iter()
        .map(|operation| match operation {
            EditOperationArg::Replace {
                start,
                end,
                content,
            } => AnchoredEditOperation::Replace(ReplaceOperation {
                start,
                end,
                content,
            }),
            EditOperationArg::Delete { start, end } => {
                AnchoredEditOperation::Delete(DeleteOperation { start, end })
            }
            EditOperationArg::InsertBefore { anchor, content } => {
                AnchoredEditOperation::InsertBefore(InsertOperation { anchor, content })
            }
            EditOperationArg::InsertAfter { anchor, content } => {
                AnchoredEditOperation::InsertAfter(InsertOperation { anchor, content })
            }
        })
        .collect();
    let reply =
        artist_plugin_sdk::artist::plugin::host_resources::edit_anchored(&AnchoredEditRequest {
            uri: args.uri,
            operations,
        })
        .map_err(artist_plugin_sdk::resource_error)?;
    Ok(artist_plugin_sdk::serde_json::json!({
        "revision": reply.revision,
        "diff": reply.diff,
        "updated_lines": reply.updated_lines.into_iter().map(|line| artist_plugin_sdk::serde_json::json!({
            "anchor": line.anchor,
            "text": line.text,
        })).collect::<Vec<_>>(),
        "diff_truncated": reply.diff_truncated,
        "anchors_truncated": reply.anchors_truncated,
    }).to_string())
}

artist_plugin_sdk::tool_component!(
    EditTool,
    "artist.tool.edit",
    EditArgs,
    "edit",
    "Apply line-anchor-targeted changes at a resource URI.",
    vec![artist_plugin_sdk::artist::plugin::types::ToolEffect::Mutate],
    invoke
);
