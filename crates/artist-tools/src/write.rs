use crate::{ToolError, Workspace};
use hashline_tools::WriteCondition;
use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::{Value, json};

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
        "Atomically create or overwrite a complete project-relative file.".into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false})
    }
    async fn call(&self, args: WriteArgs) -> Result<String, ToolError> {
        let target = self.0.resolve_new(&args.path)?;
        let created = !target.exists();
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        self.0
            .files
            .write_file(
                &self.0.actor,
                args.path.clone(),
                args.content.clone(),
                WriteCondition::Any,
            )
            .await?;
        self.0.refresh_index(&target);
        Ok(format!(
            "Written {} ({} bytes; {}).",
            args.path,
            args.content.len(),
            if created { "created" } else { "overwritten" }
        ))
    }
}
