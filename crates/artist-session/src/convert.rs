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
    AssistantContent, DocumentSourceKind, Image, ImageMediaType, Message, Reasoning,
    ReasoningContent, Text, ToolCall, ToolFunction, ToolResultContent, UserContent,
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

/// Key under which an externalized image carries its attachment digest.
///
/// It rides in `additional_params`, which is `#[serde(flatten)]` on rig's
/// `Image`, so an externalized message round-trips through rig's own serde with
/// no schema change to [`ConversationMessages`](crate::event::ConversationMessages).
const ATTACHMENT_PARAM: &str = "artist_attachment";

/// Replace inline image payloads with attachment references, storing the bytes.
///
/// rig commits tool results verbatim, so an inline base64 screenshot would land
/// in `events.jsonl`, be re-read on every load, be copied whole on fork, and be
/// re-serialized in full by every subsequent reset snapshot. Externalizing at
/// the memory boundary keeps the log proportional to the number of *distinct*
/// images rather than to how long the session runs.
///
/// Images whose bytes cannot be stored are left inline: degrading to a
/// reference we cannot resolve would lose content, and staying inline only
/// costs space.
pub fn externalize_images(messages: &mut [Message], attachments: &AttachmentStore) {
    visit_images(messages, &mut |image| {
        if attachment_digest(image).is_some() {
            return None;
        }
        let bytes = match &image.data {
            DocumentSourceKind::Base64(data) => base64::engine::general_purpose::STANDARD
                .decode(data)
                .ok()?,
            DocumentSourceKind::Raw(bytes) => bytes.clone(),
            _ => return None,
        };
        let digest = attachments.put(&bytes).ok()?;
        image.data = DocumentSourceKind::Unknown;
        let params = image
            .additional_params
            .get_or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(map) = params.as_object_mut() {
            map.insert(ATTACHMENT_PARAM.to_owned(), digest.into());
        }
        None
    });
}

/// Restore inline payloads from attachment references.
///
/// A missing blob degrades to a text placeholder rather than an error: a lost
/// attachment must never make a session unopenable.
pub fn rehydrate_images(messages: &mut [Message], attachments: &AttachmentStore) {
    visit_images(messages, &mut |image| {
        let digest = attachment_digest(image)?;
        let Ok(bytes) = attachments.get(&digest) else {
            return Some(format!("[image unavailable img:{digest}]"));
        };
        image.data =
            DocumentSourceKind::Base64(base64::engine::general_purpose::STANDARD.encode(bytes));
        if let Some(map) = image
            .additional_params
            .as_mut()
            .and_then(serde_json::Value::as_object_mut)
        {
            map.remove(ATTACHMENT_PARAM);
        }
        normalize_params(&mut image.additional_params);
        None
    });
}

/// Every attachment digest any of these events refers to.
///
/// Deliberately scans **all** events, including ones a rewind mask hides: the
/// log is append-only and a mask can be undone, so a blob referenced only by
/// masked history is still live. Walks the payload JSON generically rather than
/// matching each event kind, so a new kind that carries an image cannot silently
/// have its blobs pruned out from under it.
pub fn referenced_attachments(events: &[crate::Envelope]) -> std::collections::HashSet<String> {
    fn walk(value: &serde_json::Value, found: &mut std::collections::HashSet<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, item) in map {
                    // `attachment` is the legacy ContentBlock::Image field;
                    // `artist_attachment` is the externalized-image param.
                    if (key == "attachment" || key == ATTACHMENT_PARAM)
                        && let Some(digest) = item.as_str()
                    {
                        found.insert(digest.to_owned());
                    }
                    walk(item, found);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| walk(item, found)),
            _ => {}
        }
    }

    let mut found = std::collections::HashSet::new();
    for envelope in events {
        walk(&envelope.payload, &mut found);
    }
    found
}

/// A one-line display label for an image, naming its attachment when it has one.
///
/// The digest is the affordance: the blob lives at
/// `<session>/attachments/<sha>`, so a reader can go look at what the model saw.
pub(crate) fn image_marker(image: &Image) -> String {
    match attachment_digest(image) {
        Some(digest) => format!("[image img:{}]", &digest[..digest.len().min(12)]),
        None => "[image]".to_owned(),
    }
}

fn attachment_digest(image: &Image) -> Option<String> {
    image
        .additional_params
        .as_ref()?
        .get(ATTACHMENT_PARAM)?
        .as_str()
        .map(str::to_owned)
}

/// Visit every image reachable from a message, in user content, assistant
/// content, and tool results alike. A visitor returning `Some(text)` replaces
/// the image with a text block of the enclosing content type.
fn visit_images(messages: &mut [Message], visit: &mut impl FnMut(&mut Image) -> Option<String>) {
    for message in messages {
        match message {
            Message::System { .. } => {}
            Message::Assistant { content, .. } => {
                for item in content.iter_mut() {
                    let replacement = match item {
                        AssistantContent::Image(image) => visit(image),
                        _ => None,
                    };
                    if let Some(text) = replacement {
                        *item = AssistantContent::Text(Text::new(text));
                    }
                }
            }
            Message::User { content } => {
                for item in content.iter_mut() {
                    match item {
                        UserContent::Image(image) => {
                            if let Some(text) = visit(image) {
                                *item = UserContent::Text(Text::new(text));
                            }
                        }
                        UserContent::ToolResult(result) => {
                            for block in result.content.iter_mut() {
                                let replacement = match block {
                                    ToolResultContent::Image(image) => visit(image),
                                    _ => None,
                                };
                                if let Some(text) = replacement {
                                    *block = ToolResultContent::text(text);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
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

    fn png(bytes: &[u8]) -> Image {
        Image {
            data: DocumentSourceKind::Base64(
                base64::engine::general_purpose::STANDARD.encode(bytes),
            ),
            media_type: Some(ImageMediaType::PNG),
            detail: None,
            additional_params: None,
        }
    }

    fn tool_result_with(content: Vec<ToolResultContent>) -> Message {
        Message::User {
            content: OneOrMany::one(UserContent::ToolResult(
                rig_core::completion::message::ToolResult {
                    id: "fc_1".into(),
                    call_id: None,
                    content: OneOrMany::many(content).unwrap(),
                },
            )),
        }
    }

    #[test]
    fn tool_result_images_externalize_and_rehydrate_exactly() {
        let (_dir, attachments) = store();
        let original = vec![tool_result_with(vec![
            ToolResultContent::text("<observation surface=\"win:3\">"),
            ToolResultContent::Image(png(b"fake screenshot bytes")),
        ])];

        let mut messages = original.clone();
        externalize_images(&mut messages, &attachments);

        let wire = serde_json::to_string(&messages).unwrap();
        assert!(
            !wire.contains(
                &base64::engine::general_purpose::STANDARD.encode(b"fake screenshot bytes")
            ),
            "externalized messages must carry no inline base64: {wire}"
        );
        assert_eq!(
            wire.matches(ATTACHMENT_PARAM).count(),
            1,
            "exactly one attachment reference per image: {wire}"
        );

        rehydrate_images(&mut messages, &attachments);
        assert_eq!(messages, original, "externalize then rehydrate is exact");

        // And it survives the serde round trip the event log performs. Compared
        // per-image rather than whole-message: rig's `additional_params` are
        // `#[serde(flatten)]`, so a `None` comes back as `Some({})` on every
        // text block regardless of images — the pre-existing quirk that
        // `normalize_params` documents, which `native_conversation` also does
        // not correct.
        let mut messages = original.clone();
        externalize_images(&mut messages, &attachments);
        let mut restored: Vec<Message> = serde_json::from_str(&wire).unwrap();
        rehydrate_images(&mut restored, &attachments);
        assert_eq!(images_of(&restored), images_of(&original));
    }

    fn images_of(messages: &[Message]) -> Vec<Image> {
        let mut found = Vec::new();
        let mut messages = messages.to_vec();
        visit_images(&mut messages, &mut |image| {
            found.push(image.clone());
            None
        });
        found
    }

    #[test]
    fn user_and_assistant_images_externalize_and_rehydrate() {
        let (_dir, attachments) = store();
        let original = vec![
            Message::User {
                content: OneOrMany::many(vec![
                    UserContent::Text(Text::new("look")),
                    UserContent::Image(png(b"user image")),
                ])
                .unwrap(),
            },
            Message::Assistant {
                id: Some("msg_1".into()),
                content: OneOrMany::one(AssistantContent::Image(png(b"assistant image"))),
            },
        ];

        let mut messages = original.clone();
        externalize_images(&mut messages, &attachments);
        assert!(!serde_json::to_string(&messages).unwrap().contains("dXNlciBpbWFnZQ"));
        rehydrate_images(&mut messages, &attachments);
        assert_eq!(messages, original);
    }

    #[test]
    fn externalizing_twice_is_idempotent() {
        let (_dir, attachments) = store();
        let original = vec![tool_result_with(vec![ToolResultContent::Image(png(b"once"))])];

        let mut messages = original.clone();
        externalize_images(&mut messages, &attachments);
        let after_first = messages.clone();
        externalize_images(&mut messages, &attachments);
        assert_eq!(
            messages, after_first,
            "an already-externalized image must not be re-stored or double-wrapped"
        );

        rehydrate_images(&mut messages, &attachments);
        assert_eq!(messages, original);
    }

    #[test]
    fn identical_images_share_one_attachment() {
        let (_dir, attachments) = store();
        let mut messages = vec![tool_result_with(vec![
            ToolResultContent::Image(png(b"same bytes")),
            ToolResultContent::Image(png(b"same bytes")),
        ])];
        externalize_images(&mut messages, &attachments);
        let stored = std::fs::read_dir(attachments.dir()).unwrap().count();
        assert_eq!(stored, 1, "content addressing must deduplicate identical frames");
    }

    #[test]
    fn a_missing_blob_degrades_to_text_rather_than_failing() {
        let (_dir, attachments) = store();
        let mut messages = vec![tool_result_with(vec![
            ToolResultContent::text("observation"),
            ToolResultContent::Image(png(b"will be deleted")),
        ])];
        externalize_images(&mut messages, &attachments);

        for entry in std::fs::read_dir(attachments.dir()).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        rehydrate_images(&mut messages, &attachments);

        let Message::User { content } = &messages[0] else {
            panic!("expected a user message");
        };
        let UserContent::ToolResult(result) = content.first_ref() else {
            panic!("expected a tool result");
        };
        assert_eq!(result.content.len(), 2, "arity must be preserved");
        let placeholder = result.content.last_ref().as_text().unwrap();
        assert!(
            placeholder.starts_with("[image unavailable img:"),
            "a lost attachment must never make a session unopenable: {placeholder}"
        );
    }

    #[test]
    fn referenced_attachments_finds_both_encodings() {
        let (_dir, attachments) = store();
        let mut messages = vec![tool_result_with(vec![ToolResultContent::Image(png(
            b"externalized",
        ))])];
        externalize_images(&mut messages, &attachments);
        let externalized = attachments.put(b"externalized").unwrap();
        let legacy = attachments.put(b"legacy").unwrap();

        let envelope = |payload: serde_json::Value| crate::Envelope {
            v: 1,
            seq: 1,
            ts: 0,
            session: "s".into(),
            run: None,
            lineage: "main".into(),
            kind: "conversation.messages".into(),
            payload,
        };
        let events = vec![
            envelope(serde_json::json!({ "messages": messages })),
            // The legacy ContentBlock::Image encoding must be found too.
            envelope(serde_json::json!({
                "blocks": [{ "type": "image", "attachment": legacy, "media_type": "png" }]
            })),
        ];

        let found = referenced_attachments(&events);
        assert!(found.contains(&externalized), "missed artist_attachment");
        assert!(found.contains(&legacy), "missed legacy attachment field");
        assert_eq!(found.len(), 2);
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
