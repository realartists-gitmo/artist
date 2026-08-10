use crate::{ToolError, Workspace, output};
use dashmap::{DashMap, DashSet};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use wezterm_term::{Terminal, TerminalConfiguration, TerminalSize, color::ColorPalette};

const EXEC_CAP: usize = 50 * 1024;
const INPUT_SESSION_ID: &str = "artist-input-shell";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BashStatus {
    Completed,
    Failed,
    TimedOut,
    Superseded,
    Running,
    Stopping,
    Stopped,
    Listed,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BashResult {
    pub status: BashStatus,
    pub exit_code: Option<i32>,
    pub session_id: Option<String>,
    pub stdout: String,
    pub stderr: String,
    pub output: String,
    pub duration_ms: Option<u64>,
    pub timeout_secs: Option<u64>,
    pub terminated_by: Option<String>,
    pub truncated: bool,
    pub retry_as_background: bool,
}

impl BashResult {
    pub fn parse(text: &str) -> Self {
        let mut status = BashStatus::Unknown;
        let mut exit_code = None;
        let mut session_id = None;
        let mut duration_ms = None;
        let mut timeout_secs = None;
        let mut terminated_by = None;
        let mut truncated = false;
        let mut header_done = false;
        let mut output_lines = Vec::new();
        for line in text.lines() {
            if !header_done {
                if let Some(value) = line.strip_prefix("status: ") {
                    status = match value.trim() {
                        "completed" => BashStatus::Completed,
                        "failed" => BashStatus::Failed,
                        "timedOut" | "timed_out" => BashStatus::TimedOut,
                        "superseded" => BashStatus::Superseded,
                        "running" => BashStatus::Running,
                        value if value.starts_with("stopping") => BashStatus::Stopping,
                        value if value.starts_with("stopped") => BashStatus::Stopped,
                        _ => BashStatus::Unknown,
                    };
                    continue;
                }
                if let Some(value) = line.strip_prefix("exitCode: ") {
                    exit_code = value.trim().parse().ok();
                    continue;
                }
                if let Some(value) = line.strip_prefix("sessionId: ") {
                    session_id = Some(value.trim().to_owned());
                    continue;
                }
                if let Some(value) = line.strip_prefix("durationMs: ") {
                    duration_ms = value.trim().parse().ok();
                    continue;
                }
                if let Some(value) = line.strip_prefix("timeoutSecs: ") {
                    timeout_secs = value.trim().parse().ok();
                    continue;
                }
                if let Some(value) = line.strip_prefix("terminatedBy: ") {
                    if value.trim() != "none" {
                        terminated_by = Some(value.trim().to_owned());
                    }
                    continue;
                }
                if let Some(value) = line.strip_prefix("truncated: ") {
                    truncated = value.trim() == "true";
                    continue;
                }
                if line == "--- output ---" || line == "--- stdout ---" {
                    header_done = true;
                    continue;
                }
                if text.starts_with("sessions:") {
                    return Self {
                        status: BashStatus::Listed,
                        exit_code: None,
                        session_id: None,
                        stdout: String::new(),
                        stderr: String::new(),
                        output: text.to_owned(),
                        duration_ms: None,
                        timeout_secs: None,
                        terminated_by: None,
                        truncated: false,
                        retry_as_background: false,
                    };
                }
            } else {
                output_lines.push(line);
            }
        }
        let joined = output_lines.join("\n");
        let (stdout, stderr) = match joined.split_once("\n--- stderr ---\n") {
            Some((stdout, stderr)) => (stdout.to_owned(), stderr.to_owned()),
            None => (String::new(), String::new()),
        };
        let output = if stdout.is_empty() && stderr.is_empty() {
            joined
        } else {
            String::new()
        };
        let retry_as_background = matches!(status, BashStatus::TimedOut);
        Self {
            status,
            exit_code,
            session_id,
            stdout,
            stderr,
            output,
            duration_ms,
            timeout_secs,
            terminated_by,
            truncated,
            retry_as_background,
        }
    }
}

#[derive(Clone)]
pub struct BashTool {
    workspace: Workspace,
    sessions: Arc<DashMap<String, Arc<Session>>>,
    starting: Arc<DashSet<String>>,
}
struct Session {
    output: Arc<Mutex<String>>,
    terminal: Arc<Mutex<Terminal>>,
    cursor: Arc<Mutex<usize>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
}

/// Artist owns no visual terminal UI, but must model the same terminal state
/// a user would see. The upstream emulator may emit query responses while it
/// parses output; the PTY-facing writer remains owned by the session, so those
/// UI-only responses are intentionally discarded here.
#[derive(Debug)]
struct ArtistTerminalConfig;

impl TerminalConfiguration for ArtistTerminalConfig {
    fn color_palette(&self) -> ColorPalette {
        ColorPalette::default()
    }
}

struct DiscardTerminalResponses;

impl Write for DiscardTerminalResponses {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn new_terminal() -> Terminal {
    Terminal::new(
        TerminalSize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 0,
        },
        Arc::new(ArtistTerminalConfig),
        "Artist",
        env!("CARGO_PKG_VERSION"),
        Box::new(DiscardTerminalResponses),
    )
}

/// Render only the currently visible screen. The separate `output` transcript
/// remains append-only for `read(bash://...)`; `poll(bash://...)` uses this
/// terminal rendering and therefore honors cursor movement, erases, and the
/// alternate screen.
fn render_terminal_screen(terminal: &Terminal) -> String {
    let screen = terminal.screen();
    let total = screen.scrollback_rows();
    let start = total.saturating_sub(terminal.get_size().rows);
    let mut lines = screen
        .lines_in_phys_range(start..total)
        .into_iter()
        .map(|line| line.as_str().trim_end().to_owned())
        .collect::<Vec<_>>();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

impl Drop for Session {
    fn drop(&mut self) {
        // Kill the PTY child on drop so persistent/background sessions don't
        // leave orphan processes (and the reader thread, which loops until PTY
        // EOF, then exits once the child is gone).
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

impl BashTool {
    pub fn new(workspace: Workspace) -> Self {
        Self {
            workspace,
            sessions: Arc::new(DashMap::new()),
            starting: Arc::new(DashSet::new()),
        }
    }

    /// Workspace used for durable process path resolution.
    pub fn workspace(&self) -> Workspace {
        self.workspace.clone()
    }

    /// Spawn a PTY-backed process for the harness-level durable session layer.
    /// The returned id is strictly an internal process-local handle.
    pub async fn managed_start(
        &self,
        command: String,
        cwd: Option<String>,
        env: Option<BTreeMap<String, String>>,
    ) -> Result<String, ToolError> {
        let result = self
            .start(BashArgs {
                command: Some(command),
                session_id: None,
                input: None,
                wait_ms: Some(0),
                max_bytes: Some(EXEC_CAP),
                cwd,
                env,
            })
            .await?;
        BashResult::parse(&result).session_id.ok_or_else(|| {
            ToolError::Message("managed bash failed to allocate an internal session".into())
        })
    }

    /// Non-draining current snapshot for the universal session registry.
    pub fn managed_snapshot(&self, id: &str) -> Result<BashResult, ToolError> {
        let session = self.session(id)?;
        let waited = session
            .child
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .try_wait()?;
        let (status, exit_code) = match waited {
            None => (BashStatus::Running, None),
            Some(status) if status.success() => {
                (BashStatus::Completed, Some(status.exit_code() as i32))
            }
            Some(status) => (BashStatus::Failed, Some(status.exit_code() as i32)),
        };
        let output = session
            .output
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        Ok(BashResult {
            status,
            exit_code,
            session_id: None,
            stdout: String::new(),
            stderr: String::new(),
            output,
            duration_ms: None,
            timeout_secs: None,
            terminated_by: None,
            truncated: false,
            retry_as_background: false,
        })
    }

    /// Current visible emulator screen for `poll`. Unlike the semantic
    /// transcript, this models cursor and alternate-screen behavior.
    pub fn managed_screen(&self, id: &str) -> Result<String, ToolError> {
        let session = self.session(id)?;
        let terminal = session
            .terminal
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        Ok(render_terminal_screen(&terminal))
    }

    /// Whether the live PTY input channel is currently able to accept another unit.
    /// This is an integration state, not an elapsed-time or output-quiet heuristic.
    pub fn managed_input_ready(&self, id: &str) -> Result<bool, ToolError> {
        let session = self.session(id)?;
        let running = session
            .child
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .try_wait()?
            .is_none();
        if !running {
            return Ok(false);
        }
        Ok(session.writer.try_lock().is_ok())
    }

    /// Write one unit to a managed PTY. Completion of this method means the PTY
    /// integration accepted the bytes; it does not use a quiet-period heuristic.
    pub fn managed_send(&self, id: &str, input: &str) -> Result<(), ToolError> {
        let session = self.session(id)?;
        let mut writer = session
            .writer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        writer.write_all(input.as_bytes())?;
        writer.flush()?;
        Ok(())
    }

    /// Stop a locally owned managed process immediately.
    pub fn managed_abort(&self, id: &str) -> Result<(), ToolError> {
        let session = self.session(id)?;
        {
            let mut child = session
                .child
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            child.kill()?;
            let _ = child.wait();
        }
        self.sessions.remove(id);
        Ok(())
    }

    /// Send a command to the single persistent shell used by `!` input.
    /// An empty command reads any output produced since the previous request.
    pub async fn run_input(&self, command: &str) -> Result<String, ToolError> {
        if self.sessions.get(INPUT_SESSION_ID).is_some_and(|session| {
            session
                .child
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .try_wait()
                .ok()
                .flatten()
                .is_some()
        }) {
            self.sessions.remove(INPUT_SESSION_ID);
        }
        if !self.sessions.contains_key(INPUT_SESSION_ID) {
            self.start(BashArgs {
                command: Some(input_shell_command()),
                session_id: Some(INPUT_SESSION_ID.into()),
                input: None,
                wait_ms: Some(300),
                max_bytes: Some(EXEC_CAP),
                cwd: None,
                env: Some(BTreeMap::from([("TERM".into(), "dumb".into())])),
            })
            .await?;
            // Shell initialization can continue writing after the first PTY read.
            // Drain it before accepting a command so it cannot leak into that
            // command's output.
            let _ = self.read(BashArgs::for_input(None)).await?;
        }
        if command.trim().is_empty() {
            return self
                .read(BashArgs::for_input(None))
                .await
                .map(|output| clean_input_output(&output, None));
        }
        self.send(BashArgs::for_input(Some(format!("{command}\n"))))
            .await
            .map(|output| clean_input_output(&output, Some(command)))
    }

    /// Send text to a persistent PTY session owned by this bundle.
    ///
    /// Session hosts use this instead of reconstructing a model-facing tool call,
    /// so terminal state remains attached to the root runtime across GUI turns.
    pub async fn send_session_input(
        &self,
        session_id: &str,
        input: &str,
    ) -> Result<String, ToolError> {
        self.send(BashArgs {
            command: None,
            session_id: Some(session_id.into()),
            input: Some(input.into()),
            wait_ms: None,
            max_bytes: None,
            cwd: None,
            env: None,
        })
        .await
    }
}

fn clean_input_output(output: &str, command: Option<&str>) -> String {
    // Consume the complete machine-readable header through the output marker.
    // The header grows as lifecycle fields are added, so a marker is stable
    // while a fixed line count or `sessionId` boundary is not.
    let mut lines = output.lines();
    for line in lines.by_ref() {
        if line == "--- output ---" {
            break;
        }
    }
    let mut lines = lines.peekable();
    if let Some(command) = command {
        if lines
            .peek()
            .is_some_and(|line| line.trim_end_matches('\r').trim_end().ends_with(command))
        {
            lines.next();
        }
    }
    lines.collect::<Vec<_>>().join("\n")
}

fn input_shell_command() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| Path::new(shell).is_absolute() && Path::new(shell).is_file())
        .unwrap_or_else(|| "/bin/sh".into())
}

/// Arguments for the internal session operations. Never reaches a model: the
/// session-shaped surface is `artist-agent`'s, which drives these methods.
struct BashArgs {
    command: Option<String>,
    session_id: Option<String>,
    input: Option<String>,
    wait_ms: Option<u64>,
    max_bytes: Option<usize>,
    cwd: Option<String>,
    env: Option<BTreeMap<String, String>>,
}

impl BashArgs {
    fn for_input(input: Option<String>) -> Self {
        Self {
            command: None,
            session_id: Some(INPUT_SESSION_ID.into()),
            input,
            wait_ms: Some(250),
            max_bytes: Some(EXEC_CAP),
            cwd: None,
            env: None,
        }
    }
}

impl BashTool {
    async fn start(&self, args: BashArgs) -> Result<String, ToolError> {
        let command = args
            .command
            .ok_or_else(|| ToolError::Message("command is required".into()))?;
        let id = if let Some(id) = args.session_id {
            if self.sessions.contains_key(&id) || !self.starting.insert(id.clone()) {
                return Err(ToolError::Message(format!("session already exists: {id}")));
            }
            id
        } else {
            loop {
                let candidate = crate::short_id("t");
                if !self.sessions.contains_key(&candidate)
                    && self.starting.insert(candidate.clone())
                {
                    break candidate;
                }
            }
        };
        let pair = NativePtySystem::default().openpty(PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut builder = if id == INPUT_SESSION_ID {
            let mut shell = CommandBuilder::new(&command);
            shell.arg("-c");
            let executable = Path::new(&command)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            shell.arg(if executable == "fish" {
                "while read --prompt-str='' -l line; eval $line; end"
            } else {
                "while IFS= read -r line; do eval \"$line\"; done"
            });
            shell
        } else {
            let mut shell = CommandBuilder::new("/bin/bash");
            shell.arg("-lc");
            shell.arg(&command);
            shell
        };
        builder.cwd(self.cwd(args.cwd.as_deref())?);
        if let Some(env) = args.env {
            for (key, value) in env {
                builder.env(key, value);
            }
        }
        let mut child = match pair.slave.spawn_command(builder) {
            Ok(child) => child,
            Err(error) => {
                self.starting.remove(&id);
                return Err(error.into());
            }
        };
        drop(pair.slave);
        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(error) => {
                let _ = child.kill();
                self.starting.remove(&id);
                return Err(error.into());
            }
        };
        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                self.starting.remove(&id);
                return Err(error.into());
            }
        };
        let output = Arc::new(Mutex::new(String::new()));
        let sink = output.clone();
        let terminal = Arc::new(Mutex::new(new_terminal()));
        let terminal_sink = Arc::clone(&terminal);
        let cursor = Arc::new(Mutex::new(0usize));
        std::thread::spawn(move || {
            let mut bytes = [0u8; 4096];
            // Carry an incomplete trailing UTF-8 sequence across reads so a
            // multi-byte char split at a 4096-byte boundary isn't corrupted
            // into replacement characters.
            let mut carry: Vec<u8> = Vec::new();
            let mut pending_control = String::new();
            while let Ok(count) = reader.read(&mut bytes) {
                if count == 0 {
                    break;
                }
                terminal_sink
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .advance_bytes(&bytes[..count]);
                let mut buf = std::mem::take(&mut carry);
                buf.extend_from_slice(&bytes[..count]);
                let decoded = match std::str::from_utf8(&buf) {
                    Ok(valid) => valid.to_owned(),
                    Err(error) => {
                        let valid_up_to = error.valid_up_to();
                        let mut piece =
                            std::str::from_utf8(&buf[..valid_up_to]).unwrap().to_owned();
                        match error.error_len() {
                            // Incomplete sequence at the tail: hold it for the
                            // next read.
                            None => carry = buf[valid_up_to..].to_vec(),
                            // Genuinely invalid bytes: emit replacements now.
                            Some(_) => {
                                piece.push_str(&String::from_utf8_lossy(&buf[valid_up_to..]))
                            }
                        }
                        piece
                    }
                };
                let decoded = sanitize_terminal_chunk(&decoded, &mut pending_control);
                let mut text = sink.lock().unwrap_or_else(|poison| poison.into_inner());
                text.push_str(&decoded);
            }
        });
        self.sessions.insert(
            id.clone(),
            Arc::new(Session {
                output,
                terminal,
                cursor,
                writer: Mutex::new(writer),
                child: Mutex::new(child),
            }),
        );
        self.starting.remove(&id);
        tokio::time::sleep(Duration::from_millis(args.wait_ms.unwrap_or(250))).await;
        let output = self.session_output(&id, args.max_bytes.unwrap_or(20 * 1024))?;
        Ok(format!(
            "status: {}\nsessionId: {id}\ntruncated: false\n--- output ---\n{output}",
            self.session_status(&id)?
        ))
    }
    async fn send(&self, args: BashArgs) -> Result<String, ToolError> {
        let id = args
            .session_id
            .ok_or_else(|| ToolError::Message("sessionId is required".into()))?;
        let input = args
            .input
            .ok_or_else(|| ToolError::Message("input is required".into()))?;
        let session = self.session(&id)?;
        session.writer.lock().unwrap().write_all(input.as_bytes())?;
        session.writer.lock().unwrap().flush()?;
        tokio::time::sleep(Duration::from_millis(args.wait_ms.unwrap_or(100))).await;
        Ok(format!(
            "status: {}\nsessionId: {id}\ntruncated: false\n--- output ---\n{}",
            self.session_status(&id)?,
            self.session_output(&id, args.max_bytes.unwrap_or(20 * 1024))?
        ))
    }
    async fn read(&self, args: BashArgs) -> Result<String, ToolError> {
        let id = args
            .session_id
            .ok_or_else(|| ToolError::Message("sessionId is required".into()))?;
        tokio::time::sleep(Duration::from_millis(args.wait_ms.unwrap_or(0))).await;
        Ok(format!(
            "status: {}\nsessionId: {id}\ntruncated: false\n--- output ---\n{}",
            self.session_status(&id)?,
            self.session_output(&id, args.max_bytes.unwrap_or(20 * 1024))?
        ))
    }
    fn cwd(&self, input: Option<&str>) -> Result<std::path::PathBuf, ToolError> {
        Ok(match input {
            Some(path) => self.workspace.resolve_existing(path)?,
            None => self.workspace.root().to_owned(),
        })
    }
    fn session(&self, id: &str) -> Result<Arc<Session>, ToolError> {
        self.sessions
            .get(id)
            .map(|v| v.clone())
            .ok_or_else(|| ToolError::Message(format!("unknown session: {id}")))
    }
    fn session_status(&self, id: &str) -> Result<String, ToolError> {
        let session = self.session(id)?;
        Ok(
            match session
                .child
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .try_wait()
            {
                Ok(Some(status)) if status.success() => {
                    format!("completed\nexitCode: {}\n", status.exit_code())
                }
                Ok(Some(status)) => format!("failed\nexitCode: {}\n", status.exit_code()),
                Ok(None) => "running\n".into(),
                Err(_) => "unknown\n".into(),
            },
        )
    }

    fn session_output(&self, id: &str, max: usize) -> Result<String, ToolError> {
        let s = self.session(id)?;
        let output = s.output.lock().unwrap_or_else(|poison| poison.into_inner());
        let mut cursor = s.cursor.lock().unwrap_or_else(|poison| poison.into_inner());
        let text = output
            .get((*cursor).min(output.len())..)
            .unwrap_or("")
            .to_owned();
        *cursor = output.len();
        // Plain truncation for now. `output::tail_compressed` is written,
        // reversible and tested, but compression is parked: on realistic mixed
        // agent output it bought ~7%, and the encoder underneath needs a
        // clearer fidelity story before it goes near tool results.
        Ok(output::tail(text, max.min(50 * 1024)).0)
    }
}

/// Remove terminal control traffic before it becomes semantic transcript text.
/// Newlines remain structural text; CR, BEL, CSI/OSC escapes, and other raw
/// controls never cross the model boundary. Incomplete escape sequences carry
/// to the next PTY read so splitting a sequence cannot leak its tail.
fn sanitize_terminal_chunk(chunk: &str, pending: &mut String) -> String {
    let mut input = std::mem::take(pending);
    input.push_str(chunk);
    let bytes = input.as_bytes();
    let mut index = 0;
    let mut output = String::with_capacity(input.len());
    while index < bytes.len() {
        if bytes[index] == 0x1b {
            let start = index;
            index += 1;
            let Some(&kind) = bytes.get(index) else {
                pending.push_str(&input[start..]);
                break;
            };
            index += 1;
            match kind {
                b'[' => {
                    while let Some(&byte) = bytes.get(index) {
                        index += 1;
                        if (0x40..=0x7e).contains(&byte) {
                            break;
                        }
                    }
                    if index == bytes.len()
                        && bytes
                            .last()
                            .is_some_and(|byte| !(0x40..=0x7e).contains(byte))
                    {
                        pending.push_str(&input[start..]);
                    }
                }
                b']' => {
                    let mut terminated = false;
                    while let Some(&byte) = bytes.get(index) {
                        if byte == 0x07 {
                            index += 1;
                            terminated = true;
                            break;
                        }
                        if byte == 0x1b {
                            if bytes.get(index + 1) == Some(&b'\\') {
                                index += 2;
                                terminated = true;
                                break;
                            }
                            index += 1;
                            continue;
                        }
                        index += 1;
                    }
                    if !terminated {
                        pending.push_str(&input[start..]);
                    }
                }
                _ => {}
            }
            continue;
        }
        let character = input[index..].chars().next().expect("valid utf-8");
        index += character.len_utf8();
        if character == '\n' {
            output.push(character);
        } else if !character.is_control() {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wezterm_screen_models_cursor_movement_without_changing_transcript_rules() {
        let mut terminal = new_terminal();
        terminal.advance_bytes(b"one\rT");
        assert_eq!(render_terminal_screen(&terminal), "Tne");
    }

    #[tokio::test]
    async fn managed_readiness_comes_from_the_live_pty_input_channel() {
        let root = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(root.path(), &root.path().join("state"), "test").unwrap();
        let tool = BashTool::new(workspace);
        let id = tool
            .managed_start("cat".into(), None, None)
            .await
            .expect("start managed PTY");
        assert!(tool.managed_input_ready(&id).unwrap());
        tool.managed_send(&id, "hello\n").unwrap();
        assert!(tool.managed_input_ready(&id).unwrap());
        tool.managed_abort(&id).unwrap();
    }
}
