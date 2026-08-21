use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;

use artist_session::{
    EventLog, SessionId, SessionStore, Snapshot, StoreError, Workspace, WorkspaceId,
};
use async_trait::async_trait;
use fs2::FileExt;
use llm_provider::{Message, ModelProvider, Role, ToolDefinition};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::broadcast;

use crate::protocol::{RpcError, RpcFrame};
use crate::rpc::{RpcEventSource, RpcHandler};

/// A cheap trait-object adapter used by hosts that select providers from a
/// component catalog at runtime.
pub struct SharedProvider(pub Arc<dyn ModelProvider>);

impl ModelProvider for SharedProvider {
    fn provider_name(&self) -> &str {
        self.0.provider_name()
    }

    fn capabilities(&self, model: &str) -> llm_provider::ModelCapabilities {
        self.0.capabilities(model)
    }

    fn register_formal_tools(
        &self,
        model: &str,
        tools: &[ToolDefinition],
    ) -> Result<(), llm_provider::ProviderError> {
        self.0.register_formal_tools(model, tools)
    }

    fn stream<'a>(
        &'a self,
        request: llm_provider::ModelRequest,
    ) -> llm_provider::ModelEventStream<'a> {
        self.0.stream(request)
    }
}

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
    #[error("invalid session metadata: {0}")]
    InvalidSessionMetadata(String),
    #[error("session {0} is already running a turn")]
    SessionBusy(SessionId),
    #[error("session {0} has been closed")]
    SessionClosed(SessionId),
    #[error("session {0} is not running a turn")]
    SessionNotRunning(SessionId),
    #[error("session runner is not configured")]
    RunnerUnavailable,
    #[error("session request {0:?} was already accepted")]
    DuplicateRequest(String),
    #[error("invalid session request: {0}")]
    InvalidRequest(String),
    #[error("unsupported session signal {0:?}")]
    UnsupportedSignal(String),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    Idle,
    Running,
    Completed,
    Failed,
    Closed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SessionMetadata {
    pub version: u16,
    pub workspace_id: String,
    pub session_id: String,
    pub identity: Option<String>,
    pub profile_id: Option<String>,
    #[serde(default)]
    pub profile_resource_id: Option<String>,
    pub provider_config_id: Option<String>,
    pub model: Option<String>,
    pub compaction_resource_id: Option<String>,
    pub composition_generation: Option<String>,
}

pub const SESSION_METADATA_VERSION: u16 = 1;

impl SessionMetadata {
    fn validate(&self) -> Result<(), DaemonError> {
        if self.version != SESSION_METADATA_VERSION {
            return Err(DaemonError::RegistryVersion(self.version));
        }
        if self.workspace_id.is_empty() || self.session_id.is_empty() {
            return Err(DaemonError::InvalidSessionMetadata(
                "workspace and session ids are required".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SessionTurnRequest {
    pub request_id: String,
    pub model: String,
    pub text: String,
}

#[async_trait]
pub trait SessionTurnRunner: Send + Sync {
    async fn run(
        &self,
        log: Arc<EventLog>,
        request: SessionTurnRequest,
        control: crate::TurnControl,
    ) -> Result<crate::TurnOutcome, crate::AgentError>;
}

/// Resolves the component-backed runtime for a durable session. The daemon
/// keeps this factory deliberately small: provider/profile/tool assembly is a
/// host concern, while the selected implementations remain behind their
/// component sockets.
#[async_trait]
pub trait SessionRuntimeFactory: Send + Sync {
    async fn resolve(
        &self,
        workspace: Workspace,
        metadata: SessionMetadata,
        log: Arc<EventLog>,
    ) -> Result<Arc<dyn SessionTurnRunner>, crate::AgentError>;
}

struct DeferredSessionRunner {
    factory: Arc<dyn SessionRuntimeFactory>,
    workspace: Workspace,
    metadata: SessionMetadata,
}

#[async_trait]
impl SessionTurnRunner for DeferredSessionRunner {
    async fn run(
        &self,
        log: Arc<EventLog>,
        request: SessionTurnRequest,
        control: crate::TurnControl,
    ) -> Result<crate::TurnOutcome, crate::AgentError> {
        let runner = self
            .factory
            .resolve(self.workspace.clone(), self.metadata.clone(), log.clone())
            .await?;
        runner.run(log, request, control).await
    }
}

/// Adapter that lets a daemon host the normal agent engine without making
/// the daemon own provider or tool policy.
pub struct EngineTurnRunner<P, T> {
    provider: Arc<P>,
    tools: Arc<T>,
    definitions: Vec<ToolDefinition>,
    compaction: Option<Arc<dyn artist_component::CompactionComponent>>,
    compaction_context_limit: Option<u64>,
    context: Option<Snapshot>,
    tool_surface: Option<Arc<crate::ToolSurface>>,
    composition_updater: Option<Arc<dyn artist_component::CompositionUpdater>>,
    composition_input: Option<artist_component::CompositionInput>,
    provider_resolver: Option<Arc<dyn crate::ProviderResolver>>,
    handoff_handler: Option<Arc<dyn crate::HandoffHandler>>,
    fork_executor: Option<Arc<dyn crate::ForkExecutor>>,
    default_model: Option<String>,
}

impl<P, T> EngineTurnRunner<P, T> {
    pub fn new(provider: Arc<P>, tools: Arc<T>) -> Self {
        Self {
            provider,
            tools,
            definitions: Vec::new(),
            compaction: None,
            compaction_context_limit: None,
            context: None,
            tool_surface: None,
            composition_updater: None,
            composition_input: None,
            provider_resolver: None,
            handoff_handler: None,
            fork_executor: None,
            default_model: None,
        }
    }

    pub fn with_tools(mut self, definitions: Vec<ToolDefinition>) -> Self {
        self.definitions = definitions;
        self
    }

    pub fn with_compaction(
        mut self,
        component: Arc<dyn artist_component::CompactionComponent>,
        context_limit: Option<u64>,
    ) -> Self {
        self.compaction = Some(component);
        self.compaction_context_limit = context_limit;
        self
    }

    pub fn with_context(mut self, context: Snapshot) -> Self {
        self.context = Some(context);
        self
    }

    pub fn with_tool_surface(mut self, surface: Arc<crate::ToolSurface>) -> Self {
        self.tool_surface = Some(surface);
        self
    }

    pub fn with_composition(
        mut self,
        updater: Arc<dyn artist_component::CompositionUpdater>,
        input: artist_component::CompositionInput,
    ) -> Self {
        self.composition_updater = Some(updater);
        self.composition_input = Some(input);
        self
    }

    pub fn with_provider_resolver(mut self, resolver: Arc<dyn crate::ProviderResolver>) -> Self {
        self.provider_resolver = Some(resolver);
        self
    }

    pub fn with_handoff_handler(mut self, handler: Arc<dyn crate::HandoffHandler>) -> Self {
        self.handoff_handler = Some(handler);
        self
    }

    pub fn with_fork_executor(mut self, executor: Arc<dyn crate::ForkExecutor>) -> Self {
        self.fork_executor = Some(executor);
        self
    }

    pub fn with_default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = Some(model.into());
        self
    }
}

#[async_trait]
impl<P, T> SessionTurnRunner for EngineTurnRunner<P, T>
where
    P: ModelProvider + 'static,
    T: crate::ToolInvoker + 'static,
{
    async fn run(
        &self,
        log: Arc<EventLog>,
        request: SessionTurnRequest,
        control: crate::TurnControl,
    ) -> Result<crate::TurnOutcome, crate::AgentError> {
        let model = if request.model.trim().is_empty() {
            self.default_model.clone().ok_or_else(|| {
                crate::AgentError::ProviderSelection("no model is selected".into())
            })?
        } else {
            request.model
        };
        let mut engine =
            crate::AgentEngine::new(Arc::clone(&self.provider), Arc::clone(&self.tools), log);
        if let Some(compaction) = &self.compaction {
            engine = engine.with_compaction(
                Arc::clone(compaction),
                self.compaction_context_limit,
                serde_json::json!({}),
            );
        }
        if let Some(resolver) = &self.provider_resolver {
            engine = engine.with_provider_resolver(Arc::clone(resolver));
        }
        if let Some(handler) = &self.handoff_handler {
            engine = engine.with_handoff_handler(Arc::clone(handler));
        }
        if let Some(executor) = &self.fork_executor {
            engine = engine.with_fork_executor(Arc::clone(executor));
        }
        engine
            .run_turn(crate::TurnRequest {
                model,
                user: Message::text(Role::User, request.text),
                tools: self.definitions.clone(),
                tool_surface: self.tool_surface.clone(),
                composition_updater: self.composition_updater.clone(),
                composition_input: self.composition_input.clone(),
                context: self.context.clone(),
                initial_messages: None,
                control,
            })
            .await
    }
}

struct LiveState {
    phase: SessionPhase,
    active_request: Option<String>,
    control: Option<crate::TurnControl>,
    published_sequence: u64,
    pending_events: BTreeMap<u64, artist_session::LogRecord>,
    metadata: SessionMetadata,
    accepted_requests: BTreeSet<String>,
}

pub struct LiveSession {
    workspace_id: WorkspaceId,
    session_id: SessionId,
    log: Arc<EventLog>,
    state: Mutex<LiveState>,
    events: broadcast::Sender<RpcFrame>,
    global_events: broadcast::Sender<RpcFrame>,
    runner: RwLock<Option<Arc<dyn SessionTurnRunner>>>,
}

impl LiveSession {
    fn new(
        workspace_id: WorkspaceId,
        session_id: SessionId,
        log: Arc<EventLog>,
        metadata: SessionMetadata,
        global_events: broadcast::Sender<RpcFrame>,
    ) -> Self {
        let records = log.records().unwrap_or_default();
        let accepted_requests = records
            .iter()
            .filter(|record| record.event_type == "session.turn_started")
            .filter_map(|record| {
                record
                    .payload
                    .get("request_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect();
        let phase = if log.is_closed() {
            SessionPhase::Closed
        } else {
            infer_phase(&log).unwrap_or(SessionPhase::Idle)
        };
        let published_sequence = log.next_sequence().unwrap_or(1).saturating_sub(1);
        let (events, _) = broadcast::channel(256);
        Self {
            workspace_id,
            session_id,
            log,
            state: Mutex::new(LiveState {
                phase,
                active_request: None,
                control: None,
                published_sequence,
                pending_events: BTreeMap::new(),
                metadata,
                accepted_requests,
            }),
            events,
            global_events,
            runner: RwLock::new(None),
        }
    }

    pub fn log(&self) -> Arc<EventLog> {
        Arc::clone(&self.log)
    }

    pub fn phase(&self) -> SessionPhase {
        self.state.lock().unwrap().phase.clone()
    }

    pub fn metadata(&self) -> SessionMetadata {
        self.state.lock().unwrap().metadata.clone()
    }

    pub fn workspace_id(&self) -> &WorkspaceId {
        &self.workspace_id
    }

    /// Event streams are globally scoped inside a daemon. The durable log
    /// remains keyed by the session ID within its workspace, while the
    /// transport identity also includes the workspace so two workspaces may
    /// legitimately contain a session with the same local ID without leaking
    /// live events across subscriptions.
    pub fn stream_id(&self) -> String {
        format!("{}/{}", self.workspace_id, self.session_id)
    }

    pub fn set_runner(&self, runner: Arc<dyn SessionTurnRunner>) {
        *self.runner.write().unwrap() = Some(runner);
    }

    /// Bridge direct agent-log appends into the live session event stream.
    /// AgentEngine writes authoritative records directly to EventLog; this
    /// relay keeps that storage boundary intact while making those records
    /// visible to already-connected subscribers.
    pub fn start_event_relay(self: &Arc<Self>) {
        let mut receiver = self.log.subscribe();
        let session = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(record) => {
                        let Some(session) = session.upgrade() else {
                            break;
                        };
                        session.publish_record(record);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // The durable replay path is the recovery mechanism;
                        // the next reconnect can request the missing range.
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    fn journal(&self, event_type: &str, payload: Value) -> Result<(), DaemonError> {
        let record = self
            .log
            .append(event_type, payload)
            .map_err(|error| DaemonError::Store(StoreError::Log(error)))?;
        self.publish_record(record);
        Ok(())
    }

    fn publish_record(&self, record: artist_session::LogRecord) {
        let mut state = self.state.lock().unwrap();
        if record.sequence <= state.published_sequence
            || state.pending_events.contains_key(&record.sequence)
        {
            return;
        }
        state.pending_events.insert(record.sequence, record);
        loop {
            let next_sequence = state.published_sequence.saturating_add(1);
            let Some(record) = state.pending_events.remove(&next_sequence) else {
                break;
            };
            let frame = RpcFrame::Event {
                stream_id: self.stream_id(),
                sequence: record.sequence,
                event_type: record.event_type,
                payload: record.payload,
            };
            let _ = self.events.send(frame.clone());
            let _ = self.global_events.send(frame);
            state.published_sequence = record.sequence;
        }
    }

    pub fn replay_from(&self, after: u64) -> Result<Vec<RpcFrame>, DaemonError> {
        let records = self
            .log
            .records()
            .map_err(|error| DaemonError::Store(StoreError::Log(error)))?;
        Ok(records
            .into_iter()
            .filter(|record| record.sequence > after)
            .map(|record| RpcFrame::Event {
                stream_id: self.stream_id(),
                sequence: record.sequence,
                event_type: record.event_type,
                payload: record.payload,
            })
            .collect())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RpcFrame> {
        self.events.subscribe()
    }

    pub fn start_turn(self: &Arc<Self>, request: SessionTurnRequest) -> Result<(), DaemonError> {
        if request.request_id.trim().is_empty() {
            return Err(DaemonError::InvalidRequest(
                "request_id must not be empty".into(),
            ));
        }
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            DaemonError::InvalidRequest("session turns require an active async runtime".into())
        })?;
        let control = crate::TurnControl::new();
        {
            let mut state = self.state.lock().unwrap();
            if state.phase == SessionPhase::Closed {
                return Err(DaemonError::SessionClosed(self.session_id.clone()));
            }
            if state.accepted_requests.contains(&request.request_id) {
                return Err(DaemonError::DuplicateRequest(request.request_id));
            }
            if state.phase == SessionPhase::Running {
                return Err(DaemonError::SessionBusy(self.session_id.clone()));
            }
            state.phase = SessionPhase::Running;
            state.active_request = Some(request.request_id.clone());
            state.control = Some(control.clone());
            state.accepted_requests.insert(request.request_id.clone());
        }
        let runner = self.runner.read().unwrap().clone();
        let Some(runner) = runner else {
            let mut state = self.state.lock().unwrap();
            state.phase = SessionPhase::Failed;
            state.active_request = None;
            state.control = None;
            state.accepted_requests.remove(&request.request_id);
            return Err(DaemonError::RunnerUnavailable);
        };
        if let Err(error) = self.journal(
            "session.turn_started",
            serde_json::to_value(&request).expect("session request is serializable"),
        ) {
            let mut state = self.state.lock().unwrap();
            state.phase = SessionPhase::Failed;
            state.active_request = None;
            state.control = None;
            state.accepted_requests.remove(&request.request_id);
            return Err(error);
        }
        let session = Arc::clone(self);
        runtime.spawn(async move {
            let result = runner
                .run(Arc::clone(&session.log), request, control.clone())
                .await;
            let (phase, payload) = match result {
                Ok(outcome) => (
                    SessionPhase::Completed,
                    serde_json::json!({"usage": outcome.usage, "tool_rounds": outcome.tool_rounds}),
                ),
                Err(error) => (
                    SessionPhase::Failed,
                    serde_json::json!({"error": error.to_string()}),
                ),
            };
            let event_type = if phase == SessionPhase::Completed {
                "session.turn_completed"
            } else {
                "session.turn_failed"
            };
            let journaled = session.journal(event_type, payload).is_ok();
            if let Ok(mut state) = session.state.lock() {
                state.phase = if journaled {
                    phase
                } else {
                    SessionPhase::Failed
                };
                state.active_request = None;
                state.control = None;
            }
        });
        Ok(())
    }

    pub fn write_stdin(&self, message: Message) -> Result<(), DaemonError> {
        let state = self.state.lock().unwrap();
        let Some(control) = state.control.clone() else {
            return Err(DaemonError::SessionNotRunning(self.session_id.clone()));
        };
        drop(state);
        // The durable record is the acceptance boundary. If the daemon dies
        // between this write and the in-memory wake-up, the agent restores the
        // pending stdin record before making another final-response decision.
        self.journal(
            "session.stdin",
            serde_json::json!({"message": message.clone()}),
        )?;
        control.steer(message);
        Ok(())
    }

    pub fn signal(&self, signal: &str) -> Result<(), DaemonError> {
        let state = self.state.lock().unwrap();
        let Some(control) = &state.control else {
            return Ok(());
        };
        match signal {
            "cancel" | "interrupt" | "terminate" => control.cancel(),
            _ => return Err(DaemonError::UnsupportedSignal(signal.into())),
        }
        drop(state);
        self.journal("session.signal", serde_json::json!({"signal": signal}))
    }

    pub fn close(&self) -> Result<(), DaemonError> {
        let phase = self.phase();
        if phase == SessionPhase::Closed {
            return Ok(());
        }
        if phase == SessionPhase::Running {
            return Err(DaemonError::SessionBusy(self.session_id.clone()));
        }
        self.journal("session.closed", serde_json::json!({}))?;
        self.log
            .close()
            .map_err(|error| DaemonError::Store(StoreError::Log(error)))?;
        self.state.lock().unwrap().phase = SessionPhase::Closed;
        Ok(())
    }
}

/// In-process daemon domain state. Transports and process supervision wrap this
/// type; they do not own workspace/session semantics.
pub struct Daemon {
    store: SessionStore,
    workspaces: RwLock<HashMap<WorkspaceId, Workspace>>,
    sessions: RwLock<HashMap<(WorkspaceId, SessionId), Arc<LiveSession>>>,
    session_open_lock: Mutex<()>,
    runner: RwLock<Option<Arc<dyn SessionTurnRunner>>>,
    runtime_factory: RwLock<Option<Arc<dyn SessionRuntimeFactory>>>,
    global_events: broadcast::Sender<RpcFrame>,
}

impl Daemon {
    pub fn new(state_root: impl Into<std::path::PathBuf>) -> Self {
        Self::open(state_root).expect("daemon state root must be readable")
    }

    pub fn open(state_root: impl Into<std::path::PathBuf>) -> Result<Self, DaemonError> {
        let state_root = state_root.into();
        let (global_events, _) = broadcast::channel(1024);
        Ok(Self {
            store: SessionStore::new(&state_root),
            workspaces: RwLock::new(load_workspaces(&state_root)?),
            sessions: RwLock::new(HashMap::new()),
            session_open_lock: Mutex::new(()),
            runner: RwLock::new(None),
            runtime_factory: RwLock::new(None),
            global_events,
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
        match self.persist_workspaces(&workspaces) {
            Ok(merged) => *workspaces = merged,
            Err(error) => {
                workspaces.remove(&inserted_id);
                return Err(error);
            }
        }
        Ok(())
    }

    fn persist_workspaces(
        &self,
        workspaces: &HashMap<WorkspaceId, Workspace>,
    ) -> Result<HashMap<WorkspaceId, Workspace>, DaemonError> {
        fs::create_dir_all(self.store.root())?;
        let lock_path = self.store.root().join("workspaces.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        lock.lock_exclusive()?;

        // A daemon may have opened the state root before another daemon
        // registered a workspace. Merge the current durable registry while
        // holding the lock so serialized writers cannot lose each other's
        // additions merely because their in-memory snapshots are stale.
        let mut merged = load_workspaces(self.store.root())?;
        for (id, workspace) in workspaces {
            if let Some(existing) = merged.get(id) {
                if existing != workspace {
                    return Err(DaemonError::DuplicateWorkspace(id.clone()));
                }
            } else {
                merged.insert(id.clone(), workspace.clone());
            }
        }
        let mut records: Vec<_> = merged.values().map(WorkspaceRecord::from).collect();
        records.sort_by(|a, b| a.id.cmp(&b.id));
        let path = self.store.root().join("workspaces.json");
        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(&WorkspaceRegistry {
            version: REGISTRY_VERSION,
            workspaces: records,
        })?;
        let mut file = File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        replace_registry_file(&temporary, &path)?;
        if let Ok(directory) = File::open(self.store.root()) {
            let _ = directory.sync_all();
        }
        lock.unlock()?;
        Ok(merged)
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

    pub fn set_turn_runner(&self, runner: Arc<dyn SessionTurnRunner>) {
        *self.runner.write().unwrap() = Some(Arc::clone(&runner));
        for session in self.sessions.read().unwrap().values() {
            session.set_runner(Arc::clone(&runner));
        }
    }

    /// Install the component-backed runtime resolver used for sessions that
    /// are opened before a concrete turn is requested. Existing live sessions
    /// are rebound as well, which is what makes daemon restart/reconnect able
    /// to execute another turn instead of merely replaying history.
    pub fn set_runtime_factory(&self, factory: Arc<dyn SessionRuntimeFactory>) {
        *self.runtime_factory.write().unwrap() = Some(Arc::clone(&factory));
        for ((workspace_id, _session_id), session) in self.sessions.read().unwrap().iter() {
            if let Some(workspace) = self.workspace(workspace_id) {
                session.set_runner(Arc::new(DeferredSessionRunner {
                    factory: Arc::clone(&factory),
                    workspace,
                    metadata: session.metadata(),
                }));
            }
        }
    }

    pub fn open_live_session(
        &self,
        workspace: &WorkspaceId,
        session: SessionId,
        metadata: SessionMetadata,
    ) -> Result<Arc<LiveSession>, DaemonError> {
        // Opening is a check-then-create operation over one durable log. The
        // per-daemon lock prevents two reconnecting clients from constructing
        // separate live runtimes for the same workspace/session key.
        let _open_guard = self.session_open_lock.lock().unwrap();
        let workspace_record = self
            .workspace(workspace)
            .ok_or_else(|| DaemonError::UnknownWorkspace(workspace.clone()))?;
        let key = (workspace.clone(), session.clone());
        if let Some(existing) = self.sessions.read().unwrap().get(&key).cloned() {
            return Ok(existing);
        }
        let log = Arc::new(
            self.store
                .open_session(&workspace_record, session.clone())?,
        );
        let records = log
            .records()
            .map_err(|error| DaemonError::Store(StoreError::Log(error)))?;
        let stored_metadata = records
            .iter()
            .rev()
            .find(|record| record.event_type == "session.metadata")
            .map(|record| {
                serde_json::from_value::<SessionMetadata>(record.payload.clone())
                    .map_err(DaemonError::RegistryFormat)
            })
            .transpose()?;
        let has_stored_metadata = stored_metadata.is_some();
        let metadata = stored_metadata.unwrap_or(metadata);
        metadata.validate()?;
        if metadata.workspace_id != workspace.to_string()
            || metadata.session_id != session.to_string()
        {
            return Err(DaemonError::InvalidSessionMetadata(
                "stored session metadata addresses a different session".into(),
            ));
        }
        let live = Arc::new(LiveSession::new(
            workspace.clone(),
            session.clone(),
            Arc::clone(&log),
            metadata,
            self.global_events.clone(),
        ));
        if !has_stored_metadata && !log.is_closed() {
            live.journal(
                "session.metadata",
                serde_json::to_value(live.metadata()).map_err(DaemonError::RegistryFormat)?,
            )?;
        }
        if !log.is_closed() && has_unfinished_turn(&log)? {
            live.journal(
                "session.turn_interrupted",
                serde_json::json!({"reason": "daemon_reopened_before_turn_completion"}),
            )?;
        }
        if let Some(runner) = self.runner.read().unwrap().clone() {
            live.set_runner(runner);
        } else if let Some(factory) = self.runtime_factory.read().unwrap().clone() {
            live.set_runner(Arc::new(DeferredSessionRunner {
                factory,
                workspace: workspace_record,
                metadata: live.metadata(),
            }));
        }
        self.sessions
            .write()
            .unwrap()
            .insert(key, Arc::clone(&live));
        if tokio::runtime::Handle::try_current().is_ok() {
            live.start_event_relay();
        }
        Ok(live)
    }

    pub fn live_session(
        &self,
        workspace: &WorkspaceId,
        session: &SessionId,
    ) -> Option<Arc<LiveSession>> {
        self.sessions
            .read()
            .unwrap()
            .get(&(workspace.clone(), session.clone()))
            .cloned()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<RpcFrame> {
        self.global_events.subscribe()
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

#[cfg(not(windows))]
fn replace_registry_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_registry_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    // Windows does not replace an existing file with rename. Keep a recoverable
    // backup while swapping the fully-synced temporary registry into place.
    let backup = destination.with_extension("json.bak");
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    if destination.exists() {
        fs::rename(destination, &backup)?;
    }
    match fs::rename(temporary, destination) {
        Ok(()) => {
            let _ = fs::remove_file(&backup);
            Ok(())
        }
        Err(error) => {
            if backup.exists() && !destination.exists() {
                let _ = fs::rename(&backup, destination);
            }
            Err(error)
        }
    }
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

fn infer_phase(log: &EventLog) -> Option<SessionPhase> {
    let records = log.records().ok()?;
    let mut phase = None;
    let mut active = false;
    for record in records {
        match record.event_type.as_str() {
            "session.closed" => {
                phase = Some(SessionPhase::Closed);
                active = false;
            }
            "session.turn_started" => {
                phase = Some(SessionPhase::Running);
                active = true;
            }
            "session.turn_completed" => {
                phase = Some(SessionPhase::Completed);
                active = false;
            }
            "session.turn_failed" | "session.turn_interrupted" | "agent.turn_aborted" => {
                phase = Some(SessionPhase::Failed);
                active = false;
            }
            _ => {}
        }
    }
    if active {
        Some(SessionPhase::Failed)
    } else {
        phase
    }
}

fn has_unfinished_turn(log: &EventLog) -> Result<bool, DaemonError> {
    let records = log
        .records()
        .map_err(|error| DaemonError::Store(StoreError::Log(error)))?;
    let mut active = false;
    for record in records {
        match record.event_type.as_str() {
            "session.turn_started" => active = true,
            "session.turn_completed"
            | "session.turn_failed"
            | "session.turn_interrupted"
            | "agent.turn_aborted" => active = false,
            _ => {}
        }
    }
    Ok(active)
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
    #[serde(default)]
    identity: Option<String>,
    #[serde(default)]
    profile_id: Option<String>,
    #[serde(default)]
    profile_resource_id: Option<String>,
    #[serde(default)]
    provider_config_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    compaction_resource_id: Option<String>,
    #[serde(default)]
    composition_generation: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionTurnParams {
    workspace_id: String,
    session_id: String,
    request_id: String,
    #[serde(default)]
    model: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct SessionStreamParams {
    workspace_id: String,
    session_id: String,
    #[serde(default)]
    after: u64,
}

#[derive(Debug, Deserialize)]
struct SessionWriteParams {
    workspace_id: String,
    session_id: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct SessionSignalParams {
    workspace_id: String,
    session_id: String,
    signal: String,
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

    fn live(&self, id: &str, workspace: &str, session: &str) -> Result<Arc<LiveSession>, RpcError> {
        let workspace = WorkspaceId::new(workspace.to_owned())
            .map_err(|error| Self::invalid_request(id.to_owned(), error.to_string()))?;
        let session = SessionId::new(session.to_owned())
            .map_err(|error| Self::invalid_request(id.to_owned(), error.to_string()))?;
        self.daemon
            .live_session(&workspace, &session)
            .ok_or_else(|| RpcError {
                code: "session_not_open".into(),
                message: format!("session {workspace}/{session} is not open"),
            })
    }
}

impl RpcEventSource for DaemonRpcHandler {
    fn subscribe_events(&self) -> broadcast::Receiver<RpcFrame> {
        self.daemon.subscribe_events()
    }

    fn event_stream_ids(&self, request: &RpcFrame) -> Vec<String> {
        let RpcFrame::Request { method, params, .. } = request else {
            return Vec::new();
        };
        if !method.starts_with("session/") && !method.starts_with("resource/") {
            return Vec::new();
        }
        params
            .get("workspace_id")
            .and_then(Value::as_str)
            .zip(params.get("session_id").and_then(Value::as_str))
            .filter(|(workspace, session)| !workspace.is_empty() && !session.is_empty())
            .map(|(workspace, session)| vec![format!("{workspace}/{session}")])
            .unwrap_or_default()
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
                let metadata = SessionMetadata {
                    version: SESSION_METADATA_VERSION,
                    workspace_id: workspace_id.to_string(),
                    session_id: session_id.to_string(),
                    identity: params.identity,
                    profile_id: params.profile_id,
                    profile_resource_id: params.profile_resource_id,
                    provider_config_id: params.provider_config_id,
                    model: params.model,
                    compaction_resource_id: params.compaction_resource_id,
                    composition_generation: params.composition_generation,
                };
                let live = self
                    .daemon
                    .open_live_session(&workspace_id, session_id, metadata.clone())
                    .map_err(|error| RpcError {
                        code: "session_error".into(),
                        message: error.to_string(),
                    })?;
                Ok(vec![RpcFrame::response(
                    id,
                    serde_json::json!({"stream_id": live.stream_id()}),
                )])
            }
            "session/state" => {
                let params: SessionStreamParams = serde_json::from_value(params)
                    .map_err(|error| Self::invalid_request(id.clone(), error.to_string()))?;
                let live = self.live(&id, &params.workspace_id, &params.session_id)?;
                Ok(vec![RpcFrame::response(
                    id,
                    serde_json::json!({
                        "stream_id": live.stream_id(),
                        "phase": live.phase(),
                        "metadata": live.metadata(),
                        "last_sequence": live.log().next_sequence().unwrap_or(1).saturating_sub(1),
                    }),
                )])
            }
            "session/start" | "session/turn" => {
                let params: SessionTurnParams = serde_json::from_value(params)
                    .map_err(|error| Self::invalid_request(id.clone(), error.to_string()))?;
                let live = self.live(&id, &params.workspace_id, &params.session_id)?;
                live.start_turn(SessionTurnRequest {
                    request_id: params.request_id.clone(),
                    model: params.model,
                    text: params.text,
                })
                .map_err(|error| RpcError {
                    code: "session_error".into(),
                    message: error.to_string(),
                })?;
                Ok(vec![RpcFrame::response(
                    id,
                    serde_json::json!({"accepted": true, "request_id": params.request_id}),
                )])
            }
            "session/subscribe" => {
                let params: SessionStreamParams = serde_json::from_value(params)
                    .map_err(|error| Self::invalid_request(id.clone(), error.to_string()))?;
                let live = self.live(&id, &params.workspace_id, &params.session_id)?;
                let frames = live.replay_from(params.after).map_err(|error| RpcError {
                    code: "session_error".into(),
                    message: error.to_string(),
                })?;
                Ok(vec![RpcFrame::response(
                    id,
                    serde_json::to_value(frames).map_err(|error| RpcError {
                        code: "session_error".into(),
                        message: error.to_string(),
                    })?,
                )])
            }
            "session/write" | "resource/write" => {
                let params: SessionWriteParams = serde_json::from_value(params)
                    .map_err(|error| Self::invalid_request(id.clone(), error.to_string()))?;
                let live = self.live(&id, &params.workspace_id, &params.session_id)?;
                live.write_stdin(Message::text(Role::User, params.text))
                    .map_err(|error| RpcError {
                        code: "session_error".into(),
                        message: error.to_string(),
                    })?;
                Ok(vec![RpcFrame::response(
                    id,
                    serde_json::json!({"accepted": true}),
                )])
            }
            "session/signal" | "resource/signal" => {
                let params: SessionSignalParams = serde_json::from_value(params)
                    .map_err(|error| Self::invalid_request(id.clone(), error.to_string()))?;
                let live = self.live(&id, &params.workspace_id, &params.session_id)?;
                live.signal(&params.signal).map_err(|error| RpcError {
                    code: "session_error".into(),
                    message: error.to_string(),
                })?;
                Ok(vec![RpcFrame::response(
                    id,
                    serde_json::json!({"accepted": true}),
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tokio::time::{Duration, sleep, timeout};

    struct RecordingRunner {
        calls: Arc<AtomicUsize>,
        delay: Duration,
    }

    #[async_trait]
    impl SessionTurnRunner for RecordingRunner {
        async fn run(
            &self,
            log: Arc<EventLog>,
            request: SessionTurnRequest,
            control: crate::TurnControl,
        ) -> Result<crate::TurnOutcome, crate::AgentError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            sleep(self.delay).await;
            if control.is_cancelled() {
                return Err(crate::AgentError::Cancelled);
            }
            log.append(
                "agent.text_delta",
                serde_json::json!({"text": request.text}),
            )?;
            Ok(crate::TurnOutcome::default())
        }
    }

    fn metadata(workspace: &str, session: &str) -> SessionMetadata {
        SessionMetadata {
            version: SESSION_METADATA_VERSION,
            workspace_id: workspace.into(),
            session_id: session.into(),
            identity: Some("agent-1".into()),
            profile_id: Some("default".into()),
            profile_resource_id: Some("profile://filesystem".into()),
            provider_config_id: Some("account-a".into()),
            model: Some("model-a".into()),
            compaction_resource_id: Some("compaction://native".into()),
            composition_generation: Some("generation-1".into()),
        }
    }

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
    fn concurrent_daemon_snapshots_merge_workspace_registrations() {
        let dir = tempdir().unwrap();
        let first = Daemon::new(dir.path());
        let second = Daemon::open(dir.path()).unwrap();
        first
            .register_workspace(Workspace::new(WorkspaceId::new("first").unwrap(), "/first"))
            .unwrap();
        second
            .register_workspace(Workspace::new(
                WorkspaceId::new("second").unwrap(),
                "/second",
            ))
            .unwrap();

        let reopened = Daemon::open(dir.path()).unwrap();
        assert_eq!(
            reopened
                .workspaces()
                .into_iter()
                .map(|workspace| workspace.id.to_string())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
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
            RpcFrame::response("3", serde_json::json!({"stream_id": "repo/s1"}))
        );
    }

    #[tokio::test]
    async fn live_events_replay_and_request_ids_survive_reconnect() {
        let dir = tempdir().unwrap();
        let daemon = Daemon::new(dir.path());
        let workspace = WorkspaceId::new("repo").unwrap();
        daemon
            .register_workspace(Workspace::new(workspace.clone(), "/repo"))
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        daemon.set_turn_runner(Arc::new(RecordingRunner {
            calls: Arc::clone(&calls),
            delay: Duration::from_millis(5),
        }));
        let live = daemon
            .open_live_session(
                &workspace,
                SessionId::new("s1").unwrap(),
                metadata("repo", "s1"),
            )
            .unwrap();
        let mut events = live.subscribe();
        live.start_turn(SessionTurnRequest {
            request_id: "request-1".into(),
            model: "model-a".into(),
            text: "hello".into(),
        })
        .unwrap();
        let mut observed = Vec::new();
        for _ in 0..3 {
            let event = timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap();
            if let RpcFrame::Event { sequence, .. } = event {
                observed.push(sequence);
            }
        }
        assert!(observed.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            live.start_turn(SessionTurnRequest {
                request_id: "request-1".into(),
                model: "model-a".into(),
                text: "duplicate".into(),
            }),
            Err(DaemonError::DuplicateRequest(_))
        ));

        let reopened_daemon = Daemon::open(dir.path()).unwrap();
        let reopened = reopened_daemon
            .open_live_session(
                &workspace,
                SessionId::new("s1").unwrap(),
                metadata("repo", "s1"),
            )
            .unwrap();
        assert_eq!(reopened.metadata(), metadata("repo", "s1"));
        assert!(reopened.replay_from(0).unwrap().len() >= 3);
        assert!(matches!(
            reopened.start_turn(SessionTurnRequest {
                request_id: "request-1".into(),
                model: "model-a".into(),
                text: "duplicate after restart".into(),
            }),
            Err(DaemonError::DuplicateRequest(_))
        ));
    }

    #[tokio::test]
    async fn independent_sessions_run_concurrently_and_can_be_reconnected() {
        let dir = tempdir().unwrap();
        let daemon = Daemon::new(dir.path());
        let workspace = WorkspaceId::new("repo").unwrap();
        daemon
            .register_workspace(Workspace::new(workspace.clone(), "/repo"))
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        daemon.set_turn_runner(Arc::new(RecordingRunner {
            calls: Arc::clone(&calls),
            delay: Duration::from_millis(40),
        }));
        let first = daemon
            .open_live_session(
                &workspace,
                SessionId::new("a").unwrap(),
                metadata("repo", "a"),
            )
            .unwrap();
        let second = daemon
            .open_live_session(
                &workspace,
                SessionId::new("b").unwrap(),
                metadata("repo", "b"),
            )
            .unwrap();
        first
            .start_turn(SessionTurnRequest {
                request_id: "a-1".into(),
                model: "model-a".into(),
                text: "a".into(),
            })
            .unwrap();
        second
            .start_turn(SessionTurnRequest {
                request_id: "b-1".into(),
                model: "model-a".into(),
                text: "b".into(),
            })
            .unwrap();
        timeout(Duration::from_secs(1), async {
            loop {
                if first.phase() == SessionPhase::Completed
                    && second.phase() == SessionPhase::Completed
                {
                    break;
                }
                sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
