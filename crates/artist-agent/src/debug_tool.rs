//! Model-facing durable DAP sessions.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use artist_registry::SessionStatus;
use dashmap::DashMap;
use rig_core::tool::{PortableTool, ToolExecutionError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{
    dap::DapSession,
    session_tools::{KindRegistration, SendPolicy, SessionHub},
};

static SESSIONS: OnceLock<DashMap<String, Arc<Mutex<DapSession>>>> = OnceLock::new();

#[derive(Clone)]
pub(crate) struct DebugTool {
    project: PathBuf,
    workspace: artist_tools::Workspace,
    hub: SessionHub,
    relationships: crate::relationships::RelationshipStore,
}

impl DebugTool {
    pub(crate) fn new(
        workspace: artist_tools::Workspace,
        hub: SessionHub,
        relationships: crate::relationships::RelationshipStore,
    ) -> Self {
        hub.register_kind(KindRegistration::one_shot("debug", SendPolicy::String));
        Self {
            project: workspace.root().to_path_buf(),
            workspace,
            hub,
            relationships,
        }
    }

    fn registry_id(id: &str) -> String {
        format!("debug:{id}")
    }

    fn managed_adapter_cache() -> Option<PathBuf> {
        std::env::var_os("ARTIST_DAP_CACHE_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::cache_dir().map(|path| path.join("artist/dap")))
    }

    fn resolve_adapter_in(
        name: &str,
        extra: Vec<String>,
        cache: Option<&Path>,
    ) -> (String, Vec<String>, String, String) {
        // Stable model-facing names select a shared, optional Artist cache
        // before PATH. Artist does not install adapters implicitly: placing an
        // executable in this cache is an explicit host-management operation.
        // `command:<path>` remains the escape hatch for a project-specific
        // adapter and never consults the cache.
        let (path_in_cache, command, mut args, label) = match name {
            "python" => (
                cache.and_then(|root| {
                    [
                        root.join("python/bin/python3"),
                        root.join("python/bin/python"),
                    ]
                    .into_iter()
                    .find(|path| path.is_file())
                }),
                "python3".into(),
                vec!["-m".into(), "debugpy.adapter".into()],
                "python".into(),
            ),
            "rust" | "lldb" => (
                cache
                    .map(|root| root.join("lldb/lldb-dap"))
                    .filter(|path| path.is_file()),
                "lldb-dap".into(),
                Vec::new(),
                "lldb".into(),
            ),
            "go" => (
                cache
                    .map(|root| root.join("go/dlv"))
                    .filter(|path| path.is_file()),
                "dlv".into(),
                vec!["dap".into()],
                "go".into(),
            ),
            "javascript" | "typescript" | "node" => (
                cache
                    .map(|root| root.join("javascript/js-debug-adapter"))
                    .filter(|path| path.is_file()),
                "js-debug-adapter".into(),
                Vec::new(),
                "javascript".into(),
            ),
            command if command.starts_with("command:") => (
                None,
                command.trim_start_matches("command:").into(),
                Vec::new(),
                "custom".into(),
            ),
            // Compatibility for an existing explicit executable invocation.
            command => (None, command.into(), Vec::new(), "custom".into()),
        };
        args.extend(extra);
        match path_in_cache {
            Some(path) => (
                path.display().to_string(),
                args,
                label,
                "artist-cache".into(),
            ),
            None => (command, args, label, "path-or-explicit".into()),
        }
    }

    fn resolve_adapter(name: &str, extra: Vec<String>) -> (String, Vec<String>, String, String) {
        Self::resolve_adapter_in(name, extra, Self::managed_adapter_cache().as_deref())
    }

    async fn annotate_stack_frames(&self, body: &mut Value) {
        let Some(frames) = body.get_mut("stackFrames").and_then(Value::as_array_mut) else {
            return;
        };
        for frame in frames {
            let Some(source_path) = frame
                .get("source")
                .and_then(|source| source.get("path"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let line = frame.get("line").and_then(Value::as_u64).unwrap_or(0);
            if line == 0 {
                continue;
            }
            let candidate = if Path::new(source_path).is_absolute() {
                PathBuf::from(source_path)
            } else {
                self.workspace.root().join(source_path)
            };
            let Ok(canonical) = std::fs::canonicalize(candidate) else {
                continue;
            };
            let Ok(relative) = canonical.strip_prefix(self.workspace.root()) else {
                continue;
            };
            let Ok(anchors) = self
                .workspace
                .anchors_for(&relative.to_string_lossy())
                .await
            else {
                continue;
            };
            let Some((_, anchor)) = anchors
                .into_iter()
                .find(|(number, _)| *number == line as usize)
            else {
                continue;
            };
            frame
                .as_object_mut()
                .expect("DAP frame is an object")
                .insert(
                    "artistLocation".into(),
                    json!({"path":canonical,"anchor":anchor,"line":line}),
                );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_adapter_names_expand_to_standard_dap_commands() {
        assert_eq!(
            DebugTool::resolve_adapter_in("go", vec!["--log".into()], None),
            (
                "dlv".into(),
                vec!["dap".into(), "--log".into()],
                "go".into(),
                "path-or-explicit".into()
            )
        );
        assert_eq!(
            DebugTool::resolve_adapter_in("command:/tmp/adapter", Vec::new(), None),
            (
                "/tmp/adapter".into(),
                Vec::new(),
                "custom".into(),
                "path-or-explicit".into()
            )
        );
    }

    #[test]
    fn managed_cache_precedes_path_for_named_adapters() {
        let cache = tempfile::tempdir().unwrap();
        let adapter = cache.path().join("go/dlv");
        std::fs::create_dir_all(adapter.parent().unwrap()).unwrap();
        std::fs::write(&adapter, "").unwrap();
        assert_eq!(
            DebugTool::resolve_adapter_in("go", Vec::new(), Some(cache.path())),
            (
                adapter.display().to_string(),
                vec!["dap".into()],
                "go".into(),
                "artist-cache".into()
            )
        );
    }

    #[tokio::test]
    async fn stack_frames_receive_current_workspace_anchors() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("main.rs"), "fn main() {}\n").unwrap();
        let workspace = artist_tools::Workspace::open(
            project.path(),
            project.path().join(".artist/state"),
            "ada",
        )
        .unwrap();
        let tool = DebugTool::new(
            workspace,
            SessionHub::standard(project.path(), "ada", None),
            crate::relationships::RelationshipStore::for_project(project.path()).unwrap(),
        );
        let mut body = json!({"stackFrames":[{"source":{"path":"main.rs"},"line":1}]});
        tool.annotate_stack_frames(&mut body).await;
        assert!(
            body["stackFrames"][0]["artistLocation"]["anchor"]
                .as_str()
                .is_some_and(|anchor| anchor.starts_with('#'))
        );
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct DebugArgs {
    action: String,
    session: Option<String>,
    adapter: Option<String>,
    #[serde(default)]
    adapter_args: Vec<String>,
    #[serde(default)]
    configuration: Value,
    thread_id: Option<u64>,
    frame_id: Option<u64>,
    reference: Option<u64>,
    expression: Option<String>,
    source: Option<Value>,
    #[serde(default)]
    lines: Vec<u64>,
}

#[derive(thiserror::Error, Debug)]
#[error("{0}")]
pub(crate) struct DebugError(String);
impl From<DebugError> for ToolExecutionError {
    fn from(error: DebugError) -> Self {
        ToolExecutionError::other(error.to_string()).with_code("debug_error")
    }
}

impl PortableTool for DebugTool {
    const NAME: &'static str = "debug";
    type Error = DebugError;
    type Args = DebugArgs;
    type Output = Value;

    fn description(&self) -> String {
        "Launch/attach and control standard DAP debug sessions. Every observation updates the durable debug://<session> snapshot; adapter-specific wire protocols remain behind DAP.".into()
    }

    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"action":{"type":"string","enum":["launch","attach","breakpoints","continue","pause","next","step_in","step_over","step_out","stack","scopes","variables","evaluate","terminate"]},"session":{"type":"string"},"adapter":{"type":"string","description":"python, rust/lldb, go, javascript/typescript/node, or explicit command:<path>"},"adapter_args":{"type":"array","items":{"type":"string"}},"configuration":{},"thread_id":{"type":"integer"},"frame_id":{"type":"integer"},"reference":{"type":"integer"},"expression":{"type":"string"},"source":{},"lines":{"type":"array","items":{"type":"integer"}}},"required":["action"],"additionalProperties":false})
    }

    async fn call(&self, args: DebugArgs) -> Result<Value, DebugError> {
        if matches!(args.action.as_str(), "launch" | "attach") {
            return self.start(args).await;
        }
        let id = args
            .session
            .clone()
            .ok_or_else(|| DebugError("session is required".into()))?;
        let session = SESSIONS
            .get_or_init(DashMap::new)
            .get(&id)
            .ok_or_else(|| DebugError(format!("unknown debug://{id}")))?
            .clone();
        let mut dap = session.lock().await;
        let action = args.action.as_str();
        let dap_command = match action {
            "step_in" => "stepIn",
            "step_over" => "next",
            "step_out" => "stepOut",
            other => other,
        };
        let mut body = match dap_command {
            "breakpoints" => {
                dap.set_breakpoints(args.source.unwrap_or_else(|| json!({})), args.lines)
                    .await
            }
            "continue" | "pause" | "next" | "stepIn" | "stepOut" => {
                dap.control(dap_command, args.thread_id).await
            }
            "stack" => {
                dap.stack(
                    args.thread_id
                        .ok_or_else(|| DebugError("thread_id required for stack".into()))?,
                )
                .await
            }
            "scopes" => {
                dap.scopes(
                    args.frame_id
                        .ok_or_else(|| DebugError("frame_id required for scopes".into()))?,
                )
                .await
            }
            "variables" => {
                dap.variables(
                    args.reference
                        .ok_or_else(|| DebugError("reference required for variables".into()))?,
                )
                .await
            }
            "evaluate" => {
                dap.evaluate(
                    &args
                        .expression
                        .ok_or_else(|| DebugError("expression required for evaluate".into()))?,
                    args.frame_id,
                )
                .await
            }
            "terminate" => {
                dap.terminate().await;
                Ok(json!({"terminated": true}))
            }
            _ => return Err(DebugError("unknown DAP action".into())),
        }
        .map_err(|error| DebugError(error.to_string()))?;
        drop(dap);
        if dap_command == "stack" {
            self.annotate_stack_frames(&mut body).await;
        }

        let result = json!({"path":format!("debug://{id}"),"action":action,"result":body});
        let snapshot = json!({
            "path": format!("debug://{id}"),
            "lastAction": action,
            "lastResult": result["result"].clone(),
        });
        if action == "terminate" {
            SESSIONS.get_or_init(DashMap::new).remove(&id);
            self.hub
                .registry()
                .finish(
                    &Self::registry_id(&id),
                    SessionStatus::Completed,
                    Some(snapshot),
                )
                .map_err(|error| DebugError(error.to_string()))?;
        } else {
            self.hub
                .registry()
                .set_snapshot(&Self::registry_id(&id), snapshot)
                .map_err(|error| DebugError(error.to_string()))?;
        }
        Ok(result)
    }
}

impl DebugTool {
    async fn start(&self, args: DebugArgs) -> Result<Value, DebugError> {
        let adapter = args
            .adapter
            .ok_or_else(|| DebugError("adapter is required for launch/attach".into()))?;
        let (command, adapter_args, adapter_kind, adapter_source) =
            Self::resolve_adapter(&adapter, args.adapter_args);
        let id = format!("debug-{}", uuid::Uuid::new_v4().simple());
        let request = args.action.clone();
        let session = DapSession::launch(
            &command,
            &adapter_args,
            &self.project,
            &request,
            args.configuration,
        )
        .await
        .map_err(|error| DebugError(error.to_string()))?;
        SESSIONS
            .get_or_init(DashMap::new)
            .insert(id.clone(), Arc::new(Mutex::new(session)));
        self.hub.registry().create_exact(
            &Self::registry_id(&id), "debug", self.hub.artist(), None,
            json!({"path":format!("debug://{id}"),"adapter":adapter,"adapterCommand":command,"adapterKind":adapter_kind,"adapterSource":adapter_source,"request":request,"state":"initialized"}),
        ).map_err(|error| DebugError(error.to_string()))?;
        self.relationships
            .add(
                &format!("agent://{}", self.hub.artist()),
                "child",
                &format!("debug://{id}"),
            )
            .map_err(|error| DebugError(error.to_string()))?;
        Ok(json!({
            "path": format!("debug://{id}"),
            "request": request,
            "adapter": adapter,
            "adapterCommand": command,
            "adapterKind": adapter_kind,
            "adapterSource": adapter_source,
        }))
    }
}
