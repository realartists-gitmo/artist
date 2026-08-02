//! Finding and replacing images that ride a request as bytes.
//!
//! A stateless completion API resends the entire conversation every turn, so an
//! image inlined as a base64 data URL is re-uploaded for the rest of the
//! session. Providers that expose a files endpoint accept a `file_id` in the
//! same content block instead, which turns an unbounded repeated cost into a
//! single upload.
//!
//! Two properties make that worth doing beyond the bandwidth. Bytes that are
//! not in the request cannot vary between requests, so they stop being a way to
//! break a prompt cache. And the digest used to key a handle is the same
//! content address [`crate::AttachmentStore`] already assigns, so an image the
//! session has stored locally and one the provider holds remotely are the same
//! object under the same name.
//!
//! The walk is recursive over arbitrary JSON rather than targeted at the two
//! shapes that carry images today — a user message and a tool result. A content
//! shape added later is covered without being taught about, which is the same
//! reason the drift hook rides tool dispatch rather than individual tools.

use std::collections::BTreeMap;

use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// An image currently being sent as bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineImage {
    /// SHA-256 of the decoded bytes, hex — the same address
    /// [`crate::AttachmentStore::put`] returns, and the [`crate::HandleLedger`]
    /// key.
    pub digest: String,
    pub bytes: Vec<u8>,
    /// The media type declared by the data URL, e.g. `image/png`.
    pub media_type: String,
}

/// Every image sent inline anywhere in a provider input array.
///
/// Deduplicated by digest: the same screenshot referenced twice needs one
/// upload, and callers use this list to decide what to upload.
pub fn inline_images(input: &[Value]) -> Vec<InlineImage> {
    let mut found: BTreeMap<String, InlineImage> = BTreeMap::new();
    for item in input {
        walk(item, &mut |url| {
            if let Some(image) = decode_data_url(url) {
                found.entry(image.digest.clone()).or_insert(image);
            }
        });
    }
    found.into_values().collect()
}

/// Total bytes that would go on the wire as inline image data.
///
/// Counts the encoded length, not the decoded one: base64 is what is actually
/// transmitted, and it is a third larger than the image.
pub fn inline_bytes(input: &[Value]) -> usize {
    let mut total = 0;
    for item in input {
        walk(item, &mut |url| {
            if let Some((_, payload)) = split_data_url(url) {
                total += payload.len();
            }
        });
    }
    total
}

/// Swap inline image data for provider file ids, where one is known.
///
/// Returns the number of blocks rewritten. An image whose digest is absent from
/// `handles` is left exactly as it was — a missing handle degrades to today's
/// behaviour, never to a broken request.
///
/// `image_url` is removed rather than left alongside `file_id`: the two are
/// documented as mutually exclusive, and sending both would at best waste the
/// bytes this exists to save.
pub fn apply_handles(input: &mut [Value], handles: &BTreeMap<String, String>) -> usize {
    let mut replaced = 0;
    for item in input.iter_mut() {
        walk_mut(item, &mut |block| {
            let Some(url) = block.get("image_url").and_then(Value::as_str) else {
                return;
            };
            let Some((_, payload)) = split_data_url(url) else {
                return;
            };
            let Some(bytes) = decode(payload) else {
                return;
            };
            let Some(file_id) = handles.get(&hex_digest(&bytes)) else {
                return;
            };
            let Some(object) = block.as_object_mut() else {
                return;
            };
            object.remove("image_url");
            object.insert("file_id".into(), Value::String(file_id.clone()));
            replaced += 1;
        });
    }
    replaced
}

/// Visit every `image_url` string in a JSON tree.
fn walk(value: &Value, visit: &mut impl FnMut(&str)) {
    match value {
        Value::Object(map) => {
            if let Some(url) = map.get("image_url").and_then(Value::as_str) {
                visit(url);
            }
            for nested in map.values() {
                walk(nested, visit);
            }
        }
        Value::Array(items) => {
            for nested in items {
                walk(nested, visit);
            }
        }
        _ => {}
    }
}

/// Visit every object that carries an `image_url`, mutably.
fn walk_mut(value: &mut Value, visit: &mut impl FnMut(&mut Value)) {
    match value {
        Value::Object(map) => {
            if map.contains_key("image_url") {
                visit(value);
            }
            // Re-borrowed after the visit above, which may have replaced keys.
            if let Value::Object(map) = value {
                for nested in map.values_mut() {
                    walk_mut(nested, visit);
                }
            }
        }
        Value::Array(items) => {
            for nested in items {
                walk_mut(nested, visit);
            }
        }
        _ => {}
    }
}

/// `data:image/png;base64,AAAA` → `("image/png", "AAAA")`.
///
/// Anything else — an `https://` URL the model was handed, a malformed value —
/// yields `None` and is left untouched. We only ever substitute for content we
/// are ourselves inlining.
fn split_data_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let media_type = meta.strip_suffix(";base64")?;
    Some((media_type, payload))
}

fn decode_data_url(url: &str) -> Option<InlineImage> {
    let (media_type, payload) = split_data_url(url)?;
    let bytes = decode(payload)?;
    Some(InlineImage {
        digest: hex_digest(&bytes),
        bytes,
        media_type: media_type.to_owned(),
    })
}

fn decode(payload: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(payload)
        .ok()
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn data_url(bytes: &[u8]) -> String {
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    }

    /// The shape a pasted or read image arrives in.
    fn user_message(bytes: &[u8]) -> Value {
        json!({
            "type": "message",
            "role": "user",
            "content": [
                {"type": "input_text", "text": "look at this"},
                {"type": "input_image", "image_url": data_url(bytes), "detail": "auto"}
            ]
        })
    }

    /// The shape a screenshot arrives in — nested one level deeper, inside a
    /// tool result rather than a message.
    fn tool_result(bytes: &[u8]) -> Value {
        json!({
            "type": "function_call_output",
            "call_id": "call_1",
            "output": [
                {"type": "input_image", "image_url": data_url(bytes), "detail": "auto"}
            ]
        })
    }

    #[test]
    fn an_image_in_a_user_message_is_found() {
        let found = inline_images(&[user_message(b"png bytes")]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].bytes, b"png bytes");
        assert_eq!(found[0].media_type, "image/png");
    }

    /// Screenshots are the reason this exists, and they arrive nested inside a
    /// tool result rather than as a message.
    #[test]
    fn an_image_in_a_tool_result_is_found() {
        let found = inline_images(&[tool_result(b"screenshot")]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].bytes, b"screenshot");
    }

    /// The digest has to be the address the attachment store already uses, or a
    /// locally stored image and a remotely held one are two different objects.
    #[test]
    fn the_digest_matches_the_attachment_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::AttachmentStore::new(dir.path().join("a"));
        let stored = store.put(b"png bytes").unwrap();
        let found = inline_images(&[user_message(b"png bytes")]);
        assert_eq!(found[0].digest, stored);
    }

    /// One upload per distinct image, however many times it is referenced.
    #[test]
    fn the_same_image_twice_is_one_upload() {
        let found = inline_images(&[user_message(b"same"), tool_result(b"same")]);
        assert_eq!(found.len(), 1);
    }

    /// A URL we did not construct is not ours to rewrite.
    #[test]
    fn a_remote_url_is_left_alone() {
        let input = [json!({
            "type": "input_image",
            "image_url": "https://example.com/cat.png"
        })];
        assert!(inline_images(&input).is_empty());
        assert_eq!(inline_bytes(&input), 0);
    }

    #[test]
    fn a_known_handle_replaces_the_bytes() {
        let mut input = [tool_result(b"screenshot")];
        let digest = inline_images(&input)[0].digest.clone();
        let handles = BTreeMap::from([(digest, "file-abc".to_owned())]);

        assert_eq!(apply_handles(&mut input, &handles), 1);

        let block = &input[0]["output"][0];
        assert_eq!(block["file_id"], "file-abc");
        assert!(
            block.get("image_url").is_none(),
            "image_url and file_id are mutually exclusive: {block}"
        );
        // The detail preference is not ours to discard.
        assert_eq!(block["detail"], "auto");
    }

    /// The degraded path, and the one that must never break a request: no
    /// handle means the bytes go as they always did.
    #[test]
    fn an_unknown_digest_is_left_inline() {
        let mut input = [user_message(b"png bytes")];
        let before = input.clone();
        assert_eq!(apply_handles(&mut input, &BTreeMap::new()), 0);
        assert_eq!(input, before);
    }

    /// Requests get rebuilt and re-prepared; substituting twice must not
    /// corrupt an already-substituted block.
    #[test]
    fn substitution_is_idempotent() {
        let mut input = [tool_result(b"screenshot")];
        let digest = inline_images(&input)[0].digest.clone();
        let handles = BTreeMap::from([(digest, "file-abc".to_owned())]);

        apply_handles(&mut input, &handles);
        let once = input.clone();
        assert_eq!(apply_handles(&mut input, &handles), 0);
        assert_eq!(input, once);
    }

    /// After substitution there is nothing left to find, which is the whole
    /// point: the bytes have left the request.
    #[test]
    fn substitution_removes_the_bytes_from_the_wire() {
        let mut input = [user_message(&[7u8; 4096])];
        let before = inline_bytes(&input);
        assert!(before > 4096, "base64 inflates: {before}");

        let digest = inline_images(&input)[0].digest.clone();
        apply_handles(&mut input, &BTreeMap::from([(digest, "file-x".into())]));

        assert_eq!(inline_bytes(&input), 0);
        assert!(inline_images(&input).is_empty());
    }

    /// Several images across several turns, which is what a computer-use
    /// session actually looks like by the time it matters.
    #[test]
    fn many_images_across_many_turns_all_resolve() {
        let mut input: Vec<Value> = (0..10)
            .map(|index| tool_result(format!("shot-{index}").as_bytes()))
            .collect();
        let handles: BTreeMap<String, String> = inline_images(&input)
            .into_iter()
            .enumerate()
            .map(|(index, image)| (image.digest, format!("file-{index}")))
            .collect();

        assert_eq!(apply_handles(&mut input, &handles), 10);
        assert_eq!(inline_bytes(&input), 0);
    }

    #[test]
    fn text_only_input_is_untouched() {
        let mut input = [json!({
            "type": "message", "role": "user",
            "content": [{"type": "input_text", "text": "no pictures here"}]
        })];
        let before = input.clone();
        assert!(inline_images(&input).is_empty());
        assert_eq!(apply_handles(&mut input, &BTreeMap::new()), 0);
        assert_eq!(input, before);
    }

    /// Malformed base64 must not panic or half-rewrite; it is simply not
    /// something we can address, so it stays as it is.
    #[test]
    fn an_undecodable_payload_is_left_alone() {
        let mut input = [json!({
            "type": "input_image",
            "image_url": "data:image/png;base64,!!!not base64!!!"
        })];
        assert!(inline_images(&input).is_empty());
        assert_eq!(apply_handles(&mut input, &BTreeMap::new()), 0);
        assert!(input[0].get("image_url").is_some());
    }
}
