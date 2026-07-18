use crate::{ToolError, Workspace, output};
use hashline_tools::{EditOperation, EditRequest};
use rig_core::tool::Tool;
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
impl Tool for EditTool {
    const NAME: &'static str = "edit";
    type Error = ToolError;
    type Args = EditArgs;
    type Output = String;
    fn description(&self) -> String {
        "Atomically replace lines using ANCHORs from the latest read."
            .into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"replacements":{"type":"array","items":{"type":"object","properties":{"start":{"type":"string"},"end":{"type":"string"},"content":{"type":"string"}},"required":["start","content"],"additionalProperties":false},"minItems":1}},"required":["path","replacements"],"additionalProperties":false})
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
                hash: r.start,
                end_hash: r.end,
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
        Ok(output::head(
            format!(
                "Applied edit to {}.\n\nMnemonic updates:\n{}\n\nDiff:\n{}",
                args.path, updates, diff
            ),
            output::OUTPUT_CAP,
        ))
    }
}
