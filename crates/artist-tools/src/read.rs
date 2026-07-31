use crate::{ToolError, Workspace};
use base64::Engine as _;
use hashline_tools::ReadFileRequest;
use rig_core::completion::message::{
    DocumentSourceKind, Image, ImageMediaType, ToolResultContent,
};
use rig_core::tool::{PortableTool, ToolOutput};
use serde::Deserialize;
use serde_json::{Value, json};

const READ_BYTES: usize = 50 * 1024;

/// Largest image inlined into the tool channel, before base64 expansion.
///
/// Past this a description is returned instead: an oversized frame costs far
/// more context than it is ever worth, and silently truncating image bytes
/// would hand the model a corrupt picture.
const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Clone)]
pub struct ReadTool(pub Workspace);
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadArgs {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}
impl PortableTool for ReadTool {
    const NAME: &'static str = "read";
    type Error = ToolError;
    type Args = ReadArgs;
    type Output = ToolOutput;
    fn description(&self) -> String {
        "Read a project-relative or absolute file. Each line renders as `ANCHOR: CONTENT` (for example, `abc: hello`). Use only the token before the colon as the `start`/`end` anchor in edit; the content after the colon is not an anchor."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":1},"limit":{"type":"integer","minimum":1}},"required":["path"],"additionalProperties":false})
    }
    async fn call(&self, args: ReadArgs) -> Result<ToolOutput, ToolError> {
        let path = self.0.resolve_existing(&args.path)?;
        let extension = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "heic", "heif"]
            .contains(&extension.as_str())
        {
            let size = tokio::fs::metadata(&path).await?.len();
            let Some(media_type) = image_media_type(&extension) else {
                return Ok(ToolOutput::text(format!(
                    "Image {} ({size} bytes). The {extension} format cannot be sent to a model; convert it first.",
                    args.path
                )));
            };
            if size > MAX_IMAGE_BYTES {
                return Ok(ToolOutput::text(format!(
                    "Image {} is {size} bytes, over the {MAX_IMAGE_BYTES}-byte inline limit. Resize or crop it first.",
                    args.path
                )));
            }
            let bytes = tokio::fs::read(&path).await?;
            return Ok(ToolOutput::one(ToolResultContent::Image(Image {
                data: DocumentSourceKind::Base64(
                    base64::engine::general_purpose::STANDARD.encode(&bytes),
                ),
                media_type: Some(media_type),
                detail: None,
                additional_params: None,
            })));
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
            let rendered = format!("{}: {}\n", line.anchor, line.text);
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
        Ok(ToolOutput::text(output))
    }
}

/// `bmp` has no rig media type, so it is read as a file but never inlined.
fn image_media_type(extension: &str) -> Option<ImageMediaType> {
    Some(match extension {
        "png" => ImageMediaType::PNG,
        "jpg" | "jpeg" => ImageMediaType::JPEG,
        "gif" => ImageMediaType::GIF,
        "webp" => ImageMediaType::WEBP,
        "svg" => ImageMediaType::SVG,
        "heic" => ImageMediaType::HEIC,
        "heif" => ImageMediaType::HEIF,
        _ => return None,
    })
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}
