//! Rung 1 for terminals: a PTY driven through a vt100 screen buffer.
//!
//! A terminal is the degenerate computer-use target, and the most valuable one
//! to build first. Its observation channel is perfect and completely
//! deterministic — no compositor, no toolkit, no rendering — so anchors, deltas,
//! settle and expect can all be exercised by feeding a byte array to a parser.
//! If this abstraction cannot drive a curses application, it is not general, and
//! finding that out here costs nothing.
//!
//! Two modes, because a terminal is really two different surfaces:
//!
//! * **Alternate screen** (vim, htop, less): the screen is addressed, so a row
//!   index *is* a stable identity. A redraw that changes three rows produces
//!   three deltas and no anchor churn.
//! * **Primary screen** (a shell): output scrolls and rows mean nothing, so
//!   there are no row anchors — the observation is the appended text, and only
//!   keys and text can be sent.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::model::{Caps, Node, Role, Rung, Snapshot, SurfaceId};
use crate::program::{Settle, SettleKind, SettleOutcome, Step, StepError};
use crate::surface::{Surface, SettleWatch};

/// How long the screen must stop changing before `quiet` settles.
const QUIET_MS: u64 = 250;
/// Poll granularity while waiting to settle.
const POLL_MS: u64 = 25;

/// The screen state shared between the reader thread and the surface.
pub struct Screen {
    parser: vt100::Parser,
    /// Bumped on every byte received, so settling is a comparison rather than a
    /// full screen diff.
    revision: u64,
    last_change: Instant,
    /// How much of the primary-screen scrollback the model has already seen.
    cursor: usize,
    text: String,
}

impl Screen {
    fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, 0),
            revision: 0,
            last_change: Instant::now(),
            cursor: 0,
            text: String::new(),
        }
    }

    /// Feed terminal output in.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.revision += 1;
        self.last_change = Instant::now();
        if !self.parser.screen().alternate_screen() {
            self.text.push_str(&String::from_utf8_lossy(bytes));
        }
    }

    fn alternate(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    /// The nodes this screen currently presents.
    fn nodes(&mut self) -> Vec<Node> {
        if self.alternate() {
            return self.rows();
        }
        // Primary screen: hand over what has arrived since the last look and
        // advance. Rows are meaningless here — the same text is at a different
        // row a moment later — so there is exactly one node and no row anchors.
        let fresh = self.text[self.cursor.min(self.text.len())..].to_owned();
        self.cursor = self.text.len();
        if fresh.trim().is_empty() {
            return Vec::new();
        }
        vec![Node::new("pty:output", Role::Text, fresh.trim_end())]
    }

    fn rows(&self) -> Vec<Node> {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let mut nodes = Vec::with_capacity(rows as usize + 1);
        for (row, text) in screen.rows(0, cols).enumerate() {
            // A blank row carries no information and would only consume a
            // handle and a rendered line.
            if text.trim().is_empty() {
                continue;
            }
            nodes.push(Node::new(
                format!("pty:row:{row}"),
                Role::Row,
                text.trim_end(),
            ));
        }
        let (cursor_row, cursor_col) = screen.cursor_position();
        nodes.push(
            Node::new("pty:cursor", Role::Cursor, "")
                .with_value(format!("row {cursor_row}, column {cursor_col}")),
        );
        nodes
    }

    fn screen_digest(&self) -> [u8; 32] {
        let screen = self.parser.screen();
        let (_, cols) = screen.size();
        let mut hasher = blake3::Hasher::new();
        for text in screen.rows(0, cols) {
            hasher.update(text.as_bytes());
            hasher.update(b"\n");
        }
        *hasher.finalize().as_bytes()
    }
}

/// A terminal surface.
pub struct PtySurface {
    id: SurfaceId,
    screen: Arc<Mutex<Screen>>,
    writer: Option<Mutex<Box<dyn Write + Send>>>,
    title: String,
    child: Option<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
}

impl Drop for PtySurface {
    fn drop(&mut self) {
        // Closing a surface must not leave a process holding a terminal — and
        // must reap it, or every closed surface leaves a zombie behind.
        if let Some(child) = &self.child {
            let mut child = child.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl PtySurface {
    /// A surface over a screen with no attached process.
    ///
    /// This is what the tests drive: scripted escape sequences in, node sets
    /// out, with no subprocess and no timing to be flaky about.
    pub fn detached(id: impl Into<String>, rows: u16, cols: u16) -> Self {
        Self {
            id: SurfaceId::new(id),
            screen: Arc::new(Mutex::new(Screen::new(rows, cols))),
            writer: None,
            title: "terminal".into(),
            child: None,
        }
    }

    /// Spawn a program on a real PTY and stream its output into the screen.
    ///
    /// The reader thread is the only writer of screen state, exactly as in
    /// `BashTool`; everything else observes under the mutex. The child is killed
    /// when the surface drops, so closing a surface cannot leave a process
    /// behind holding a terminal.
    pub fn spawn(
        id: impl Into<String>,
        command: &str,
        cwd: Option<&std::path::Path>,
        rows: u16,
        cols: u16,
    ) -> Result<Self, StepError> {
        use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

        let pty = NativePtySystem::default()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| StepError::Backend(format!("open pty: {error}")))?;

        let mut builder = CommandBuilder::new("/bin/sh");
        builder.args(["-lc", command]);
        if let Some(cwd) = cwd {
            builder.cwd(cwd);
        }
        // Curses applications key off TERM; without it many refuse to use the
        // alternate screen at all, which is exactly the mode worth driving.
        builder.env("TERM", "xterm-256color");

        let child = pty
            .slave
            .spawn_command(builder)
            .map_err(|error| StepError::Backend(format!("spawn {command:?}: {error}")))?;
        let writer = pty
            .master
            .take_writer()
            .map_err(|error| StepError::Backend(format!("take pty writer: {error}")))?;
        let mut reader = pty
            .master
            .try_clone_reader()
            .map_err(|error| StepError::Backend(format!("clone pty reader: {error}")))?;

        let surface = Self {
            id: SurfaceId::new(id),
            screen: Arc::new(Mutex::new(Screen::new(rows, cols))),
            writer: Some(Mutex::new(writer)),
            title: command.to_owned(),
            child: Some(Mutex::new(child)),
        };

        let screen = Arc::clone(&surface.screen);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                screen.lock().unwrap().feed(&buffer[..count]);
            }
        });
        Ok(surface)
    }

    pub fn with_writer(mut self, writer: Box<dyn Write + Send>) -> Self {
        self.writer = Some(Mutex::new(writer));
        self
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn screen(&self) -> Arc<Mutex<Screen>> {
        Arc::clone(&self.screen)
    }

    /// Feed output in, as the reader thread does in a live session.
    pub fn feed(&self, bytes: &[u8]) {
        self.screen.lock().unwrap().feed(bytes);
    }

    fn send(&self, bytes: &[u8]) -> Result<(), StepError> {
        let Some(writer) = &self.writer else {
            // A detached surface accepts input silently so tests can drive the
            // observation half without a process; a live one always has a writer.
            return Ok(());
        };
        let mut writer = writer.lock().unwrap();
        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .map_err(|error| StepError::Backend(format!("write to terminal: {error}")))
    }
}

/// Translate a key name or chord into the bytes a terminal expects.
///
/// Kept explicit rather than derived from a keymap: a terminal takes bytes, not
/// keysyms, and the mapping from "ctrl+c" to 0x03 is arithmetic, not layout.
pub fn key_bytes(key: &str) -> Result<Vec<u8>, StepError> {
    crate::keys::terminal_bytes(&crate::keys::parse(key)?)
}

#[async_trait::async_trait]
impl Surface for PtySurface {
    fn id(&self) -> &SurfaceId {
        &self.id
    }

    fn rung(&self) -> Rung {
        Rung::Engine
    }

    fn caps(&self) -> Caps {
        let alternate = self.screen.lock().unwrap().alternate();
        Caps {
            // A terminal has no pointer. Clicking a row is meaningless, and
            // pretending otherwise would make the model try it.
            click: false,
            type_text: true,
            key: true,
            scroll: alternate,
            pixels: false,
        }
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    async fn snapshot(&self) -> Result<Snapshot, StepError> {
        Ok(Snapshot::new(self.screen.lock().unwrap().nodes()))
    }

    async fn watch(&self, settle: &Settle) -> Result<SettleWatch, StepError> {
        let screen = Arc::clone(&self.screen);
        let settle = settle.clone();
        let started = Instant::now();
        // Capture the revision *now*, before the action, so a change that lands
        // between arming and the first poll still counts.
        let armed_revision = screen.lock().unwrap().revision;

        Ok(SettleWatch(Box::new(Box::pin(async move {
            match settle.until {
                SettleKind::None => return SettleOutcome::Settled { after_ms: 0 },
                SettleKind::NetworkIdle => return SettleOutcome::Unsupported,
                SettleKind::Anchor(_) => return SettleOutcome::Unsupported,
                SettleKind::Quiet => {}
            }
            let deadline = started + Duration::from_millis(settle.timeout_ms);
            let mut digest = screen.lock().unwrap().screen_digest();
            loop {
                tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
                let (revision, last_change, current) = {
                    let screen = screen.lock().unwrap();
                    (screen.revision, screen.last_change, screen.screen_digest())
                };
                let quiet_for = last_change.elapsed();
                // Settled means: something happened, and then stopped happening.
                if revision != armed_revision
                    && digest == current
                    && quiet_for >= Duration::from_millis(QUIET_MS)
                {
                    return SettleOutcome::Settled {
                        after_ms: started.elapsed().as_millis() as u64,
                    };
                }
                digest = current;
                if Instant::now() >= deadline {
                    return SettleOutcome::TimedOut {
                        after_ms: started.elapsed().as_millis() as u64,
                    };
                }
            }
        }))))
    }

    async fn apply(&self, step: &Step, node: Option<&Node>) -> Result<(), StepError> {
        match step {
            Step::Key(key) => self.send(&key_bytes(key)?),
            Step::Type { text, .. } => self.send(text.as_bytes()),
            Step::Click(target) => Err(StepError::Unsupported {
                anchor: target.anchor.clone(),
                role: node.map(|node| node.role.label().to_owned()).unwrap_or_default(),
                name: node.map(|node| node.name.clone()).unwrap_or_default(),
                action: "click",
            }),
            Step::Scroll { amount, .. } => {
                let key = if *amount >= 0 { "PageDown" } else { "PageUp" };
                let repeats = amount.unsigned_abs().max(1).min(20);
                let bytes = key_bytes(key)?;
                for _ in 0..repeats {
                    self.send(&bytes)?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchors::{AnchorBook, Change};

    /// Move the cursor and write text, as a curses application would.
    fn at(row: u16, col: u16, text: &str) -> Vec<u8> {
        format!("\x1b[{};{}H{text}", row + 1, col + 1).into_bytes()
    }

    const ENTER_ALT: &[u8] = b"\x1b[?1049h";

    #[tokio::test]
    async fn an_alternate_screen_exposes_rows_as_stable_identities() {
        let surface = PtySurface::detached("pty:1", 6, 40);
        surface.feed(ENTER_ALT);
        surface.feed(&at(0, 0, "PID  COMMAND"));
        surface.feed(&at(1, 0, "1    init"));

        let mut book = AnchorBook::new();
        let observed = book.observe(&surface.snapshot().await.unwrap(), false);

        let rendered = crate::render::observation("pty:1", &observed, None);
        assert!(rendered.contains("PID  COMMAND"), "{rendered}");
        assert!(rendered.contains("1    init"), "{rendered}");
        assert!(rendered.contains("cursor"), "{rendered}");
    }

    #[tokio::test]
    async fn a_redraw_changes_only_the_rows_that_changed() {
        let surface = PtySurface::detached("pty:1", 6, 40);
        surface.feed(ENTER_ALT);
        surface.feed(&at(0, 0, "PID  COMMAND"));
        surface.feed(&at(1, 0, "1    init"));
        surface.feed(&at(2, 0, "2    bash"));

        let mut book = AnchorBook::new();
        book.observe(&surface.snapshot().await.unwrap(), false);

        // Only the middle row's content changes.
        surface.feed(&at(1, 0, "1    systemd "));
        let delta = book.observe(&surface.snapshot().await.unwrap(), false);

        let changed: Vec<_> = delta
            .entries
            .iter()
            .filter(|entry| entry.node.role == Role::Row)
            .collect();
        assert_eq!(
            changed.len(),
            1,
            "a redraw must not churn every row: {:?}",
            delta.entries
        );
        assert_eq!(changed[0].change, Some(Change::Changed));
        assert!(changed[0].node.name.contains("systemd"));
    }

    #[tokio::test]
    async fn row_anchors_survive_a_full_repaint() {
        let surface = PtySurface::detached("pty:1", 6, 40);
        surface.feed(ENTER_ALT);
        surface.feed(&at(0, 0, "header"));
        let mut book = AnchorBook::new();
        let first = book.observe(&surface.snapshot().await.unwrap(), false);
        let header = first
            .entries
            .iter()
            .find(|entry| entry.node.name == "header")
            .unwrap()
            .anchor
            .clone();

        surface.feed(&at(1, 0, "body"));
        let second = book.observe(&surface.snapshot().await.unwrap(), true);
        let still = second
            .entries
            .iter()
            .find(|entry| entry.node.name == "header")
            .unwrap();
        assert_eq!(still.anchor, header, "an unchanged row must keep its name");
    }

    #[tokio::test]
    async fn the_primary_screen_hands_over_appended_output_once() {
        let surface = PtySurface::detached("pty:1", 6, 40);
        surface.feed(b"hello\r\n");

        let first = surface.snapshot().await.unwrap();
        assert_eq!(first.nodes.len(), 1);
        assert!(first.nodes[0].name.contains("hello"));

        // Already consumed: a second look with no new output has nothing to say.
        let second = surface.snapshot().await.unwrap();
        assert!(second.nodes.is_empty());

        surface.feed(b"world\r\n");
        let third = surface.snapshot().await.unwrap();
        assert!(third.nodes[0].name.contains("world"));
        assert!(!third.nodes[0].name.contains("hello"));
    }

    #[tokio::test]
    async fn a_shell_surface_refuses_clicks_and_offers_no_pointer() {
        let surface = PtySurface::detached("pty:1", 6, 40);
        assert!(!surface.caps().click, "a terminal has no pointer");

        let error = surface
            .apply(
                &Step::Click(crate::program::Target {
                    anchor: "kv7".into(),
                    label: Some("row".into()),
                }),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, StepError::Unsupported { .. }));
    }

    #[test]
    fn key_names_map_to_terminal_bytes() {
        assert_eq!(key_bytes("Enter").unwrap(), b"\r");
        assert_eq!(key_bytes("Escape").unwrap(), vec![0x1b]);
        assert_eq!(key_bytes("Up").unwrap(), b"\x1b[A");
        assert_eq!(key_bytes("ctrl+c").unwrap(), vec![0x03]);
        assert_eq!(key_bytes("ctrl+d").unwrap(), vec![0x04]);
        assert_eq!(key_bytes("alt+b").unwrap(), vec![0x1b, b'b']);
        assert_eq!(key_bytes("a").unwrap(), b"a");
    }

    #[test]
    fn an_unknown_key_is_a_clear_error_rather_than_a_silent_no_op() {
        let error = key_bytes("SuperTurbo").unwrap_err();
        assert!(error.to_string().contains("unknown key"));
    }

    #[tokio::test]
    async fn quiet_settles_once_output_stops() {
        let surface = PtySurface::detached("pty:1", 6, 40);
        let settle = Settle {
            until: SettleKind::Quiet,
            timeout_ms: 3_000,
        };
        let watch = surface.watch(&settle).await.unwrap();
        surface.feed(b"working...");

        let outcome = watch.wait().await;
        assert!(
            matches!(outcome, SettleOutcome::Settled { .. }),
            "output followed by silence must settle: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn quiet_times_out_when_nothing_ever_happens() {
        let surface = PtySurface::detached("pty:1", 6, 40);
        let settle = Settle {
            until: SettleKind::Quiet,
            timeout_ms: 120,
        };
        let outcome = surface.watch(&settle).await.unwrap().wait().await;
        assert!(matches!(outcome, SettleOutcome::TimedOut { .. }));
    }
}
