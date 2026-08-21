//! Durable append-only streams used by Artist sessions and daemon state.
//!
//! The log is deliberately below the session and agent layers. It stores
//! self-describing JSON records, one complete record per line, and does not
//! assign meaning to event payloads. Higher layers own event schemas and
//! projections.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::broadcast;

pub mod context;

/// The only event-log schema currently understood by this crate.
pub const EVENT_LOG_VERSION: u16 = 1;

pub use context::{
    ContextController, ContextError, ContextEvent, ContextState, Contribution, Snapshot,
};

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
        let path = self.session_path(workspace, &session);
        Ok(EventLog::open(path, session.to_string())?)
    }

    pub fn session_path(&self, workspace: &Workspace, session: &SessionId) -> PathBuf {
        self.root
            .join("workspaces")
            .join(workspace.id.as_str())
            .join("sessions")
            .join(format!("{}.jsonl", session.as_str()))
    }

    /// Stable coordination path for the live daemon lease. The lease is
    /// deliberately separate from the JSONL stream so event-log readers can
    /// remain read-only while a daemon owns session execution.
    pub fn session_lease_path(&self, workspace: &Workspace, session: &SessionId) -> PathBuf {
        self.session_path(workspace, session)
            .with_extension("lease")
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
            version: EVENT_LOG_VERSION,
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
    #[error("unsupported event log schema version {found}; supported version is {supported}")]
    UnsupportedVersion { found: u16, supported: u16 },
    #[error("event log is closed and cannot accept active writes")]
    Closed,
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
    closed_marker: PathBuf,
    closed: AtomicBool,
    events: broadcast::Sender<LogRecord>,
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
        let closed_marker = path.with_extension("closed");
        let (events, _) = broadcast::channel(256);
        let log = Self {
            path,
            stream_id,
            closed: AtomicBool::new(closed_marker.exists()),
            closed_marker,
            events,
        };
        if !log.is_closed() {
            log.repair_tail()?;
        }
        log.validate_records()?;
        Ok(log)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// Subscribe to records appended through this open log instance. Durable
    /// replay remains authoritative; this channel only gives live hosts a
    /// prompt notification path while a turn is running.
    pub fn subscribe(&self) -> broadcast::Receiver<LogRecord> {
        self.events.subscribe()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Close the durable stream. Existing records remain readable, but no
    /// future active append is permitted, including after reopening the log.
    pub fn close(&self) -> Result<(), LogError> {
        if self.is_closed() {
            return Ok(());
        }
        let log_file = self.open_rw()?;
        log_file
            .lock_exclusive()
            .map_err(|source| io_error(&self.path, source))?;
        if self.closed_marker.exists() {
            self.closed.store(true, Ordering::Release);
            let _ = log_file.unlock();
            return Ok(());
        }
        let temporary = self.closed_marker.with_extension("closed.tmp");
        let mut marker =
            File::create(&temporary).map_err(|source| io_error(&self.closed_marker, source))?;
        marker
            .write_all(b"closed\n")
            .map_err(|source| io_error(&self.closed_marker, source))?;
        marker
            .sync_data()
            .map_err(|source| io_error(&self.closed_marker, source))?;
        drop(marker);
        std::fs::rename(&temporary, &self.closed_marker)
            .map_err(|source| io_error(&self.closed_marker, source))?;
        self.closed.store(true, Ordering::Release);
        log_file
            .unlock()
            .map_err(|source| io_error(&self.path, source))?;
        Ok(())
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
        if self.is_closed() {
            return Err(LogError::Closed);
        }
        let mut file = self.open_rw()?;
        file.lock_exclusive()
            .map_err(|source| io_error(&self.path, source))?;
        if self.closed_marker.exists() {
            self.closed.store(true, Ordering::Release);
            let _ = file.unlock();
            return Err(LogError::Closed);
        }
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
        let _ = self.events.send(record.clone());
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
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.path)
            .map_err(|source| io_error(&self.path, source))
    }

    fn last_sequence_from(&self, file: &mut File) -> Result<u64, LogError> {
        const INITIAL_TAIL_BYTES: u64 = 16 * 1024;
        let length = file
            .metadata()
            .map_err(|source| io_error(&self.path, source))?
            .len();
        if length == 0 {
            return Ok(0);
        }

        // Appends already hold the exclusive stream lock. Only the final
        // JSONL record is needed to allocate the next sequence; rereading the
        // entire history for every streamed delta made long sessions
        // quadratic in both I/O and deserialization.
        let mut start = length.saturating_sub(INITIAL_TAIL_BYTES);
        loop {
            file.seek(SeekFrom::Start(start))
                .map_err(|source| io_error(&self.path, source))?;
            let mut tail = Vec::new();
            file.read_to_end(&mut tail)
                .map_err(|source| io_error(&self.path, source))?;
            let end = tail
                .iter()
                .rposition(|byte| *byte == b'\n')
                .unwrap_or(tail.len());
            let before_end = &tail[..end];
            let line_start = before_end
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |index| index + 1);
            if line_start > 0 || start == 0 {
                let line = &before_end[line_start..];
                if line.is_empty() {
                    return Ok(0);
                }
                let record: LogRecord = serde_json::from_slice(line)
                    .map_err(|source| LogError::InvalidRecord { line: 0, source })?;
                self.validate_record(&record, record.sequence)?;
                return Ok(record.sequence);
            }
            if start == 0 {
                return Ok(0);
            }
            start = start.saturating_sub(INITIAL_TAIL_BYTES);
        }
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
        if record.version != EVENT_LOG_VERSION {
            return Err(LogError::UnsupportedVersion {
                found: record.version,
                supported: EVENT_LOG_VERSION,
            });
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

        // A complete final JSON object may have been written before the
        // process died while the newline was still buffered. Preserve it by
        // completing the record. Anything else is the recoverable torn tail;
        // middle records are still rejected by `read_records` below.
        let last_start = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        let final_line = &bytes[last_start..];
        let complete = std::str::from_utf8(final_line)
            .ok()
            .and_then(|line| serde_json::from_str::<LogRecord>(line).ok())
            .is_some();
        if complete {
            file.seek(SeekFrom::End(0))
                .map_err(|source| io_error(&self.path, source))?;
            file.write_all(b"\n")
                .map_err(|source| io_error(&self.path, source))?;
        } else if let Some(end) = bytes.iter().rposition(|byte| *byte == b'\n') {
            file.set_len((end + 1) as u64)
                .map_err(|source| io_error(&self.path, source))?;
        } else {
            file.set_len(0)
                .map_err(|source| io_error(&self.path, source))?;
        }
        file.sync_data()
            .map_err(|source| io_error(&self.path, source))?;
        file.unlock()
            .map_err(|source| io_error(&self.path, source))?;
        Ok(())
    }
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
    fn preserves_a_complete_final_record_without_a_newline() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("complete.jsonl");
        let log = EventLog::open(&path, "session-1").unwrap();
        let record = log.append("created", serde_json::json!({})).unwrap();
        let mut bytes = log.bytes().unwrap();
        assert_eq!(bytes.pop(), Some(b'\n'));
        std::fs::write(&path, bytes).unwrap();

        let reopened = EventLog::open(&path, "session-1").unwrap();
        assert_eq!(reopened.records().unwrap(), vec![record]);
        assert!(reopened.bytes().unwrap().ends_with(b"\n"));
    }

    #[test]
    fn rejects_unsupported_event_versions() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("future.jsonl");
        std::fs::write(
            &path,
            "{\"version\":99,\"stream_id\":\"session-1\",\"sequence\":1,\"event_type\":\"x\",\"timestamp_ms\":0,\"payload\":{}}\n",
        )
        .unwrap();
        assert!(matches!(
            EventLog::open(path, "session-1"),
            Err(LogError::UnsupportedVersion { found: 99, .. })
        ));
    }

    #[test]
    fn rejects_corruption_in_the_middle_of_a_log() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("middle.jsonl");
        let log = EventLog::open(&path, "session-1").unwrap();
        let first = log.append("first", serde_json::json!({})).unwrap();
        let second = log.append("second", serde_json::json!({})).unwrap();
        let lines = format!(
            "{}\nnot-json\n{}\n",
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap()
        );
        std::fs::write(&path, lines).unwrap();
        assert!(matches!(
            EventLog::open(path, "session-1"),
            Err(LogError::InvalidRecord { line: 2, .. })
        ));
    }

    #[test]
    fn closed_logs_reject_writes_after_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("closed.jsonl");
        let log = EventLog::open(&path, "session-1").unwrap();
        log.close().unwrap();
        drop(log);
        let reopened = EventLog::open(&path, "session-1").unwrap();
        assert!(reopened.is_closed());
        assert!(matches!(
            reopened.append("late", serde_json::json!({})),
            Err(LogError::Closed)
        ));
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

    #[test]
    fn durable_ids_cannot_escape_store_root() {
        assert!(WorkspaceId::new("../outside").is_err());
        assert!(SessionId::new("nested/session").is_err());
        assert!(SessionId::new("").is_err());
    }
}
