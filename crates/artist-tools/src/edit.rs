use crate::{ToolError, Workspace, output, resource_path::ResourcePath};
use hashline_tools::{EditOperation, EditRequest};
use rig_core::tool::PortableTool;
use serde::Deserialize;
use serde_json::{Value, json};
use similar::TextDiff;

#[derive(Clone)]
pub struct EditTool(pub Workspace);
#[derive(Deserialize)]
pub struct EditArgs {
    path: String,
    revision: String,
    replacements: Vec<Replacement>,
}
#[derive(Deserialize)]
pub struct Replacement {
    start: String,
    end: Option<String>,
    content: String,
}
impl PortableTool for EditTool {
    const NAME: &'static str = "edit";
    type Error = ToolError;
    type Args = EditArgs;
    type Output = String;
    fn description(&self) -> String {
        "Atomically replace line ranges in a project-relative or absolute file. Supply the exact revision returned by read; stale revisions fail without writing. Use anchors from that same read."
            .into()
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Project-relative or absolute path to the file being edited. Anchors must come from a read of this same file."
                },
                "revision": {
                    "type": "string",
                    "description": "Exact revision returned by read for this resource. It is required to prevent stale edits."
                },
                "replacements": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "start": {
                                "type": "string",
                                "description": "Exact opaque anchor from read/code output, including the leading `#`. Pass it byte-for-byte; never trim, normalize, case-fold, fuzzy-match, or use a line number."
                            },
                            "end": {
                                "anyOf": [
                                    {"type": "string"},
                                    {"type": "null"}
                                ],
                                "description": "Exact ending anchor, or null to replace only the start line."
                            },
                            "content": {
                                "type": "string",
                                "description": "Replacement text for the selected line range."
                            }
                        },
                        "required": ["start", "end", "content"],
                        "additionalProperties": false
                    },
                    "minItems": 1
                }
            },
            "required": ["path", "revision", "replacements"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: EditArgs) -> Result<String, ToolError> {
        reject_unresolved_virtual_mutation(&args.path)?;
        let target = self.0.resolve_existing(&args.path)?;
        if args.replacements.is_empty() {
            return Err(ToolError::Message("replacements cannot be empty".into()));
        }
        let before = tokio::fs::read_to_string(&target).await?;
        let operations = args
            .replacements
            .into_iter()
            .map(|r| EditOperation::Replace {
                anchor: r.start,
                end_anchor: r.end,
                content: r.content,
            })
            .collect();
        let result = self
            .0
            .files
            .edit_file_at_revision(
                &self.0.actor,
                EditRequest {
                    path: args.path.clone(),
                    operations,
                },
                Some(&args.revision),
            )
            .await
            .map_err(|error| {
                let detail = error.to_string();
                if detail.contains("content hash mismatch") {
                    ToolError::Message(format!(
                        "stale_revision: {} changed after it was read; start a fresh read(path=\"{}\") before editing it",
                        args.path, args.path
                    ))
                } else if detail.contains("stale") || detail.contains("anchor") {
                    ToolError::Message(format!(
                        "stale_revision: {detail}; start a fresh read(path=\"{}\") before editing it",
                        args.path
                    ))
                } else {
                    ToolError::Anyhow(error)
                }
            })?;
        self.0.refresh_index(&target);
        let after = tokio::fs::read_to_string(&target).await?;
        let diff = TextDiff::from_lines(&before, &after)
            .unified_diff()
            .context_radius(3)
            .to_string();
        let diff = output::anchored_diff(&diff, &result.result.before_lines, &result.result.lines);
        let before_lines: std::collections::HashSet<&str> = before.lines().collect();
        let updates = result
            .result
            .lines
            .iter()
            .filter(|line| !before_lines.contains(line.text.as_str()))
            .take(50)
            .map(|line| format!("{}: {}", line.anchor, line.text))
            .collect::<Vec<_>>()
            .join("\n");
        // Who calls what was just changed. Reported unasked because asking
        // requires already suspecting there are callers — see `crate::annotate`.
        let changed: Vec<usize> = result
            .result
            .lines
            .iter()
            .filter(|line| !before_lines.contains(line.text.as_str()))
            .map(|line| line.line_number)
            .collect();
        let impact = crate::annotate::after_commit(&self.0, &target, &changed)
            .await
            .unwrap_or_default();

        Ok(output::head(
            format!(
                "Applied edit to {}.\n\nAnchor updates:\n{}\n\nDiff:\n{}{impact}",
                args.path, updates, diff
            ),
            output::OUTPUT_CAP,
        ))
    }
}

pub(crate) fn reject_unresolved_virtual_mutation(path: &str) -> Result<(), ToolError> {
    match ResourcePath::parse(path).map_err(|error| ToolError::Message(error.to_string()))? {
        ResourcePath::Real(_) => Ok(()),
        ResourcePath::Virtual { scheme, .. } => Err(ToolError::Message(format!(
            "{path} is a typed {scheme}:// resource, not a host file; read its canonical projection and use the owning runtime's mutation operation"
        ))),
    }
}
