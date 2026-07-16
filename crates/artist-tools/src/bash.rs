use crate::{ToolError, Workspace, output};
use dashmap::DashMap;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

const EXEC_CAP: usize = 50 * 1024;
const SESSION_CAP: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct BashTool {
    workspace: Workspace,
    sessions: Arc<DashMap<String, Arc<Session>>>,
}
struct Session {
    command: String,
    output: Arc<Mutex<String>>,
    cursor: Arc<Mutex<usize>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
}

impl BashTool {
    pub fn new(workspace: Workspace) -> Self {
        Self {
            workspace,
            sessions: Arc::new(DashMap::new()),
        }
    }
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
}
impl Tool for BashTool {
    const NAME: &'static str = "bash";
    type Error = ToolError;
    type Args = BashArgs;
    type Output = String;
    fn description(&self) -> String {
        "Run tests, builds, diagnostics, package commands, or persistent terminal sessions from the project root. Use the dedicated find, grep, and read tools instead of shell file discovery or content-search commands."
            .into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"mode":{"enum":["exec","start","send","read","stop","list"]},"command":{"type":"string"},"sessionId":{"type":"string"},"input":{"type":"string"},"timeout":{"type":"integer"},"waitMs":{"type":"integer"},"maxBytes":{"type":"integer"},"cwd":{"type":"string"},"env":{"type":"object","additionalProperties":{"type":"string"}},"signal":{"enum":["SIGINT","SIGTERM","SIGKILL"]}},"additionalProperties":false})
    }
    async fn call(&self, args: BashArgs) -> Result<String, ToolError> {
        let mode = args.mode.as_deref().unwrap_or(if args.command.is_some() {
            "exec"
        } else {
            "list"
        });
        match mode {
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
        let stdout = tokio::spawn(pump(child.stdout.take().unwrap(), buffer.clone(), cap));
        let stderr = tokio::spawn(pump(child.stderr.take().unwrap(), buffer.clone(), cap));
        let timeout = Duration::from_secs(args.timeout.unwrap_or(120));
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
        let _ = tokio::join!(stdout, stderr);
        let buffer = buffer.lock().await;
        let output = String::from_utf8_lossy(&buffer.0);
        Ok(format!(
            "status: {status}\nexitCode: {exit_code:?}\ntruncated: {}\n{output}",
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
        if self.sessions.contains_key(&id) {
            return Err(ToolError::Message(format!("session already exists: {id}")));
        }
        let pair = NativePtySystem::default().openpty(PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut builder = CommandBuilder::new("/bin/bash");
        builder.arg("-lc");
        builder.arg(&command);
        builder.cwd(self.cwd(args.cwd.as_deref())?);
        if let Some(env) = args.env {
            for (key, value) in env {
                builder.env(key, value);
            }
        }
        let child = pair.slave.spawn_command(builder)?;
        drop(pair.slave);
        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;
        let output = Arc::new(Mutex::new(String::new()));
        let sink = output.clone();
        let cursor = Arc::new(Mutex::new(0usize));
        let reader_cursor = cursor.clone();
        std::thread::spawn(move || {
            let mut bytes = [0u8; 4096];
            while let Ok(count) = reader.read(&mut bytes) {
                if count == 0 {
                    break;
                }
                let mut text = sink.lock().unwrap_or_else(|poison| poison.into_inner());
                text.push_str(&String::from_utf8_lossy(&bytes[..count]));
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
        tokio::time::sleep(Duration::from_millis(args.wait_ms.unwrap_or(250))).await;
        let output = self.session_output(&id, args.max_bytes.unwrap_or(20 * 1024))?;
        Ok(format!("status: running\nsessionId: {id}\n{output}"))
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
            "status: running\nsessionId: {id}\n{}",
            self.session_output(&id, args.max_bytes.unwrap_or(20 * 1024))?
        ))
    }
    async fn read(&self, args: BashArgs) -> Result<String, ToolError> {
        let id = args
            .session_id
            .ok_or_else(|| ToolError::Message("sessionId is required".into()))?;
        tokio::time::sleep(Duration::from_millis(args.wait_ms.unwrap_or(0))).await;
        Ok(format!(
            "sessionId: {id}\n{}",
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
        Ok(format!(
            "status: stopped\nsessionId: {id}\n{}",
            self.session_output(&id, args.max_bytes.unwrap_or(20 * 1024))?
        ))
    }
    fn list(&self) -> String {
        if self.sessions.is_empty() {
            return "sessions: []".into();
        }
        self.sessions
            .iter()
            .map(|entry| {
                let status = match entry
                    .value()
                    .child
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .try_wait()
                {
                    Ok(Some(_)) => "exited",
                    Ok(None) => "running",
                    Err(_) => "unknown",
                };
                format!("{}\t{status}\t{}", entry.key(), entry.value().command)
            })
            .collect::<Vec<_>>()
            .join("\n")
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
