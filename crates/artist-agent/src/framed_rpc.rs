//! Small, strict stdio JSON-RPC transport shared by DAP and LSP clients.
//!
//! Both protocols use the same `Content-Length` framing.  Keeping framing and
//! request-id handling here prevents an LSP implementation from quietly
//! becoming a second debugger multiplexer with different cancellation/error
//! behavior.

use std::path::Path;

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum RpcError {
    #[error("starting RPC process: {0}")]
    Spawn(String),
    #[error("RPC transport closed")]
    Closed,
    #[error("RPC framing error: {0}")]
    Frame(String),
    #[error("RPC I/O error: {0}")]
    Io(String),
    #[error("RPC response error: {0}")]
    Response(String),
}

pub(crate) struct FramedRpc {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl FramedRpc {
    pub(crate) async fn spawn(
        command: &str,
        args: &[String],
        cwd: &Path,
    ) -> Result<Self, RpcError> {
        let mut child = Command::new(command)
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| RpcError::Spawn(error.to_string()))?;
        let stdin = child.stdin.take().ok_or(RpcError::Closed)?;
        let stdout = child.stdout.take().ok_or(RpcError::Closed)?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    pub(crate) async fn request(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"seq": id, "type": "request", "command": method, "arguments": params}))
            .await?;
        loop {
            let message = self.read().await?;
            // DAP replies to `request_seq`; LSP replies to `id`. Supporting
            // both lets callers share framing while preserving protocol-level
            // schemas above this transport.
            let matches = message.get("request_seq").and_then(Value::as_u64) == Some(id)
                || message.get("id").and_then(Value::as_u64) == Some(id);
            if !matches {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(RpcError::Response(error.to_string()));
            }
            if message.get("success").and_then(Value::as_bool) == Some(false) {
                return Err(RpcError::Response(
                    message
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("request failed")
                        .into(),
                ));
            }
            return Ok(message
                .get("body")
                .cloned()
                .or_else(|| message.get("result").cloned())
                .unwrap_or(Value::Null));
        }
    }

    pub(crate) async fn request_lsp(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        loop {
            let message = self.read().await?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(RpcError::Response(error.to_string()));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    pub(crate) async fn notify_lsp(&mut self, method: &str, params: Value) -> Result<(), RpcError> {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    async fn write(&mut self, message: &Value) -> Result<(), RpcError> {
        let bytes =
            serde_json::to_vec(message).map_err(|error| RpcError::Frame(error.to_string()))?;
        self.stdin
            .write_all(format!("Content-Length: {}\r\n\r\n", bytes.len()).as_bytes())
            .await
            .map_err(io)?;
        self.stdin.write_all(&bytes).await.map_err(io)?;
        self.stdin.flush().await.map_err(io)
    }

    async fn read(&mut self) -> Result<Value, RpcError> {
        let mut length = None;
        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).await.map_err(io)?;
            if read == 0 {
                return Err(RpcError::Closed);
            }
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| RpcError::Frame("invalid Content-Length".into()))?,
                );
            }
        }
        let length = length.ok_or_else(|| RpcError::Frame("missing Content-Length".into()))?;
        let mut bytes = vec![0; length];
        self.stdout.read_exact(&mut bytes).await.map_err(io)?;
        serde_json::from_slice(&bytes).map_err(|error| RpcError::Frame(error.to_string()))
    }

    pub(crate) async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }
}

fn io(error: std::io::Error) -> RpcError {
    RpcError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_and_matches_a_dap_response() {
        let script = r#"import sys,json
for line in sys.stdin.buffer:
    if not line.startswith(b'Content-Length:'): continue
    n=int(line.split(b':')[1]); sys.stdin.buffer.readline(); msg=json.loads(sys.stdin.buffer.read(n))
    out=json.dumps({'type':'response','request_seq':msg['seq'],'success':True,'body':{'ok':msg['command']}}).encode()
    sys.stdout.buffer.write(b'Content-Length: '+str(len(out)).encode()+b'\r\n\r\n'+out); sys.stdout.buffer.flush()
"#;
        let cwd = tempfile::tempdir().unwrap();
        let mut rpc = FramedRpc::spawn(
            "python3",
            &["-u".into(), "-c".into(), script.into()],
            cwd.path(),
        )
        .await
        .unwrap();
        assert_eq!(
            rpc.request("initialize", json!({})).await.unwrap()["ok"],
            "initialize"
        );
        rpc.kill().await;
    }
}
