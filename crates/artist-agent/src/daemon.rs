use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

use artist_session::{EventLog, SessionId, SessionStore, StoreError, Workspace, WorkspaceId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::protocol::{RpcError, RpcFrame};
use crate::rpc::RpcHandler;

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("workspace {0} is already registered")]
    DuplicateWorkspace(WorkspaceId),
    #[error("workspace {0} is not registered")]
    UnknownWorkspace(WorkspaceId),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("could not persist daemon workspace registry: {0}")]
    RegistryIo(#[from] std::io::Error),
    #[error("could not encode daemon workspace registry: {0}")]
    RegistryFormat(#[from] serde_json::Error),
    #[error("unsupported daemon workspace registry version {0}")]
    RegistryVersion(u16),
}

/// In-process daemon domain state. Transports and process supervision wrap this
/// type; they do not own workspace/session semantics.
pub struct Daemon {
    store: SessionStore,
    workspaces: RwLock<HashMap<WorkspaceId, Workspace>>,
}

impl Daemon {
    pub fn new(state_root: impl Into<std::path::PathBuf>) -> Self {
        Self::open(state_root).expect("daemon state root must be readable")
    }

    pub fn open(state_root: impl Into<std::path::PathBuf>) -> Result<Self, DaemonError> {
        let state_root = state_root.into();
        Ok(Self {
            store: SessionStore::new(&state_root),
            workspaces: RwLock::new(load_workspaces(&state_root)?),
        })
    }

    pub fn state_root(&self) -> &Path {
        self.store.root()
    }

    pub fn register_workspace(&self, workspace: Workspace) -> Result<(), DaemonError> {
        let mut workspaces = self.workspaces.write().unwrap();
        if workspaces.contains_key(&workspace.id) {
            return Err(DaemonError::DuplicateWorkspace(workspace.id));
        }
        let inserted_id = workspace.id.clone();
        workspaces.insert(inserted_id.clone(), workspace);
        if let Err(error) = self.persist_workspaces(&workspaces) {
            workspaces.remove(&inserted_id);
            return Err(error);
        }
        Ok(())
    }

    fn persist_workspaces(
        &self,
        workspaces: &HashMap<WorkspaceId, Workspace>,
    ) -> Result<(), DaemonError> {
        let mut records: Vec<_> = workspaces.values().map(WorkspaceRecord::from).collect();
        records.sort_by(|a, b| a.id.cmp(&b.id));
        let path = self.store.root().join("workspaces.json");
        fs::create_dir_all(self.store.root())?;
        let temporary = path.with_extension("json.tmp");
        fs::write(
            &temporary,
            serde_json::to_vec_pretty(&WorkspaceRegistry {
                version: REGISTRY_VERSION,
                workspaces: records,
            })?,
        )?;
        fs::rename(temporary, path)?;
        Ok(())
    }

    pub fn workspace(&self, id: &WorkspaceId) -> Option<Workspace> {
        self.workspaces.read().unwrap().get(id).cloned()
    }

    pub fn workspaces(&self) -> Vec<Workspace> {
        let mut result: Vec<_> = self.workspaces.read().unwrap().values().cloned().collect();
        result.sort_by(|a, b| a.id.cmp(&b.id));
        result
    }

    pub fn open_session(
        &self,
        workspace: &WorkspaceId,
        session: SessionId,
    ) -> Result<EventLog, DaemonError> {
        let workspace = self
            .workspace(workspace)
            .ok_or_else(|| DaemonError::UnknownWorkspace(workspace.clone()))?;
        Ok(self.store.open_session(&workspace, session)?)
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct WorkspaceRecord {
    id: String,
    root: std::path::PathBuf,
}

const REGISTRY_VERSION: u16 = 1;

#[derive(Debug, Deserialize, Serialize)]
struct WorkspaceRegistry {
    version: u16,
    workspaces: Vec<WorkspaceRecord>,
}

impl From<&Workspace> for WorkspaceRecord {
    fn from(workspace: &Workspace) -> Self {
        Self {
            id: workspace.id.to_string(),
            root: workspace.root.clone(),
        }
    }
}

fn load_workspaces(state_root: &Path) -> Result<HashMap<WorkspaceId, Workspace>, DaemonError> {
    let path = state_root.join("workspaces.json");
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => return Err(DaemonError::RegistryIo(error)),
    };
    // The first implementation wrote a bare array. Accepting it here makes
    // the registry migration explicit instead of silently discarding state.
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let records = if value.is_array() {
        serde_json::from_value(value)?
    } else {
        let registry: WorkspaceRegistry = serde_json::from_value(value)?;
        if registry.version != REGISTRY_VERSION {
            return Err(DaemonError::RegistryVersion(registry.version));
        }
        registry.workspaces
    };
    records
        .into_iter()
        .map(|record| {
            let id = WorkspaceId::new(record.id)?;
            Ok((id.clone(), Workspace::new(id, record.root)))
        })
        .collect::<Result<HashMap<_, _>, artist_session::InvalidId>>()
        .map_err(|error| DaemonError::Store(StoreError::InvalidId(error)))
}

#[derive(Debug, Deserialize)]
struct RegisterWorkspaceParams {
    id: String,
    root: std::path::PathBuf,
}

#[derive(Debug, Deserialize)]
struct OpenSessionParams {
    workspace_id: String,
    session_id: String,
}

#[derive(Debug, Serialize)]
struct WorkspaceView {
    id: String,
    root: std::path::PathBuf,
}

/// The concrete daemon handler for the transport-neutral RPC server.
///
/// This intentionally exposes only lifecycle/domain operations. Agent turns,
/// tools, and resource semantics are layered on later without changing the
/// durable workspace/session identity model.
pub struct DaemonRpcHandler {
    daemon: Arc<Daemon>,
}

impl DaemonRpcHandler {
    pub fn new(daemon: Arc<Daemon>) -> Self {
        Self { daemon }
    }

    fn invalid_request(id: String, message: impl Into<String>) -> RpcError {
        RpcError {
            code: "invalid_request".into(),
            message: format!("{id}: {}", message.into()),
        }
    }
}

#[async_trait]
impl RpcHandler for DaemonRpcHandler {
    async fn handle(&self, request: RpcFrame) -> Result<Vec<RpcFrame>, RpcError> {
        let RpcFrame::Request { id, method, params } = request else {
            return Err(RpcError {
                code: "request_required".into(),
                message: "daemon handler accepts request frames only".into(),
            });
        };

        match method.as_str() {
            "workspace/list" => {
                let workspaces: Vec<_> = self
                    .daemon
                    .workspaces()
                    .into_iter()
                    .map(|workspace| WorkspaceView {
                        id: workspace.id.to_string(),
                        root: workspace.root,
                    })
                    .collect();
                Ok(vec![RpcFrame::response(
                    id,
                    serde_json::to_value(workspaces).unwrap(),
                )])
            }
            "workspace/register" => {
                let params: RegisterWorkspaceParams = serde_json::from_value(params)
                    .map_err(|error| Self::invalid_request(id.clone(), error.to_string()))?;
                let workspace_id = WorkspaceId::new(params.id)
                    .map_err(|error| Self::invalid_request(id.clone(), error.to_string()))?;
                self.daemon
                    .register_workspace(Workspace::new(workspace_id.clone(), params.root))
                    .map_err(|error| RpcError {
                        code: "workspace_error".into(),
                        message: error.to_string(),
                    })?;
                Ok(vec![RpcFrame::response(
                    id,
                    serde_json::json!({"workspace_id": workspace_id.to_string()}),
                )])
            }
            "session/open" => {
                let params: OpenSessionParams = serde_json::from_value(params)
                    .map_err(|error| Self::invalid_request(id.clone(), error.to_string()))?;
                let workspace_id = WorkspaceId::new(params.workspace_id)
                    .map_err(|error| Self::invalid_request(id.clone(), error.to_string()))?;
                let session_id = SessionId::new(params.session_id)
                    .map_err(|error| Self::invalid_request(id.clone(), error.to_string()))?;
                let log = self
                    .daemon
                    .open_session(&workspace_id, session_id)
                    .map_err(|error| RpcError {
                        code: "session_error".into(),
                        message: error.to_string(),
                    })?;
                Ok(vec![RpcFrame::response(
                    id,
                    serde_json::json!({"stream_id": log.stream_id()}),
                )])
            }
            _ => Err(RpcError {
                code: "method_not_found".into(),
                message: format!("unsupported daemon method {method:?}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::RpcHandler;
    use serde_json::Value;
    use tempfile::tempdir;

    #[test]
    fn daemon_owns_multiple_workspaces_and_separates_sessions() {
        let dir = tempdir().unwrap();
        let daemon = Daemon::new(dir.path());
        let a = Workspace::new(WorkspaceId::new("a").unwrap(), "/repo/a");
        let b = Workspace::new(WorkspaceId::new("b").unwrap(), "/repo/b");
        daemon.register_workspace(a).unwrap();
        daemon.register_workspace(b).unwrap();

        let log_a = daemon
            .open_session(
                &WorkspaceId::new("a").unwrap(),
                SessionId::new("s").unwrap(),
            )
            .unwrap();
        let log_b = daemon
            .open_session(
                &WorkspaceId::new("b").unwrap(),
                SessionId::new("s").unwrap(),
            )
            .unwrap();
        assert_ne!(log_a.path(), log_b.path());
        assert_eq!(daemon.workspaces().len(), 2);
    }

    #[test]
    fn workspace_registry_survives_daemon_reopen() {
        let dir = tempdir().unwrap();
        let daemon = Daemon::new(dir.path());
        daemon
            .register_workspace(Workspace::new(WorkspaceId::new("repo").unwrap(), "/repo"))
            .unwrap();
        let reopened = Daemon::open(dir.path()).unwrap();
        assert_eq!(reopened.workspaces()[0].id.to_string(), "repo");
    }

    #[test]
    fn legacy_workspace_registry_is_migrated_on_next_write() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(
            dir.path().join("workspaces.json"),
            serde_json::json!([{"id": "legacy", "root": "/legacy"}]).to_string(),
        )
        .unwrap();
        let daemon = Daemon::open(dir.path()).unwrap();
        daemon
            .register_workspace(Workspace::new(WorkspaceId::new("new").unwrap(), "/new"))
            .unwrap();
        let registry: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("workspaces.json")).unwrap())
                .unwrap();
        assert_eq!(registry["version"], REGISTRY_VERSION);
        assert_eq!(registry["workspaces"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn rpc_handler_exposes_workspace_and_session_lifecycle() {
        let dir = tempdir().unwrap();
        let daemon = Arc::new(Daemon::new(dir.path()));
        let handler = DaemonRpcHandler::new(daemon);

        handler
            .handle(RpcFrame::request(
                "1",
                "workspace/register",
                serde_json::json!({"id": "repo", "root": "/repo"}),
            ))
            .await
            .unwrap();

        let listed = handler
            .handle(RpcFrame::request("2", "workspace/list", Value::Null))
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0],
            RpcFrame::response(
                "2",
                serde_json::json!([{
                    "id": "repo",
                    "root": "/repo"
                }])
            )
        );

        let opened = handler
            .handle(RpcFrame::request(
                "3",
                "session/open",
                serde_json::json!({"workspace_id": "repo", "session_id": "s1"}),
            ))
            .await
            .unwrap();
        assert_eq!(
            opened[0],
            RpcFrame::response("3", serde_json::json!({"stream_id": "s1"}))
        );
    }
}
