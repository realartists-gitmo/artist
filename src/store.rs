use std::path::Path;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;
use uuid::Uuid;

use crate::domain::{Event, EventData, Node, NodeId, Run, RunId};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid stored timestamp: {0}")]
    Timestamp(#[from] chrono::ParseError),
    #[error("invalid stored UUID: {0}")]
    Uuid(#[from] uuid::Error),
    #[error("node {0} not found")]
    NodeNotFound(NodeId),
    #[error("run {0} not found")]
    RunNotFound(RunId),
    #[error("corrupt run: {0}")]
    CorruptRun(String),
    #[error("node IDs and name/version pairs are immutable")]
    ImmutableNode,
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// SQLite event store. A single event append is one durable transaction.
pub struct SqliteStore {
    connection: Mutex<Connection>,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        let store = Self {
            connection: Mutex::new(connection),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        let store = Self {
            connection: Mutex::new(connection),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.connection.lock().execute_batch(
            "BEGIN;
             CREATE TABLE IF NOT EXISTS nodes (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                version INTEGER NOT NULL,
                body TEXT NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE(name, version)
             );
             CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                occurred_at TEXT NOT NULL,
                body TEXT NOT NULL,
                UNIQUE(run_id, sequence)
             );
             CREATE INDEX IF NOT EXISTS events_run ON events(run_id, sequence);
             COMMIT;",
        )?;
        Ok(())
    }

    pub fn insert_node(&self, node: &Node) -> Result<()> {
        let body = serde_json::to_string(node)?;
        let result = self.connection.lock().execute(
            "INSERT INTO nodes(id, name, version, body, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                node.id.to_string(),
                node.name,
                node.version,
                body,
                node.created_at.to_rfc3339()
            ],
        );
        match result {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(StoreError::ImmutableNode)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn get_node(&self, id: NodeId) -> Result<Node> {
        let body: Option<String> = self
            .connection
            .lock()
            .query_row(
                "SELECT body FROM nodes WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        body.map(|body| serde_json::from_str(&body))
            .transpose()?
            .ok_or(StoreError::NodeNotFound(id))
    }

    pub fn latest_node(&self, name: &str) -> Result<Option<Node>> {
        let body: Option<String> = self
            .connection
            .lock()
            .query_row(
                "SELECT body FROM nodes WHERE name = ?1 ORDER BY version DESC LIMIT 1",
                [name],
                |row| row.get(0),
            )
            .optional()?;
        Ok(body.map(|body| serde_json::from_str(&body)).transpose()?)
    }

    pub fn list_nodes(&self) -> Result<Vec<Node>> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare("SELECT body FROM nodes ORDER BY name, version")?;
        let bodies = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        bodies
            .into_iter()
            .map(|body| serde_json::from_str(&body).map_err(Into::into))
            .collect()
    }

    pub fn append(&self, run_id: RunId, data: EventData) -> Result<Event> {
        Ok(self.append_batch(vec![(run_id, data)])?.remove(0))
    }

    /// Appends several events in one transaction. This makes fork/replace graph changes atomic.
    pub fn append_batch(&self, entries: Vec<(RunId, EventData)>) -> Result<Vec<Event>> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let occurred_at = Utc::now();
        let mut appended = Vec::with_capacity(entries.len());
        for (run_id, data) in entries {
            let next: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(sequence), 0) + 1 FROM events WHERE run_id = ?1",
                [run_id.to_string()],
                |row| row.get(0),
            )?;
            let body = serde_json::to_string(&data)?;
            transaction.execute(
                "INSERT INTO events(run_id, sequence, occurred_at, body) VALUES (?1, ?2, ?3, ?4)",
                params![run_id.to_string(), next, occurred_at.to_rfc3339(), body],
            )?;
            appended.push(Event {
                id: transaction.last_insert_rowid(),
                run_id,
                sequence: next as u64,
                occurred_at,
                data,
            });
        }
        transaction.commit()?;
        Ok(appended)
    }

    pub fn events(&self, run_id: RunId) -> Result<Vec<Event>> {
        self.events_through(run_id, None)
    }

    pub fn events_through(&self, run_id: RunId, through: Option<u64>) -> Result<Vec<Event>> {
        let connection = self.connection.lock();
        let sql = if through.is_some() {
            "SELECT id, sequence, occurred_at, body FROM events WHERE run_id = ?1 AND sequence <= ?2 ORDER BY sequence"
        } else {
            "SELECT id, sequence, occurred_at, body FROM events WHERE run_id = ?1 ORDER BY sequence"
        };
        let mut statement = connection.prepare(sql)?;
        let parse = |row: &rusqlite::Row<'_>| -> rusqlite::Result<(i64, i64, String, String)> {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        };
        let rows = if let Some(through) = through {
            statement
                .query_map(params![run_id.to_string(), through as i64], parse)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            statement
                .query_map([run_id.to_string()], parse)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        rows.into_iter()
            .map(|(id, sequence, occurred_at, body)| {
                Ok(Event {
                    id,
                    run_id,
                    sequence: sequence as u64,
                    occurred_at: DateTime::parse_from_rfc3339(&occurred_at)?.with_timezone(&Utc),
                    data: serde_json::from_str(&body)?,
                })
            })
            .collect()
    }

    pub fn run(&self, run_id: RunId) -> Result<Run> {
        let events = self.events(run_id)?;
        if events.is_empty() {
            return Err(StoreError::RunNotFound(run_id));
        }
        Run::rebuild(&events).map_err(StoreError::CorruptRun)
    }

    pub fn list_runs(&self) -> Result<Vec<Run>> {
        let ids = {
            let connection = self.connection.lock();
            let mut statement =
                connection.prepare("SELECT DISTINCT run_id FROM events ORDER BY id")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        ids.into_iter()
            .map(|id| self.run(Uuid::parse_str(&id)?))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::domain::InitialContext;

    #[test]
    fn events_survive_reopen_and_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artist.db");
        let run_id = Uuid::new_v4();
        let node_id = Uuid::new_v4();
        {
            let store = SqliteStore::open(&path).unwrap();
            store
                .append(
                    run_id,
                    EventData::RunCreated {
                        node_id,
                        input: json!({}),
                        initial_context: InitialContext {
                            system: "s".into(),
                            agents: "a".into(),
                            node: "n".into(),
                        },
                        parent_id: None,
                        child_kind: None,
                        fork_point: None,
                    },
                )
                .unwrap();
            store.append(run_id, EventData::RunStarted).unwrap();
        }
        let store = SqliteStore::open(&path).unwrap();
        let run = store.run(run_id).unwrap();
        assert_eq!(run.status, crate::domain::RunStatus::Running);
        assert_eq!(run.last_sequence, 2);
    }
}
