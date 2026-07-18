use crate::{ToolError, Workspace, output};
use hashline_tools::WriteCondition;
use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::{Value, json};
use similar::TextDiff;

#[derive(Clone)]
pub struct WriteTool(pub Workspace);
#[derive(Deserialize)]
pub struct WriteArgs {
    path: String,
    content: String,
}
impl Tool for WriteTool {
    const NAME: &'static str = "write";
    type Error = ToolError;
    type Args = WriteArgs;
    type Output = String;
    fn description(&self) -> String {
        "Create or fully overwrite a project-relative file; use read+edit for targeted changes.".into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false})
    }
    async fn call(&self, args: WriteArgs) -> Result<String, ToolError> {
        let target = self.0.resolve_new(&args.path)?;
        let created = !target.exists();
        let before = if created {
            String::new()
        } else {
            tokio::fs::read_to_string(&target).await?
        };
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let result = self
            .0
            .files
            .write_file(
                &self.0.actor,
                args.path.clone(),
                args.content.clone(),
                WriteCondition::Any,
            )
            .await?;
        self.0.refresh_index(&target);
        let diff = TextDiff::from_lines(&before, &args.content)
            .unified_diff()
            .context_radius(3)
            .to_string();
        // Removed lines are gone from the new file, so they get no anchor.
        let diff = output::anchored_diff(&diff, &[], &result.result.lines);
        Ok(output::head(
            format!(
                "Written {} ({} bytes; {}).\n\nDiff:\n{}",
                args.path,
                args.content.len(),
                if created { "created" } else { "overwritten" },
                diff
            ),
            output::OUTPUT_CAP,
        ))
    }
}
