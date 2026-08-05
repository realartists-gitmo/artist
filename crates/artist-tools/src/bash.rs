use crate::{ToolError, Workspace, output};
use dashmap::{DashMap, DashSet};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use rig_core::tool::PortableTool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

const EXEC_CAP: usize = 50 * 1024;
const DEFAULT_EXEC_TIMEOUT_SECS: u64 = 300;
const SESSION_CAP: usize = 2 * 1024 * 1024;
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
    command: String,
    output: Arc<Mutex<String>>,
    cursor: Arc<Mutex<usize>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
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
                mode: Some("start".into()),
                command: Some(input_shell_command()),
                session_id: Some(INPUT_SESSION_ID.into()),
                input: None,
                timeout: None,
                wait_ms: Some(300),
                max_bytes: Some(EXEC_CAP),
                cwd: None,
                env: Some(BTreeMap::from([("TERM".into(), "dumb".into())])),
                signal: None,
                background: None,
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
            mode: Some("send".into()),
            command: None,
            session_id: Some(session_id.into()),
            input: Some(input.into()),
            timeout: None,
            wait_ms: None,
            max_bytes: None,
            cwd: None,
            env: None,
            signal: None,
            background: None,
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
    if let Some(command) = command
        && lines
            .peek()
            .is_some_and(|line| line.trim_end_matches('\r').trim_end().ends_with(command))
    {
        lines.next();
    }
    lines.collect::<Vec<_>>().join("\n")
}

fn input_shell_command() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| Path::new(shell).is_absolute() && Path::new(shell).is_file())
        .unwrap_or_else(|| "/bin/sh".into())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashArgs {
    mode: Option<String>,
    command: Option<String>,
    session_id: Option<String>,
    input: Option<String>,
    timeout: Option<u64>,
    wait_ms: Option<u64>,
    max_bytes: Option<usize>,
    cwd: Option<String>,
    env: Option<BTreeMap<String, String>>,
    signal: Option<String>,
    background: Option<bool>,
}

impl BashArgs {
    fn for_input(input: Option<String>) -> Self {
        Self {
            mode: Some(if input.is_some() { "send" } else { "read" }.into()),
            command: None,
            session_id: Some(INPUT_SESSION_ID.into()),
            input,
            timeout: None,
            wait_ms: Some(250),
            max_bytes: Some(EXEC_CAP),
            cwd: None,
            env: None,
            signal: None,
            background: None,
        }
    }
}
impl PortableTool for BashTool {
    const NAME: &'static str = "bash";
    type Error = ToolError;
    type Args = BashArgs;
    type Output = String;
    fn description(&self) -> String {
        "Run shell commands or manage persistent terminal sessions.".into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"mode":{"enum":["exec","start","send","read","stop","list"],"description":"Operation to perform. Defaults to `exec` when command is provided, otherwise `list`."},"command":{"type":"string","description":"Shell command to execute."},"background":{"type":"boolean","default":false,"description":"Start a persistent session immediately and return without waiting for completion."},"sessionId":{"type":"string","description":"Persistent session identifier for session operations."},"input":{"type":"string","description":"Input to send to a persistent session."},"timeout":{"type":"integer","minimum":1,"default":DEFAULT_EXEC_TIMEOUT_SECS,"description":"Maximum seconds to wait for a foreground command; timed-out commands are killed."},"waitMs":{"type":"integer","description":"Milliseconds to wait when reading a persistent session."},"maxBytes":{"type":"integer","description":"Maximum output bytes to return."},"cwd":{"type":"string","description":"Project-relative or absolute working directory. Defaults to the project root."},"env":{"type":"object","additionalProperties":{"type":"string"},"description":"Environment variables for the command."},"signal":{"enum":["SIGINT","SIGTERM","SIGKILL"],"description":"Signal to send when stopping a persistent session."}},"additionalProperties":false})
    }
    async fn call(&self, args: BashArgs) -> Result<String, ToolError> {
        let mode = args.mode.as_deref().unwrap_or(if args.command.is_some() {
            "exec"
        } else {
            "list"
        });
        match mode {
            "exec" if args.background.unwrap_or(false) => self.start(args).await,
            "exec" => self.exec(args).await,
            "start" => self.start(args).await,
            "send" => self.send(args).await,
            "read" => self.read(args).await,
            "stop" => self.stop(args).await,
            "list" => Ok(self.list()),
            other => Err(ToolError::Message(format!("invalid bash mode: {other}"))),
        }
    }
}
impl BashTool {
    /// Run a foreground command, coalescing it with an identical one already in
    /// flight when the command is a tree job.
    ///
    /// Only the foreground path reaches the coalescer, and every request from
    /// it is [`Mode::Blocking`]. Artist's background bash mode starts a
    /// *persistent session* rather than a detached build — a different thing
    /// with a different return shape — so there is currently no caller that can
    /// supersede without blocking, and the no-starvation bound holds trivially.
    /// `Mode::Background` exists for when a detached build path lands; it is
    /// the rule that keeps the bound once one does.
    async fn exec(&self, args: BashArgs) -> Result<String, ToolError> {
        let Some(command) = args.command.clone() else {
            return Err(ToolError::Message("command is required".into()));
        };
        let Some(job) = crate::tree_jobs::classify(&command, self.workspace.root()) else {
            return self.exec_once(args, None).await;
        };

        let shared = crate::coalesce::global()
            .run(&job, crate::coalesce::Mode::Blocking, |cancel| async move {
                self.exec_once(args, Some(cancel))
                    .await
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(ToolError::Message)?;

        // Say when a result covers more than the caller's own work. A model
        // that reads "failed" needs to know the failure may be in another
        // agent's edits, not its own — misattribution is the failure mode of a
        // shared result, not staleness.
        Ok(if shared.waiters > 1 || shared.superseded_earlier {
            format!(
                "note: this run was shared with {} concurrent request(s) on this worktree, so \
                 its result may include changes made by other agents\n{}",
                shared.waiters, shared.value
            )
        } else {
            shared.value
        })
    }

    async fn exec_once(
        &self,
        args: BashArgs,
        cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<String, ToolError> {
        let command = args
            .command
            .ok_or_else(|| ToolError::Message("command is required".into()))?;
        let cwd = self.cwd(args.cwd.as_deref())?;
        let mut process = Command::new("/bin/bash");
        process
            .arg("-lc")
            .arg(command)
            .current_dir(cwd)
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        process.process_group(0);
        if let Some(env) = args.env {
            process.envs(env);
        }
        let cap = args.max_bytes.unwrap_or(EXEC_CAP).min(EXEC_CAP);
        let started = Instant::now();
        let mut child = process.spawn()?;
        let stdout_buffer = Arc::new(tokio::sync::Mutex::new((Vec::new(), false)));
        let stderr_buffer = Arc::new(tokio::sync::Mutex::new((Vec::new(), false)));
        let mut stdout = tokio::spawn(pump(
            child.stdout.take().unwrap(),
            stdout_buffer.clone(),
            cap,
        ));
        let mut stderr = tokio::spawn(pump(
            child.stderr.take().unwrap(),
            stderr_buffer.clone(),
            cap,
        ));
        let timeout_secs = args.timeout.unwrap_or(DEFAULT_EXEC_TIMEOUT_SECS);
        let timeout = Duration::from_secs(timeout_secs);
        // Supersession is why the token is here: a newer request for the same
        // command means this build is producing artifacts for a tree state that
        // has already moved on, so it is killed rather than left to finish and
        // write fingerprints that no longer match the source.
        let waited = match &cancel {
            Some(token) => {
                tokio::select! {
                    result = tokio::time::timeout(timeout, child.wait()) => result,
                    () = token.cancelled() => Ok(Err(std::io::Error::other("superseded"))),
                }
            }
            None => tokio::time::timeout(timeout, child.wait()).await,
        };
        let superseded = cancel.as_ref().is_some_and(|token| token.is_cancelled());
        let (status, exit_code) = match waited {
            Ok(result) if !superseded => {
                let status = result?;
                (
                    if status.success() {
                        "completed"
                    } else {
                        "failed"
                    },
                    status.code(),
                )
            }
            _ => {
                #[cfg(unix)]
                if let Some(pid) = child.id() {
                    let _ = nix::sys::signal::killpg(
                        nix::unistd::Pid::from_raw(pid as i32),
                        nix::sys::signal::Signal::SIGKILL,
                    );
                }
                let _ = child.kill().await;
                // A superseded run is not a timeout, and must not read as one:
                // the caller is about to be carried onto a newer run, and
                // "timedOut" would tell the model its command was too slow.
                (if superseded { "superseded" } else { "timedOut" }, None)
            }
        };
        // Bound the wait for the pipes to close: a daemonizing grandchild that
        // escaped the killed process group can hold stdout/stderr open forever,
        // which would otherwise hang this call even though the child exited.
        if tokio::time::timeout(Duration::from_secs(2), async {
            let _ = tokio::join!(&mut stdout, &mut stderr);
        })
        .await
        .is_err()
        {
            stdout.abort();
            stderr.abort();
        }
        let stdout_buffer = stdout_buffer.lock().await;
        let stderr_buffer = stderr_buffer.lock().await;
        let stdout_text = String::from_utf8_lossy(&stdout_buffer.0);
        let stderr_text = String::from_utf8_lossy(&stderr_buffer.0);
        let duration_ms = started.elapsed().as_millis();
        let terminated_by = if matches!(status, "timedOut" | "superseded") {
            "SIGKILL"
        } else {
            "none"
        };
        Ok(format!(
            "status: {status}\nexitCode: {}\ndurationMs: {duration_ms}\ntimeoutSecs: {timeout_secs}\nterminatedBy: {terminated_by}\ntruncated: {}\n--- stdout ---\n{stdout_text}\n--- stderr ---\n{stderr_text}",
            exit_code.map_or_else(|| "none".to_owned(), |code| code.to_string()),
            stdout_buffer.1 || stderr_buffer.1,
        ))
    }
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
        let cursor = Arc::new(Mutex::new(0usize));
        let reader_cursor = cursor.clone();
        std::thread::spawn(move || {
            let mut bytes = [0u8; 4096];
            // Carry an incomplete trailing UTF-8 sequence across reads so a
            // multi-byte char split at a 4096-byte boundary isn't corrupted
            // into replacement characters.
            let mut carry: Vec<u8> = Vec::new();
            while let Ok(count) = reader.read(&mut bytes) {
                if count == 0 {
                    break;
                }
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
                let mut text = sink.lock().unwrap_or_else(|poison| poison.into_inner());
                text.push_str(&decoded);
                if text.len() > SESSION_CAP {
                    let mut drain = text.len() - SESSION_CAP;
                    while drain < text.len() && !text.is_char_boundary(drain) {
                        drain += 1;
                    }
                    text.drain(..drain);
                    let mut cursor = reader_cursor
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    *cursor = cursor.saturating_sub(drain);
                }
            }
        });
        self.sessions.insert(
            id.clone(),
            Arc::new(Session {
                command,
                output,
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
    async fn stop(&self, args: BashArgs) -> Result<String, ToolError> {
        let id = args
            .session_id
            .ok_or_else(|| ToolError::Message("sessionId is required".into()))?;
        let session = self.session(&id)?;
        let requested = args.signal.as_deref().unwrap_or("SIGINT");
        #[cfg(unix)]
        {
            use nix::{
                sys::signal::{Signal, killpg},
                unistd::Pid,
            };
            let signal = match requested {
                "SIGINT" => Signal::SIGINT,
                "SIGTERM" => Signal::SIGTERM,
                "SIGKILL" => Signal::SIGKILL,
                other => return Err(ToolError::Message(format!("invalid signal: {other}"))),
            };
            if let Some(pid) = session
                .child
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .process_id()
            {
                let _ = killpg(Pid::from_raw(pid as i32), signal);
            }
        }
        #[cfg(not(unix))]
        session
            .child
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .kill()?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let output = self.session_output(&id, args.max_bytes.unwrap_or(20 * 1024))?;
        // Reap the map entry once the child is gone so long-lived processes
        // don't accumulate dead sessions and their output buffers.
        let exited = session
            .child
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .try_wait()
            .ok()
            .flatten()
            .is_some();
        let status = if exited {
            self.sessions.remove(&id);
            "stopped (session removed)"
        } else {
            "stopping"
        };
        Ok(format!(
            "status: {status}\nsessionId: {id}\ntruncated: false\n--- output ---\n{output}"
        ))
    }
    fn list(&self) -> String {
        if self.sessions.is_empty() {
            return "sessions: []".into();
        }
        // Exited sessions appear once (as a tombstone) and are then reaped.
        let mut exited = Vec::new();
        let lines = self
            .sessions
            .iter()
            .map(|entry| {
                let status = match entry
                    .value()
                    .child
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .try_wait()
                {
                    Ok(Some(_)) => {
                        exited.push(entry.key().clone());
                        "exited (removed)"
                    }
                    Ok(None) => "running",
                    Err(_) => "unknown",
                };
                format!("{}\t{status}\t{}", entry.key(), entry.value().command)
            })
            .collect::<Vec<_>>()
            .join("\n");
        for id in exited {
            self.sessions.remove(&id);
        }
        format!("sessions:\n{lines}")
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

async fn pump(
    mut reader: impl AsyncRead + Unpin,
    buffer: Arc<tokio::sync::Mutex<(Vec<u8>, bool)>>,
    cap: usize,
) {
    let mut chunk = [0u8; 4096];
    while let Ok(count) = reader.read(&mut chunk).await {
        if count == 0 {
            break;
        }
        let mut output = buffer.lock().await;
        output.0.extend_from_slice(&chunk[..count]);
        if output.0.len() > cap {
            let drain = output.0.len() - cap;
            output.0.drain(..drain);
            output.1 = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nonempty_session_listing_is_typed_as_listed() {
        let parsed = BashResult::parse("sessions:\nt-red-wolf\trunning\tcargo test");
        assert_eq!(parsed.status, BashStatus::Listed);
        assert!(parsed.output.contains("t-red-wolf"));
    }
}
