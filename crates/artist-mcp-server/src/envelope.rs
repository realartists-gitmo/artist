//! Durable request envelopes: the replay shield for MCP tool calls.
//!
//! MCP's transport is not at-least-once. A stdio connection dropped by a tunnel
//! or a client that retries a call after a timeout is indistinguishable at the
//! MCP layer from a brand-new call, so without a memory of past calls a `bash`
//! command or an `edit` runs twice. Every call that carries an
//! `_meta.idempotencyKey` is recorded here *before* it is replayed — the record
//! is fsynced before the response is returned, so a crash after the client
//! acknowledged the response cannot lose it — and a later call with the same
//! key returns the stored result instead of running again.
//!
//! The key contract is the caller's: reusing a key means "give me the result of
//! that same logical operation", which is exactly what a reconnect wants and
//! exactly why a key must never be reused for two genuinely different calls.

use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use serde::{Deserialize, Serialize};

/// One record in the append-only log: the call and its result.
#[derive(Serialize, Deserialize)]
struct Record {
    key: String,
    tool: String,
    args: serde_json::Value,
    result: serde_json::Value,
    ts: u64,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// File-backed, append-only log of completed tool calls, keyed by idempotency
/// key. Read at open into an in-memory map for replay; appended (and fsynced)
/// on every commit.
#[derive(Default, Clone)]
pub struct Envelope {
    path: Option<Arc<PathBuf>>,
    seen: Arc<RwLock<HashMap<String, serde_json::Value>>>,
}

impl Envelope {
    /// Open the envelope store under `state_dir`, recovering every completed
    /// call from a previous process. `None` state dir means "volatile": replay
    /// works within this process, but nothing survives a restart.
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
                if let Ok(record) = serde_json::from_str::<Record>(line) {
                    seen.insert(record.key, record.result);
                }
            }
        }
        Ok(Self {
            path: Some(Arc::new(path)),
            seen: Arc::new(RwLock::new(seen)),
        })
    }

    /// The stored result for `key`, if a call with that key already completed.
    pub fn replay(&self, key: &str) -> Option<serde_json::Value> {
        self.seen
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(key)
            .cloned()
    }

    /// Record `result` for `key` durably, then expose it for replay.
    ///
    /// `result` is the serialized MCP [`rmcp::model::CallToolResult`], so a
    /// replay is byte-identical to the original response.
    pub fn commit(
        &self,
        key: &str,
        tool: &str,
        args: &serde_json::Value,
        result: &serde_json::Value,
    ) -> anyhow::Result<()> {
        let record = Record {
            key: key.to_owned(),
            tool: tool.to_owned(),
            args: args.clone(),
            result: result.clone(),
            ts: now(),
        };
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path.as_deref().expect("envelope has no state dir"))?;
        serde_json::to_writer(&mut file, &record)?;
        file.write_all(b"\n")?;
        // The fsync is the durability: without it the kernel may still be
        // holding the bytes when the process dies, and the replay promise —
        // "we saw this response once, you may not see it again" — quietly
        // breaks.
        file.sync_all()?;
        self.seen
            .write()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(key.to_owned(), result.clone());
        Ok(())
    }
}
