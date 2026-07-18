//! The append-only `events.jsonl` log: single-writer, O(1) append,
//! torn-tail-tolerant reads, and the rewind API.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fs2::FileExt;

use crate::event::{Envelope, HistoryRewind, SCHEMA_VERSION, SessionEvent};

pub const EVENTS_FILE: &str = "events.jsonl";
const WRITER_LOCK_FILE: &str = "writer.lock";

/// Read-only view of a session's event log.
pub struct EventLogReader {
    path: PathBuf,
}

impl EventLogReader {
    pub fn new(session_dir: &Path) -> Self {
        Self {
            path: session_dir.join(EVENTS_FILE),
        }
    }

    /// All complete, parseable records in seq order. A torn final line
    /// (crash mid-append) is dropped; interior garbage lines are skipped.
    /// Returns an empty vec when the log does not exist yet.
    pub fn read_all(&self) -> Result<Vec<Envelope>> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error).with_context(|| format!("open {}", self.path.display()));
            }
        };
        let mut reader = BufReader::new(file);
        let mut events = Vec::new();
        let mut line = String::new();
        loop {
            line.clear();
            let read = reader.read_line(&mut line).context("read event line")?;
            if read == 0 {
                break;
            }
            // A line without a trailing newline is a torn tail from a crash
            // mid-append: drop it. sync_data runs after the full line lands,
            // so at most the final line can be torn.
            if !line.ends_with('\n') {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(envelope) = serde_json::from_str::<Envelope>(trimmed) {
                events.push(envelope);
            }
        }
        Ok(events)
    }
}

/// The single writer for a session's event log. Holds the events file open
/// in append mode plus an exclusive advisory lock for the process lifetime,
/// so appends are O(1) and a second process fails fast on open.
pub struct EventLogWriter {
    session: String,
    file: File,
    _lock: File,
    next_seq: u64,
}

impl EventLogWriter {
    /// Open (creating the directory and log if needed). Fails if another
    /// process holds the session's writer lock.
    pub fn open(session_dir: &Path, session: &str) -> Result<Self> {
        fs::create_dir_all(session_dir)
            .with_context(|| format!("create session dir {}", session_dir.display()))?;
        let lock_path = session_dir.join(WRITER_LOCK_FILE);
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open {}", lock_path.display()))?;
        lock.try_lock_exclusive()
            .context("session is active in another process")?;

        // Repair a torn tail from a crash mid-append: without this, the next
        // append would concatenate onto the partial line and both records
        // would read back as one garbage line.
        truncate_torn_tail(&session_dir.join(EVENTS_FILE))?;

        // Recover next_seq with the same tolerant scan resume uses anyway.
        let next_seq = EventLogReader::new(session_dir)
            .read_all()?
            .last()
            .map(|envelope| envelope.seq + 1)
            .unwrap_or(0);

        let path = session_dir.join(EVENTS_FILE);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        Ok(Self {
            session: session.to_owned(),
            file,
            _lock: lock,
            next_seq,
        })
    }

    /// Append one event, returning its assigned seq. Durable on return.
    pub fn append(
        &mut self,
        run: Option<&str>,
        lineage: &str,
        event: &SessionEvent,
    ) -> Result<u64> {
        let seq = self.next_seq;
        let envelope = Envelope {
            v: SCHEMA_VERSION,
            seq,
            ts: unix_millis(),
            session: self.session.clone(),
            run: run.map(str::to_owned),
            lineage: lineage.to_owned(),
            kind: event.kind().to_owned(),
            payload: event.payload(),
        };
        let mut line = serde_json::to_string(&envelope).context("serialize event")?;
        line.push('\n');
        self.file
            .write_all(line.as_bytes())
            .context("append event")?;
        self.file.sync_data().context("sync event log")?;
        self.next_seq = seq + 1;
        Ok(seq)
    }

    /// Append a `history.rewind` mask event. Nothing is deleted; projections
    /// ignore events with `to_seq < seq <= (returned seq)`.
    pub fn append_rewind(&mut self, to_seq: u64, reason: &str, by: &str) -> Result<u64> {
        self.append(
            None,
            crate::event::MAIN_LINEAGE,
            &SessionEvent::HistoryRewind(HistoryRewind {
                to_seq,
                reason: reason.to_owned(),
                by: by.to_owned(),
            }),
        )
    }

    /// Append a verbatim envelope under this log's session id and the next
    /// seq. Used by session forking to copy a prefix with full fidelity
    /// (including event kinds this binary does not model). The caller must
    /// preserve source order so control-event seq references stay valid.
    pub fn append_raw(&mut self, envelope: &Envelope) -> Result<u64> {
        let seq = self.next_seq;
        let envelope = Envelope {
            v: envelope.v,
            seq,
            ts: envelope.ts,
            session: self.session.clone(),
            run: envelope.run.clone(),
            lineage: envelope.lineage.clone(),
            kind: envelope.kind.clone(),
            payload: envelope.payload.clone(),
        };
        let mut line = serde_json::to_string(&envelope).context("serialize event")?;
        line.push('\n');
        self.file
            .write_all(line.as_bytes())
            .context("append event")?;
        self.file.sync_data().context("sync event log")?;
        self.next_seq = seq + 1;
        Ok(seq)
    }

    pub fn last_seq(&self) -> Option<u64> {
        self.next_seq.checked_sub(1)
    }
}

/// Truncate a trailing unterminated line, if any. No-op for a missing file.
fn truncate_torn_tail(path: &Path) -> Result<()> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    if bytes.is_empty() || bytes.ends_with(b"\n") {
        return Ok(());
    }
    let keep = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|position| position + 1)
        .unwrap_or(0);
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("open {} for tail repair", path.display()))?;
    file.set_len(keep as u64).context("truncate torn tail")?;
    file.sync_data().context("sync tail repair")?;
    Ok(())
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{LegacyTurn, TurnUser};

    fn user_turn(text: &str) -> SessionEvent {
        SessionEvent::TurnUser(TurnUser {
            content: vec![crate::event::ContentBlock::Text { text: text.into() }],
            display: None,
            source: "prompt".into(),
        })
    }

    #[test]
    fn append_and_read_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = EventLogWriter::open(dir.path(), "s-1").unwrap();
        assert_eq!(
            writer
                .append(Some("r-1"), "main", &user_turn("one"))
                .unwrap(),
            0
        );
        assert_eq!(writer.append(None, "main", &user_turn("two")).unwrap(), 1);

        let events = EventLogReader::new(dir.path()).read_all().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].seq, 0);
        assert_eq!(events[0].run.as_deref(), Some("r-1"));
        assert_eq!(events[1].seq, 1);
        assert_eq!(events[1].run, None);
    }

    #[test]
    fn seq_resumes_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut writer = EventLogWriter::open(dir.path(), "s-1").unwrap();
            writer.append(None, "main", &user_turn("one")).unwrap();
        }
        let mut writer = EventLogWriter::open(dir.path(), "s-1").unwrap();
        assert_eq!(writer.append(None, "main", &user_turn("two")).unwrap(), 1);
    }

    #[test]
    fn torn_tail_is_dropped_and_seq_reuses_its_slot() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut writer = EventLogWriter::open(dir.path(), "s-1").unwrap();
            writer.append(None, "main", &user_turn("one")).unwrap();
        }
        // Simulate a crash mid-append: partial JSON, no trailing newline.
        let path = dir.path().join(EVENTS_FILE);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"v\":1,\"seq\":1,\"ts\":12").unwrap();
        drop(file);

        let events = EventLogReader::new(dir.path()).read_all().unwrap();
        assert_eq!(events.len(), 1);

        // Reopening truncates nothing but reuses seq 1 — after the append the
        // torn bytes are followed by a valid line, which the tolerant reader
        // must still skip (it stops at the first unterminated line, so we
        // repair by truncating the torn tail on open).
        let mut writer = EventLogWriter::open(dir.path(), "s-1").unwrap();
        writer.append(None, "main", &user_turn("two")).unwrap();
        let events = EventLogReader::new(dir.path()).read_all().unwrap();
        assert_eq!(events.len(), 2, "torn tail must not shadow later appends");
    }

    #[test]
    fn second_writer_fails_fast() {
        let dir = tempfile::tempdir().unwrap();
        let _writer = EventLogWriter::open(dir.path(), "s-1").unwrap();
        assert!(EventLogWriter::open(dir.path(), "s-1").is_err());
    }

    #[test]
    fn legacy_turn_events_decode() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = EventLogWriter::open(dir.path(), "s-1").unwrap();
        writer
            .append(
                None,
                "main",
                &SessionEvent::LegacyTurn(LegacyTurn {
                    role: "assistant".into(),
                    content: "old transcript".into(),
                }),
            )
            .unwrap();
        let events = EventLogReader::new(dir.path()).read_all().unwrap();
        match events[0].event() {
            SessionEvent::LegacyTurn(turn) => assert_eq!(turn.role, "assistant"),
            other => panic!("unexpected {other:?}"),
        }
    }
}
