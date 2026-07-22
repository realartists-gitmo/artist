use crate::{ToolError, Workspace, output};
use dashmap::{DashMap, DashSet};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

const EXEC_CAP: usize = 50 * 1024;
const DEFAULT_EXEC_TIMEOUT_SECS: u64 = 10;
const SESSION_CAP: usize = 2 * 1024 * 1024;
const INPUT_SESSION_ID: &str = "artist-input-shell";

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
}

fn clean_input_output(output: &str, command: Option<&str>) -> String {
    // The status header spans a variable number of lines — `status: <word>`,
    // an optional `exitCode:` line for a finished child, then `sessionId:` —
    // so consume through the `sessionId:` line rather than a fixed count, which
    // used to leak the `exitCode:`/`sessionId:` line for completed commands.
    let mut lines = output.lines();
    for line in lines.by_ref() {
        if line.starts_with("sessionId:") {
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
impl Tool for BashTool {
    const NAME: &'static str = "bash";
    type Error = ToolError;
    type Args = BashArgs;
    type Output = String;
    fn description(&self) -> String {
        "Run tests, builds, diagnostics, package commands, or persistent terminal sessions. Commands default to the project root; cwd may be project-relative or absolute. Use the dedicated find, grep, and read tools instead of shell file discovery or content-search commands."
            .into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"mode":{"enum":["exec","start","send","read","stop","list"]},"command":{"type":"string"},"background":{"type":"boolean","default":false,"description":"Return a persistent session immediately so other work can continue."},"sessionId":{"type":"string"},"input":{"type":"string"},"timeout":{"type":"integer","minimum":1,"default":DEFAULT_EXEC_TIMEOUT_SECS,"description":"Maximum time in seconds for a foreground exec command. Defaults to 10 seconds. If exceeded, the command is killed and the result explicitly reports the timeout."},"waitMs":{"type":"integer"},"maxBytes":{"type":"integer"},"cwd":{"type":"string","description":"Project-relative or absolute working directory."},"env":{"type":"object","additionalProperties":{"type":"string"}},"signal":{"enum":["SIGINT","SIGTERM","SIGKILL"]}},"additionalProperties":false})
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
    async fn exec(&self, args: BashArgs) -> Result<String, ToolError> {
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
        let mut child = process.spawn()?;
        let buffer = Arc::new(tokio::sync::Mutex::new((Vec::new(), false)));
        let mut stdout = tokio::spawn(pump(child.stdout.take().unwrap(), buffer.clone(), cap));
        let mut stderr = tokio::spawn(pump(child.stderr.take().unwrap(), buffer.clone(), cap));
        let timeout_secs = args.timeout.unwrap_or(DEFAULT_EXEC_TIMEOUT_SECS);
        let timeout = Duration::from_secs(timeout_secs);
        let (status, exit_code) = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(result) => {
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
            Err(_) => {
                #[cfg(unix)]
                if let Some(pid) = child.id() {
                    let _ = nix::sys::signal::killpg(
                        nix::unistd::Pid::from_raw(pid as i32),
                        nix::sys::signal::Signal::SIGKILL,
                    );
                }
                let _ = child.kill().await;
                ("timedOut", None)
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
        let buffer = buffer.lock().await;
        let output = String::from_utf8_lossy(&buffer.0);
        let timeout_notice = if status == "timedOut" {
            format!("timeout: command exceeded {timeout_secs}s and was terminated\n")
        } else {
            String::new()
        };
        Ok(format!(
            "status: {status}\nexitCode: {exit_code:?}\n{timeout_notice}truncated: {}\n{output}",
            buffer.1
        ))
    }
    async fn start(&self, args: BashArgs) -> Result<String, ToolError> {
        let command = args
            .command
            .ok_or_else(|| ToolError::Message("command is required".into()))?;
        let id = args
            .session_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
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
        if self.sessions.contains_key(&id) || !self.starting.insert(id.clone()) {
            return Err(ToolError::Message(format!("session already exists: {id}")));
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
            "status: {}sessionId: {id}\n{output}",
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
            "status: {}sessionId: {id}\n{}",
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
            "status: {}sessionId: {id}\n{}",
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
        Ok(format!("status: {status}\nsessionId: {id}\n{output}"))
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
        lines
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
