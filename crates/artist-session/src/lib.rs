//! Durable append-only streams used by Artist sessions and daemon state.
//!
//! The log is deliberately below the session and agent layers. It stores
//! self-describing JSON records, one complete record per line, and does not
//! assign meaning to event payloads. Higher layers own event schemas and
//! projections.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use artist_kernel::{AgentTranscript, ResourceError, ResourceErrorCode};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// A validated identifier used to address a durable workspace.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkspaceId(String);

/// A validated identifier used to address a durable session.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionId(String);

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("invalid durable identifier {value:?}")]
pub struct InvalidId {
    value: String,
}

fn validate_id(value: String) -> Result<String, InvalidId> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.contains('\0')
    {
        return Err(InvalidId { value });
    }
    Ok(value)
}

macro_rules! durable_id {
    ($type:ty) => {
        impl $type {
            pub fn new(value: impl Into<String>) -> Result<Self, InvalidId> {
                Ok(Self(validate_id(value.into())?))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $type {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl std::fmt::Display for $type {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

durable_id!(WorkspaceId);
durable_id!(SessionId);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub root: PathBuf,
}

impl Workspace {
    pub fn new(id: WorkspaceId, root: impl Into<PathBuf>) -> Self {
        Self {
            id,
            root: root.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    InvalidId(#[from] InvalidId),
    #[error(transparent)]
    Log(#[from] LogError),
}

/// Maps durable workspace/session identities to their individual event logs.
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn open_session(
        &self,
        workspace: &Workspace,
        session: SessionId,
    ) -> Result<EventLog, StoreError> {
        let path = self
            .root
            .join("workspaces")
            .join(workspace.id.as_str())
            .join("sessions")
            .join(format!("{}.jsonl", session.as_str()));
        Ok(EventLog::open(path, session.to_string())?)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct LogRecord {
    pub version: u16,
    pub stream_id: String,
    pub sequence: u64,
    pub event_type: String,
    pub timestamp_ms: u64,
    pub payload: Value,
}

impl LogRecord {
    pub fn new(
        stream_id: impl Into<String>,
        sequence: u64,
        event_type: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            version: 1,
            stream_id: stream_id.into(),
            sequence,
            event_type: event_type.into(),
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            payload,
        }
    }
}

#[derive(Debug, Error)]
pub enum LogError {
    #[error("I/O error for event log {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("invalid event log record at line {line}: {source}")]
    InvalidRecord {
        line: usize,
        source: serde_json::Error,
    },
    #[error("event log record has sequence {found}, expected {expected}")]
    NonMonotonic { found: u64, expected: u64 },
    #[error("event log record belongs to stream {found:?}, expected {expected:?}")]
    WrongStream { found: String, expected: String },
    #[error("event log stream id cannot be empty")]
    EmptyStreamId,
    #[error("event type cannot be empty")]
    EmptyEventType,
}

fn io_error(path: &Path, source: io::Error) -> LogError {
    LogError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// A single append-only JSONL stream.
pub struct EventLog {
    path: PathBuf,
    stream_id: String,
}

impl EventLog {
    /// Open or create a stream and repair an incomplete final write.
    pub fn open(path: impl Into<PathBuf>, stream_id: impl Into<String>) -> Result<Self, LogError> {
        let path = path.into();
        let stream_id = stream_id.into();
        if stream_id.is_empty() {
            return Err(LogError::EmptyStreamId);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| io_error(&path, source))?;
        }
        let log = Self { path, stream_id };
        log.repair_tail()?;
        log.validate_records()?;
        Ok(log)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// Append one record under an exclusive file lock and flush it to the OS.
    pub fn append(
        &self,
        event_type: impl Into<String>,
        payload: Value,
    ) -> Result<LogRecord, LogError> {
        let event_type = event_type.into();
        if event_type.is_empty() {
            return Err(LogError::EmptyEventType);
        }
        let mut file = self.open_rw()?;
        file.lock_exclusive()
            .map_err(|source| io_error(&self.path, source))?;
        let sequence = self.last_sequence_from(&mut file)? + 1;
        let record = LogRecord::new(&self.stream_id, sequence, event_type, payload);
        let mut bytes = serde_json::to_vec(&record).map_err(|source| LogError::InvalidRecord {
            line: sequence as usize,
            source,
        })?;
        bytes.push(b'\n');
        file.seek(SeekFrom::End(0))
            .map_err(|source| io_error(&self.path, source))?;
        file.write_all(&bytes)
            .map_err(|source| io_error(&self.path, source))?;
        file.sync_data()
            .map_err(|source| io_error(&self.path, source))?;
        file.unlock()
            .map_err(|source| io_error(&self.path, source))?;
        Ok(record)
    }

    pub fn records(&self) -> Result<Vec<LogRecord>, LogError> {
        let file = File::open(&self.path).map_err(|source| io_error(&self.path, source))?;
        file.lock_shared()
            .map_err(|source| io_error(&self.path, source))?;
        self.read_records(file)
    }

    pub fn next_sequence(&self) -> Result<u64, LogError> {
        Ok(self
            .records()?
            .last()
            .map_or(1, |record| record.sequence + 1))
    }

    pub fn bytes(&self) -> Result<Vec<u8>, LogError> {
        std::fs::read(&self.path).map_err(|source| io_error(&self.path, source))
    }

    fn open_rw(&self) -> Result<File, LogError> {
        OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&self.path)
            .map_err(|source| io_error(&self.path, source))
    }

    fn last_sequence_from(&self, file: &mut File) -> Result<u64, LogError> {
        file.seek(SeekFrom::Start(0))
            .map_err(|source| io_error(&self.path, source))?;
        let clone = file
            .try_clone()
            .map_err(|source| io_error(&self.path, source))?;
        let records = self.read_records(clone)?;
        Ok(records.last().map_or(0, |record| record.sequence))
    }

    fn read_records(&self, file: File) -> Result<Vec<LogRecord>, LogError> {
        let reader = BufReader::new(file);
        let mut records: Vec<LogRecord> = Vec::new();
        for (index, line) in reader.lines().enumerate() {
            let line = line.map_err(|source| io_error(&self.path, source))?;
            if line.is_empty() {
                continue;
            }
            let record: LogRecord =
                serde_json::from_str(&line).map_err(|source| LogError::InvalidRecord {
                    line: index + 1,
                    source,
                })?;
            self.validate_record(&record, records.last().map(|r| r.sequence + 1).unwrap_or(1))?;
            records.push(record);
        }
        Ok(records)
    }

    fn validate_record(&self, record: &LogRecord, expected: u64) -> Result<(), LogError> {
        if record.stream_id != self.stream_id {
            return Err(LogError::WrongStream {
                found: record.stream_id.clone(),
                expected: self.stream_id.clone(),
            });
        }
        if record.sequence != expected {
            return Err(LogError::NonMonotonic {
                found: record.sequence,
                expected,
            });
        }
        if record.event_type.is_empty() {
            return Err(LogError::EmptyEventType);
        }
        Ok(())
    }

    fn validate_records(&self) -> Result<(), LogError> {
        self.records().map(|_| ())
    }

    fn repair_tail(&self) -> Result<(), LogError> {
        let mut file = self.open_rw()?;
        file.lock_exclusive()
            .map_err(|source| io_error(&self.path, source))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|source| io_error(&self.path, source))?;
        if bytes.last().is_some_and(|byte| *byte == b'\n') {
            file.unlock()
                .map_err(|source| io_error(&self.path, source))?;
            return Ok(());
        }
        let Some(end) = bytes.iter().rposition(|byte| *byte == b'\n') else {
            file.set_len(0)
                .map_err(|source| io_error(&self.path, source))?;
            file.unlock()
                .map_err(|source| io_error(&self.path, source))?;
            return Ok(());
        };
        file.set_len((end + 1) as u64)
            .map_err(|source| io_error(&self.path, source))?;
        file.sync_data()
            .map_err(|source| io_error(&self.path, source))?;
        file.unlock()
            .map_err(|source| io_error(&self.path, source))?;
        Ok(())
    }
}

/// Kernel resource adapter for an existing durable event log. The log remains
/// the source of truth; this adapter only gives it `agent://` addressing.
pub struct EventLogTranscript {
    log: std::sync::Arc<EventLog>,
    closed_marker: PathBuf,
    closed: std::sync::atomic::AtomicBool,
}

impl EventLogTranscript {
    pub fn new(log: std::sync::Arc<EventLog>) -> Self {
        let closed_marker = log.path().with_extension("closed");
        let closed = closed_marker.exists();
        Self {
            log,
            closed_marker,
            closed: std::sync::atomic::AtomicBool::new(closed),
        }
    }
}

#[async_trait::async_trait]
impl AgentTranscript for EventLogTranscript {
    async fn read(&self) -> Result<Vec<u8>, ResourceError> {
        self.log.bytes().map_err(log_resource_error)
    }

    async fn append(
        &self,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<(), ResourceError> {
        if self.is_closed() {
            return Err(ResourceError::new(
                ResourceErrorCode::Conflict,
                "historical transcript is immutable",
            ));
        }
        self.log
            .append(event_type, payload)
            .map(|_| ())
            .map_err(log_resource_error)
    }

    async fn close(&self) -> Result<(), ResourceError> {
        std::fs::write(&self.closed_marker, b"closed")
            .map_err(|error| ResourceError::new(ResourceErrorCode::Io, error.to_string()))?;
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::Acquire)
    }
}

fn log_resource_error(error: LogError) -> ResourceError {
    ResourceError::new(ResourceErrorCode::Io, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn appends_and_replays_ordered_records() {
        let dir = tempdir().unwrap();
        let log = EventLog::open(dir.path().join("session.jsonl"), "session-1").unwrap();
        let first = log
            .append("created", serde_json::json!({"cwd": "/repo"}))
            .unwrap();
        let second = log
            .append("prompt", serde_json::json!({"text": "hello"}))
            .unwrap();
        assert_eq!(first.sequence, 1);
        assert_eq!(second.sequence, 2);
        assert_eq!(log.next_sequence().unwrap(), 3);
        assert_eq!(log.records().unwrap(), vec![first, second]);
    }

    #[test]
    fn repairs_an_incomplete_final_record() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let log = EventLog::open(&path, "session-1").unwrap();
        let record = log.append("created", serde_json::json!({})).unwrap();
        drop(log);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"version\":1,\"stream_id\":\"session-1\"")
            .unwrap();
        file.sync_data().unwrap();
        let reopened = EventLog::open(&path, "session-1").unwrap();
        assert_eq!(reopened.records().unwrap(), vec![record]);
    }

    #[test]
    fn rejects_wrong_stream() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bad.jsonl");
        std::fs::write(&path, "{\"version\":1,\"stream_id\":\"other\",\"sequence\":1,\"event_type\":\"x\",\"timestamp_ms\":0,\"payload\":{}}\n").unwrap();
        assert!(matches!(
            EventLog::open(path, "session-1"),
            Err(LogError::WrongStream { .. })
        ));
    }

    #[test]
    fn session_store_separates_workspaces_and_sessions() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path());
        let workspace = Workspace::new(WorkspaceId::new("repo-a").unwrap(), "/checkout/a");
        let session = SessionId::new("session-1").unwrap();
        let log = store.open_session(&workspace, session).unwrap();
        log.append("created", serde_json::json!({})).unwrap();

        assert_eq!(
            log.path(),
            dir.path()
                .join("workspaces/repo-a/sessions/session-1.jsonl")
        );
        assert_eq!(log.records().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn transcript_close_survives_reopen() {
        let dir = tempdir().unwrap();
        let log = std::sync::Arc::new(
            EventLog::open(dir.path().join("session.jsonl"), "session-1").unwrap(),
        );
        let transcript = EventLogTranscript::new(std::sync::Arc::clone(&log));
        transcript.close().await.unwrap();
        assert!(transcript.is_closed());

        let reopened = EventLogTranscript::new(log);
        assert!(reopened.is_closed());
        assert!(matches!(
            reopened.append("late", serde_json::json!({})).await,
            Err(ResourceError {
                code: ResourceErrorCode::Conflict,
                ..
            })
        ));
    }

    #[test]
    fn durable_ids_cannot_escape_store_root() {
        assert!(WorkspaceId::new("../outside").is_err());
        assert!(SessionId::new("nested/session").is_err());
        assert!(SessionId::new("").is_err());
    }
}
