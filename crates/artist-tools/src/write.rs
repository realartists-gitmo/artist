use crate::{ToolError, Workspace, output};
use hashline_tools::WriteCondition;
use rig_core::tool::PortableTool;
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
impl PortableTool for WriteTool {
    const NAME: &'static str = "write";
    type Error = ToolError;
    type Args = WriteArgs;
    type Output = String;
    fn description(&self) -> String {
        "Create or fully overwrite a project-relative or absolute file; use read+edit for targeted changes."
            .into()
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Project-relative or absolute path. Created if missing; replaced in full if it already exists."
                },
                "content": {
                    "type": "string",
                    "description": "The complete new contents of the file. This is the whole file, not a fragment appended or spliced into the existing one."
                }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        })
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
        let diff = output::anchored_diff(&diff, &[], &result.result.lines);
        // Creating a file is how a new module — and sometimes a new unit —
        // enters the project, so this is a commit like any other. No changed
        // lines are passed: a whole-file write encloses every declaration in
        // it, and "everything you just wrote has callers" is not a useful
        // sentence. The architectural half of the note is the part that
        // matters here.
        let aftermath = crate::annotate::after_commit(&self.0, &target, &[])
            .await
            .unwrap_or_default();
        Ok(output::head(
            format!(
                "Written {} ({} bytes; {}).\n\nDiff:\n{}{aftermath}",
                args.path,
                args.content.len(),
                if created { "created" } else { "overwritten" },
                diff
            ),
            output::OUTPUT_CAP,
        ))
    }
}
