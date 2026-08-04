//! Durable, opaque continuation cursors for oversized tool results.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use artist_tool_api::PageInfo;
use rig_core::tool::ToolExecutionError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const MAX_INLINE_RESULT_BYTES: usize = 64 * 1024;
pub const DEFAULT_PAGE_BYTES: usize = 32 * 1024;
pub const MAX_PAGE_BYTES: usize = 64 * 1024;
const PREVIEW_BYTES: usize = 8 * 1024;
const CURSOR_TTL_MS: u64 = 24 * 60 * 60 * 1000;

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

impl PageStore {
    pub fn open(state_dir: Option<&Path>) -> anyhow::Result<Self> {
        let root = state_dir.map(|state| state.join("mcp-pages"));
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

        let artifact_id = Uuid::new_v4().simple().to_string();
        let cursor = Uuid::new_v4().simple().to_string();
        let artifact = Artifact {
            tool: tool.to_owned(),
            content_type: "application/json".into(),
            content: payload,
            created_at_ms: now_ms(),
        };
        let record = CursorRecord {
            artifact_id: artifact_id.clone(),
            offset: 0,
            expires_at_ms: now_ms().saturating_add(CURSOR_TTL_MS),
        };
        self.write_artifact(&artifact_id, &artifact)?;
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

    fn write_artifact(&self, id: &str, artifact: &Artifact) -> Result<(), ToolExecutionError> {
        if let Some(root) = &self.root {
            write_json_atomic(&root.join("artifacts").join(format!("{id}.json")), artifact)
                .map_err(|error| ToolExecutionError::other(error.to_string()))
        } else {
            self.memory
                .artifacts
                .lock()
                .expect("page artifact mutex poisoned")
                .insert(id.to_owned(), artifact.clone());
            Ok(())
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
}
