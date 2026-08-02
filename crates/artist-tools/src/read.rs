use crate::{ToolError, Workspace};
use base64::Engine as _;
use hashline_tools::ReadFileRequest;
use rig_core::completion::message::{DocumentSourceKind, Image, ImageMediaType, ToolResultContent};
use rig_core::tool::{PortableTool, ToolOutput};
use serde::Deserialize;
use serde_json::{Value, json};

const READ_BYTES: usize = 50 * 1024;

/// Lines of content returned per read, and the point past which the file's
/// shape is appended.
///
/// Was 2000, which is more than most files and meant a read either returned
/// everything or a slab so large the model could not hold it. 200 is about a
/// screen: enough to work in, small enough that anything longer is obviously
/// partial — and anything longer now arrives with an outline of the rest, so
/// "partial" no longer means "blind".
const READ_LINES: usize = 200;

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
        format!(
            "Read a project-relative or absolute file. Each line renders as `ANCHOR: CONTENT` \
             (for example, `abc: hello`). Use only the token before the colon as the `start`/`end` \
             anchor in edit; the content after the colon is not an anchor. Returns up to \
             {READ_LINES} lines; a longer file also gets an outline of its whole shape, whose \
             anchors work in edit without reading that part first."
        )
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
        if [
            "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "heic", "heif",
        ]
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
        let limit = args.limit.unwrap_or(READ_LINES).min(READ_LINES);
        // Always request the whole file, then window for display.
        //
        // Two reasons. The anchor ledger reconciles over the entire file
        // whichever way this is called — `max_lines` only slices what comes
        // back — so asking for everything costs nothing extra. And the outline
        // appended below needs anchors for declarations *outside* the window,
        // which a windowed request cannot supply.
        let result = self
            .0
            .files
            .read_file(
                &self.0.actor,
                ReadFileRequest {
                    path: args.path.clone(),
                    start_line: 1,
                    max_lines: None,
                },
            )
            .await?;
        let all = &result.result.lines;
        let window = all.iter().skip(offset.saturating_sub(1)).take(limit);

        let mut output = String::new();
        let mut shown = 0;
        for line in window {
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

        // Past the window size, content alone is not enough to act on: the
        // model has seen a slice and has no idea what else is in the file.
        // Append the shape of the *whole* file, addressed by the same anchors
        // as the body above, so it can edit anything it can see here without
        // paging through to find it first.
        //
        // Appended rather than substituted — the head of a file is usually
        // wanted too, and replacing it with an outline would trade one blind
        // spot for another.
        // First observation of this path: say what the model cannot ask about
        // because it does not yet know to. See `crate::annotate`.
        if self.0.first_observation(&path)
            && let Some(note) = crate::annotate::first_touch(&self.0, &path).await
        {
            output.push_str(&note);
        }

        if result.result.total_lines > READ_LINES
            && let Some(parsed) = artist_ast::parse_file(&path)
        {
            let anchors = crate::outline::anchor_map(all);
            let (body, coverage) = crate::outline::render_with_coverage(
                &parsed.declarations,
                &anchors,
                &crate::outline::OutlineOptions::default(),
                parsed.language,
            );
            // Say how much of the shape this is. The header used to claim "the
            // whole file" unconditionally; on a declaration-dense file the
            // outline budget cut it short and said nothing, so the model was
            // told it had everything while a fifth was missing.
            let extent = if coverage.is_complete() {
                format!("shape of the whole file ({} declarations)", coverage.total)
            } else {
                format!(
                    "shape: {} of {} declarations — outermost first, the rest are nested deeper. \
                     Use code_map with a larger budget for all of them",
                    coverage.shown, coverage.total
                )
            };
            output.push_str(&format!(
                "\n\n[{} lines total — {extent}. Anchors usable directly in edit]\n{body}",
                result.result.total_lines
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
