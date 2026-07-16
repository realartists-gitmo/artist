//! Slim SQLite persistence for per-agent mnemonic anchor state.
//!
//! Tables that belonged to RealArtist shells/commands are intentionally omitted.

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

                CREATE TABLE IF NOT EXISTS anchor_states (
                    agent_id TEXT NOT NULL,
                    canonical_path TEXT NOT NULL,
                    prefixes_json TEXT NOT NULL,
                    updated_at INTEGER NOT NULL,
                    PRIMARY KEY (agent_id, canonical_path)
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

                CREATE TABLE IF NOT EXISTS anchor_states (
                    agent_id TEXT NOT NULL,
                    canonical_path TEXT NOT NULL,
                    prefixes_json TEXT NOT NULL,
                    updated_at INTEGER NOT NULL,
                    PRIMARY KEY (agent_id, canonical_path)
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

    /// Load the full path → (anchor → packed binding) map for one agent.
    pub async fn load_anchor_state(
        &self,
        agent_id: &AgentId,
    ) -> Result<HashMap<String, HashMap<String, String>>, HashlineError> {
        let connection = self.connection.lock().await;
        let mut statement = connection
            .prepare("SELECT canonical_path, prefixes_json FROM anchor_states WHERE agent_id=?1")
            .map_err(sql_error)?;
        let mut rows = statement.query(params![agent_id.0]).map_err(sql_error)?;
        let mut result = HashMap::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            let path: String = row.get(0).map_err(sql_error)?;
            let json: String = row.get(1).map_err(sql_error)?;
            let prefixes = serde_json::from_str(&json).map_err(|error| {
                HashlineError::new(
                    HashlineErrorCode::Internal,
                    format!("invalid persisted anchor state for {path}: {error}"),
                    false,
                )
            })?;
            result.insert(path, prefixes);
        }
        Ok(result)
    }

    /// Replace all anchor state for an agent (delete-then-insert in one txn).
    pub async fn replace_anchor_state(
        &self,
        agent_id: &AgentId,
        state: &HashMap<String, HashMap<String, String>>,
    ) -> Result<(), HashlineError> {
        let now = now_ms();
        let mut connection = self.connection.lock().await;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        transaction
            .execute(
                "DELETE FROM anchor_states WHERE agent_id=?1",
                params![agent_id.0],
            )
            .map_err(sql_error)?;
        for (path, prefixes) in state {
            let json = serde_json::to_string(prefixes).map_err(|error| {
                HashlineError::new(HashlineErrorCode::Internal, error.to_string(), false)
            })?;
            transaction
                .execute(
                    "INSERT INTO anchor_states(agent_id, canonical_path, prefixes_json, updated_at) VALUES(?1, ?2, ?3, ?4)",
                    params![agent_id.0, path, json, now],
                )
                .map_err(sql_error)?;
        }
        transaction.commit().map_err(sql_error)?;
        Ok(())
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
    async fn round_trips_anchor_state() {
        let store = StateStore::open_in_memory().unwrap();
        let actor = AgentIdentity::new();
        store.register_agent(&actor).await.unwrap();

        let mut state = HashMap::new();
        let mut path_map = HashMap::new();
        path_map.insert("time".into(), "abc\u{1f}a".into());
        state.insert("/tmp/demo.rs".into(), path_map);

        store.replace_anchor_state(&actor.id, &state).await.unwrap();
        let loaded = store.load_anchor_state(&actor.id).await.unwrap();
        assert_eq!(loaded, state);
    }
}
