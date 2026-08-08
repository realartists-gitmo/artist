use crate::{ToolError, Workspace, output};
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
        "Atomically replace line ranges in a project-relative or absolute file. Use anchors from a recent read of the same file. Re-read before retrying if an anchor is stale or unknown."
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
            "required": ["path", "replacements"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: EditArgs) -> Result<String, ToolError> {
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
            .edit_file(
                &self.0.actor,
                EditRequest {
                    path: args.path.clone(),
                    operations,
                },
            )
            .await?;
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
