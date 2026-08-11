//! The ordinary Artist paging mechanism for oversized tool results.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use artist_tool_api::{
    ArtistDynamicTool, ArtistToolAnnotations, ArtistToolDefinition, PageInfo, ToolCategory,
    schema_for,
};
use base64::Engine as _;
use fs2::FileExt;
use rig_core::tool::{ToolExecutionError, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use teca::default_address;
use uuid::Uuid;

pub const MAX_INLINE_RESULT_BYTES: usize = 64 * 1024;
pub const DEFAULT_PAGE_BYTES: usize = 32 * 1024;
pub const MAX_PAGE_BYTES: usize = 64 * 1024;
const PREVIEW_BYTES: usize = 8 * 1024;
const CURSOR_TTL_MS: u64 = 24 * 60 * 60 * 1000;
/// How many TECA lexicon atoms render into an artifact path id.
const ARTIFACT_ID_ATOMS: usize = 5;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorRecord {
    artifact_id: String,
    offset: usize,
    expires_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Artifact {
    tool: String,
    content_type: String,
    content: String,
    created_at_ms: u64,
    /// Present when this artifact carries a binary/multimodal payload
    /// (screenshot, image, yield blob). Raw bytes stay out of model text and
    /// are only decoded on demand through the metadata or raw projections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    media: Option<ArtifactMedia>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactMedia {
    /// MIME type, e.g. `image/png`.
    media_type: String,
    /// Base64-encoded exact payload bytes.
    bytes: String,
    /// Deterministic content address of the payload bytes alone (TECA render),
    /// independent of the occurrence serial, so a payload revision stays
    /// addressable even when only metadata is projected.
    revision: String,
}

impl Artifact {
    fn payload_bytes(&self) -> usize {
        self.media
            .as_ref()
            .map(|media| decoded_len(&media.bytes))
            .unwrap_or_else(|| self.content.len())
    }
}

/// Safe, model-visible metadata for a durable artifact. The payload itself
/// stays behind the pagination cursor and is never duplicated into an
/// `artifact://` read; for media artifacts the exact bytes are likewise kept
/// out of model text and decoded only through the raw projection.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactInfo {
    pub id: String,
    pub tool: String,
    pub content_type: String,
    pub total_bytes: usize,
    pub created_at_ms: u64,
    /// Present when the artifact carries a binary/multimodal payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Content address of the payload bytes alone; the payload revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PageChunk {
    pub cursor: Option<String>,
    pub content: String,
    pub content_type: String,
    pub offset: usize,
    pub returned_bytes: usize,
    pub total_bytes: usize,
    pub has_more: bool,
    pub summary: String,
}

#[derive(Clone, Default)]
struct MemoryStore {
    cursors: Arc<Mutex<HashMap<String, CursorRecord>>>,
    artifacts: Arc<Mutex<HashMap<String, Artifact>>>,
}

#[derive(Clone)]
pub struct PageStore {
    root: Option<PathBuf>,
    memory: MemoryStore,
}

pub fn page_tool(store: PageStore) -> ArtistDynamicTool {
    ArtistDynamicTool::new(
        ArtistToolDefinition {
            name: "page".into(),
            title: "Read Result Page".into(),
            description: "Read the next bounded chunk of an oversized Artist tool result using the opaque cursor returned in that result's page metadata.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "cursor": {"type": "string", "description": "Opaque cursor returned by a previous tool result or page call."},
                    "maxBytes": {"type": "integer", "minimum": 1, "maximum": MAX_PAGE_BYTES, "default": DEFAULT_PAGE_BYTES}
                },
                "required": ["cursor"],
                "additionalProperties": false
            }),
            output_schema: schema_for::<PageChunk>(),
            category: ToolCategory::Administration,
            annotations: ArtistToolAnnotations::read_only(),
        },
        move |arguments| {
            let store = store.clone();
            Box::pin(async move {
                let cursor = arguments
                    .get("cursor")
                    .and_then(Value::as_str)
                    .filter(|cursor| !cursor.is_empty())
                    .ok_or_else(|| ToolExecutionError::invalid_args("cursor is required"))?;
                let max_bytes = arguments
                    .get("maxBytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(DEFAULT_PAGE_BYTES as u64)
                    .clamp(1, MAX_PAGE_BYTES as u64) as usize;
                let chunk = store.read(cursor, max_bytes)?;
                let structured = serde_json::to_value(&chunk)
                    .map_err(ToolExecutionError::from_error)?;
                Ok(artist_tool_api::ArtistToolOutput {
                    presentation: ToolOutput::text(format!("{}\n\n{}", chunk.content, chunk.summary)),
                    structured,
                })
            })
        },
    )
}

impl PageStore {
    pub fn memory() -> Self {
        Self {
            root: None,
            memory: MemoryStore::default(),
        }
    }

    pub fn open(state_dir: Option<&Path>) -> anyhow::Result<Self> {
        let root = state_dir.map(|state| state.join("pages"));
        if let Some(root) = &root {
            std::fs::create_dir_all(root.join("cursors"))?;
            std::fs::create_dir_all(root.join("artifacts"))?;
        }
        let store = Self {
            root,
            memory: MemoryStore::default(),
        };
        store.cleanup_expired();
        Ok(store)
    }

    pub fn paginate(
        &self,
        tool: &str,
        structured: &Value,
        rendered: &str,
    ) -> Result<Option<PageInfo>, ToolExecutionError> {
        let payload = serde_json::to_string(&serde_json::json!({
            "tool": tool,
            "structured": structured,
            "text": rendered,
        }))
        .map_err(|error| ToolExecutionError::other(error.to_string()))?;
        if payload.len() <= MAX_INLINE_RESULT_BYTES && rendered.len() <= MAX_INLINE_RESULT_BYTES {
            return Ok(None);
        }

        let cursor = Uuid::new_v4().simple().to_string();
        let artifact = Artifact {
            tool: tool.to_owned(),
            content_type: "application/json".into(),
            content: payload,
            media: None,
            created_at_ms: now_ms(),
        };
        // The id is derived from the exact canonical occurrence envelope plus
        // payload bytes; the occurrence serial is part of the TECA input
        // material, so identical bytes from distinct occurrences stay distinct
        // without an unrelated rendered serial suffix.
        let artifact_id = self.store_artifact(&artifact)?;
        let record = CursorRecord {
            artifact_id,
            offset: 0,
            expires_at_ms: now_ms().saturating_add(CURSOR_TTL_MS),
        };
        self.write_cursor(&cursor, &record)?;

        let preview_source = if rendered.is_empty() {
            &artifact.content
        } else {
            rendered
        };
        let preview = prefix(preview_source, PREVIEW_BYTES);
        Ok(Some(PageInfo {
            cursor,
            returned_bytes: preview.len(),
            total_bytes: artifact.content.len(),
            has_more: true,
            content_type: artifact.content_type,
            summary: format!(
                "Oversized {tool} result stored as a durable paged artifact ({} bytes total).",
                artifact.content.len()
            ),
            preview,
        }))
    }

    /// Enumerate retained artifact metadata in a stable order.
    pub fn artifacts(&self) -> Result<Vec<ArtifactInfo>, ToolExecutionError> {
        let mut artifacts = if let Some(root) = &self.root {
            std::fs::read_dir(root.join("artifacts"))
                .map_err(|error| ToolExecutionError::other(error.to_string()))?
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let path = entry.path();
                    (path.is_file()
                        && path
                            .extension()
                            .is_some_and(|extension| extension == "json"))
                    .then_some(path)
                })
                .filter_map(|path| {
                    let id = path.file_stem()?.to_str()?.to_owned();
                    read_json::<Artifact>(&path)
                        .ok()
                        .flatten()
                        .map(|artifact| artifact_info(id, artifact))
                })
                .collect::<Vec<_>>()
        } else {
            self.memory
                .artifacts
                .lock()
                .expect("page artifact mutex poisoned")
                .iter()
                .map(|(id, artifact)| artifact_info(id.clone(), artifact.clone()))
                .collect()
        };
        artifacts.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(artifacts)
    }

    /// Retrieve metadata for one opaque artifact id.
    pub fn artifact(&self, id: &str) -> Result<Option<ArtifactInfo>, ToolExecutionError> {
        if !valid_artifact_id(id) {
            return Ok(None);
        }
        Ok(self
            .read_artifact(id)?
            .map(|artifact| artifact_info(id.to_owned(), artifact)))
    }

    pub fn read(&self, cursor: &str, max_bytes: usize) -> Result<PageChunk, ToolExecutionError> {
        let record = self.read_cursor(cursor)?.ok_or_else(|| {
            ToolExecutionError::not_found("pagination cursor was not found or has expired")
                .with_code("cursor_invalid")
        })?;
        if record.expires_at_ms < now_ms() {
            self.delete_cursor(cursor);
            return Err(
                ToolExecutionError::not_found("pagination cursor has expired")
                    .with_code("cursor_stale"),
            );
        }
        let artifact = self.read_artifact(&record.artifact_id)?.ok_or_else(|| {
            ToolExecutionError::not_found("paged artifact no longer exists")
                .with_code("cursor_stale")
        })?;
        if record.offset > artifact.content.len() {
            return Err(
                ToolExecutionError::invalid_args("pagination cursor offset is invalid")
                    .with_code("cursor_invalid"),
            );
        }

        let max_bytes = max_bytes.clamp(1, MAX_PAGE_BYTES);
        let end = char_boundary_at_or_before(
            &artifact.content,
            record
                .offset
                .saturating_add(max_bytes)
                .min(artifact.content.len()),
        );
        let content = artifact.content[record.offset..end].to_owned();
        let has_more = end < artifact.content.len();
        let next_cursor = if has_more {
            let next = Uuid::new_v4().simple().to_string();
            self.write_cursor(
                &next,
                &CursorRecord {
                    artifact_id: record.artifact_id.clone(),
                    offset: end,
                    expires_at_ms: now_ms().saturating_add(CURSOR_TTL_MS),
                },
            )?;
            Some(next)
        } else {
            None
        };

        Ok(PageChunk {
            cursor: next_cursor,
            returned_bytes: content.len(),
            total_bytes: artifact.content.len(),
            offset: record.offset,
            has_more,
            content_type: artifact.content_type,
            summary: if has_more {
                format!(
                    "Returned bytes {}..{} of {}.",
                    record.offset,
                    end,
                    artifact.content.len()
                )
            } else {
                format!("Returned the final {} bytes.", content.len())
            },
            content,
        })
    }

    fn store_artifact(&self, artifact: &Artifact) -> Result<String, ToolExecutionError> {
        if let Some(root) = &self.root {
            let directory = root.join("artifacts");
            std::fs::create_dir_all(&directory)
                .map_err(|error| ToolExecutionError::other(error.to_string()))?;
            let lock_path = directory.join(".lock");
            let lock = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(lock_path)
                .map_err(|error| ToolExecutionError::other(error.to_string()))?;
            lock.lock_exclusive()
                .map_err(|error| ToolExecutionError::other(error.to_string()))?;
            let id = next_artifact_id(artifact, 1, |candidate| {
                directory.join(format!("{candidate}.json")).exists()
            })?;
            let result = write_json_atomic(&directory.join(format!("{id}.json")), artifact)
                .map_err(|error| ToolExecutionError::other(error.to_string()));
            let _ = FileExt::unlock(&lock);
            result.map(|()| id)
        } else {
            let mut artifacts = self
                .memory
                .artifacts
                .lock()
                .expect("page artifact mutex poisoned");
            let id = next_artifact_id(artifact, 1, |candidate| artifacts.contains_key(candidate))?;
            artifacts.insert(id.clone(), artifact.clone());
            Ok(id)
        }
    }

    fn read_artifact(&self, id: &str) -> Result<Option<Artifact>, ToolExecutionError> {
        if let Some(root) = &self.root {
            read_json(&root.join("artifacts").join(format!("{id}.json")))
                .map_err(|error| ToolExecutionError::other(error.to_string()))
        } else {
            Ok(self
                .memory
                .artifacts
                .lock()
                .expect("page artifact mutex poisoned")
                .get(id)
                .cloned())
        }
    }

    /// Capture a binary/multimodal payload (screenshot, image, yield blob) as a
    /// durable artifact, returning its TECA-derived id. Occurrence distinction
    /// lives in the TECA input material: identical bytes from a distinct
    /// occurrence render a different id. The payload revision is the TECA
    /// content address of the raw bytes alone.
    pub fn capture_media(
        &self,
        tool: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> Result<String, ToolExecutionError> {
        let artifact = Artifact {
            tool: tool.to_owned(),
            content_type: media_type.to_owned(),
            content: String::new(),
            media: Some(ArtifactMedia {
                media_type: media_type.to_owned(),
                bytes: base64::engine::general_purpose::STANDARD.encode(bytes),
                revision: payload_revision(bytes),
            }),
            created_at_ms: now_ms(),
        };
        self.store_artifact(&artifact)
    }

    /// Retrieve the raw payload bytes of a media artifact, decoded. Returns
    /// `None` for an unknown id or a text-backed artifact.
    pub fn media_bytes(&self, id: &str) -> Result<Option<Vec<u8>>, ToolExecutionError> {
        if !valid_artifact_id(id) {
            return Ok(None);
        }
        Ok(self.read_artifact(id)?.and_then(|artifact| {
            artifact.media.map(|media| {
                base64::engine::general_purpose::STANDARD
                    .decode(&media.bytes)
                    .unwrap_or_default()
            })
        }))
    }

    fn write_cursor(&self, cursor: &str, record: &CursorRecord) -> Result<(), ToolExecutionError> {
        if let Some(root) = &self.root {
            write_json_atomic(&root.join("cursors").join(format!("{cursor}.json")), record)
                .map_err(|error| ToolExecutionError::other(error.to_string()))
        } else {
            self.memory
                .cursors
                .lock()
                .expect("page cursor mutex poisoned")
                .insert(cursor.to_owned(), record.clone());
            Ok(())
        }
    }

    fn read_cursor(&self, cursor: &str) -> Result<Option<CursorRecord>, ToolExecutionError> {
        if !cursor.bytes().all(|byte| byte.is_ascii_hexdigit()) || cursor.len() != 32 {
            return Ok(None);
        }
        if let Some(root) = &self.root {
            read_json(&root.join("cursors").join(format!("{cursor}.json")))
                .map_err(|error| ToolExecutionError::other(error.to_string()))
        } else {
            Ok(self
                .memory
                .cursors
                .lock()
                .expect("page cursor mutex poisoned")
                .get(cursor)
                .cloned())
        }
    }

    fn delete_cursor(&self, cursor: &str) {
        if let Some(root) = &self.root {
            let _ = std::fs::remove_file(root.join("cursors").join(format!("{cursor}.json")));
        } else {
            self.memory
                .cursors
                .lock()
                .expect("page cursor mutex poisoned")
                .remove(cursor);
        }
    }

    fn cleanup_expired(&self) {
        let Some(root) = &self.root else {
            return;
        };
        let now = now_ms();
        if let Ok(entries) = std::fs::read_dir(root.join("cursors")) {
            for entry in entries.flatten() {
                let path = entry.path();
                let expired = read_json::<CursorRecord>(&path)
                    .ok()
                    .flatten()
                    .is_none_or(|record| record.expires_at_ms < now);
                if expired {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        if let Ok(entries) = std::fs::read_dir(root.join("artifacts")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let expired = read_json::<Artifact>(&path)
                    .ok()
                    .flatten()
                    .is_none_or(|artifact| {
                        artifact.created_at_ms.saturating_add(CURSOR_TTL_MS) < now
                    });
                if expired {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
}

fn valid_artifact_id(id: &str) -> bool {
    // Pre-migration UUID artifacts remain readable. Current IDs are a
    // canonical `a-<teca-atoms-joined-by-dashes>` render.
    if id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return true;
    }
    let Some(rest) = id.strip_prefix("a-") else {
        return false;
    };
    !rest.is_empty()
        && rest.split('-').all(|atom| {
            !atom.is_empty()
                && atom
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte >= 0x80)
        })
}

/// Derive a deterministic, content-addressed artifact id from the canonical
/// occurrence envelope plus the exact payload bytes.
///
/// The occurrence serial is part of the TECA input material, not a separate
/// rendered suffix: identical bytes emitted in a distinct occurrence render a
/// completely different id because their envelope bytes differ. A fixed prefix
/// of the TECA address stream is joined with `-`, which never appears in a
/// lexicon atom, so the render is injective over atom sequences.
fn artifact_base_id(artifact: &Artifact, occurrence: u64) -> Result<String, ToolExecutionError> {
    let mut envelope = serde_json::json!({
        "tool": artifact.tool,
        "contentType": artifact.content_type,
        "content": artifact.content,
        "occurrence": occurrence,
    });
    if let Some(media) = &artifact.media {
        envelope["media"] = serde_json::json!(media.bytes);
    }
    let envelope = serde_json::to_vec(&envelope)
        .map_err(|error| ToolExecutionError::other(error.to_string()))?;
    let mut id = String::from("a-");
    for (index, atom) in default_address(&envelope)
        .take(ARTIFACT_ID_ATOMS)
        .enumerate()
    {
        if index != 0 {
            id.push('-');
        }
        id.push_str(std::str::from_utf8(atom).expect("TECA lexicon atoms are valid UTF-8"));
    }
    Ok(id)
}

/// Allocate an id for `artifact`, bumping the occurrence serial only when the
/// prior envelope's render already exists.
fn next_artifact_id(
    artifact: &Artifact,
    mut occurrence: u64,
    occupied: impl Fn(&str) -> bool,
) -> Result<String, ToolExecutionError> {
    loop {
        let candidate = artifact_base_id(artifact, occurrence)?;
        if !occupied(&candidate) {
            return Ok(candidate);
        }
        occurrence = occurrence.saturating_add(1);
    }
}

fn artifact_info(id: String, artifact: Artifact) -> ArtifactInfo {
    let media = artifact.media.as_ref();
    let total_bytes = artifact.payload_bytes();
    ArtifactInfo {
        id,
        tool: artifact.tool,
        content_type: artifact.content_type,
        total_bytes,
        created_at_ms: artifact.created_at_ms,
        media_type: media.map(|media| media.media_type.clone()),
        revision: media.map(|media| media.revision.clone()),
    }
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    let parent = path.parent().expect("page path has a parent");
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, serde_json::to_vec(value)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn payload_revision(bytes: &[u8]) -> String {
    let mut id = String::from("a-");
    for (index, atom) in default_address(bytes).take(ARTIFACT_ID_ATOMS).enumerate() {
        if index != 0 {
            id.push('-');
        }
        id.push_str(std::str::from_utf8(atom).expect("TECA lexicon atoms are valid UTF-8"));
    }
    id
}

/// Decoded byte length of a base64 string without allocating.
fn decoded_len(encoded: &str) -> usize {
    let padding = encoded
        .len()
        .saturating_sub(encoded.trim_end_matches('=').len());
    (encoded.len() / 4)
        .saturating_mul(3)
        .saturating_sub(padding)
}

fn prefix(value: &str, max_bytes: usize) -> String {
    let end = char_boundary_at_or_before(value, value.len().min(max_bytes));
    value[..end].to_owned()
}

fn char_boundary_at_or_before(value: &str, mut position: usize) -> usize {
    while position > 0 && !value.is_char_boundary(position) {
        position -= 1;
    }
    position
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_results_survive_reopen_and_page_to_completion() {
        let state = tempfile::tempdir().unwrap();
        let store = PageStore::open(Some(state.path())).unwrap();
        let text = "x".repeat(MAX_INLINE_RESULT_BYTES + 1234);
        let page = store
            .paginate("read", &serde_json::json!({"text": text}), &text)
            .unwrap()
            .unwrap();
        drop(store);

        let store = PageStore::open(Some(state.path())).unwrap();
        let mut cursor = Some(page.cursor);
        let mut received = String::new();
        while let Some(current) = cursor {
            let chunk = store.read(&current, 4096).unwrap();
            received.push_str(&chunk.content);
            cursor = chunk.cursor;
        }
        assert!(received.contains(&"x".repeat(1024)));
        assert!(received.len() > MAX_INLINE_RESULT_BYTES);
    }

    #[test]
    fn page_cursors_are_safe_to_retry() {
        let store = PageStore::open(None).unwrap();
        let text = "y".repeat(MAX_INLINE_RESULT_BYTES + 5000);
        let page = store
            .paginate("bash", &serde_json::json!({"text": text}), &text)
            .unwrap()
            .unwrap();
        let first = store.read(&page.cursor, 4096).unwrap();
        let retry = store.read(&page.cursor, 4096).unwrap();
        assert_eq!(first.content, retry.content);
        assert_eq!(first.offset, retry.offset);
        assert_eq!(first.returned_bytes, retry.returned_bytes);
    }

    #[test]
    fn malformed_cursors_are_rejected() {
        let store = PageStore::open(None).unwrap();
        let error = store.read("../secret", 10).unwrap_err();
        assert_eq!(error.code(), Some("cursor_invalid"));
    }

    #[test]
    fn artifact_metadata_survives_reopen_without_exposing_payload() {
        let state = tempfile::tempdir().unwrap();
        let store = PageStore::open(Some(state.path())).unwrap();
        let secret = "private artifact payload".repeat(5_000);
        let page = store
            .paginate("read", &serde_json::json!({"text": secret}), &secret)
            .unwrap()
            .unwrap();
        let artifact_id = store
            .read_cursor(&page.cursor)
            .unwrap()
            .unwrap()
            .artifact_id;
        let artifact_path = state
            .path()
            .join("pages/artifacts")
            .join(format!("{artifact_id}.json"));
        assert!(artifact_path.exists());
        assert!(read_json::<Artifact>(&artifact_path).unwrap().is_some());
        drop(store);

        let store = PageStore::open(Some(state.path())).unwrap();
        assert!(
            artifact_path.exists(),
            "artifact removed during reopen cleanup"
        );
        let info = store.artifact(&artifact_id).unwrap().unwrap();
        assert_eq!(info.id, artifact_id);
        assert_eq!(info.tool, "read");
        assert!(info.total_bytes > MAX_INLINE_RESULT_BYTES);
        assert_eq!(store.artifacts().unwrap(), vec![info]);
    }

    #[test]
    fn artifact_ids_are_teca_derived_with_occurrence_in_the_input_material() {
        let store = PageStore::memory();
        let text = "same occurrence bytes".repeat(5_000);
        let first = store
            .paginate("read", &serde_json::json!({"text": text}), &text)
            .unwrap()
            .unwrap();
        let second = store
            .paginate("read", &serde_json::json!({"text": text}), &text)
            .unwrap()
            .unwrap();
        let first_id = store
            .read_cursor(&first.cursor)
            .unwrap()
            .unwrap()
            .artifact_id;
        let second_id = store
            .read_cursor(&second.cursor)
            .unwrap()
            .unwrap()
            .artifact_id;

        assert!(first_id.starts_with("a-"));
        assert!(valid_artifact_id(&first_id));
        assert!(valid_artifact_id(&second_id));
        // The occurrence serial is part of the TECA input material, so a
        // second occurrence of identical bytes renders a different id rather
        // than a `-2` suffix on the same rendered id.
        assert_ne!(first_id, second_id);
    }

    #[test]
    fn artifact_ids_are_deterministic_across_stores() {
        let store = PageStore::memory();
        let text = "stable occurrence bytes".repeat(5_000);
        let page = store
            .paginate("read", &serde_json::json!({"text": text}), &text)
            .unwrap()
            .unwrap();
        let first_id = store
            .read_cursor(&page.cursor)
            .unwrap()
            .unwrap()
            .artifact_id;

        let other = PageStore::memory();
        let page = other
            .paginate("read", &serde_json::json!({"text": text}), &text)
            .unwrap()
            .unwrap();
        let other_id = other
            .read_cursor(&page.cursor)
            .unwrap()
            .unwrap()
            .artifact_id;

        assert_eq!(first_id, other_id);
        assert!(first_id.contains('-'));
        assert!(!first_id.contains('/'));
    }

    #[test]
    fn media_artifacts_round_trip_through_disk_with_metadata() {
        let state = tempfile::tempdir().unwrap();
        let store = PageStore::open(Some(state.path())).unwrap();
        let png = b"\x89PNG\r\n\x1a\nscreenshot bytes".to_vec();
        let id = store.capture_media("computer", "image/png", &png).unwrap();
        assert!(valid_artifact_id(&id));
        assert!(
            store
                .media_bytes(&id)
                .unwrap()
                .is_some_and(|bytes| bytes == png)
        );
        let info = store.artifact(&id).unwrap().unwrap();
        assert_eq!(info.media_type.as_deref(), Some("image/png"));
        assert_eq!(info.total_bytes, png.len());
        assert!(
            info.revision
                .as_deref()
                .is_some_and(|revision| revision.starts_with('a'))
        );
        assert_eq!(store.artifacts().unwrap(), vec![info]);
        drop(store);

        let store = PageStore::open(Some(state.path())).unwrap();
        assert!(
            store
                .media_bytes(&id)
                .unwrap()
                .is_some_and(|bytes| bytes == png)
        );
        assert_eq!(store.artifact(&id).unwrap().unwrap().total_bytes, png.len());
    }

    #[test]
    fn recurring_media_payloads_stay_occurrence_distinct_in_teca_material() {
        let store = PageStore::memory();
        let png = b"recurring frame bytes".to_vec();
        let first = store.capture_media("computer", "image/png", &png).unwrap();
        let second = store.capture_media("computer", "image/png", &png).unwrap();
        assert_ne!(first, second);
        assert_eq!(store.media_bytes(&first).unwrap(), Some(png.clone()));
        assert_eq!(store.media_bytes(&second).unwrap(), Some(png));
    }

    #[test]
    fn media_artifact_revision_is_the_payload_content_address() {
        let store = PageStore::memory();
        let one = b"revision addressed bytes".to_vec();
        let two = b"revision addressed bytes".to_vec();
        let first = store.capture_media("read", "image/webp", &one).unwrap();
        let second = store.capture_media("bash", "image/webp", &two).unwrap();
        let first_info = store.artifact(&first).unwrap().unwrap();
        let second_info = store.artifact(&second).unwrap().unwrap();
        assert_eq!(first_info.revision, second_info.revision);
        assert_ne!(first_info.id, second_info.id);
        assert_eq!(first_info.tool, "read");
        assert_eq!(second_info.tool, "bash");
    }

    #[test]
    fn text_and_media_artifacts_share_one_store() {
        let store = PageStore::memory();
        let text = "mixed store text".repeat(5_000);
        let page = store
            .paginate("read", &serde_json::json!({"text": text}), &text)
            .unwrap()
            .unwrap();
        let text_id = store
            .read_cursor(&page.cursor)
            .unwrap()
            .unwrap()
            .artifact_id;
        assert!(
            store
                .artifact(&text_id)
                .unwrap()
                .unwrap()
                .media_type
                .is_none()
        );
        let media_id = store.capture_media("canvas", "image/png", b"png").unwrap();
        assert!(store.media_bytes(&media_id).unwrap().is_some());
        assert_eq!(store.artifacts().unwrap().len(), 2);
    }
}
