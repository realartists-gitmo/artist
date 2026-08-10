//! SQLite persistence for coordination metadata only.
//!
//! Anchor identity/address state is deliberately absent: v1 anchors are stateless.

use std::{
    collections::HashMap,
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, TransactionBehavior};
use tokio::sync::Mutex;

use crate::{AgentId, AgentIdentity, HashlineError, HashlineErrorCode};

#[derive(Clone)]
pub struct StateStore {
    connection: Arc<Mutex<Connection>>,
}

impl StateStore {
    pub fn open(path: &Path) -> Result<Self, HashlineError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        let connection = Connection::open(path).map_err(sql_error)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(sql_error)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(sql_error)?;
        connection
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS agents (
                    agent_id TEXT PRIMARY KEY,
                    created_at INTEGER NOT NULL,
                    last_seen_at INTEGER NOT NULL
                );

                -- v1 anchors are stateless. Remove any persisted allocator state
                -- left by pre-v1 builds so it cannot accidentally become authoritative.
                DROP TABLE IF EXISTS anchor_states;

                -- Who last wrote each path, and what they left there. Shared
                -- across processes, which is the whole point: an in-memory map
                -- can only attribute writes made by this process, so a second
                -- artist in the same worktree was invisible.
                --
                -- `content_hash` is what makes attribution a claim rather than
                -- a record. An agent wrote this path once; the user may have
                -- edited it since, and naming the agent then would send the
                -- model coordinating with one that did nothing.
                CREATE TABLE IF NOT EXISTS file_writers (
                    canonical_path TEXT PRIMARY KEY,
                    agent_id TEXT NOT NULL,
                    content_hash TEXT NOT NULL,
                    written_at INTEGER NOT NULL
                );
                "#,
            )
            .map_err(sql_error)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Open an in-memory store (useful for tests and single-shot harnesses).
    pub fn open_in_memory() -> Result<Self, HashlineError> {
        let connection = Connection::open_in_memory().map_err(sql_error)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(sql_error)?;
        connection
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS agents (
                    agent_id TEXT PRIMARY KEY,
                    created_at INTEGER NOT NULL,
                    last_seen_at INTEGER NOT NULL
                );

                -- v1 anchors are stateless. Remove any persisted allocator state
                -- left by pre-v1 builds so it cannot accidentally become authoritative.
                DROP TABLE IF EXISTS anchor_states;

                -- Who last wrote each path, and what they left there. Shared
                -- across processes, which is the whole point: an in-memory map
                -- can only attribute writes made by this process, so a second
                -- artist in the same worktree was invisible.
                --
                -- `content_hash` is what makes attribution a claim rather than
                -- a record. An agent wrote this path once; the user may have
                -- edited it since, and naming the agent then would send the
                -- model coordinating with one that did nothing.
                CREATE TABLE IF NOT EXISTS file_writers (
                    canonical_path TEXT PRIMARY KEY,
                    agent_id TEXT NOT NULL,
                    content_hash TEXT NOT NULL,
                    written_at INTEGER NOT NULL
                );
                "#,
            )
            .map_err(sql_error)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    pub async fn register_agent(&self, actor: &AgentIdentity) -> Result<(), HashlineError> {
        let now = now_ms();
        self.connection
            .lock()
            .await
            .execute(
                "INSERT INTO agents(agent_id, created_at, last_seen_at) VALUES(?1, ?2, ?2)
                 ON CONFLICT(agent_id) DO UPDATE SET last_seen_at=excluded.last_seen_at",
                params![actor.id.0, now],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    /// Record who wrote a path and what they left there.
    ///
    pub async fn record_writer(
        &self,
        path: &str,
        agent_id: &AgentId,
        content_hash: &str,
    ) -> Result<(), HashlineError> {
        let now = now_ms();
        self.connection
            .lock()
            .await
            .execute(
                "INSERT INTO file_writers(canonical_path, agent_id, content_hash, written_at)
                 VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(canonical_path) DO UPDATE SET
                     agent_id=excluded.agent_id,
                     content_hash=excluded.content_hash,
                     written_at=excluded.written_at",
                params![path, agent_id.0, content_hash, now],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    /// Who wrote each of `paths`, and the hash they left.
    ///
    /// Queried only for paths that have actually drifted, which is rare — so
    /// this never runs on the hot path even though the write side does.
    pub async fn writers_for(
        &self,
        paths: &[String],
    ) -> Result<HashMap<String, (String, String)>, HashlineError> {
        if paths.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = std::iter::repeat_n("?", paths.len())
            .collect::<Vec<_>>()
            .join(",");
        let connection = self.connection.lock().await;
        let mut statement = connection
            .prepare(&format!(
                "SELECT canonical_path, agent_id, content_hash FROM file_writers
                 WHERE canonical_path IN ({placeholders})"
            ))
            .map_err(sql_error)?;
        let mut rows = statement
            .query(rusqlite::params_from_iter(paths.iter()))
            .map_err(sql_error)?;
        let mut result = HashMap::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            let path: String = row.get(0).map_err(sql_error)?;
            let agent: String = row.get(1).map_err(sql_error)?;
            let hash: String = row.get(2).map_err(sql_error)?;
            result.insert(path, (agent, hash));
        }
        Ok(result)
    }

    /// Everything a session leaves behind, gone with the session.
    ///
    /// Exact rather than heuristic: the caller knows this conversation is over,
    /// so nothing here is a guess about whether the state is still wanted. The
    /// write attributions go too — they are keyed by path rather than by agent,
    /// so nothing else would ever collect them.
    pub async fn forget_agent(&self, agent_id: &str) -> Result<usize, HashlineError> {
        let mut connection = self.connection.lock().await;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let mut removed = transaction
            .execute(
                "DELETE FROM file_writers WHERE agent_id=?1",
                params![agent_id],
            )
            .map_err(sql_error)?;
        removed += transaction
            .execute("DELETE FROM agents WHERE agent_id=?1", params![agent_id])
            .map_err(sql_error)?;
        transaction.commit().map_err(sql_error)?;
        Ok(removed)
    }

    /// Retire registration rows for conversations nobody has touched in `days`.
    ///
    /// `agents.last_seen_at` is upserted on every read, write and edit, so it
    /// tracks use rather than creation — an old conversation still in daily use
    /// is not stale.
    pub async fn retire_idle_agents(&self, days: u64) -> Result<usize, HashlineError> {
        let cutoff = now_ms().saturating_sub(days.saturating_mul(24 * 60 * 60 * 1000) as i64);
        let mut connection = self.connection.lock().await;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let removed = transaction
            .execute(
                "DELETE FROM agents WHERE last_seen_at < ?1",
                params![cutoff],
            )
            .map_err(sql_error)?;
        transaction.commit().map_err(sql_error)?;
        Ok(removed)
    }

    /// Keep only the `max` most recently used conversation registration rows.
    pub async fn cap_agents(&self, max: usize) -> Result<usize, HashlineError> {
        let mut connection = self.connection.lock().await;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let removed = transaction
            .execute(
                "DELETE FROM agents WHERE agent_id NOT IN
                 (SELECT agent_id FROM agents ORDER BY last_seen_at DESC LIMIT ?1)",
                params![max as i64],
            )
            .map_err(sql_error)?;
        transaction.commit().map_err(sql_error)?;
        Ok(removed)
    }

    /// Forget who wrote what, past `days`.
    ///
    /// Attribution is only useful
    /// while a change is recent — nobody needs telling that another session
    /// touched a file last month. A missing row degrades to unattributed, which
    /// is the safe direction, so this can afford to be aggressive.
    pub async fn forget_stale_writers(&self, days: u64) -> Result<usize, HashlineError> {
        let cutoff = now_ms().saturating_sub(days.saturating_mul(24 * 60 * 60 * 1000) as i64);
        let removed = self
            .connection
            .lock()
            .await
            .execute(
                "DELETE FROM file_writers WHERE written_at < ?1",
                params![cutoff],
            )
            .map_err(sql_error)?;
        Ok(removed)
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn sql_error(error: rusqlite::Error) -> HashlineError {
    HashlineError::new(
        HashlineErrorCode::Internal,
        format!("sqlite error: {error}"),
        false,
    )
}

fn io_error(error: std::io::Error) -> HashlineError {
    HashlineError::new(HashlineErrorCode::Io, format!("io error: {error}"), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writer_attribution_round_trips_without_anchor_state() {
        let store = StateStore::open_in_memory().unwrap();
        let actor = AgentIdentity::new();
        store.register_agent(&actor).await.unwrap();
        store
            .record_writer("/tmp/demo.rs", &actor.id, "deadbeef")
            .await
            .unwrap();
        let writers = store
            .writers_for(&["/tmp/demo.rs".to_owned()])
            .await
            .unwrap();
        assert_eq!(
            writers.get("/tmp/demo.rs"),
            Some(&(actor.id.0.clone(), "deadbeef".to_owned()))
        );
    }
}
