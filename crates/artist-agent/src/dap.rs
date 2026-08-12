//! Standard Debug Adapter Protocol session commands.
//!
//! This layer deliberately contains no debugger-specific wire protocol: every
//! operation is a DAP request over [`crate::framed_rpc::FramedRpc`].

use crate::framed_rpc::{FramedRpc, RpcError};
use serde_json::{Value, json};
use std::path::Path;

pub(crate) struct DapSession {
    rpc: FramedRpc,
}

impl DapSession {
    pub(crate) async fn launch(
        command: &str,
        args: &[String],
        cwd: &Path,
        request: &str,
        configuration: Value,
    ) -> Result<Self, RpcError> {
        let mut rpc = FramedRpc::spawn(command, args, cwd).await?;
        rpc.request("initialize", json!({"clientID":"artist","adapterID":"artist","pathFormat":"path","linesStartAt1":true,"columnsStartAt1":true})).await?;
        rpc.request(request, configuration).await?;
        rpc.request("configurationDone", json!({})).await?;
        Ok(Self { rpc })
    }
    pub(crate) async fn set_breakpoints(
        &mut self,
        source: Value,
        lines: Vec<u64>,
    ) -> Result<Value, RpcError> {
        self.rpc.request("setBreakpoints", json!({"source":source,"breakpoints":lines.into_iter().map(|line| json!({"line":line})).collect::<Vec<_>>() })).await
    }
    pub(crate) async fn control(
        &mut self,
        command: &str,
        thread_id: Option<u64>,
    ) -> Result<Value, RpcError> {
        let mut args = json!({});
        if let Some(id) = thread_id {
            args["threadId"] = json!(id);
        }
        self.rpc.request(command, args).await
    }
    pub(crate) async fn stack(&mut self, thread_id: u64) -> Result<Value, RpcError> {
        self.rpc
            .request("stackTrace", json!({"threadId":thread_id}))
            .await
    }
    pub(crate) async fn scopes(&mut self, frame_id: u64) -> Result<Value, RpcError> {
        self.rpc
            .request("scopes", json!({"frameId":frame_id}))
            .await
    }
    pub(crate) async fn variables(&mut self, reference: u64) -> Result<Value, RpcError> {
        self.rpc
            .request("variables", json!({"variablesReference":reference}))
            .await
    }
    pub(crate) async fn evaluate(
        &mut self,
        expression: &str,
        frame_id: Option<u64>,
    ) -> Result<Value, RpcError> {
        self.rpc
            .request(
                "evaluate",
                json!({"expression":expression,"frameId":frame_id}),
            )
            .await
    }
    pub(crate) async fn terminate(&mut self) {
        let _ = self
            .rpc
            .request("disconnect", json!({"terminateDebuggee":true}))
            .await;
        self.rpc.kill().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn standard_dap_commands_use_the_shared_transport() {
        let script = r#"import sys,json
while True:
 h=sys.stdin.buffer.readline()
 if not h: break
 n=int(h.split(b':')[1]); sys.stdin.buffer.readline(); q=json.loads(sys.stdin.buffer.read(n))
 out=json.dumps({'type':'response','request_seq':q['seq'],'success':True,'body':{'command':q['command']}}).encode()
 sys.stdout.buffer.write(b'Content-Length: '+str(len(out)).encode()+b'\r\n\r\n'+out);sys.stdout.buffer.flush()
"#;
        let root = tempfile::tempdir().unwrap();
        let mut session = DapSession::launch(
            "python3",
            &["-u".into(), "-c".into(), script.into()],
            root.path(),
            "launch",
            json!({}),
        )
        .await
        .unwrap();
        assert_eq!(
            session.control("continue", Some(1)).await.unwrap()["command"],
            "continue"
        );
        assert_eq!(
            session.evaluate("x", None).await.unwrap()["command"],
            "evaluate"
        );
        session.terminate().await;
    }
}
