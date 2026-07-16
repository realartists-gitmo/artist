use crate::{ToolError, Workspace};
use hashline_tools::ReadFileRequest;
use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::{Value, json};

const READ_BYTES: usize = 50 * 1024;

#[derive(Clone)]
pub struct ReadTool(pub Workspace);
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadArgs {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}
impl Tool for ReadTool {
    const NAME: &'static str = "read";
    type Error = ToolError;
    type Args = ReadArgs;
    type Output = String;
    fn description(&self) -> String {
        "Read a project-relative text file with mnemonic line anchors. Call this before edit."
            .into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":1},"limit":{"type":"integer","minimum":1}},"required":["path"],"additionalProperties":false})
    }
    async fn call(&self, args: ReadArgs) -> Result<String, ToolError> {
        let path = self.0.resolve_existing(&args.path)?;
        let extension = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ["png", "jpg", "jpeg", "gif", "webp", "bmp"].contains(&extension.as_str()) {
            let metadata = tokio::fs::metadata(&path).await?;
            return Ok(format!(
                "Image {} ({} bytes). Image attachments are unavailable in this provider tool channel.",
                args.path,
                metadata.len()
            ));
        }
        let offset = args.offset.unwrap_or(1).max(1);
        let limit = args.limit.unwrap_or(2000).min(2000);
        let result = self
            .0
            .files
            .read_file(
                &self.0.actor,
                ReadFileRequest {
                    path: args.path.clone(),
                    start_line: offset,
                    max_lines: Some(limit),
                },
            )
            .await?;
        let mut output = String::new();
        let mut shown = 0;
        for line in &result.result.lines {
            let rendered = format!("{} | {}\n", line.anchor, line.text);
            if shown > 0 && output.len() + rendered.len() > READ_BYTES.saturating_sub(200) {
                break;
            }
            output.push_str(&rendered);
            shown += 1;
            if output.len() > READ_BYTES.saturating_sub(200) {
                output.truncate(floor_char_boundary(&output, READ_BYTES.saturating_sub(200)));
                break;
            }
        }
        let truncated = result.result.total_lines > offset.saturating_sub(1) + shown;
        if truncated {
            let next = offset + shown;
            output.push_str(&format!(
                "\n[truncated: continue with read(path=\"{}\", offset={next})]",
                args.path
            ));
        }
        Ok(output)
    }
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}
