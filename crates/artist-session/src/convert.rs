//! Conversions between the on-disk [`ContentBlock`] schema and rig's message
//! content types.
//!
//! Every conversion is total in the store direction: content we cannot model
//! faithfully degrades to [`ContentBlock::Opaque`] (verbatim rig serde) rather
//! than losing data. The rebuild direction is fallible only for missing
//! attachments or corrupt opaque payloads.

use anyhow::{Context, Result};
use base64::Engine as _;
use rig_core::OneOrMany;
use rig_core::completion::message::{
    AssistantContent, DocumentSourceKind, Image, ImageMediaType, Reasoning, ReasoningContent, Text,
    ToolCall, ToolFunction, ToolResultContent, UserContent,
};

use crate::attachments::AttachmentStore;
use crate::event::ContentBlock;

/// Store one rig assistant content item as content blocks.
pub fn assistant_to_blocks(
    content: &AssistantContent,
    attachments: &AttachmentStore,
) -> Vec<ContentBlock> {
    match content {
        AssistantContent::Text(text) if text.additional_params.is_none() => {
            vec![ContentBlock::Text {
                text: text.text.clone(),
            }]
        }
        AssistantContent::ToolCall(call) if call.additional_params.is_none() => {
            vec![ContentBlock::ToolCall {
                id: call.id.clone(),
                call_id: call.call_id.clone(),
                name: call.function.name.clone(),
                arguments: call.function.arguments.clone(),
                signature: call.signature.clone(),
            }]
        }
        AssistantContent::Reasoning(reasoning) => {
            reasoning_to_blocks(reasoning).unwrap_or_else(|| vec![opaque(content)])
        }
        AssistantContent::Image(image) => {
            image_to_block(image, attachments).unwrap_or_else(|| vec![opaque(content)])
        }
        // Text/ToolCall with provider-specific additional_params, and any
        // future variants: keep the verbatim rig encoding.
        _ => vec![opaque(content)],
    }
}

/// Rebuild rig assistant content from stored blocks. Consecutive reasoning
/// blocks sharing an id are merged back into one `Reasoning` item, matching
/// how they were split on store.
pub fn blocks_to_assistant(
    blocks: &[ContentBlock],
    attachments: &AttachmentStore,
) -> Result<Vec<AssistantContent>> {
    let mut out: Vec<AssistantContent> = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            ContentBlock::Text { text } => {
                out.push(AssistantContent::Text(Text::new(text.clone())));
            }
            ContentBlock::ToolCall {
                id,
                call_id,
                name,
                arguments,
                signature,
            } => {
                let mut call = ToolCall::new(
                    id.clone(),
                    ToolFunction::new(name.clone(), arguments.clone()),
                )
                .with_signature(signature.clone());
                call.call_id = call_id.clone();
                out.push(AssistantContent::ToolCall(call));
            }
            ContentBlock::ReasoningSummary { id, text } => {
                push_reasoning(&mut out, id, ReasoningContent::Summary(text.clone()))
            }
            ContentBlock::ReasoningText {
                id,
                text,
                signature,
            } => push_reasoning(
                &mut out,
                id,
                ReasoningContent::Text {
                    text: text.clone(),
                    signature: signature.clone(),
                },
            ),
            ContentBlock::ReasoningEncrypted { id, data } => {
                push_reasoning(&mut out, id, ReasoningContent::Encrypted(data.clone()))
            }
            ContentBlock::ReasoningRedacted { id, data } => push_reasoning(
                &mut out,
                id,
                ReasoningContent::Redacted { data: data.clone() },
            ),
            ContentBlock::Image {
                attachment,
                media_type,
            } => {
                out.push(AssistantContent::Image(block_to_image(
                    attachment,
                    media_type.as_deref(),
                    attachments,
                )?));
            }
            ContentBlock::Opaque { rig } => {
                let mut content: AssistantContent = serde_json::from_value(rig.clone())
                    .context("decode opaque assistant content")?;
                normalize_assistant_params(&mut content);
                out.push(content);
            }
        }
    }
    Ok(out)
}

/// Store rig user content (prompt text and images; tool results are separate
/// `tool.result` events and never pass through here).
pub fn user_to_blocks(
    content: &OneOrMany<UserContent>,
    attachments: &AttachmentStore,
) -> Vec<ContentBlock> {
    content
        .iter()
        .flat_map(|item| match item {
            UserContent::Text(text) if text.additional_params.is_none() => {
                vec![ContentBlock::Text {
                    text: text.text.clone(),
                }]
            }
            UserContent::Image(image) => {
                image_to_block(image, attachments).unwrap_or_else(|| vec![opaque(item)])
            }
            _ => vec![opaque(item)],
        })
        .collect()
}

/// Rebuild rig user content from stored blocks.
pub fn blocks_to_user(
    blocks: &[ContentBlock],
    attachments: &AttachmentStore,
) -> Result<Vec<UserContent>> {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => Ok(UserContent::Text(Text::new(text.clone()))),
            ContentBlock::Image {
                attachment,
                media_type,
            } => Ok(UserContent::Image(block_to_image(
                attachment,
                media_type.as_deref(),
                attachments,
            )?)),
            ContentBlock::Opaque { rig } => {
                let mut content: UserContent =
                    serde_json::from_value(rig.clone()).context("decode opaque user content")?;
                normalize_user_params(&mut content);
                Ok(content)
            }
            other => anyhow::bail!("content block {other:?} is not valid user content"),
        })
        .collect()
}

fn reasoning_to_blocks(reasoning: &Reasoning) -> Option<Vec<ContentBlock>> {
    let id = &reasoning.id;
    reasoning
        .content
        .iter()
        .map(|item| match item {
            ReasoningContent::Text { text, signature } => Some(ContentBlock::ReasoningText {
                id: id.clone(),
                text: text.clone(),
                signature: signature.clone(),
            }),
            ReasoningContent::Encrypted(data) => Some(ContentBlock::ReasoningEncrypted {
                id: id.clone(),
                data: data.clone(),
            }),
            ReasoningContent::Redacted { data } => Some(ContentBlock::ReasoningRedacted {
                id: id.clone(),
                data: data.clone(),
            }),
            ReasoningContent::Summary(text) => Some(ContentBlock::ReasoningSummary {
                id: id.clone(),
                text: text.clone(),
            }),
            // ReasoningContent is #[non_exhaustive]; an unknown variant makes
            // the whole item fall back to Opaque in the caller.
            _ => None,
        })
        .collect()
}

fn push_reasoning(out: &mut Vec<AssistantContent>, id: &Option<String>, item: ReasoningContent) {
    // Only merge into the previous item when both carry the same *explicit* id —
    // two adjacent id-less (`None`) reasoning items are distinct groups (e.g.
    // Anthropic thinking blocks) and must not be collapsed into one.
    if let Some(AssistantContent::Reasoning(last)) = out.last_mut()
        && id.is_some()
        && last.id == *id
    {
        last.content.push(item);
        return;
    }
    let reasoning = Reasoning::new("").optional_id(id.clone());
    let mut reasoning = reasoning;
    reasoning.content = vec![item];
    out.push(AssistantContent::Reasoning(reasoning));
}

/// Returns None (caller degrades to Opaque) for non-base64 image sources.
fn image_to_block(image: &Image, attachments: &AttachmentStore) -> Option<Vec<ContentBlock>> {
    if image.detail.is_some() || image.additional_params.is_some() {
        return None;
    }
    let bytes = match &image.data {
        DocumentSourceKind::Base64(data) => base64::engine::general_purpose::STANDARD
            .decode(data)
            .ok()?,
        DocumentSourceKind::Raw(bytes) => bytes.clone(),
        _ => return None,
    };
    let attachment = attachments.put(&bytes).ok()?;
    Some(vec![ContentBlock::Image {
        attachment,
        media_type: image.media_type.as_ref().map(media_type_str),
    }])
}

/// Store a tool-result image into the attachment store, returning its block
/// reference (`None` for an unstorable/opaque image).
pub fn store_tool_image(image: &Image, attachments: &AttachmentStore) -> Option<ContentBlock> {
    image_to_block(image, attachments).and_then(|mut blocks| blocks.pop())
}

/// Rebuild a tool-result image from its stored `ContentBlock::Image` reference.
pub fn tool_image_from_block(
    block: &ContentBlock,
    attachments: &AttachmentStore,
) -> Option<ToolResultContent> {
    let ContentBlock::Image {
        attachment,
        media_type,
    } = block
    else {
        return None;
    };
    block_to_image(attachment, media_type.as_deref(), attachments)
        .ok()
        .map(ToolResultContent::Image)
}

fn block_to_image(
    attachment: &str,
    media_type: Option<&str>,
    attachments: &AttachmentStore,
) -> Result<Image> {
    let bytes = attachments.get(attachment)?;
    Ok(Image {
        data: DocumentSourceKind::Base64(base64::engine::general_purpose::STANDARD.encode(bytes)),
        media_type: media_type.and_then(parse_media_type),
        detail: None,
        additional_params: None,
    })
}

fn media_type_str(media_type: &ImageMediaType) -> String {
    serde_json::to_value(media_type)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "png".to_owned())
}

fn parse_media_type(name: &str) -> Option<ImageMediaType> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
}

/// rig's `additional_params` fields are `#[serde(flatten)]`, so a value that
/// was `None` on serialize comes back as `Some({})` — wire-identical, but it
/// breaks exact round-trips. Collapse empty objects back to `None`.
fn normalize_params(params: &mut Option<serde_json::Value>) {
    if matches!(params, Some(serde_json::Value::Object(map)) if map.is_empty()) {
        *params = None;
    }
}

fn normalize_assistant_params(content: &mut AssistantContent) {
    match content {
        AssistantContent::Text(text) => normalize_params(&mut text.additional_params),
        AssistantContent::ToolCall(call) => normalize_params(&mut call.additional_params),
        AssistantContent::Image(image) => normalize_params(&mut image.additional_params),
        AssistantContent::Reasoning(_) => {}
    }
}

fn normalize_user_params(content: &mut UserContent) {
    match content {
        UserContent::Text(text) => normalize_params(&mut text.additional_params),
        UserContent::Image(image) => normalize_params(&mut image.additional_params),
        _ => {}
    }
}

fn opaque<T: serde::Serialize + std::fmt::Debug>(content: &T) -> ContentBlock {
    ContentBlock::Opaque {
        rig: serde_json::to_value(content)
            .unwrap_or_else(|_| serde_json::Value::String(format!("{content:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, AttachmentStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(dir.path().join("attachments"));
        (dir, store)
    }

    fn round_trip_assistant(content: AssistantContent) {
        let (_dir, attachments) = store();
        let blocks = assistant_to_blocks(&content, &attachments);
        let rebuilt = blocks_to_assistant(&blocks, &attachments).unwrap();
        assert_eq!(rebuilt, vec![content]);
    }

    #[test]
    fn text_round_trips() {
        round_trip_assistant(AssistantContent::Text(Text::new("hello world")));
    }

    #[test]
    fn tool_call_round_trips_with_call_id_and_signature() {
        let mut call = ToolCall::new(
            "fc_1".into(),
            ToolFunction::new("edit".into(), serde_json::json!({"anchor": "ka"})),
        )
        .with_signature(Some("sig".into()));
        call.call_id = Some("call_9".into());
        round_trip_assistant(AssistantContent::ToolCall(call));
    }

    #[test]
    fn multi_block_reasoning_round_trips_as_one_item() {
        let mut reasoning = Reasoning::new("").optional_id(Some("rs_1".into()));
        reasoning.content = vec![
            ReasoningContent::Encrypted("gAAA".into()),
            ReasoningContent::Summary("thinking about it".into()),
            ReasoningContent::Text {
                text: "raw".into(),
                signature: Some("sig".into()),
            },
            ReasoningContent::Redacted { data: "xx".into() },
        ];
        round_trip_assistant(AssistantContent::Reasoning(reasoning));
    }

    #[test]
    fn adjacent_reasoning_items_with_distinct_ids_stay_distinct() {
        let (_dir, attachments) = store();
        let first =
            AssistantContent::Reasoning(Reasoning::new("a").optional_id(Some("rs_1".into())));
        let second =
            AssistantContent::Reasoning(Reasoning::new("b").optional_id(Some("rs_2".into())));
        let mut blocks = assistant_to_blocks(&first, &attachments);
        blocks.extend(assistant_to_blocks(&second, &attachments));
        let rebuilt = blocks_to_assistant(&blocks, &attachments).unwrap();
        assert_eq!(rebuilt, vec![first, second]);
    }

    #[test]
    fn base64_image_round_trips_via_attachment_store() {
        let (_dir, attachments) = store();
        let image = Image {
            data: DocumentSourceKind::Base64(
                base64::engine::general_purpose::STANDARD.encode(b"fake png"),
            ),
            media_type: Some(ImageMediaType::PNG),
            detail: None,
            additional_params: None,
        };
        let content = AssistantContent::Image(image.clone());
        let blocks = assistant_to_blocks(&content, &attachments);
        assert!(matches!(blocks[0], ContentBlock::Image { .. }));
        let rebuilt = blocks_to_assistant(&blocks, &attachments).unwrap();
        assert_eq!(rebuilt, vec![content]);
    }

    #[test]
    fn url_image_degrades_to_opaque_without_loss() {
        let (_dir, attachments) = store();
        let content = AssistantContent::Image(Image {
            data: DocumentSourceKind::Url("https://example.com/x.png".into()),
            media_type: None,
            detail: None,
            additional_params: None,
        });
        let blocks = assistant_to_blocks(&content, &attachments);
        assert!(matches!(blocks[0], ContentBlock::Opaque { .. }));
        let rebuilt = blocks_to_assistant(&blocks, &attachments).unwrap();
        assert_eq!(rebuilt, vec![content]);
    }

    #[test]
    fn text_with_additional_params_degrades_to_opaque_without_loss() {
        let content = AssistantContent::Text(Text {
            text: "cited".into(),
            additional_params: Some(serde_json::json!({"citation": "doc-1"})),
        });
        let (_dir, attachments) = store();
        let blocks = assistant_to_blocks(&content, &attachments);
        assert!(matches!(blocks[0], ContentBlock::Opaque { .. }));
        let rebuilt = blocks_to_assistant(&blocks, &attachments).unwrap();
        assert_eq!(rebuilt, vec![content]);
    }

    #[test]
    fn user_text_and_image_round_trip() {
        let (_dir, attachments) = store();
        let content = OneOrMany::many(vec![
            UserContent::Text(Text::new("look at this")),
            UserContent::Image(Image {
                data: DocumentSourceKind::Base64(
                    base64::engine::general_purpose::STANDARD.encode(b"jpeg"),
                ),
                media_type: Some(ImageMediaType::JPEG),
                detail: None,
                additional_params: None,
            }),
        ])
        .unwrap();
        let blocks = user_to_blocks(&content, &attachments);
        let rebuilt = blocks_to_user(&blocks, &attachments).unwrap();
        assert_eq!(rebuilt, content.iter().cloned().collect::<Vec<_>>());
    }
}
