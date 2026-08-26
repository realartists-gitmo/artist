use std::{path::Path, process::Stdio};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
};
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum PythonError {
    #[error("failed to start or communicate with Python: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid Python worker message: {0}")]
    Protocol(#[from] serde_json::Error),
    #[error("Python worker disconnected")]
    Disconnected,
    #[error("Python worker did not become ready: {0}")]
    NotReady(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PythonOutput {
    pub value: Value,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
}

impl PythonOutput {
    pub fn succeeded(&self) -> bool {
        self.error.is_none()
    }
}

/// One disposable, persistent Python process for one active run.
pub struct PythonRepl {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    pub pid: u32,
    pub fff_available: bool,
    pub fff_error: Option<String>,
    pub native_bridge: bool,
}

impl PythonRepl {
    pub async fn start(python: &str, worker: &Path, workspace: &Path) -> Result<Self, PythonError> {
        let mut child = Command::new(python)
            .arg("-u")
            .arg(worker)
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child.stdin.take().ok_or(PythonError::Disconnected)?;
        let stdout = child.stdout.take().ok_or(PythonError::Disconnected)?;
        let mut lines = BufReader::new(stdout).lines();
        let line = lines.next_line().await?.ok_or(PythonError::Disconnected)?;
        let ready: Value = serde_json::from_str(&line)?;
        if ready.get("kind").and_then(Value::as_str) != Some("ready") {
            return Err(PythonError::NotReady(line));
        }
        Ok(Self {
            pid: ready.get("pid").and_then(Value::as_u64).unwrap_or_default() as u32,
            fff_available: ready
                .get("fff_available")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            fff_error: ready
                .get("fff_error")
                .and_then(Value::as_str)
                .map(str::to_owned),
            native_bridge: ready
                .get("native_bridge")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            child,
            stdin,
            stdout: lines,
        })
    }

    /// Executes code while servicing calls to the stable `artist` Python object.
    pub async fn execute<F>(&mut self, code: &str, rpc: F) -> Result<PythonOutput, PythonError>
    where
        F: Fn(&str, Value) -> std::result::Result<Value, String>,
    {
        let request_id = Uuid::new_v4();
        self.send(&json!({"kind":"execute", "id":request_id, "code":code}))
            .await?;
        loop {
            let line = self
                .stdout
                .next_line()
                .await?
                .ok_or(PythonError::Disconnected)?;
            let message: Value = serde_json::from_str(&line)?;
            match message.get("kind").and_then(Value::as_str) {
                Some("rpc") => {
                    let id = message.get("id").cloned().unwrap_or(Value::Null);
                    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
                    let params = message.get("params").cloned().unwrap_or(Value::Null);
                    let response = match rpc(method, params) {
                        Ok(value) => json!({"kind":"rpc_result", "id":id, "value":value}),
                        Err(error) => json!({"kind":"rpc_result", "id":id, "error":error}),
                    };
                    self.send(&response).await?;
                }
                Some("result") => {
                    return Ok(PythonOutput {
                        value: message.get("value").cloned().unwrap_or(Value::Null),
                        stdout: message
                            .get("stdout")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        stderr: message
                            .get("stderr")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        error: message
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    });
                }
                _ => return Err(PythonError::NotReady(line)),
            }
        }
    }

    async fn send(&mut self, value: &Value) -> Result<(), PythonError> {
        let mut encoded = serde_json::to_vec(value)?;
        encoded.push(b'\n');
        self.stdin.write_all(&encoded).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    pub async fn shutdown(mut self) -> Result<(), PythonError> {
        self.send(&json!({"kind":"shutdown"})).await?;
        self.child.wait().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn repl_keeps_memory_and_captures_output() {
        let workspace = tempfile::tempdir().unwrap();
        let worker = Path::new(env!("CARGO_MANIFEST_DIR")).join("python/worker.py");
        let mut repl = PythonRepl::start("python3", &worker, workspace.path())
            .await
            .unwrap();
        repl.execute("x = 40", |_, _| Err("no rpc".into()))
            .await
            .unwrap();
        let output = repl
            .execute("print('hello'); x + 2", |_, _| Err("no rpc".into()))
            .await
            .unwrap();
        assert_eq!(output.value, json!(42));
        assert_eq!(output.stdout, "hello\n");
        repl.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn repl_services_harness_rpc() {
        let workspace = tempfile::tempdir().unwrap();
        let worker = Path::new(env!("CARGO_MANIFEST_DIR")).join("python/worker.py");
        let mut repl = PythonRepl::start("python3", &worker, workspace.path())
            .await
            .unwrap();
        let output = repl
            .execute("artist.call('echo', {'x': 1})", |method, params| {
                assert_eq!(method, "echo");
                Ok(params)
            })
            .await
            .unwrap();
        assert_eq!(output.value, json!({"x":1}));
    }
}
