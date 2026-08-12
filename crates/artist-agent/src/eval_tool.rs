//! Persistent Python scratchpads with policy-preserving Artist callbacks.
//!
//! The Python process owns only Python state.  Every effectful capability is
//! sent back through the current published Artist tool registry, so executing
//! locally never turns into a policy bypass.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use dashmap::DashMap;
use rig_core::tool::{PortableTool, ToolExecutionError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};

use crate::{
    ToolRegistryHandle,
    session_tools::{KindRegistration, SendPolicy, SessionHub},
};

static KERNELS: OnceLock<DashMap<String, Arc<Mutex<Kernel>>>> = OnceLock::new();

#[derive(Clone)]
pub(crate) struct EvalTool {
    project: PathBuf,
    sessions: SessionHub,
    tools: ToolRegistryHandle,
    relationships: crate::relationships::RelationshipStore,
    key: String,
    path: String,
}

impl EvalTool {
    pub(crate) fn new(
        project: PathBuf,
        sessions: SessionHub,
        tools: ToolRegistryHandle,
        relationships: crate::relationships::RelationshipStore,
    ) -> Self {
        sessions.register_kind(KindRegistration::one_shot("eval", SendPolicy::String));
        let slug = sessions.artist().replace(
            |character: char| !character.is_ascii_alphanumeric() && character != '-',
            "-",
        );
        Self {
            key: format!("{}::{slug}", project.display()),
            path: format!("eval://{slug}"),
            project,
            sessions,
            tools,
            relationships,
        }
    }

    fn ensure_session(&self) -> Result<(), EvalError> {
        let id = format!("eval:{}", self.path.trim_start_matches("eval://"));
        if self
            .sessions
            .registry()
            .get(&id)
            .map_err(registry_error)?
            .is_none()
        {
            self.sessions
                .registry()
                .create_exact(
                    &id,
                    "eval",
                    self.sessions.artist(),
                    None,
                    json!({"path": self.path, "state": "ready", "persistence": "agent-session"}),
                )
                .map_err(registry_error)?;
            self.relationships
                .add(
                    &format!("agent://{}", self.sessions.artist()),
                    "child",
                    &self.path,
                )
                .map_err(|error| EvalError(error.to_string()))?;
        }
        Ok(())
    }

    async fn kernel(&self) -> Result<Arc<Mutex<Kernel>>, EvalError> {
        let kernels = KERNELS.get_or_init(DashMap::new);
        if let Some(kernel) = kernels.get(&self.key) {
            return Ok(Arc::clone(kernel.value()));
        }
        let kernel = Arc::new(Mutex::new(Kernel::spawn(&self.project).await?));
        match kernels.entry(self.key.clone()) {
            dashmap::mapref::entry::Entry::Occupied(entry) => Ok(Arc::clone(entry.get())),
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                entry.insert(Arc::clone(&kernel));
                Ok(kernel)
            }
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct EvalArgs {
    pub(crate) code: Option<String>,
    #[serde(default)]
    pub(crate) reset: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvalResult {
    path: String,
    stdout: String,
    stderr: String,
    value: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    callbacks: Vec<EvalCallback>,
}

/// One mediated call made by the Python scratchpad.  This is intentionally
/// part of the eval result: the parent tool event is the durable provenance
/// for effects requested from inside the interpreter.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvalCallback {
    tool: String,
    arguments: Value,
    response: Value,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct EvalError(String);

impl From<EvalError> for ToolExecutionError {
    fn from(value: EvalError) -> Self {
        ToolExecutionError::other(value.to_string()).with_code("eval_error")
    }
}

impl PortableTool for EvalTool {
    const NAME: &'static str = "eval";
    type Error = EvalError;
    type Args = EvalArgs;
    type Output = Value;

    fn description(&self) -> String {
        "Execute Python in this agent's persistent workspace scratchpad and return its eval:// session path plus structured stdout, stderr, value, and errors. `artist.call(tool, arguments)` invokes only tools currently allowed by the agent profile.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "code": {"type": "string", "description": "Python source to execute in the persistent scratchpad."},
                "reset": {"type": "boolean", "default": false, "description": "Terminate and discard this agent session's in-memory Python state."}
            },
            "additionalProperties": false
        })
    }

    async fn call(&self, args: EvalArgs) -> Result<Value, EvalError> {
        self.ensure_session()?;
        let kernels = KERNELS.get_or_init(DashMap::new);
        if args.reset {
            if let Some((_, kernel)) = kernels.remove(&self.key) {
                kernel.lock().await.stop().await;
            }
            return Ok(json!({"path": self.path, "reset": true}));
        }
        let code = args
            .code
            .filter(|code| !code.trim().is_empty())
            .ok_or_else(|| EvalError("eval code is required unless reset=true".into()))?;
        let kernel = self.kernel().await?;
        let result = kernel.lock().await.execute(&code, &self.tools).await?;
        serde_json::to_value(EvalResult {
            path: self.path.clone(),
            ..result
        })
        .map_err(|error| EvalError(error.to_string()))
    }
}

struct Kernel {
    child: Child,
    stdin: ChildStdin,
    stdout: tokio::io::Lines<BufReader<ChildStdout>>,
}

impl Kernel {
    async fn spawn(project: &Path) -> Result<Self, EvalError> {
        let interpreter = discover_python(project);
        let mut child = Command::new(&interpreter)
            .arg("-u")
            .arg("-c")
            .arg(PYTHON_BRIDGE)
            .current_dir(project)
            // The session shares the workspace, not the parent process's
            // ambient credential bag. Tool access is mediated by Rust.
            .env_remove("GITHUB_TOKEN")
            .env_remove("OPENAI_API_KEY")
            .env_remove("ANTHROPIC_API_KEY")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|error| {
                EvalError(format!(
                    "starting Python interpreter {}: {error}",
                    interpreter.display()
                ))
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| EvalError("Python stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| EvalError("Python stdout unavailable".into()))?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
        })
    }

    async fn execute(
        &mut self,
        code: &str,
        tools: &ToolRegistryHandle,
    ) -> Result<EvalResult, EvalError> {
        let mut callbacks = Vec::new();
        self.write(json!({"type":"exec", "code":code})).await?;
        loop {
            let line = self
                .stdout
                .next_line()
                .await
                .map_err(|error| EvalError(format!("reading Python response: {error}")))?
                .ok_or_else(|| EvalError("Python scratchpad exited unexpectedly".into()))?;
            let message: Value = serde_json::from_str(&line)
                .map_err(|error| EvalError(format!("invalid Python bridge response: {error}")))?;
            match message.get("type").and_then(Value::as_str) {
                Some("callback") => {
                    let tool = message
                        .get("tool")
                        .and_then(Value::as_str)
                        .ok_or_else(|| EvalError("Python callback omitted tool".into()))?;
                    let arguments = message
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    let response = if tool == "eval" {
                        json!({"ok": false, "error": "eval cannot recursively call itself"})
                    } else {
                        match tools.execute(tool, arguments.clone()).await {
                            Some(Ok(value)) => json!({"ok": true, "value": value}),
                            Some(Err(error)) => json!({"ok": false, "error": error.to_string()}),
                            None => {
                                json!({"ok": false, "error": format!("tool `{tool}` is not permitted in this active profile")})
                            }
                        }
                    };
                    callbacks.push(EvalCallback {
                        tool: tool.to_owned(),
                        arguments,
                        response: response.clone(),
                    });
                    self.write(json!({"type":"callback_result", "response":response}))
                        .await?;
                }
                Some("result") => {
                    return Ok(EvalResult {
                        path: String::new(),
                        stdout: message
                            .get("stdout")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        stderr: message
                            .get("stderr")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        value: message
                            .get("value")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        callbacks,
                    });
                }
                Some("error") => {
                    return Err(EvalError(
                        message
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("Python execution failed")
                            .to_owned(),
                    ));
                }
                _ => return Err(EvalError("unknown Python bridge message".into())),
            }
        }
    }

    async fn write(&mut self, value: Value) -> Result<(), EvalError> {
        let line = serde_json::to_string(&value).map_err(|error| EvalError(error.to_string()))?;
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| EvalError(error.to_string()))?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|error| EvalError(error.to_string()))?;
        self.stdin
            .flush()
            .await
            .map_err(|error| EvalError(error.to_string()))
    }

    async fn stop(&mut self) {
        let _ = self.child.kill().await;
    }
}

fn discover_python(project: &Path) -> PathBuf {
    for candidate in [".venv/bin/python", "venv/bin/python", ".env/bin/python"] {
        let path = project.join(candidate);
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from("python3")
}

fn registry_error(error: artist_registry::Error) -> EvalError {
    EvalError(error.to_string())
}

const PYTHON_BRIDGE: &str = r#"
import contextlib, io, json, sys, traceback

class Artist:
    def call(self, tool, arguments=None):
        print(json.dumps({"type":"callback","tool":tool,"arguments":arguments or {}}), file=sys.__stdout__, flush=True)
        reply = json.loads(sys.stdin.readline())
        response = reply.get("response", {})
        if not response.get("ok"):
            raise RuntimeError(response.get("error", "Artist tool callback failed"))
        return response.get("value")

scope = {"artist": Artist(), "__name__": "__artist_eval__"}
for raw in sys.stdin:
    try:
        command = json.loads(raw)
        if command.get("type") != "exec":
            continue
        stdout, stderr = io.StringIO(), io.StringIO()
        value = None
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            try:
                code = command["code"]
                try:
                    compiled = compile(code, "<artist-eval>", "eval")
                except SyntaxError:
                    compiled = compile(code, "<artist-eval>", "exec")
                    exec(compiled, scope, scope)
                else:
                    value = repr(eval(compiled, scope, scope))
            except Exception:
                traceback.print_exc(file=stderr)
        print(json.dumps({"type":"result", "stdout":stdout.getvalue(), "stderr":stderr.getvalue(), "value":value}), flush=True)
    except Exception as error:
        print(json.dumps({"type":"error", "error":str(error)}), flush=True)
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use artist_tool_api::{
        ArtistDynamicTool, ArtistToolAnnotations, ToolCategory, text_output_schema,
    };

    #[tokio::test]
    async fn scratchpad_persists_state_until_explicit_reset() {
        let project = tempfile::tempdir().unwrap();
        let sessions = SessionHub::standard(project.path(), "ada", None);
        let relationships =
            crate::relationships::RelationshipStore::for_project(project.path()).unwrap();
        let tool = EvalTool::new(
            project.path().to_path_buf(),
            sessions,
            ToolRegistryHandle::default(),
            relationships.clone(),
        );
        let first = tool
            .call(EvalArgs {
                code: Some("value = 41".into()),
                reset: false,
            })
            .await
            .unwrap();
        assert_eq!(first["path"], "eval://ada");
        assert_eq!(
            relationships
                .list(Some("agent://ada"), None, None)
                .unwrap()
                .len(),
            1
        );
        let second = tool
            .call(EvalArgs {
                code: Some("value + 1".into()),
                reset: false,
            })
            .await
            .unwrap();
        assert_eq!(second["value"], "42");
        assert_eq!(
            tool.call(EvalArgs {
                code: None,
                reset: true
            })
            .await
            .unwrap(),
            json!({"path": "eval://ada", "reset": true})
        );
    }

    #[tokio::test]
    async fn callbacks_use_the_published_tool_surface() {
        let project = tempfile::tempdir().unwrap();
        let registry = ToolRegistryHandle::default();
        let echo = rig_core::tool::PortableDynamicTool::new(
            "echo",
            "echo",
            json!({"type":"object"}),
            |arguments: Value| {
                Box::pin(async move {
                    Ok(rig_core::tool::ToolOutput::text(
                        arguments["text"].as_str().unwrap_or_default().to_owned(),
                    ))
                })
            },
        );
        registry.publish(vec![ArtistDynamicTool::from_portable(
            echo,
            text_output_schema("echo", "echo"),
            ToolCategory::Administration,
            ArtistToolAnnotations::read_only(),
        )]);
        let tool = EvalTool::new(
            project.path().to_path_buf(),
            SessionHub::standard(project.path(), "bea", None),
            registry,
            crate::relationships::RelationshipStore::for_project(project.path()).unwrap(),
        );
        let output = tool
            .call(EvalArgs {
                code: Some("artist.call('echo', {'text': 'hello'})".into()),
                reset: false,
            })
            .await
            .unwrap();
        assert_eq!(output["value"], "\"hello\"");
        assert_eq!(output["callbacks"][0]["tool"], "echo");
        assert_eq!(output["callbacks"][0]["arguments"]["text"], "hello");
        assert_eq!(output["callbacks"][0]["response"]["ok"], true);
    }

    #[tokio::test]
    async fn callbacks_cannot_bypass_the_published_tool_surface() {
        let project = tempfile::tempdir().unwrap();
        // An empty registry is the policy-filtered surface for a profile that
        // permits no callbacks. The bridge must not fall back to local process
        // access or a separately constructed tool set.
        let tool = EvalTool::new(
            project.path().to_path_buf(),
            SessionHub::standard(project.path(), "cy", None),
            ToolRegistryHandle::default(),
            crate::relationships::RelationshipStore::for_project(project.path()).unwrap(),
        );
        let output = tool
            .call(EvalArgs {
                code: Some("artist.call('bash', {'command': 'echo bypass'})".into()),
                reset: false,
            })
            .await
            .unwrap();
        assert!(
            output["stderr"]
                .as_str()
                .unwrap_or_default()
                .contains("not permitted in this active profile"),
            "callback must report the policy denial: {output}"
        );
        assert!(
            !output["stdout"]
                .as_str()
                .unwrap_or_default()
                .contains("bypass\n"),
            "callback must not execute bash"
        );
        assert_eq!(output["callbacks"][0]["tool"], "bash");
        assert_eq!(output["callbacks"][0]["response"]["ok"], false);
    }
}
