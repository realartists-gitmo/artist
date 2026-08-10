//! Path-first executable dispatch.
//!
//! `bash` remains the escape hatch for a shell command. `run` is narrower: it
//! resolves one addressable executable/script path, quotes arguments exactly,
//! and creates the same durable `bash://` resource that direct shell execution
//! uses. This preserves lifecycle, stop, delete, and transcript semantics.

use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::Arc,
};

use artist_tool_api::ArtistDynamicTool;
use artist_tools::resource_path::ResourcePath;
use rig_core::tool::{PortableTool, ToolExecutionError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{bash_tool::BashTool, session_tools::SessionHub};

#[derive(Clone)]
pub(crate) struct RunTool {
    bash: BashTool,
    workspace: artist_tools::Workspace,
    extension_runs: HashMap<String, ArtistDynamicTool>,
    extension_manager: Option<Arc<artist_extensions::Manager>>,
    canvas: Option<crate::canvas::CanvasTool>,
}

impl RunTool {
    pub fn new(
        backend: artist_tools::BashTool,
        sessions: SessionHub,
        extension_runs: Vec<(String, ArtistDynamicTool)>,
    ) -> Self {
        Self {
            bash: BashTool::new(backend.clone(), sessions),
            workspace: backend.workspace(),
            extension_runs: extension_runs.into_iter().collect(),
            extension_manager: None,
            canvas: None,
        }
    }

    pub(crate) fn with_extension_manager(
        mut self,
        extension_manager: Option<Arc<artist_extensions::Manager>>,
    ) -> Self {
        self.extension_manager = extension_manager;
        self
    }

    pub(crate) fn with_canvas(mut self, canvas: Option<crate::canvas::CanvasTool>) -> Self {
        self.canvas = canvas;
        self
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunArgs {
    path: String,
    #[serde(default)]
    args: Vec<String>,
    cwd: Option<String>,
    env: Option<BTreeMap<String, String>>,
    /// JSON arguments supplied to a typed `tools://` resource. Real paths use
    /// the string-vector `args` field instead.
    arguments: Option<Value>,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RunError(String);

impl From<RunError> for ToolExecutionError {
    fn from(value: RunError) -> Self {
        ToolExecutionError::other(value.to_string()).with_code("run_error")
    }
}

impl PortableTool for RunTool {
    const NAME: &'static str = "run";
    type Error = RunError;
    type Args = RunArgs;
    type Output = String;

    fn description(&self) -> String {
        "Run one real executable or script path with exact arguments and return its durable bash session id. Use bash for shell expressions; use run for a path. Typed virtual paths require their owning runtime dispatcher.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Real executable or script path. A typed virtual path is not dispatched by this base run implementation."},
                "args": {"type": "array", "items": {"type": "string"}, "default": []},
                "cwd": {"type": "string"},
                "env": {"type": "object", "additionalProperties": {"type": "string"}},
                "arguments": {"description": "JSON arguments for a tools://<extension>/tool/<name> resource"}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: RunArgs) -> Result<String, RunError> {
        match ResourcePath::parse(&args.path).map_err(|error| RunError(error.to_string()))? {
            ResourcePath::Virtual {
                scheme: artist_tools::resource_path::ResourceScheme::Tools,
                segments,
            } => {
                return self
                    .run_extension(&args.path, &segments, args.arguments)
                    .await;
            }
            ResourcePath::Virtual { .. } => {
                return Err(RunError(format!(
                    "{} is a typed virtual resource; use its owning runtime dispatcher",
                    args.path
                )));
            }
            ResourcePath::Real(_) => {}
        }
        let target = self
            .workspace
            .resolve_existing(&args.path)
            .map_err(|error| RunError(error.to_string()))?;
        let metadata = std::fs::metadata(&target).map_err(|error| RunError(error.to_string()))?;
        if !metadata.is_file() {
            return Err(RunError(format!(
                "{} is not a runnable file",
                target.display()
            )));
        }
        if let Some(canvas) = &self.canvas
            && let Some(id) = canvas
                .run_source(&target)
                .await
                .map_err(|error| RunError(error.to_string()))?
        {
            if !args.args.is_empty() || args.cwd.is_some() || args.env.is_some() {
                return Err(RunError(
                    "canvas sources do not accept run args, cwd, or env; run the source path alone"
                        .into(),
                ));
            }
            return Ok(id);
        }
        let program = run_program(&target).ok_or_else(|| {
            RunError(format!(
                "{} is neither executable nor a supported script (.sh, .bash, .py, .js); make it executable or invoke an interpreter with bash",
                target.display()
            ))
        })?;
        let native = program.is_empty();
        let command = program
            .into_iter()
            .map(|program| shell_quote(program))
            .chain(std::iter::once(shell_quote(&target.to_string_lossy())))
            .chain(args.args.iter().map(|argument| shell_quote(argument)))
            .collect::<Vec<_>>()
            .join(" ");
        if native {
            self.bash
                .start_process_session(command, args.cwd, args.env)
                .await
                .map_err(|error| RunError(error.to_string()))
        } else {
            self.bash
                .start_session(command, args.cwd, args.env, false)
                .await
                .map_err(|error| RunError(error.to_string()))
        }
    }
}

impl RunTool {
    async fn run_extension(
        &self,
        path: &str,
        segments: &[String],
        arguments: Option<Value>,
    ) -> Result<String, RunError> {
        if segments.len() != 3 || segments[1] != "tool" {
            return Err(RunError(format!(
                "{path} is not a runnable extension tool; use tools://<extension>/tool/<name>"
            )));
        }
        if let Some(manager) = &self.extension_manager {
            let output = manager
                .invoke_tool(
                    &segments[0],
                    &segments[2],
                    &arguments.unwrap_or_else(|| json!({})),
                )
                .await
                .map_err(|error| RunError(error.to_string()))?;
            return Ok(output.presentation.render());
        }
        let tool = self.extension_runs.get(path).ok_or_else(|| {
            RunError(format!(
                "unknown or inactive extension tool {path}; read tools://{} for its runnable children",
                segments[0]
            ))
        })?;
        let output = tool
            .execute(arguments.unwrap_or_else(|| json!({})))
            .await
            .map_err(|error| RunError(error.to_string()))?;
        Ok(output.presentation.render())
    }
}

/// Interpreter prefix for a non-executable script; an empty prefix means the
/// path itself is executable.  Keeping this table explicit prevents an
/// extension from becoming a shell-evaluated command by accident.
fn run_program(path: &Path) -> Option<Vec<&'static str>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
        {
            return Some(Vec::new());
        }
    }
    let script = path
        .extension()
        .and_then(|extension| extension.to_str())
        .and_then(|extension| match extension {
            "sh" | "bash" => Some(vec!["/bin/bash"]),
            "py" => Some(vec!["python3"]),
            "js" => Some(vec!["node"]),
            _ => None,
        });
    if script.is_some() {
        script
    } else {
        #[cfg(not(unix))]
        {
            path.extension()
                .and_then(|extension| extension.to_str())
                .filter(|extension| matches!(*extension, "exe" | "cmd" | "bat"))
                .map(|_| Vec::new())
        }
        #[cfg(unix)]
        {
            None
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_preserves_one_argument() {
        assert_eq!(shell_quote("a b'c"), "'a b'\"'\"'c'");
    }

    #[test]
    fn supported_scripts_map_to_explicit_interpreters() {
        assert_eq!(run_program(Path::new("demo.sh")), Some(vec!["/bin/bash"]));
        assert_eq!(run_program(Path::new("runner.py")), Some(vec!["python3"]));
    }

    #[tokio::test]
    async fn script_path_creates_the_standard_durable_bash_resource() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let script = root.path().join("task.sh");
        std::fs::write(&script, "printf 'ran from run\\n'\n").unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let hub = SessionHub::standard(root.path(), "goethe", None);
        let tool = RunTool::new(
            artist_tools::BashTool::new(workspace),
            hub.clone(),
            Vec::new(),
        );

        let id = tool
            .call(RunArgs {
                path: "task.sh".into(),
                args: Vec::new(),
                cwd: None,
                env: None,
                arguments: None,
            })
            .await
            .unwrap();

        let record = hub
            .registry()
            .get(&id)
            .unwrap()
            .expect("durable run record");
        assert_eq!(record.kind, "bash");
        assert_eq!(record.artist, "goethe");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_executable_creates_a_one_shot_process_resource() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let executable = root.path().join("worker");
        std::fs::write(&executable, "#!/bin/sh\nprintf native\n").unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), state.path(), "goethe").unwrap();
        let hub = SessionHub::standard(root.path(), "goethe", None);
        let tool = RunTool::new(
            artist_tools::BashTool::new(workspace),
            hub.clone(),
            Vec::new(),
        );

        let id = tool
            .call(RunArgs {
                path: "worker".into(),
                args: Vec::new(),
                cwd: None,
                env: None,
                arguments: None,
            })
            .await
            .unwrap();
        let record = hub.registry().get(&id).unwrap().unwrap();
        assert!(id.starts_with("process:"));
        assert_eq!(record.kind, "bash");
        assert_eq!(record.snapshot["resourceType"], "process");
    }

    #[tokio::test]
    async fn canonical_tools_path_dispatches_to_the_live_extension_callback() {
        let root = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(root.path(), root.path(), "goethe").unwrap();
        let hub = SessionHub::standard(root.path(), "goethe", None);
        let definition = artist_tool_api::ArtistToolDefinition {
            name: "inspect".into(),
            title: "Inspect".into(),
            description: "test extension tool".into(),
            input_schema: json!({"type":"object"}),
            output_schema: artist_tool_api::text_output_schema("inspect", "test output"),
            category: artist_tool_api::ToolCategory::External,
            annotations: artist_tool_api::ArtistToolAnnotations::external_read(),
        };
        let extension = ArtistDynamicTool::new(definition, |arguments| {
            Box::pin(async move {
                Ok(artist_tool_api::ArtistToolOutput::text(format!(
                    "extension received {arguments}"
                )))
            })
        });
        let tool = RunTool::new(
            artist_tools::BashTool::new(workspace),
            hub,
            vec![("tools://demo/tool/inspect".into(), extension)],
        );

        let result = tool
            .call(RunArgs {
                path: "tools://demo/tool/inspect".into(),
                args: Vec::new(),
                cwd: None,
                env: None,
                arguments: Some(json!({"subject":"Artist"})),
            })
            .await
            .unwrap();

        assert!(result.contains("extension received"));
        assert!(result.contains("Artist"));
    }
}
