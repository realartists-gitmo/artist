//! The [`Recorder`] handle producers append through, and the single writer
//! task that assigns seqs and performs file I/O.
//!
//! All producers (CLI, capture hooks, delegate hooks) clone one `Recorder`;
//! events funnel through an unbounded channel into one blocking writer loop,
//! giving a total order and O(1) appends without any producer doing I/O.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use tokio::sync::mpsc;

use crate::event::{MAIN_LINEAGE, SessionEvent};
use crate::log::EventLogWriter;

enum WriterMessage {
    Event(Box<RecordedEvent>),
    /// Barrier: acked once every previously sent event is durably appended.
    Flush(tokio::sync::oneshot::Sender<()>),
}

struct RecordedEvent {
    run: Option<String>,
    lineage: String,
    event: SessionEvent,
}

#[derive(Clone, Default)]
struct Watermark(Arc<AtomicU64>);

impl Watermark {
    fn store(&self, seq: u64) {
        // +1 so 0 can mean "nothing committed yet".
        self.0.store(seq + 1, Ordering::Release);
    }
    fn load(&self) -> Option<u64> {
        self.0.load(Ordering::Acquire).checked_sub(1)
    }
}

/// Clonable, I/O-free event producer. A `Recorder` is bound to a lineage and
/// an optional run id; deriving scoped recorders is cheap.
#[derive(Clone)]
pub struct Recorder {
    sender: Option<mpsc::UnboundedSender<WriterMessage>>,
    lineage: String,
    run: Option<String>,
    watermark: Watermark,
}

impl Recorder {
    /// A recorder that discards everything (for callers that don't persist).
    pub fn noop() -> Self {
        Self {
            sender: None,
            lineage: MAIN_LINEAGE.to_owned(),
            run: None,
            watermark: Watermark::default(),
        }
    }

    /// Record an event with this recorder's run + lineage. Never blocks;
    /// silently drops if the writer task has shut down (session closing).
    pub fn record(&self, event: impl Into<SessionEvent>) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(WriterMessage::Event(Box::new(RecordedEvent {
                run: self.run.clone(),
                lineage: self.lineage.clone(),
                event: event.into(),
            })));
        }
    }

    /// Wait until everything recorded so far is durably in the log — call
    /// before rebuilding history from disk at a turn boundary.
    pub async fn flush(&self) {
        let Some(sender) = &self.sender else {
            return;
        };
        let (ack, done) = tokio::sync::oneshot::channel();
        if sender.send(WriterMessage::Flush(ack)).is_ok() {
            let _ = done.await;
        }
    }

    /// A recorder scoped to a child lineage, e.g. `delegate-<uuid>`.
    pub fn child_lineage(&self, child: &str) -> Self {
        let mut scoped = self.clone();
        scoped.lineage = format!("{}/{}", self.lineage, child);
        scoped
    }

    /// A recorder that stamps events with the given run id.
    pub fn with_run(&self, run: &str) -> Self {
        let mut scoped = self.clone();
        scoped.run = Some(run.to_owned());
        scoped
    }

    pub fn lineage(&self) -> &str {
        &self.lineage
    }

    /// Highest seq the writer task has durably committed, if any. The live
    /// watermark TTSR uses to snapshot committed in-run history.
    pub fn last_committed_seq(&self) -> Option<u64> {
        self.watermark.load()
    }

    pub fn is_noop(&self) -> bool {
        self.sender.is_none()
    }
}

/// Handle to the writer task. Dropping every `Recorder` clone lets the task
/// drain and exit; [`WriterTask::close`] awaits that flush.
pub struct WriterTask {
    handle: tokio::task::JoinHandle<Result<()>>,
}

impl WriterTask {
    /// Await the writer draining its queue and syncing the log. Call after
    /// dropping (or scoping out) all `Recorder` clones.
    pub async fn close(self) -> Result<()> {
        self.handle.await.map_err(|error| anyhow::anyhow!(error))?
    }
}

/// Spawn the single writer task for a session. When `transcript` is given,
/// the task also appends the derived markdown fragment for each event —
/// the transcript is a regenerable projection, so its writes are
/// best-effort (never fail the log append).
pub fn spawn_writer(
    mut writer: EventLogWriter,
    transcript: Option<std::path::PathBuf>,
) -> (Recorder, WriterTask) {
    let (sender, mut receiver) = mpsc::unbounded_channel::<WriterMessage>();
    let watermark = Watermark::default();
    if let Some(seq) = writer.last_seq() {
        watermark.store(seq);
    }
    let task_watermark = watermark.clone();
    let handle = tokio::task::spawn_blocking(move || -> Result<()> {
        let mut transcript_file = transcript.as_deref().and_then(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
        });
        while let Some(message) = receiver.blocking_recv() {
            let recorded = match message {
                WriterMessage::Event(recorded) => *recorded,
                WriterMessage::Flush(ack) => {
                    let _ = ack.send(());
                    continue;
                }
            };
            let seq = writer.append(recorded.run.as_deref(), &recorded.lineage, &recorded.event)?;
            task_watermark.store(seq);
            if let Some(file) = &mut transcript_file {
                let envelope = crate::event::Envelope {
                    v: crate::event::SCHEMA_VERSION,
                    seq,
                    ts: 0,
                    session: String::new(),
                    run: recorded.run,
                    lineage: recorded.lineage,
                    kind: recorded.event.kind().to_owned(),
                    payload: recorded.event.payload(),
                };
                if let Some(fragment) = crate::replay::markdown_fragment(&envelope) {
                    use std::io::Write;
                    let _ = file.write_all(fragment.as_bytes());
                }
            }
        }
        Ok(())
    });
    (
        Recorder {
            sender: Some(sender),
            lineage: MAIN_LINEAGE.to_owned(),
            run: None,
            watermark,
        },
        WriterTask { handle },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TurnUser;
    use crate::log::EventLogReader;

    fn user_turn(text: &str) -> TurnUser {
        TurnUser {
            content: vec![crate::event::ContentBlock::Text { text: text.into() }],
            display: None,
            source: "prompt".into(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn events_from_clones_are_totally_ordered_and_flushed() {
        let dir = tempfile::tempdir().unwrap();
        let writer = EventLogWriter::open(dir.path(), "s-1").unwrap();
        let (recorder, task) = spawn_writer(writer, None);

        let delegate = recorder.child_lineage("delegate-1").with_run("r-2");
        recorder.record(user_turn("main"));
        delegate.record(user_turn("inner"));
        drop(delegate);
        drop(recorder);
        task.close().await.unwrap();

        let events = EventLogReader::new(dir.path()).read_all().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].lineage, "main");
        assert_eq!(events[1].lineage, "main/delegate-1");
        assert_eq!(events[1].run.as_deref(), Some("r-2"));
        assert_eq!(events[1].seq, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn watermark_tracks_committed_seq() {
        let dir = tempfile::tempdir().unwrap();
        let writer = EventLogWriter::open(dir.path(), "s-1").unwrap();
        let (recorder, task) = spawn_writer(writer, None);
        assert_eq!(recorder.last_committed_seq(), None);
        recorder.record(user_turn("one"));
        // The writer commits asynchronously; poll the live watermark.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while recorder.last_committed_seq() != Some(0) {
            assert!(
                std::time::Instant::now() < deadline,
                "watermark never advanced"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        drop(recorder);
        task.close().await.unwrap();
    }

    #[test]
    fn noop_recorder_is_inert() {
        let recorder = Recorder::noop();
        recorder.record(user_turn("ignored"));
        assert!(recorder.is_noop());
        assert_eq!(recorder.last_committed_seq(), None);
    }
}
