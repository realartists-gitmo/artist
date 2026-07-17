use super::Resources;
use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Component, Path};

#[derive(Clone)]
pub struct InstructionsTool(pub(super) Resources);

#[derive(Deserialize)]
pub struct InstructionsArgs {
    path: String,
}

#[derive(Debug, thiserror::Error)]
pub enum InstructionsError {
    #[error("instructions: {0}")]
    Message(String),
}

impl Tool for InstructionsTool {
    const NAME: &'static str = "instructions";
    type Error = InstructionsError;
    type Args = InstructionsArgs;
    type Output = String;

    fn description(&self) -> String {
        "Load the AGENTS.md instructions applicable to a project-relative path. Call before working in a nested project directory.".into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false})
    }
    async fn call(&self, args: InstructionsArgs) -> Result<String, InstructionsError> {
        let relative = Path::new(&args.path);
        if relative.is_absolute()
            || relative.components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(InstructionsError::Message(
                "path escapes the project".into(),
            ));
        }
        let target = self.0.0.workspace.join(relative);
        let mut output = String::new();
        for file in &self.0.0.agents {
            output.push_str(&format!(
                "<instructions path=\"{}\">\n{}\n</instructions>\n",
                file.path.display(),
                file.content
            ));
        }
        for file in &self.0.0.nested_agents {
            let Some(scope) = file.parent() else { continue };
            if target.starts_with(scope) {
                let content = std::fs::read_to_string(file)
                    .map_err(|error| InstructionsError::Message(error.to_string()))?;
                if content.len() > 128 * 1024 {
                    return Err(InstructionsError::Message(format!(
                        "{} is too large",
                        file.display()
                    )));
                }
                output.push_str(&format!(
                    "<instructions path=\"{}\">\n{}\n</instructions>\n",
                    file.display(),
                    content
                ));
            }
        }
        Ok(output)
    }
}
