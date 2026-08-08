use std::{collections::BTreeMap, sync::Arc};

use futures::future::BoxFuture;
use rig_core::tool::{PortableTool, ToolExecutionError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::session_tools::{OwnedSession, OwnedState, SessionHub};

#[derive(Clone)]
pub(crate) struct BashTool {
    backend: artist_tools::BashTool,
    sessions: SessionHub,
}

impl BashTool {
    pub fn new(backend: artist_tools::BashTool, sessions: SessionHub) -> Self {
        Self { backend, sessions }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BashArgs {
    command: String,
    cwd: Option<String>,
    env: Option<BTreeMap<String, String>>,
    interactive: Option<bool>,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct BashError(String);

impl From<BashError> for ToolExecutionError {
    fn from(value: BashError) -> Self {
        ToolExecutionError::other(value.to_string()).with_code("bash_error")
    }
}

struct BashSession {
    backend: artist_tools::BashTool,
    backend_id: String,
    interactive: bool,
}

impl BashSession {
    fn current(&self) -> Result<OwnedState, String> {
        let result = self
            .backend
            .managed_snapshot(&self.backend_id)
            .map_err(|error| error.to_string())?;
        let readiness =
            if self.interactive && matches!(result.status, artist_tools::BashStatus::Running) {
                Some(
                    if self
                        .backend
                        .managed_input_ready(&self.backend_id)
                        .map_err(|error| error.to_string())?
                    {
                        "ready"
                    } else {
                        "running"
                    },
                )
            } else {
                None
            };
        let snapshot = json!({
            "interactive": self.interactive,
            "readiness": readiness,
            "exitCode": result.exit_code,
            "output": result.output,
            "truncated": result.truncated,
        });
        match result.status {
            artist_tools::BashStatus::Running => Ok(OwnedState::live(snapshot)),
            artist_tools::BashStatus::Completed => Ok(OwnedState::stopped(
                artist_registry::SessionStatus::Completed,
                snapshot,
            )),
            artist_tools::BashStatus::Failed => Ok(OwnedState::stopped(
                artist_registry::SessionStatus::Failed,
                snapshot,
            )),
            other => Err(format!(
                "managed bash entered unsupported backend state {other:?}"
            )),
        }
    }
}

impl OwnedSession for BashSession {
    fn state(&self) -> BoxFuture<'_, Result<OwnedState, String>> {
        Box::pin(async move { self.current() })
    }

    fn send(&self, input: Value) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if !self.interactive {
                return Err("send is unsupported for non-interactive bash".into());
            }
            let input = input
                .as_str()
                .ok_or_else(|| "interactive bash input must be a string".to_owned())?;
            self.backend
                .managed_send(&self.backend_id, input)
                .map_err(|error| error.to_string())
        })
    }

    fn abort(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.backend
                .managed_abort(&self.backend_id)
                .map_err(|error| error.to_string())
        })
    }
}

impl PortableTool for BashTool {
    const NAME: &'static str = "bash";
    type Error = BashError;
    type Args = BashArgs;
    type Output = String;

    fn description(&self) -> String {
        "Spawn a shell/process session and return bash:<slug> immediately. interactive defaults to false; use universal poll/abort/send/list for lifecycle and input.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "command":{"type":"string"},
                "cwd":{"type":"string"},
                "env":{"type":"object","additionalProperties":{"type":"string"}},
                "interactive":{"type":"boolean","default":false}
            },
            "required":["command"],
            "additionalProperties":false
        })
    }

    async fn call(&self, args: BashArgs) -> Result<String, BashError> {
        let interactive = args.interactive.unwrap_or(false);
        let record = self
            .sessions
            .registry()
            .create_content(
                "bash",
                &args.command,
                self.sessions.artist(),
                None,
                json!({
                    "interactive": interactive,
                    "readiness": if interactive { Value::String("running".into()) } else { Value::Null },
                    "output": ""
                }),
            )
            .map_err(|error| BashError(error.to_string()))?;

        let backend_id = match self
            .backend
            .managed_start(args.command, args.cwd, args.env)
            .await
        {
            Ok(id) => id,
            Err(error) => {
                let _ = self.sessions.registry().finish(
                    &record.id,
                    artist_registry::SessionStatus::Failed,
                    Some(json!({"interactive":interactive,"error":error.to_string()})),
                );
                return Err(BashError(format!("{} (session {})", error, record.id)));
            }
        };

        self.sessions.own(
            record.id.clone(),
            Arc::new(BashSession {
                backend: self.backend.clone(),
                backend_id,
                interactive,
            }),
        );
        Ok(record.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_spawn_only() {
        let root = tempfile::tempdir().unwrap();
        let workspace =
            artist_tools::Workspace::open(root.path(), root.path().join(".artist/state"), "test")
                .unwrap();
        let tool = BashTool::new(
            artist_tools::BashTool::new(workspace),
            SessionHub::standard(root.path(), "Goethe", None),
        );
        let schema = tool.parameters();
        assert!(schema["properties"].get("mode").is_none());
        assert!(schema["properties"].get("background").is_none());
        assert!(schema["properties"].get("sessionId").is_none());
        assert_eq!(schema["required"], json!(["command"]));
    }
}
