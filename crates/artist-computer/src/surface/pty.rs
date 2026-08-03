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
use crate::surface::{SettleWatch, Surface};

/// How long the screen must stop changing before `quiet` settles.
const QUIET_MS: u64 = 250;
/// Poll granularity while waiting to settle.
const POLL_MS: u64 = 25;
/// How much primary-screen scrollback to keep.
///
/// A shell session is unbounded by nature — `cargo build` alone emits megabytes
/// — and the observation only ever renders what is new, so holding the whole
/// history costs memory to no purpose. Generous enough that nothing a model
/// would actually re-read falls off.
const TEXT_LIMIT: usize = 256 * 1024;

/// Drop terminal escape sequences and control bytes, keeping the text.
///
/// The model reads this as prose. Raw escapes put `\x1b[0;32m` in the middle of
/// a sentence, and a progress bar's carriage returns turn one line into
/// thousands — both cost context and neither carries meaning the model can use.
fn strip_control(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(character) = chars.next() {
        match character {
            '\u{1b}' => match chars.next() {
                // CSI: parameters and intermediates, then a final byte in
                // `@`..`~`. This is the form colour, cursor movement and screen
                // clearing all take.
                Some('[') => {
                    for parameter in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&parameter) {
                            break;
                        }
                    }
                }
                // OSC: runs until BEL or ST. Window titles live here.
                Some(']') => {
                    while let Some(parameter) = chars.next() {
                        if parameter == '\u{7}' {
                            break;
                        }
                        if parameter == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // Any other two-character escape.
                Some(_) | None => {}
            },
            // A carriage return without a newline redraws the line in place —
            // progress bars, spinners. Keeping the text of every redraw would
            // be thousands of near-identical lines, so only the last survives.
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    continue;
                }
                let keep = out.rfind('\n').map(|index| index + 1).unwrap_or(0);
                out.truncate(keep);
            }
            '\n' | '\t' => out.push(character),
            other if other.is_control() => {}
            other => out.push(other),
        }
    }
    out
}

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
        let was_alternate = self.parser.screen().alternate_screen();
        self.parser.process(bytes);
        self.revision += 1;
        self.last_change = Instant::now();

        // Keyed on the mode *before* processing: the escape that leaves the
        // alternate screen is in the same chunk as the first line of shell
        // output after it, and reading the mode afterwards attributes that whole
        // chunk to the wrong screen.
        if was_alternate && !self.parser.screen().alternate_screen() {
            return;
        }
        if self.parser.screen().alternate_screen() {
            return;
        }

        // Rendered, not raw. Appending the byte stream put `\x1b[0;32m` into the
        // node the model reads — a coloured prompt became escape codes it had to
        // parse itself, and a progress bar became kilobytes of carriage returns.
        self.text.push_str(&strip_control(bytes));

        // Bounded. This grew forever, so a long-running command was a memory
        // leak that also made every observation slower. Trimming from the front
        // is right for a scroll-back: the newest output is the interesting part.
        if self.text.len() > TEXT_LIMIT {
            let overflow = self.text.len() - TEXT_LIMIT;
            // On a character boundary, or the drain panics on UTF-8.
            let cut = (overflow..self.text.len())
                .find(|index| self.text.is_char_boundary(*index))
                .unwrap_or(self.text.len());
            self.text.drain(..cut);
            self.cursor = self.cursor.saturating_sub(cut);
        }
    }

    fn alternate(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    /// The nodes this screen currently presents.
    ///
    /// `from_start` re-reads the whole retained scrollback instead of only what
    /// is new. The incremental read is what makes a shell cheap to watch, but it
    /// is destructive — the second read returns nothing — so without a way back
    /// to the beginning an elided observation could never be recovered, however
    /// firmly the stub told the model to try.
    fn nodes(&mut self, from_start: bool) -> Vec<Node> {
        if self.alternate() {
            return self.rows();
        }
        // Primary screen: rows are meaningless here — the same text is at a
        // different row a moment later — so there is exactly one node and no row
        // anchors.
        let from = if from_start {
            0
        } else {
            self.cursor.min(self.text.len())
        };
        let text = self.text[from..].to_owned();
        self.cursor = self.text.len();
        if text.trim().is_empty() {
            return Vec::new();
        }
        vec![Node::new("pty:output", Role::Text, text.trim_end())]
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
            let mut child = child
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        Ok(Snapshot::new(self.screen.lock().unwrap().nodes(false)))
    }

    async fn snapshot_full(&self) -> Result<Snapshot, StepError> {
        Ok(Snapshot::new(self.screen.lock().unwrap().nodes(true)))
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

    async fn apply(
        &self,
        step: &Step,
        node: Option<&Node>,
        _secondary: Option<&Node>,
    ) -> Result<Option<String>, StepError> {
        match step {
            Step::Key(press) => self.send(&key_bytes(press.chord())?),
            Step::Type { text, .. } => self.send(text.as_bytes()),
            Step::Click { target, .. } => Err(StepError::Unsupported {
                anchor: target.anchor.clone(),
                role: node
                    .map(|node| node.role.label().to_owned())
                    .unwrap_or_default(),
                name: node.map(|node| node.name.clone()).unwrap_or_default(),
                action: "click",
            }),
            // A horizontal wheel has no terminal equivalent: there is no escape
            // sequence for it, and a curses application that scrolls sideways
            // does it with its own keybinding. Saying so beats paging the
            // screen vertically and reporting success.
            Step::Scroll {
                axis: crate::program::Axis::Horizontal,
                ..
            } => Err(StepError::Backend(
                "a terminal has no horizontal scroll — use the application's own key for it".into(),
            )),
            Step::Scroll { amount, .. } => {
                let key = if *amount >= 0 { "PageDown" } else { "PageUp" };
                let repeats = amount.unsigned_abs().clamp(1, 20);
                let bytes = key_bytes(key)?;
                for _ in 0..repeats {
                    self.send(&bytes)?;
                }
                Ok(())
            }
            // A terminal is a byte stream: a key is bytes, and a key held down
            // is the same bytes repeating. There is no way to express "down but
            // not up", so this refuses rather than sending the tap that a held
            // key is not.
            other @ (Step::KeyDown(_) | Step::KeyUp(_)) => Err(StepError::Backend(format!(
                "a terminal cannot {:?} — it receives bytes, and a held key is not one. \
                 Use `key` for the chord itself.",
                other.action()
            ))),
            // A terminal has no browser history and no per-element verbs. These
            // are routing mistakes rather than gaps, so they say what a terminal
            // does understand instead of failing bare.
            other @ (Step::Navigate { .. }
            | Step::Back { .. }
            | Step::Forward { .. }
            | Step::LongPress(_)
            | Step::Swipe { .. }
            | Step::Pinch { .. }
            | Step::Hover(_)
            | Step::Press { .. }
            | Step::Release { .. }
            | Step::Drag { .. }
            | Step::SetClipboard { .. }
            | Step::GetClipboard { .. }
            | Step::Upload { .. }
            | Step::Dialog { .. }
            | Step::Invoke { .. }) => Err(StepError::Backend(format!(
                "a terminal has no {:?} — drive it with key, type and scroll",
                other.action()
            ))),
        }
        .map(|()| None)
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
                &Step::click(crate::program::Target {
                    anchor: "kv7".into(),
                    label: Some("row".into()),
                }),
                None,
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

    #[tokio::test]
    async fn shell_output_reaches_the_model_as_text_not_escape_codes() {
        let surface = PtySurface::detached("pty:1", 6, 40);
        // A coloured prompt and a spinner, as any real shell emits.
        surface.feed(b"\x1b[0;32muser@host\x1b[0m:~$ ls\r\n");
        surface.feed(b"\x1b]0;window title\x07");
        surface.feed(b"downloading  10%\rdownloading  60%\rdownloading 100%\r\n");

        let snapshot = surface.snapshot().await.unwrap();
        let text = snapshot
            .nodes
            .iter()
            .map(|node| node.name.clone())
            .collect::<String>();

        assert!(text.contains("user@host:~$ ls"), "{text:?}");
        assert!(
            !text.contains('\u{1b}'),
            "raw escapes reached the model: {text:?}"
        );
        assert!(
            !text.contains("window title"),
            "OSC payload leaked: {text:?}"
        );
        // Only the final state of an in-place redraw survives.
        assert!(text.contains("downloading 100%"), "{text:?}");
        assert!(!text.contains("downloading  10%"), "{text:?}");
    }

    #[tokio::test]
    async fn the_scrollback_is_bounded() {
        let surface = PtySurface::detached("pty:1", 6, 40);
        // Well past the cap, as `cargo build` output would be.
        for _ in 0..600 {
            surface.feed(&vec![b'x'; 1024]);
            surface.feed(b"\n");
        }
        let held = surface.screen.lock().unwrap().text.len();
        assert!(
            held <= TEXT_LIMIT + 1024,
            "the buffer grew without bound: {held} bytes"
        );
    }

    #[tokio::test]
    async fn a_full_observation_can_re_read_what_an_incremental_one_consumed() {
        // The incremental read is destructive, so an elided observation could
        // never be recovered — while the stub it left behind told the model to
        // go and read it again.
        let surface = PtySurface::detached("pty:1", 6, 40);
        surface.feed(b"the answer is 42\n");

        let first = surface.snapshot().await.unwrap();
        assert!(first.nodes.iter().any(|node| node.name.contains("42")));

        let again = surface.snapshot().await.unwrap();
        assert!(again.nodes.is_empty(), "nothing new since the last look");

        let full = surface.snapshot_full().await.unwrap();
        assert!(
            full.nodes.iter().any(|node| node.name.contains("42")),
            "full=true must reach the retained scrollback"
        );
    }
}
