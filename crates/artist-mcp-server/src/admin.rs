//! Administrative tools owned by the MCP process rather than the agent core.

use std::path::{Path, PathBuf};

use artist_tool_api::{
    ArtistDynamicTool, ArtistToolAnnotations, ArtistToolDefinition, ArtistToolOutput, ToolCategory,
};
use rig_core::tool::{ToolExecutionError, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::envelope::Envelope;

pub fn operation_tool(envelope: Envelope) -> ArtistDynamicTool {
    ArtistDynamicTool::new(
        ArtistToolDefinition {
            name: "operation".into(),
            title: "Operation Recovery".into(),
            description: "Recover completed keyed operations after a timeout, reconnect, or process restart. Use action=get with an idempotency key, or action=list for recent operations.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {"enum": ["get", "list"], "default": "list"},
                    "key": {"type": "string", "description": "Idempotency key to recover when action is get."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 20}
                },
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "action": {"enum": ["get", "list"]},
                    "found": {"type": "boolean"},
                    "operation": {
                        "anyOf": [
                            {"$ref": "#/$defs/operation"},
                            {"type": "null"}
                        ]
                    },
                    "operations": {
                        "type": "array",
                        "items": {"$ref": "#/$defs/operation"}
                    }
                },
                "required": ["action", "found", "operation", "operations"],
                "additionalProperties": false,
                "$defs": {
                    "operation": {
                        "type": "object",
                        "properties": {
                            "key": {"type": "string"},
                            "tool": {"type": "string"},
                            "arguments": {},
                            "result": {},
                            "completedAtMs": {"type": "integer", "minimum": 0}
                        },
                        "required": ["key", "tool", "arguments", "result", "completedAtMs"],
                        "additionalProperties": false
                    }
                }
            }),
            category: ToolCategory::Administration,
            annotations: ArtistToolAnnotations::read_only(),
        },
        move |arguments| {
            let envelope = envelope.clone();
            Box::pin(async move {
                let action = arguments
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("list");
                let structured = match action {
                    "get" => {
                        let key = arguments
                            .get("key")
                            .and_then(Value::as_str)
                            .filter(|key| !key.trim().is_empty())
                            .ok_or_else(|| ToolExecutionError::other("key is required for action=get"))?;
                        let operation = envelope.get(key);
                        json!({
                            "action": "get",
                            "found": operation.is_some(),
                            "operation": operation,
                            "operations": []
                        })
                    }
                    "list" => {
                        let limit = arguments
                            .get("limit")
                            .and_then(Value::as_u64)
                            .unwrap_or(20)
                            .clamp(1, 100) as usize;
                        let operations = envelope.list(limit);
                        json!({
                            "action": "list",
                            "found": !operations.is_empty(),
                            "operation": null,
                            "operations": operations
                        })
                    }
                    other => {
                        return Err(ToolExecutionError::other(format!(
                            "invalid operation action: {other}"
                        )));
                    }
                };
                Ok(ArtistToolOutput {
                    presentation: ToolOutput::json(structured.clone()),
                    structured,
                })
            })
        },
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActiveWorkspace {
    path: PathBuf,
    selected_at_ms: u64,
}

#[derive(Clone)]
pub struct WorkspaceStore {
    selection_file: PathBuf,
    current: PathBuf,
}

impl WorkspaceStore {
    pub fn new(config_root: &Path, current: &Path) -> Self {
        Self {
            selection_file: config_root.join("mcp").join("active-workspace.json"),
            current: current.to_path_buf(),
        }
    }

    pub fn selected(config_root: &Path) -> anyhow::Result<Option<PathBuf>> {
        let path = config_root.join("mcp").join("active-workspace.json");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let selected: ActiveWorkspace = serde_json::from_str(&text)?;
        let canonical = std::fs::canonicalize(&selected.path)?;
        anyhow::ensure!(canonical.is_dir(), "selected workspace is not a directory");
        Ok(Some(canonical))
    }

    fn save(&self, path: &Path) -> anyhow::Result<()> {
        let parent = self
            .selection_file
            .parent()
            .expect("workspace selection has a parent");
        std::fs::create_dir_all(parent)?;
        let selected = ActiveWorkspace {
            path: path.to_path_buf(),
            selected_at_ms: now(),
        };
        let temporary = self.selection_file.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&selected)?)?;
        std::fs::rename(temporary, &self.selection_file)?;
        Ok(())
    }

    fn clear(&self) -> anyhow::Result<()> {
        match std::fs::remove_file(&self.selection_file) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

pub fn workspace_tool(store: WorkspaceStore) -> ArtistDynamicTool {
    ArtistDynamicTool::new(
        ArtistToolDefinition {
            name: "workspace".into(),
            title: "Workspace Selection".into(),
            description: "Inspect or persist the workspace Artist should open on its next daemon start. Selection never changes the root of an in-flight MCP process; select and clear return restartRequired=true when a restart is needed.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {"enum": ["status", "select", "clear"], "default": "status"},
                    "path": {"type": "string", "description": "Existing directory to select."}
                },
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "action": {"enum": ["status", "select", "clear"]},
                    "currentWorkspace": {"type": "string"},
                    "selectedWorkspace": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                    "restartRequired": {"type": "boolean"},
                    "detail": {"type": "string"}
                },
                "required": ["action", "currentWorkspace", "selectedWorkspace", "restartRequired", "detail"],
                "additionalProperties": false
            }),
            category: ToolCategory::Administration,
            annotations: ArtistToolAnnotations {
                read_only: false,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
        },
        move |arguments| {
            let store = store.clone();
            Box::pin(async move {
                let action = arguments
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("status");
                let (selected, restart_required, detail) = match action {
                    "status" => {
                        let selected = read_selection_file(&store.selection_file)?;
                        (selected, false, "Current and next-start workspace state.".to_owned())
                    }
                    "select" => {
                        let requested = arguments
                            .get("path")
                            .and_then(Value::as_str)
                            .filter(|path| !path.trim().is_empty())
                            .ok_or_else(|| ToolExecutionError::other("path is required for action=select"))?;
                        let selected = std::fs::canonicalize(requested)
                            .map_err(ToolExecutionError::from_error)?;
                        if !selected.is_dir() {
                            return Err(ToolExecutionError::other("selected workspace is not a directory"));
                        }
                        store.save(&selected).map_err(|error| ToolExecutionError::other(error.to_string()))?;
                        let restart = selected != store.current;
                        (
                            Some(selected),
                            restart,
                            if restart {
                                "Selection persisted. Restart artist-mcp.service to activate it."
                            } else {
                                "The selected workspace is already active."
                            }
                            .to_owned(),
                        )
                    }
                    "clear" => {
                        store.clear().map_err(|error| ToolExecutionError::other(error.to_string()))?;
                        (
                            None,
                            true,
                            "Persisted selection cleared. Restart artist-mcp.service to use the configured default workspace."
                                .to_owned(),
                        )
                    }
                    other => {
                        return Err(ToolExecutionError::other(format!(
                            "invalid workspace action: {other}"
                        )));
                    }
                };
                let structured = json!({
                    "action": action,
                    "currentWorkspace": store.current.display().to_string(),
                    "selectedWorkspace": selected.map(|path| path.display().to_string()),
                    "restartRequired": restart_required,
                    "detail": detail
                });
                Ok(ArtistToolOutput {
                    presentation: ToolOutput::json(structured.clone()),
                    structured,
                })
            })
        },
    )
}

fn read_selection_file(path: &Path) -> Result<Option<PathBuf>, ToolExecutionError> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let selected: ActiveWorkspace =
                serde_json::from_str(&text).map_err(ToolExecutionError::from_error)?;
            Ok(Some(selected.path))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ToolExecutionError::from_error(error)),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn workspace_selection_is_persisted_for_the_next_start() {
        let config = tempfile::tempdir().unwrap();
        let current = tempfile::tempdir().unwrap();
        let next = tempfile::tempdir().unwrap();
        let tool = workspace_tool(WorkspaceStore::new(config.path(), current.path()));

        let output = tool
            .execute(json!({"action": "select", "path": next.path()}))
            .await
            .unwrap();

        assert_eq!(output.structured["restartRequired"], true);
        assert_eq!(
            WorkspaceStore::selected(config.path()).unwrap().unwrap(),
            std::fs::canonicalize(next.path()).unwrap()
        );
        assert_eq!(
            output.structured["currentWorkspace"],
            std::fs::canonicalize(current.path())
                .unwrap()
                .display()
                .to_string()
        );
    }
}
