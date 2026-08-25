//! Host-managed session registry and service. Scheduling lives here; transcript
//! validation and persistence remain in artist-kernel and artist-store.
use artist_core::{
    ContentPart, InitialContext, InterruptionCause, RunId, RunOutcome, SessionAttachment,
    SessionId, SessionLineage, SessionMetadata, SessionRecoveryPolicy, Source, StreamEvent,
    TranscriptEntryKind,
};
use artist_kernel::{
    CreateSession, ExecutionExtensions, ProfileSource, SessionDependencies, SessionError,
    SessionHandle, StreamingModel, no_extensions,
};
use artist_store::{SessionStore, StoreError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use thiserror::Error;
use tokio::sync::{Mutex, broadcast};

pub type Attachment = SessionAttachment;
pub type RecoveryPolicy = SessionRecoveryPolicy;
#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("metadata store failed: {0}")]
    Failed(String),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionReservation {
    pub session_id: SessionId,
    pub metadata: SessionMetadata,
}

/// Durable implementations atomically reserve request IDs and metadata.
#[async_trait]
pub trait SessionMetadataStore: Send + Sync + 'static {
    async fn reserve(
        &self,
        session_id: &SessionId,
        request_id: &str,
        metadata: SessionMetadata,
    ) -> Result<SessionReservation, MetadataError>;
    async fn release(&self, session_id: &SessionId, request_id: &str) -> Result<(), MetadataError>;
    async fn load(&self, id: &SessionId) -> Result<SessionMetadata, MetadataError>;
    async fn list(&self) -> Result<Vec<SessionMetadata>, MetadataError>;
}
type MetadataTables = (
    HashMap<SessionId, SessionMetadata>,
    HashMap<String, SessionId>,
);

#[derive(Clone, Default)]
pub struct MemoryMetadataStore(Arc<Mutex<MetadataTables>>);
#[async_trait]
impl SessionMetadataStore for MemoryMetadataStore {
    async fn reserve(
        &self,
        session_id: &SessionId,
        request_id: &str,
        m: SessionMetadata,
    ) -> Result<SessionReservation, MetadataError> {
        let mut guard = self.0.lock().await;
        if let Some(existing_id) = guard.1.get(request_id).cloned()
            && let Some(existing) = guard.0.get(&existing_id).cloned()
        {
            return Ok(SessionReservation {
                session_id: existing_id,
                metadata: existing,
            });
        }
        if guard.0.contains_key(session_id) {
            return Err(MetadataError::Failed(format!(
                "session already exists: {session_id}"
            )));
        }
        guard.1.insert(request_id.to_owned(), session_id.clone());
        guard.0.insert(session_id.clone(), m.clone());
        Ok(SessionReservation {
            session_id: session_id.clone(),
            metadata: m,
        })
    }
    async fn release(&self, session_id: &SessionId, request_id: &str) -> Result<(), MetadataError> {
        let mut guard = self.0.lock().await;
        if guard.1.get(request_id) == Some(session_id) {
            guard.1.remove(request_id);
            guard.0.remove(session_id);
        }
        Ok(())
    }
    async fn load(&self, id: &SessionId) -> Result<SessionMetadata, MetadataError> {
        self.0
            .lock()
            .await
            .0
            .get(id)
            .cloned()
            .ok_or_else(|| MetadataError::Failed(format!("metadata not found: {id}")))
    }
    async fn list(&self) -> Result<Vec<SessionMetadata>, MetadataError> {
        Ok(self.0.lock().await.0.values().cloned().collect())
    }
}
#[derive(Clone)]
pub struct FileMetadataStore {
    root: Arc<PathBuf>,
}

#[derive(Default, Serialize, Deserialize)]
struct MetadataFile {
    sessions: HashMap<SessionId, SessionMetadata>,
    requests: HashMap<String, SessionId>,
}

impl FileMetadataStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, MetadataError> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|error| MetadataError::Failed(error.to_string()))?;
        Ok(Self {
            root: Arc::new(root),
        })
    }

    fn with_locked<T>(
        root: &Path,
        mutate: impl FnOnce(&mut MetadataFile) -> Result<(T, bool), MetadataError>,
    ) -> Result<T, MetadataError> {
        let lock_path = root.join("metadata.lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|error| MetadataError::Failed(error.to_string()))?;
        fs2::FileExt::lock_exclusive(&lock)
            .map_err(|error| MetadataError::Failed(error.to_string()))?;
        let path = root.join("metadata.json");
        let mut state = match std::fs::File::open(&path) {
            Ok(mut file) => {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)
                    .map_err(|error| MetadataError::Failed(error.to_string()))?;
                serde_json::from_slice(&bytes)
                    .map_err(|error| MetadataError::Failed(error.to_string()))?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => MetadataFile::default(),
            Err(error) => return Err(MetadataError::Failed(error.to_string())),
        };
        let (result, changed) = mutate(&mut state)?;
        if changed {
            let temporary = root.join(format!("metadata.tmp-{}", std::process::id()));
            let bytes = serde_json::to_vec(&state)
                .map_err(|error| MetadataError::Failed(error.to_string()))?;
            let mut file = std::fs::File::create(&temporary)
                .map_err(|error| MetadataError::Failed(error.to_string()))?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|error| MetadataError::Failed(error.to_string()))?;
            std::fs::rename(&temporary, &path)
                .map_err(|error| MetadataError::Failed(error.to_string()))?;
            std::fs::File::open(root)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| MetadataError::Failed(error.to_string()))?;
        }
        Ok(result)
    }

    async fn blocking<T: Send + 'static>(
        root: Arc<PathBuf>,
        operation: impl FnOnce(&Path) -> Result<T, MetadataError> + Send + 'static,
    ) -> Result<T, MetadataError> {
        tokio::task::spawn_blocking(move || operation(&root))
            .await
            .map_err(|error| MetadataError::Failed(error.to_string()))?
    }
}

#[async_trait]
impl SessionMetadataStore for FileMetadataStore {
    async fn reserve(
        &self,
        session_id: &SessionId,
        request_id: &str,
        metadata: SessionMetadata,
    ) -> Result<SessionReservation, MetadataError> {
        let root = self.root.clone();
        let session_id = session_id.clone();
        let request_id = request_id.to_owned();
        Self::blocking(root, move |root| {
            Self::with_locked(root, |state| {
                if let Some(existing_id) = state.requests.get(&request_id).cloned() {
                    let existing = state.sessions.get(&existing_id).cloned().ok_or_else(|| {
                        MetadataError::Failed(
                            "request reservation refers to missing metadata".into(),
                        )
                    })?;
                    return Ok((
                        SessionReservation {
                            session_id: existing_id,
                            metadata: existing,
                        },
                        false,
                    ));
                }
                if state.sessions.contains_key(&session_id) {
                    return Err(MetadataError::Failed(format!(
                        "session already exists: {session_id}"
                    )));
                }
                state.requests.insert(request_id, session_id.clone());
                state.sessions.insert(session_id.clone(), metadata.clone());
                Ok((
                    SessionReservation {
                        session_id,
                        metadata,
                    },
                    true,
                ))
            })
        })
        .await
    }

    async fn release(&self, session_id: &SessionId, request_id: &str) -> Result<(), MetadataError> {
        let root = self.root.clone();
        let session_id = session_id.clone();
        let request_id = request_id.to_owned();
        Self::blocking(root, move |root| {
            Self::with_locked(root, |state| {
                if state.requests.get(&request_id) == Some(&session_id) {
                    state.requests.remove(&request_id);
                    state.sessions.remove(&session_id);
                    Ok(((), true))
                } else {
                    Ok(((), false))
                }
            })
        })
        .await
    }

    async fn load(&self, id: &SessionId) -> Result<SessionMetadata, MetadataError> {
        let root = self.root.clone();
        let id = id.clone();
        Self::blocking(root, move |root| {
            Self::with_locked(root, |state| {
                let metadata =
                    state.sessions.get(&id).cloned().ok_or_else(|| {
                        MetadataError::Failed(format!("metadata not found: {id}"))
                    })?;
                Ok((metadata, false))
            })
        })
        .await
    }

    async fn list(&self) -> Result<Vec<SessionMetadata>, MetadataError> {
        let root = self.root.clone();
        Self::blocking(root, move |root| {
            Self::with_locked(root, |state| {
                Ok((state.sessions.values().cloned().collect(), false))
            })
        })
        .await
    }
}

#[derive(Clone, Debug)]
pub struct CreateRequest {
    pub request_id: String,
    pub session_id: SessionId,
    pub context: InitialContext,
    pub metadata: SessionMetadata,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RuntimeEvent {
    Stream(StreamEvent),
    Replay {
        sequence: u64,
        kind: TranscriptEntryKind,
    },
    /// Live subscribers fell behind; `skipped` events were covered by the
    /// durable record and can be recovered through replay.
    Lagged {
        skipped: u64,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventCursor(pub u64);
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub metadata: SessionMetadata,
    pub record_sequence: u64,
    pub active_run: Option<RunId>,
}
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Metadata(#[from] MetadataError),
    #[error("lineage parent does not exist: {0}")]
    MissingParent(SessionId),
    #[error("a session cannot be its own parent")]
    SelfParent,
    #[error("request ID is required")]
    MissingRequestId,
    #[error("session is not live")]
    NotLive,
}
struct Live {
    handle: SessionHandle,
    children: Vec<(SessionId, Attachment)>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryDirective {
    RemainInterrupted,
    ResumeQueuedWork,
}

#[async_trait]
pub trait RecoveryResolver: Send + Sync + 'static {
    async fn resolve(
        &self,
        session_id: &SessionId,
        metadata: &SessionMetadata,
    ) -> Result<RecoveryDirective, String>;
}

#[derive(Clone)]
pub struct SessionRuntime {
    store: Arc<dyn SessionStore>,
    metadata: Arc<dyn SessionMetadataStore>,
    model: Arc<dyn StreamingModel>,
    profiles: Arc<dyn ProfileSource>,
    extensions: Arc<dyn ExecutionExtensions>,
    recovery_resolver: Option<Arc<dyn RecoveryResolver>>,
    live: Arc<Mutex<HashMap<SessionId, Live>>>,

    events: broadcast::Sender<RuntimeEvent>,
}
impl SessionRuntime {
    pub fn new(
        store: Arc<dyn SessionStore>,
        metadata: Arc<dyn SessionMetadataStore>,
        model: Arc<dyn StreamingModel>,
        profiles: Arc<dyn ProfileSource>,
    ) -> Self {
        let (events, _) = broadcast::channel(128);
        Self {
            store,
            metadata,
            model,
            profiles,
            extensions: no_extensions(),
            recovery_resolver: None,
            live: Arc::default(),
            events,
        }
    }
    pub fn with_extensions(mut self, extensions: Arc<dyn ExecutionExtensions>) -> Self {
        self.extensions = extensions;
        self
    }

    pub fn with_recovery_resolver(mut self, resolver: Arc<dyn RecoveryResolver>) -> Self {
        self.recovery_resolver = Some(resolver);
        self
    }

    /// Production construction path: the model is resolved per request from
    /// an installed provider source (the activated plugin registry) through
    /// the profile router, never hand-built by the application.
    pub fn from_provider_source(
        store: Arc<dyn SessionStore>,
        metadata: Arc<dyn SessionMetadataStore>,
        providers: Arc<dyn artist_kernel::ModelProviderSource>,
        profiles: Arc<dyn ProfileSource>,
    ) -> Self {
        let router: Arc<dyn StreamingModel> =
            Arc::new(artist_kernel::ProfileModelRouter::new(providers));
        Self::new(store, metadata, router, profiles)
    }

    pub async fn create(&self, r: CreateRequest) -> Result<SessionId, RuntimeError> {
        if r.request_id.is_empty() {
            return Err(RuntimeError::MissingRequestId);
        }
        if r.metadata
            .lineage
            .as_ref()
            .is_some_and(|x| x.parent_session_id == r.session_id)
        {
            return Err(RuntimeError::SelfParent);
        }
        if let Some(l) = &r.metadata.lineage {
            self.metadata
                .load(&l.parent_session_id)
                .await
                .map_err(|_| RuntimeError::MissingParent(l.parent_session_id.clone()))?;
            self.ensure_live(&l.parent_session_id).await?;
        }
        // Prompt composition and fragment validation happen inside
        // SessionHandle::create, before the canonical record exists; a failed
        // or invalid composition releases the reservation below so no partial
        // session survives. Resume never recomposes.
        let reservation = self
            .metadata
            .reserve(&r.session_id, &r.request_id, r.metadata.clone())
            .await?;
        if reservation.session_id != r.session_id {
            self.ensure_live(&reservation.session_id).await?;
            return Ok(reservation.session_id);
        }
        let m = reservation.metadata;
        if self.store.load(&r.session_id).await.is_ok() {
            self.ensure_live(&r.session_id).await?;
            return Ok(r.session_id);
        }
        let dependencies = SessionDependencies {
            store: self.store.clone(),
            model: self.model.clone(),
            profiles: Some(self.profiles.clone()),
            slash_commands: None,
            extensions: self.extensions.clone(),
            resume_queued_work: true,
        };
        let h = match SessionHandle::create(
            CreateSession {
                session_id: r.session_id.clone(),
                metadata: m.clone(),
                context: r.context,
                initial_profile: m.initial_profile.clone(),
            },
            dependencies,
        )
        .await
        {
            Ok(h) => h,
            Err(SessionError::Store(StoreError::Exists(_))) => {
                self.ensure_live(&r.session_id).await?;
                return Ok(r.session_id);
            }
            Err(error) => {
                self.metadata.release(&r.session_id, &r.request_id).await?;
                return Err(error.into());
            }
        };
        self.install(r.session_id.clone(), h).await;
        if let Some(l) = &m.lineage
            && let Some(p) = self.live.lock().await.get_mut(&l.parent_session_id)
        {
            p.children.push((r.session_id.clone(), m.attachment));
        }
        Ok(r.session_id)
    }
    async fn install(&self, id: SessionId, h: SessionHandle) {
        let mut rx = h.subscribe();
        let out = self.events.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(e) => {
                        let _ = out.send(RuntimeEvent::Stream(e));
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        // Slow subscribers must not silently kill the bridge;
                        // replay covers the skipped range from the durable
                        // record, so forward the gap marker and continue.
                        let _ = out.send(RuntimeEvent::Lagged { skipped });
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
        let mut live = self.live.lock().await;
        live.entry(id).or_insert(Live {
            handle: h,
            children: vec![],
        });
    }
    async fn ensure_live(&self, id: &SessionId) -> Result<(), RuntimeError> {
        if self.live.lock().await.contains_key(id) {
            return Ok(());
        }
        let metadata = self.metadata.load(id).await?;
        let resume_queued_work = match metadata.recovery_policy {
            RecoveryPolicy::RemainInterrupted => false,
            RecoveryPolicy::ResumeQueuedWork => true,
            RecoveryPolicy::PluginResolved => match &self.recovery_resolver {
                Some(resolver) => matches!(
                    resolver
                        .resolve(id, &metadata)
                        .await
                        .map_err(MetadataError::Failed)?,
                    RecoveryDirective::ResumeQueuedWork
                ),
                None => {
                    return Err(RuntimeError::Metadata(MetadataError::Failed(
                        "plugin-resolved recovery requires a recovery resolver".into(),
                    )));
                }
            },
        };
        let dependencies = SessionDependencies {
            store: self.store.clone(),
            model: self.model.clone(),
            profiles: Some(self.profiles.clone()),
            slash_commands: None,
            extensions: self.extensions.clone(),
            resume_queued_work,
        };
        let h = SessionHandle::resume(id, dependencies).await?;
        self.install(id.clone(), h).await;
        Ok(())
    }
    pub async fn resume(&self, id: &SessionId) -> Result<(), RuntimeError> {
        self.ensure_live(id).await
    }
    pub async fn send(
        &self,
        id: &SessionId,
        source: Source,
        text: impl Into<String>,
    ) -> Result<(), RuntimeError> {
        self.send_content(id, source, vec![ContentPart::text(text)])
            .await
    }

    pub async fn send_content(
        &self,
        id: &SessionId,
        source: Source,
        content: Vec<ContentPart>,
    ) -> Result<(), RuntimeError> {
        self.ensure_live(id).await?;
        let h = self
            .live
            .lock()
            .await
            .get(id)
            .ok_or(RuntimeError::NotLive)?
            .handle
            .clone();
        h.input_content(source, content).await?;
        Ok(())
    }
    pub async fn steer(
        &self,
        id: &SessionId,
        source: Source,
        text: impl Into<String>,
    ) -> Result<(), RuntimeError> {
        self.steer_content(id, source, vec![ContentPart::text(text)])
            .await
    }

    pub async fn steer_content(
        &self,
        id: &SessionId,
        source: Source,
        content: Vec<ContentPart>,
    ) -> Result<(), RuntimeError> {
        self.ensure_live(id).await?;
        let h = self
            .live
            .lock()
            .await
            .get(id)
            .ok_or(RuntimeError::NotLive)?
            .handle
            .clone();
        h.steer_content(source, content).await?;
        Ok(())
    }
    pub async fn abort(
        &self,
        id: &SessionId,
        cause: InterruptionCause,
    ) -> Result<(), RuntimeError> {
        let mut todo = vec![id.clone()];
        while let Some(id) = todo.pop() {
            self.ensure_live(&id).await?;
            let (h, c) = self
                .live
                .lock()
                .await
                .get(&id)
                .ok_or(RuntimeError::NotLive)
                .map(|x| (x.handle.clone(), x.children.clone()))?;
            h.abort(cause.clone()).await?;
            todo.extend(
                c.into_iter()
                    .filter_map(|(id, a)| (a == Attachment::Attached).then_some(id)),
            );
        }
        Ok(())
    }
    pub async fn snapshot(&self, id: &SessionId) -> Result<SessionSnapshot, RuntimeError> {
        let m = self.metadata.load(id).await?;
        let r = self.store.load(id).await?;
        Ok(SessionSnapshot {
            metadata: m,
            record_sequence: r.next_sequence(),
            active_run: r.active_run().cloned(),
        })
    }
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.events.subscribe()
    }
    pub async fn replay(
        &self,
        id: &SessionId,
        c: EventCursor,
    ) -> Result<Vec<RuntimeEvent>, RuntimeError> {
        let r = self.store.load(id).await?;
        Ok(r.entries()
            .iter()
            .filter(|e| e.sequence >= c.0)
            .map(|e| RuntimeEvent::Replay {
                sequence: e.sequence,
                kind: e.kind.clone(),
            })
            .collect())
    }
    pub async fn subscribe_from(
        &self,
        id: &SessionId,
        c: EventCursor,
    ) -> Result<(Vec<RuntimeEvent>, broadcast::Receiver<RuntimeEvent>), RuntimeError> {
        Ok((self.replay(id, c).await?, self.subscribe()))
    }
    pub async fn await_terminal(&self, id: &SessionId) -> Result<RunOutcome, RuntimeError> {
        loop {
            let r = self.store.load(id).await?;
            let outcome = r.active_run().is_none().then(|| {
                r.entries().iter().rev().find_map(|e| {
                    if let TranscriptEntryKind::RunFinished { outcome, .. } = &e.kind {
                        Some(outcome.clone())
                    } else {
                        None
                    }
                })
            });
            if let Some(Some(o)) = outcome {
                return Ok(o);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await
        }
    }
    pub async fn metadata(&self, id: &SessionId) -> Result<SessionMetadata, RuntimeError> {
        Ok(self.metadata.load(id).await?)
    }

    /// Drain every live session: attached children are aborted through the
    /// kernel, detached children remain durably addressable, and the registry
    /// clears so a host can exit without orphaned actors.
    pub async fn shutdown(&self, reason: impl Into<String>) {
        let reason = reason.into();
        let mut live = self.live.lock().await;
        for session in live.values() {
            let _ = session
                .handle
                .abort(InterruptionCause::Harness {
                    reason: reason.clone(),
                })
                .await;
        }
        live.clear();
    }

    /// IDs of sessions with live actors right now.
    pub async fn live_sessions(&self) -> Vec<SessionId> {
        self.live.lock().await.keys().cloned().collect()
    }
}
#[async_trait]
impl artist_plugin::PluginSessionService for SessionRuntime {
    async fn create(
        &self,
        request: artist_plugin::PluginSessionCreate,
    ) -> Result<SessionId, String> {
        let lineage = request.parent_scope.as_ref().map(|scope| SessionLineage {
            parent_session_id: scope.session_id.clone(),
            parent_run_id: scope.run_id.clone(),
            parent_call_id: scope.call_id.clone(),
            relationship: request.relationship,
        });
        let created_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        let session_id = self
            .create(CreateRequest {
                request_id: request.request_id,
                session_id: request.session_id,
                context: InitialContext {
                    fragments: Vec::new(),
                },
                metadata: SessionMetadata {
                    created_at_ms,
                    creator_plugin_id: Some(request.creator_plugin_id),
                    lineage,
                    initial_profile: Some(request.profile),
                    attachment: if request.attached {
                        Attachment::Attached
                    } else {
                        Attachment::Detached
                    },
                    recovery_policy: match request.recovery.as_str() {
                        "remain-interrupted" => RecoveryPolicy::RemainInterrupted,
                        "resume-queued-work" => RecoveryPolicy::ResumeQueuedWork,
                        "plugin-resolved" => RecoveryPolicy::PluginResolved,
                        other => return Err(format!("unknown recovery policy `{other}`")),
                    },
                },
            })
            .await
            .map_err(|error| error.to_string())?;
        if !request.content.is_empty() {
            self.send_content(&session_id, Source::Harness, request.content)
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(session_id)
    }

    async fn send(&self, session_id: &SessionId, content: Vec<ContentPart>) -> Result<(), String> {
        self.send_content(session_id, Source::Harness, content)
            .await
            .map_err(|error| error.to_string())
    }

    async fn steer(&self, session_id: &SessionId, content: Vec<ContentPart>) -> Result<(), String> {
        self.steer_content(session_id, Source::Harness, content)
            .await
            .map_err(|error| error.to_string())
    }

    async fn snapshot(&self, session_id: &SessionId) -> Result<serde_json::Value, String> {
        SessionRuntime::snapshot(self, session_id)
            .await
            .and_then(|snapshot| {
                serde_json::to_value(snapshot).map_err(|error| {
                    RuntimeError::Metadata(MetadataError::Failed(error.to_string()))
                })
            })
            .map_err(|error| error.to_string())
    }

    async fn events(
        &self,
        session_id: &SessionId,
        cursor: u64,
        limit: u32,
    ) -> Result<(Vec<serde_json::Value>, u64), String> {
        let events = self
            .replay(session_id, EventCursor(cursor))
            .await
            .map_err(|error| error.to_string())?;
        let page = events.into_iter().take(limit as usize).collect::<Vec<_>>();
        let next = page
            .iter()
            .map(|event| match event {
                RuntimeEvent::Replay { sequence, .. } => sequence.saturating_add(1),
                RuntimeEvent::Stream(event) => event.sequence.saturating_add(1),
                // Lag markers carry no sequence; the next real event advances
                // the cursor past the skipped range.
                RuntimeEvent::Lagged { .. } => cursor,
            })
            .max()
            .unwrap_or(cursor);
        Ok((
            page.into_iter()
                .map(|event| serde_json::to_value(event).expect("runtime event is serializable"))
                .collect(),
            next,
        ))
    }

    async fn stop(&self, session_id: &SessionId, reason: String) -> Result<(), String> {
        self.abort(session_id, InterruptionCause::Harness { reason })
            .await
            .map_err(|error| error.to_string())
    }

    async fn await_terminal(&self, session_id: &SessionId) -> Result<serde_json::Value, String> {
        SessionRuntime::await_terminal(self, session_id)
            .await
            .and_then(|outcome| {
                serde_json::to_value(outcome).map_err(|error| {
                    RuntimeError::Metadata(MetadataError::Failed(error.to_string()))
                })
            })
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m(_id: &str, req: &str) -> SessionMetadata {
        let _ = req;
        SessionMetadata {
            created_at_ms: 1,
            creator_plugin_id: Some("p".into()),
            lineage: None,
            initial_profile: Some("p".into()),
            attachment: Attachment::Detached,
            recovery_policy: RecoveryPolicy::RemainInterrupted,
        }
    }
    #[tokio::test]
    async fn idempotent_request() {
        let s = MemoryMetadataStore::default();
        assert_eq!(
            s.reserve(&"a".into(), "r", m("a", "r")).await.unwrap(),
            s.reserve(&"b".into(), "r", m("b", "r")).await.unwrap()
        );
        assert_eq!(s.list().await.unwrap().len(), 1)
    }
    #[test]
    fn policies_serialize() {
        let x =
            serde_json::to_string(&(Attachment::Attached, RecoveryPolicy::PluginResolved)).unwrap();
        assert!(x.contains("attached") && x.contains("plugin_resolved"));
    }

    mod compose {
        use super::*;
        use artist_core::{ContextFragment, ContextRole};
        use artist_kernel::{
            ExecutionExtensionError, ModelEvent, ModelRequest, ModelStream, Steering,
        };
        use artist_store::MemoryStore;
        use std::collections::HashMap;

        struct DoneModel;
        impl StreamingModel for DoneModel {
            fn stream(&self, _: ModelRequest, _: Steering) -> ModelStream {
                Box::pin(futures::stream::once(async {
                    Ok(ModelEvent::Finished {
                        output: Some("done".into()),
                    })
                }))
            }
        }

        struct StaticProfiles(HashMap<String, artist_core::ProfileSnapshot>);
        #[async_trait]
        impl ProfileSource for StaticProfiles {
            async fn load(&self, name: &str) -> Result<artist_core::ProfileSnapshot, String> {
                self.0
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("unknown profile `{name}`"))
            }
        }

        fn metadata(_id: &SessionId) -> SessionMetadata {
            SessionMetadata {
                created_at_ms: 1,
                creator_plugin_id: Some("tester".into()),
                lineage: None,
                initial_profile: None,
                attachment: Attachment::Detached,
                recovery_policy: RecoveryPolicy::RemainInterrupted,
            }
        }

        async fn runtime_with(
            store: Arc<MemoryStore>,
            metadata: Arc<MemoryMetadataStore>,
            extensions: Arc<dyn ExecutionExtensions>,
        ) -> SessionRuntime {
            SessionRuntime::new(
                store,
                metadata,
                Arc::new(DoneModel),
                Arc::new(StaticProfiles(HashMap::new())),
            )
            .with_extensions(extensions)
        }

        struct AppendFragment(&'static str);
        #[async_trait]
        impl ExecutionExtensions for AppendFragment {
            async fn compose_initial_context(
                &self,
                mut context: InitialContext,
            ) -> Result<InitialContext, ExecutionExtensionError> {
                context.fragments.push(ContextFragment {
                    source: self.0.into(),
                    content: "composed".into(),
                    role: ContextRole::System,
                });
                Ok(context)
            }
        }

        struct FailCompose;
        #[async_trait]
        impl ExecutionExtensions for FailCompose {
            async fn compose_initial_context(
                &self,
                _: InitialContext,
            ) -> Result<InitialContext, ExecutionExtensionError> {
                Err(ExecutionExtensionError::new(
                    "compose-initial-context",
                    "prompt plugin exploded",
                ))
            }
        }

        struct EmptySource;
        #[async_trait]
        impl ExecutionExtensions for EmptySource {
            async fn compose_initial_context(
                &self,
                mut context: InitialContext,
            ) -> Result<InitialContext, ExecutionExtensionError> {
                context.fragments.push(ContextFragment {
                    source: "  ".into(),
                    content: "composed".into(),
                    role: ContextRole::System,
                });
                Ok(context)
            }
        }

        #[tokio::test]
        async fn composed_fragments_are_frozen_into_the_record() {
            let store = Arc::new(MemoryStore::default());
            let metadata = Arc::new(MemoryMetadataStore::default());
            let runtime =
                runtime_with(store.clone(), metadata, Arc::new(AppendFragment("fixture"))).await;
            let id: SessionId = "s".into();
            let session = runtime
                .create(CreateRequest {
                    request_id: "r1".into(),
                    session_id: id.clone(),
                    context: InitialContext { fragments: vec![] },
                    metadata: metadata_of(&id),
                })
                .await
                .unwrap();
            assert_eq!(session, id);
            let record = store.load(&id).await.unwrap();
            let fragments = &record.initial_context().fragments;
            assert_eq!(fragments.len(), 1);
            assert_eq!(fragments[0].source, "fixture");
        }

        #[tokio::test]
        async fn composition_failure_leaves_no_reservation_or_session() {
            let store = Arc::new(MemoryStore::default());
            let metadata_store = Arc::new(MemoryMetadataStore::default());
            let failing =
                runtime_with(store.clone(), metadata_store.clone(), Arc::new(FailCompose)).await;
            let id: SessionId = "s".into();
            let error = failing
                .create(CreateRequest {
                    request_id: "r1".into(),
                    session_id: id.clone(),
                    context: InitialContext { fragments: vec![] },
                    metadata: metadata_of(&id),
                })
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                RuntimeError::Session(SessionError::Extension(_))
            ));
            assert!(store.load(&id).await.is_err());

            // The same request ID can be used again once composition succeeds:
            // proof the failed attempt left no durable reservation behind.
            let healthy = runtime_with(
                store.clone(),
                metadata_store,
                Arc::new(AppendFragment("ok")),
            )
            .await;
            assert!(
                healthy
                    .create(CreateRequest {
                        request_id: "r1".into(),
                        session_id: id.clone(),
                        context: InitialContext { fragments: vec![] },
                        metadata: metadata_of(&id),
                    })
                    .await
                    .is_ok()
            );
        }

        #[tokio::test]
        async fn invalid_composed_fragments_are_rejected_before_the_record_exists() {
            let store = Arc::new(MemoryStore::default());
            let runtime = runtime_with(
                store.clone(),
                Arc::new(MemoryMetadataStore::default()),
                Arc::new(EmptySource),
            )
            .await;
            let id: SessionId = "s".into();
            let error = runtime
                .create(CreateRequest {
                    request_id: "r1".into(),
                    session_id: id.clone(),
                    context: InitialContext { fragments: vec![] },
                    metadata: metadata_of(&id),
                })
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                RuntimeError::Session(SessionError::InvalidContext(_))
            ));
            assert!(store.load(&id).await.is_err());
        }

        fn metadata_of(id: &SessionId) -> SessionMetadata {
            metadata(id)
        }

        mod sessions {
            use super::*;
            use artist_core::SessionLineage;
            use artist_kernel::{ModelEvent, ModelRequest, ModelStream};
            use std::sync::Arc;
            use std::sync::atomic::{AtomicUsize, Ordering};
            use tokio::sync::Notify;

            /// Finishes only when released, so runs can be observed mid-flight.
            struct GatedModel {
                release: Arc<Notify>,
                generation: Arc<AtomicUsize>,
            }
            impl StreamingModel for GatedModel {
                fn stream(&self, _: ModelRequest, _: Steering) -> ModelStream {
                    let release = self.release.clone();
                    let generation = self.generation.clone();
                    Box::pin(futures::stream::once(async move {
                        let my = generation.fetch_add(1, Ordering::AcqRel);
                        // Each run waits for its own release tick.
                        loop {
                            release.notified().await;
                            if generation.load(Ordering::Acquire) > my + 1 {
                                continue;
                            }
                            break;
                        }
                        Ok(ModelEvent::Finished {
                            output: Some("released".into()),
                        })
                    }))
                }
            }

            struct NoExtensions;
            #[async_trait]
            impl ExecutionExtensions for NoExtensions {}

            async fn make_runtime() -> (
                SessionRuntime,
                Arc<MemoryStore>,
                Arc<Notify>,
                Arc<AtomicUsize>,
            ) {
                let store = Arc::new(MemoryStore::default());
                let metadata = Arc::new(MemoryMetadataStore::default());
                let release = Arc::new(Notify::new());
                let generation = Arc::new(AtomicUsize::new(0));
                let runtime = SessionRuntime::new(
                    store.clone(),
                    metadata,
                    Arc::new(GatedModel {
                        release: release.clone(),
                        generation: generation.clone(),
                    }),
                    Arc::new(StaticProfiles(HashMap::new())),
                )
                .with_extensions(Arc::new(NoExtensions));
                (runtime, store, release, generation)
            }

            async fn spawn_session(
                runtime: &SessionRuntime,
                request_id: &str,
                id: &str,
                lineage: Option<SessionLineage>,
                attachment: Attachment,
            ) -> Result<SessionId, RuntimeError> {
                let session_id = SessionId::from(id);
                let mut md = metadata_of(&session_id);
                md.lineage = lineage;
                md.attachment = attachment;
                runtime
                    .create(CreateRequest {
                        request_id: request_id.into(),
                        session_id: session_id.clone(),
                        context: InitialContext { fragments: vec![] },
                        metadata: md,
                    })
                    .await
            }

            #[tokio::test]
            async fn missing_lineage_parents_are_rejected_at_creation() {
                let (runtime, store, _, _) = make_runtime().await;
                let error = spawn_session(
                    &runtime,
                    "r1",
                    "child",
                    Some(SessionLineage {
                        parent_session_id: SessionId::from("ghost"),
                        parent_run_id: None,
                        parent_call_id: None,
                        relationship: "subtask".into(),
                    }),
                    Attachment::Detached,
                )
                .await
                .unwrap_err();
                assert!(error.to_string().contains("does not exist"), "{error}");
                assert!(store.load(&SessionId::from("child")).await.is_err());
            }

            #[tokio::test]
            async fn aborting_a_parent_cascades_to_attached_runs_only() {
                let (runtime, _, release, _) = make_runtime().await;
                let parent = spawn_session(&runtime, "p", "parent", None, Attachment::Detached)
                    .await
                    .unwrap();
                let attached = spawn_session(
                    &runtime,
                    "c1",
                    "attached",
                    Some(SessionLineage {
                        parent_session_id: parent.clone(),
                        parent_run_id: None,
                        parent_call_id: None,
                        relationship: "subtask".into(),
                    }),
                    Attachment::Attached,
                )
                .await
                .unwrap();
                let detached = spawn_session(
                    &runtime,
                    "c2",
                    "detached",
                    Some(SessionLineage {
                        parent_session_id: parent.clone(),
                        parent_run_id: None,
                        parent_call_id: None,
                        relationship: "peer".into(),
                    }),
                    Attachment::Detached,
                )
                .await
                .unwrap();

                // Start active runs on every session; all block on the gate.
                for id in [&parent, &attached, &detached] {
                    runtime
                        .send_content(id, Source::User, vec![ContentPart::text("go")])
                        .await
                        .unwrap();
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;

                runtime
                    .abort(
                        &parent,
                        InterruptionCause::Harness {
                            reason: "parent stop".into(),
                        },
                    )
                    .await
                    .unwrap();

                // Parent and its attached child were interrupted...
                for id in [&parent, &attached] {
                    let outcome = runtime.await_terminal(id).await.unwrap();
                    assert!(
                        matches!(outcome, RunOutcome::Interrupted { .. }),
                        "{outcome:?}"
                    );
                }
                // ...while the detached child's run only completes when its
                // own gate opens; cancelling the parent never touched it.
                release.notify_one();
                let outcome = runtime.await_terminal(&detached).await.unwrap();
                assert!(matches!(
                    outcome,
                    RunOutcome::Completed { .. } | RunOutcome::Failed { .. }
                ));
            }

            /// Crash simulation: drop the whole runtime so every actor dies
            /// mid-run; the durable record keeps an unfinished run.
            struct CountingModel {
                hits: Arc<AtomicUsize>,
                gate: Option<Arc<Notify>>,
            }
            impl StreamingModel for CountingModel {
                fn stream(&self, _: ModelRequest, _: Steering) -> ModelStream {
                    self.hits.fetch_add(1, Ordering::AcqRel);
                    if let Some(gate) = &self.gate {
                        let gate = gate.clone();
                        Box::pin(futures::stream::once(async move {
                            gate.notified().await;
                            Ok(ModelEvent::Finished {
                                output: Some("done".into()),
                            })
                        }))
                    } else {
                        Box::pin(futures::stream::once(async {
                            Ok(ModelEvent::Finished {
                                output: Some("done".into()),
                            })
                        }))
                    }
                }
            }

            fn empty_profile() -> artist_core::ProfileSnapshot {
                artist_core::ProfileSnapshot {
                    name: "test".into(),
                    instructions: String::new(),
                    yield_schema: artist_core::default_yield_schema(),
                    policy: artist_core::ProfilePolicy::default(),
                    models: Vec::new(),
                    catalog: Vec::new(),
                }
            }

            fn counting_runtime(
                store: Arc<MemoryStore>,
                metadata: Arc<MemoryMetadataStore>,
                gated: bool,
                hits: Arc<AtomicUsize>,
            ) -> SessionRuntime {
                let gate = gated.then(|| Arc::new(Notify::new()));
                let mut profiles = HashMap::new();
                profiles.insert("test".to_string(), empty_profile());
                SessionRuntime::new(
                    store,
                    metadata,
                    Arc::new(CountingModel { hits, gate }),
                    Arc::new(StaticProfiles(profiles)),
                )
            }

            #[tokio::test]
            async fn remain_interrupted_closes_the_crashed_run_without_replaying_queue() {
                let store = Arc::new(MemoryStore::default());
                let metadata_store = Arc::new(MemoryMetadataStore::default());
                let first_hits = Arc::new(AtomicUsize::new(0));
                let runtime_a = counting_runtime(
                    store.clone(),
                    metadata_store.clone(),
                    true,
                    first_hits.clone(),
                );
                let id: SessionId = "crash-a".into();
                {
                    let mut md = metadata_of(&id);
                    md.initial_profile = Some("test".into());
                    md.recovery_policy = RecoveryPolicy::RemainInterrupted;
                    runtime_a
                        .create(CreateRequest {
                            request_id: "r1".into(),
                            session_id: id.clone(),
                            context: InitialContext { fragments: vec![] },
                            metadata: md,
                        })
                        .await
                        .unwrap();
                    // First input blocks inside the gated model; second stays
                    // durably queued behind it.
                    runtime_a
                        .send_content(&id, Source::User, vec![ContentPart::text("one")])
                        .await
                        .unwrap();
                    runtime_a
                        .send_content(&id, Source::User, vec![ContentPart::text("two")])
                        .await
                        .unwrap();
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                }
                // CRASH: drop every live actor without stopping the run.
                drop(runtime_a);
                assert_eq!(first_hits.load(Ordering::Acquire), 1);

                // Restart under remain-interrupted: the unfinished run is
                // closed, but the queued input must NOT replay.
                let second_hits = Arc::new(AtomicUsize::new(0));
                let runtime_b =
                    counting_runtime(store.clone(), metadata_store, false, second_hits.clone());
                runtime_b.ensure_live(&id).await.unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                let record = store.load(&id).await.unwrap();
                let interrupted = record
                    .entries()
                    .iter()
                    .filter(|entry| {
                        matches!(
                            entry.kind,
                            TranscriptEntryKind::RunFinished {
                                outcome: RunOutcome::Interrupted { .. },
                                ..
                            }
                        )
                    })
                    .count();
                assert_eq!(interrupted, 1, "{record:?}");
                assert_eq!(second_hits.load(Ordering::Acquire), 0);
            }

            #[tokio::test]
            async fn resume_queued_work_replays_pending_inputs_after_crash() {
                let store = Arc::new(MemoryStore::default());
                let metadata_store = Arc::new(MemoryMetadataStore::default());
                let first_hits = Arc::new(AtomicUsize::new(0));
                let runtime_a = counting_runtime(
                    store.clone(),
                    metadata_store.clone(),
                    true,
                    first_hits.clone(),
                );
                let id: SessionId = "crash-b".into();
                {
                    let mut md = metadata_of(&id);
                    md.initial_profile = Some("test".into());
                    md.recovery_policy = RecoveryPolicy::ResumeQueuedWork;
                    runtime_a
                        .create(CreateRequest {
                            request_id: "r1".into(),
                            session_id: id.clone(),
                            context: InitialContext { fragments: vec![] },
                            metadata: md,
                        })
                        .await
                        .unwrap();
                    runtime_a
                        .send_content(&id, Source::User, vec![ContentPart::text("one")])
                        .await
                        .unwrap();
                    runtime_a
                        .send_content(&id, Source::User, vec![ContentPart::text("two")])
                        .await
                        .unwrap();
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                }
                drop(runtime_a);

                let second_hits = Arc::new(AtomicUsize::new(0));
                let runtime_b =
                    counting_runtime(store.clone(), metadata_store, false, second_hits.clone());
                runtime_b.ensure_live(&id).await.unwrap();
                // The interrupted close of run one resolves any early
                // await_terminal; wait for the replay to actually finish.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while second_hits.load(Ordering::Acquire) < 1 {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "queued input was not replayed"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                let record = store.load(&id).await.unwrap();
                let last_finished = record
                    .entries()
                    .iter()
                    .rev()
                    .find_map(|entry| match &entry.kind {
                        TranscriptEntryKind::RunFinished { outcome, .. } => Some(outcome.clone()),
                        _ => None,
                    })
                    .expect("at least one terminal run");
                assert!(
                    matches!(last_finished, RunOutcome::Completed { .. }),
                    "replay did not complete: {last_finished:?}"
                );
                assert_eq!(second_hits.load(Ordering::Acquire), 1);
            }

            #[tokio::test]
            async fn shutdown_drains_the_registry_and_aborts_live_sessions() {
                let (runtime, _, _, _) = make_runtime().await;
                let first = spawn_session(&runtime, "r1", "one", None, Attachment::Detached)
                    .await
                    .unwrap();
                let _second = spawn_session(&runtime, "r2", "two", None, Attachment::Detached)
                    .await
                    .unwrap();
                runtime
                    .send_content(&first, Source::User, vec![ContentPart::text("go")])
                    .await
                    .unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;

                runtime.shutdown("host drain").await;
                assert!(runtime.live_sessions().await.is_empty());
                let outcome = runtime.await_terminal(&first).await.unwrap();
                assert!(matches!(outcome, RunOutcome::Interrupted { .. }));
            }
        }

        mod provider_source {
            use super::*;
            use artist_kernel::{
                ModelEvent, ModelProviderSource, ModelRequest, ModelStream, Steering,
            };
            use std::sync::atomic::{AtomicUsize, Ordering};

            fn test_profile_snapshot() -> artist_core::ProfileSnapshot {
                artist_core::ProfileSnapshot {
                    name: "routed".into(),
                    instructions: String::new(),
                    yield_schema: artist_core::default_yield_schema(),
                    policy: artist_core::ProfilePolicy::default(),
                    models: Vec::new(),
                    catalog: Vec::new(),
                }
            }

            struct PoolModel {
                hits: Arc<AtomicUsize>,
            }
            impl StreamingModel for PoolModel {
                fn stream(&self, _: ModelRequest, _: Steering) -> ModelStream {
                    self.hits.fetch_add(1, Ordering::AcqRel);
                    Box::pin(futures::stream::once(async {
                        Ok(ModelEvent::Finished {
                            output: Some("via-provider".into()),
                        })
                    }))
                }
            }

            /// Stands in for an activated provider-plugin registry.
            struct FakeProviders(Arc<AtomicUsize>);
            #[async_trait]
            impl ModelProviderSource for FakeProviders {
                async fn resolve(
                    &self,
                    _route: &artist_core::ModelRoute,
                    _session_id: &SessionId,
                    _profile_epoch: Option<u64>,
                ) -> Result<Arc<dyn StreamingModel>, String> {
                    Ok(Arc::new(PoolModel {
                        hits: self.0.clone(),
                    }))
                }
            }

            #[tokio::test]
            async fn sessions_resolve_models_through_the_provider_source() {
                let store = Arc::new(MemoryStore::default());
                let metadata = Arc::new(MemoryMetadataStore::default());
                let hits = Arc::new(AtomicUsize::new(0));
                let mut profiles = HashMap::new();
                let mut snapshot = test_profile_snapshot();
                snapshot.models = vec![artist_core::ModelRoute {
                    provider: "fake".into(),
                    account: Some("acct".into()),
                    api_variant: Some("native".into()),
                    model: "m1".into(),
                    reasoning: None,
                    parameters: serde_json::json!({}),
                }];
                profiles.insert("routed".to_string(), snapshot);
                let runtime = SessionRuntime::from_provider_source(
                    store.clone(),
                    metadata,
                    Arc::new(FakeProviders(hits.clone())),
                    Arc::new(StaticProfiles(profiles)),
                );
                let id: SessionId = "routed".into();
                let mut md = metadata_of(&id);
                md.initial_profile = Some("routed".into());
                runtime
                    .create(CreateRequest {
                        request_id: "r1".into(),
                        session_id: id.clone(),
                        context: InitialContext { fragments: vec![] },
                        metadata: md,
                    })
                    .await
                    .unwrap();
                runtime
                    .send_content(&id, Source::User, vec![ContentPart::text("hi")])
                    .await
                    .unwrap();
                let outcome = runtime.await_terminal(&id).await.unwrap();
                assert!(
                    matches!(outcome, RunOutcome::Completed { .. }),
                    "unexpected outcome: {outcome:?}"
                );
                assert_eq!(hits.load(Ordering::Acquire), 1);
            }
        }

        /// Gate proof: a real `ProviderRegistry` (account selection,
        /// credential fetch, driver open) drives live sessions through
        /// `from_provider_source` — no hand-built models in the app.
        mod registry_e2e {
            use super::*;
            use artist_kernel::{ModelEvent, ModelRequest, ModelStream, Steering};
            use artist_provider::{
                AccountDescriptor, Credential, CredentialStore, MemoryCredentialStore,
                NativeRigDriver, ProviderDescriptor, ProviderId, ProviderRegistry, Secret,
            };

            fn test_profile_snapshot() -> artist_core::ProfileSnapshot {
                artist_core::ProfileSnapshot {
                    name: "routed".into(),
                    instructions: String::new(),
                    yield_schema: artist_core::default_yield_schema(),
                    policy: artist_core::ProfilePolicy::default(),
                    models: Vec::new(),
                    catalog: Vec::new(),
                }
            }

            struct InstantModel;
            impl artist_kernel::StreamingModel for InstantModel {
                fn stream(&self, _: ModelRequest, _: Steering) -> ModelStream {
                    Box::pin(futures::stream::once(async {
                        Ok(ModelEvent::Finished {
                            output: Some("registry-driven".into()),
                        })
                    }))
                }
            }

            #[tokio::test]
            async fn profile_routes_resolve_through_the_provider_registry() {
                let credentials = Arc::new(MemoryCredentialStore::default());
                credentials
                    .put(
                        "keychain:fake/work",
                        Credential {
                            kind: "api-key".into(),
                            secret: Secret::new("sk-test"),
                            expires_at: None,
                            refresh: None,
                            private: Default::default(),
                        },
                    )
                    .await
                    .unwrap();
                let registry = Arc::new(ProviderRegistry::new(credentials));
                registry
                    .install(
                        ProviderDescriptor {
                            id: ProviderId("fake".into()),
                            revision: "r1".into(),
                            models: vec!["m1".into()],
                            capabilities: vec![artist_provider::Capability::new("streaming")],
                            auth_kinds: vec!["api-key".into()],
                            api_variants: vec!["native".into()],
                            parameters: serde_json::json!({}),
                        },
                        Arc::new(NativeRigDriver::new(|_, _, _| Ok(Arc::new(InstantModel)))),
                    )
                    .unwrap();
                registry
                    .upsert_account(AccountDescriptor {
                        id: "work".into(),
                        provider: ProviderId("fake".into()),
                        credential_ref: "keychain:fake/work".into(),
                        credential_kind: "api-key".into(),
                        api_variant: "native".into(),
                        default_model: "m1".into(),
                        default_reasoning: None,
                        metadata: Default::default(),
                    })
                    .unwrap();

                let store = Arc::new(MemoryStore::default());
                let metadata = Arc::new(MemoryMetadataStore::default());
                let mut profiles = HashMap::new();
                let mut snapshot = test_profile_snapshot();
                snapshot.models = vec![artist_core::ModelRoute {
                    provider: "fake".into(),
                    account: Some("work".into()),
                    api_variant: Some("native".into()),
                    model: "m1".into(),
                    reasoning: None,
                    parameters: serde_json::json!({}),
                }];
                profiles.insert("routed".to_string(), snapshot);
                let runtime = SessionRuntime::from_provider_source(
                    store.clone(),
                    metadata,
                    registry,
                    Arc::new(StaticProfiles(profiles)),
                );

                let id: SessionId = "e2e".into();
                let mut md = metadata_of(&id);
                md.initial_profile = Some("routed".into());
                runtime
                    .create(CreateRequest {
                        request_id: "r1".into(),
                        session_id: id.clone(),
                        context: InitialContext { fragments: vec![] },
                        metadata: md,
                    })
                    .await
                    .unwrap();
                runtime
                    .send_content(&id, Source::User, vec![ContentPart::text("hi")])
                    .await
                    .unwrap();
                let outcome = runtime.await_terminal(&id).await.unwrap();
                assert!(
                    matches!(outcome, RunOutcome::Completed { .. }),
                    "unexpected outcome: {outcome:?}"
                );
                // The canonical transcript recorded the registry-driven run.
                let record = store.load(&id).await.unwrap();
                assert!(
                    record
                        .entries()
                        .iter()
                        .any(|entry| matches!(entry.kind, TranscriptEntryKind::RunFinished { .. }))
                );
            }
        }
    }
}
