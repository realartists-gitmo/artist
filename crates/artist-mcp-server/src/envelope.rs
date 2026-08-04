//! Durable request envelopes and operation recovery for MCP tool calls.
//!
//! A tunnel reconnect can retry a tool call after the original response was
//! lost. Calls carrying `_meta.idempotencyKey` are therefore committed to an
//! append-only, fsynced log. Reusing the key returns the exact stored MCP
//! response. The same records form the operation ledger exposed by the
//! `operation` administrative tool.

use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use serde::{Deserialize, Serialize};

/// One completed logical operation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecord {
    pub key: String,
    pub tool: String,
    pub arguments: serde_json::Value,
    pub result: serde_json::Value,
    pub completed_at_ms: u64,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// File-backed, append-only log of completed keyed calls.
#[derive(Default, Clone)]
pub struct Envelope {
    path: Option<Arc<PathBuf>>,
    seen: Arc<RwLock<HashMap<String, OperationRecord>>>,
}

impl Envelope {
    /// Open the store under `state_dir`, recovering every valid completed call.
    /// `None` keeps replay and recovery process-local for tests and stdio users
    /// that deliberately requested volatile state.
    pub fn open(state_dir: Option<&Path>) -> anyhow::Result<Self> {
        let Some(state_dir) = state_dir else {
            return Ok(Self::default());
        };
        let dir = state_dir.join("mcp");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("envelopes.jsonl");
        let mut seen = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                if let Ok(record) = serde_json::from_str::<OperationRecord>(line) {
                    seen.insert(record.key.clone(), record);
                }
            }
        }
        Ok(Self {
            path: Some(Arc::new(path)),
            seen: Arc::new(RwLock::new(seen)),
        })
    }

    /// The exact serialized MCP result for a completed key.
    pub fn replay(&self, key: &str) -> Option<serde_json::Value> {
        self.get(key).map(|record| record.result)
    }

    /// One completed operation by idempotency key.
    pub fn get(&self, key: &str) -> Option<OperationRecord> {
        self.seen
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(key)
            .cloned()
    }

    /// Most recent completed operations, newest first.
    pub fn list(&self, limit: usize) -> Vec<OperationRecord> {
        let mut records = self
            .seen
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| std::cmp::Reverse(record.completed_at_ms));
        records.truncate(limit);
        records
    }

    /// Record `result` durably, then publish it for replay and recovery.
    pub fn commit(
        &self,
        key: &str,
        tool: &str,
        arguments: &serde_json::Value,
        result: &serde_json::Value,
    ) -> anyhow::Result<()> {
        let record = OperationRecord {
            key: key.to_owned(),
            tool: tool.to_owned(),
            arguments: arguments.clone(),
            result: result.clone(),
            completed_at_ms: now(),
        };
        if let Some(path) = self.path.as_deref() {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            serde_json::to_writer(&mut file, &record)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        self.seen
            .write()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(key.to_owned(), record);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_operations_survive_reopen() {
        let state = tempfile::tempdir().unwrap();
        let store = Envelope::open(Some(state.path())).unwrap();
        store
            .commit(
                "build-1",
                "bash",
                &serde_json::json!({"command": "cargo build"}),
                &serde_json::json!({"content": []}),
            )
            .unwrap();
        drop(store);

        let reopened = Envelope::open(Some(state.path())).unwrap();
        let record = reopened.get("build-1").unwrap();
        assert_eq!(record.tool, "bash");
        assert_eq!(reopened.list(10).len(), 1);
    }
}
