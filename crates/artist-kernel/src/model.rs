use std::{
    collections::{HashMap, VecDeque},
    pin::Pin,
    sync::{Arc, Mutex as SyncMutex},
};

use artist_core::{
    CallId, CompletionCallMetadata, ContentPart, CorrelationId, FailureClass, InvocationScope,
    MessageId, ModelFailure, ModelRoute, ProfileSnapshot, ProjectionArtifact, RunId, SessionId,
    Source, TokenUsage, ToolControl, ToolProgress,
};
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{Mutex, mpsc};

pub type ModelStream = Pin<Box<dyn Stream<Item = Result<ModelEvent, ModelError>> + Send>>;

pub trait StreamingModel: Send + Sync + 'static {
    fn stream(&self, request: ModelRequest, steering: Steering) -> ModelStream;
}

#[async_trait::async_trait]
pub trait ModelProviderSource: Send + Sync + 'static {
    async fn resolve(
        &self,
        route: &ModelRoute,
        session_id: &SessionId,
        profile_epoch: Option<u64>,
    ) -> Result<Arc<dyn StreamingModel>, String>;
}

pub struct CompositeModelProviders {
    sources: Vec<Arc<dyn ModelProviderSource>>,
}

impl CompositeModelProviders {
    pub fn new(sources: Vec<Arc<dyn ModelProviderSource>>) -> Result<Self, String> {
        if sources.is_empty() {
            return Err("at least one model provider source is required".into());
        }
        Ok(Self { sources })
    }
}

#[async_trait::async_trait]
impl ModelProviderSource for CompositeModelProviders {
    async fn resolve(
        &self,
        route: &ModelRoute,
        session_id: &SessionId,
        profile_epoch: Option<u64>,
    ) -> Result<Arc<dyn StreamingModel>, String> {
        let mut errors = Vec::new();
        for source in &self.sources {
            match source.resolve(route, session_id, profile_epoch).await {
                Ok(model) => return Ok(model),
                Err(error) => errors.push(error),
            }
        }
        Err(errors.join("; "))
    }
}

/// Resolves a profile's ordered routes through the installed provider registry
/// and retries a failed, side-effect-free attempt on the next route.
pub struct ProfileModelRouter {
    providers: Arc<dyn ModelProviderSource>,
    sticky: Arc<SyncMutex<HashMap<(SessionId, u64), usize>>>,
}

impl ProfileModelRouter {
    pub fn new(providers: Arc<dyn ModelProviderSource>) -> Self {
        Self {
            providers,
            sticky: Arc::new(SyncMutex::new(HashMap::new())),
        }
    }
}

impl StreamingModel for ProfileModelRouter {
    fn stream(&self, request: ModelRequest, steering: Steering) -> ModelStream {
        let configured = request
            .profile
            .as_ref()
            .map(|profile| profile.models.clone())
            .unwrap_or_default();
        let candidates = configured;
        let providers = self.providers.clone();
        let sticky = self.sticky.clone();
        let extensions = request.extensions.clone();
        let scope = InvocationScope {
            session_id: request.session_id.clone(),
            run_id: Some(request.run_id.clone()),
            call_id: None,
            correlation_id: CorrelationId::new(format!("{}:model", request.run_id)),
            parent_correlation_id: None,
        };
        let key = request
            .profile_epoch
            .map(|epoch| (request.session_id.clone(), epoch));
        let start = key
            .as_ref()
            .and_then(|key| {
                sticky
                    .lock()
                    .expect("sticky model lock poisoned")
                    .get(key)
                    .copied()
            })
            .unwrap_or(0)
            .min(candidates.len().saturating_sub(1));

        Box::pin(async_stream::stream! {
            if candidates.is_empty() {
                yield Err(ModelError::new("the active profile has no model routes"));
                return;
            }
            let mut failures = Vec::new();
            for index in start..candidates.len() {
                let route = &candidates[index];
                let configured = match extensions.configure_model(&scope, route.clone()).await {
                    Ok(configured) => configured,
                    Err(error) => {
                        yield Err(ModelError::new(error.to_string()));
                        return;
                    }
                };
                if configured.provider != route.provider
                    || configured.account != route.account
                    || configured.api_variant != route.api_variant
                    || configured.model != route.model
                {
                    yield Err(ModelError::new(
                        "model configuration extensions may change parameters/reasoning but not provider, account, API variant, or model",
                    ));
                    return;
                }
                let implementation = match providers
                    .resolve(
                        &configured,
                        &request.session_id,
                        request.profile_epoch,
                    )
                    .await
                {
                    Ok(implementation) => implementation,
                    Err(error) => {
                        failures.push(format!(
                            "{}/{} resolution failed: {error}",
                            configured.provider, configured.model
                        ));
                        continue;
                    }
                };
                let mut attempt = request.clone();
                attempt.selected_model = Some(configured);
                let mut stream = implementation.stream(attempt, steering.clone());
                let mut visible = false;
                let mut tool_activity = false;
                let mut failed = None;
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(event) => {
                            visible |= matches!(
                                event,
                                ModelEvent::TextDelta(_) | ModelEvent::Content(_)
                            );
                            tool_activity |= matches!(
                                event,
                                ModelEvent::ToolCallDelta { .. }
                                    | ModelEvent::ToolCall { .. }
                                    | ModelEvent::ToolExecutionCommitted { .. }
                                    | ModelEvent::ToolResult { .. }
                                    | ModelEvent::Control { .. }
                            );
                            yield Ok(event);
                        }
                        Err(error) => {
                            failed = Some(error);
                            break;
                        }
                    }
                }
                if let Some(error) = failed {
                    failures.push(error.0.message.clone());
                    if tool_activity || index + 1 == candidates.len() {
                        let mut terminal = error;
                        terminal.0.message = failures.join("; fallback: ");
                        yield Err(terminal);
                        return;
                    }
                    if visible {
                        yield Ok(ModelEvent::TextReset);
                    }
                    continue;
                }
                if let Some(key) = &key {
                    sticky
                        .lock()
                        .expect("sticky model lock poisoned")
                        .insert(key.clone(), index);
                }
                return;
            }
            yield Err(ModelError::new(format!(
                "no profile model route succeeded: {}",
                failures.join("; fallback: ")
            )));
        })
    }
}

#[derive(Clone)]
pub struct ModelRequest {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub context: String,
    pub prompt: Vec<ContentPart>,
    pub history: Vec<ModelHistoryItem>,
    pub profile: Option<Arc<ProfileSnapshot>>,
    pub profile_epoch: Option<u64>,
    pub selected_model: Option<ModelRoute>,
    pub extensions: Arc<dyn crate::ExecutionExtensions>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelHistoryItem {
    pub sequence: u64,
    pub message: ModelMessage,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ModelMessage {
    User(Vec<ContentPart>),
    Assistant(Vec<ContentPart>),
    Notification(Vec<ContentPart>),
    ToolCall {
        call_id: CallId,
        name: String,
        arguments: String,
    },
    ToolResult {
        call_id: CallId,
        content: Vec<ContentPart>,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ModelEvent {
    TextDelta(String),
    TextReset,
    ToolCallDelta {
        call_id: CallId,
        delta: String,
    },
    ToolCall {
        call_id: CallId,
        name: String,
        arguments: String,
    },
    ToolExecutionCommitted {
        call_id: CallId,
        name: String,
        arguments: String,
    },
    ToolResult {
        call_id: CallId,
        content: Vec<ContentPart>,
    },
    ToolProgress(ToolProgress),
    Content(ContentPart),
    ContextCompacted {
        through_sequence: u64,
        evicted_count: usize,
        evicted_bytes: usize,
        artifact: ProjectionArtifact,
    },
    Usage(TokenUsage),
    CompletionMetadata(Vec<CompletionCallMetadata>),
    Finished {
        output: Option<String>,
    },
    Control {
        call_id: CallId,
        control: ToolControl,
    },
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("{}", .0.message)]
pub struct ModelError(pub ModelFailure);

impl ModelError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(ModelFailure {
            message: message.into(),
            class: FailureClass::Unknown,
            retriable: false,
            provider_code: None,
            http_status: None,
            provider_request_id: None,
        })
    }
}

impl From<String> for ModelError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for ModelError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteeringNotice {
    pub message_id: MessageId,
    pub source: Source,
    pub content: Vec<ContentPart>,
}

/// A gentle notification inbox shared with the active model loop.
///
/// Model adapters call [`take`](Self::take) immediately before a model request.
/// Taking notifications reports their delivery to the session; it never cancels
/// the current request.
#[derive(Clone)]
pub struct Steering {
    queued: Arc<Mutex<VecDeque<SteeringNotice>>>,
    delivered: mpsc::UnboundedSender<Vec<MessageId>>,
}

impl Steering {
    pub fn empty() -> Self {
        Self::channel().0
    }

    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<Vec<MessageId>>) {
        Self::channel_with(VecDeque::new())
    }

    pub(crate) fn channel_with(
        queued: VecDeque<SteeringNotice>,
    ) -> (Self, mpsc::UnboundedReceiver<Vec<MessageId>>) {
        let (delivered, receiver) = mpsc::unbounded_channel();
        (
            Self {
                queued: Arc::new(Mutex::new(queued)),
                delivered,
            },
            receiver,
        )
    }

    pub(crate) async fn push(&self, notice: SteeringNotice) {
        self.queued.lock().await.push_back(notice);
    }

    pub(crate) async fn drain_for_handoff(&self) -> Vec<SteeringNotice> {
        self.queued.lock().await.drain(..).collect()
    }

    pub async fn take(&self) -> Vec<SteeringNotice> {
        let notices: Vec<_> = self.queued.lock().await.drain(..).collect();
        if !notices.is_empty() {
            let ids = notices
                .iter()
                .map(|notice| notice.message_id.clone())
                .collect();
            let _ = self.delivered.send(ids);
        }
        notices
    }

    pub async fn is_empty(&self) -> bool {
        self.queued.lock().await.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use artist_core::{ProfilePolicy, default_yield_schema};
    use futures::{StreamExt, stream};

    use super::*;

    struct ScriptModel {
        calls: AtomicUsize,
        scripts: SyncMutex<VecDeque<Vec<Result<ModelEvent, ModelError>>>>,
        selected: SyncMutex<Vec<Option<ModelRoute>>>,
    }

    impl ScriptModel {
        fn new(scripts: impl IntoIterator<Item = Vec<Result<ModelEvent, ModelError>>>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                scripts: SyncMutex::new(scripts.into_iter().collect()),
                selected: SyncMutex::new(Vec::new()),
            }
        }
    }

    struct TestProviders(HashMap<(String, String), Arc<dyn StreamingModel>>);

    #[async_trait::async_trait]
    impl ModelProviderSource for TestProviders {
        async fn resolve(
            &self,
            route: &ModelRoute,
            _session_id: &SessionId,
            _profile_epoch: Option<u64>,
        ) -> Result<Arc<dyn StreamingModel>, String> {
            self.0
                .get(&(route.provider.clone(), route.model.clone()))
                .cloned()
                .ok_or_else(|| format!("unregistered {}/{}", route.provider, route.model))
        }
    }

    fn router(
        primary: Arc<dyn StreamingModel>,
        fallback: Arc<dyn StreamingModel>,
    ) -> ProfileModelRouter {
        ProfileModelRouter::new(Arc::new(TestProviders(HashMap::from([
            (("provider".into(), "primary".into()), primary),
            (("provider".into(), "fallback".into()), fallback),
        ]))))
    }

    impl StreamingModel for ScriptModel {
        fn stream(&self, request: ModelRequest, _: Steering) -> ModelStream {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.selected.lock().unwrap().push(request.selected_model);
            Box::pin(stream::iter(
                self.scripts.lock().unwrap().pop_front().unwrap_or_default(),
            ))
        }
    }

    fn routed_request() -> ModelRequest {
        ModelRequest {
            session_id: SessionId::from("session"),
            run_id: RunId::from("run"),
            context: String::new(),
            prompt: vec![ContentPart::text("work")],
            history: Vec::new(),
            profile: Some(Arc::new(ProfileSnapshot {
                name: "worker".into(),
                instructions: String::new(),
                yield_schema: default_yield_schema(),
                policy: ProfilePolicy::default(),
                models: vec![
                    ModelRoute {
                        provider: "provider".into(),
                        account: None,
                        api_variant: None,
                        model: "primary".into(),
                        reasoning: None,
                        parameters: serde_json::json!({"temperature": 0}),
                    },
                    ModelRoute {
                        provider: "provider".into(),
                        account: None,
                        api_variant: None,
                        model: "fallback".into(),
                        reasoning: None,
                        parameters: serde_json::json!({"temperature": 1}),
                    },
                ],
                catalog: vec!["worker".into()],
            })),
            profile_epoch: Some(4),
            selected_model: None,
            extensions: crate::no_extensions(),
        }
    }

    #[tokio::test]
    async fn fallback_resets_partial_text_and_sticks_to_the_working_route() {
        let primary = Arc::new(ScriptModel::new([vec![
            Ok(ModelEvent::TextDelta("discard me".into())),
            Err(ModelError::new("primary failed")),
        ]]));
        let fallback = Arc::new(ScriptModel::new([
            vec![
                Ok(ModelEvent::TextDelta("kept".into())),
                Ok(ModelEvent::Finished { output: None }),
            ],
            vec![Ok(ModelEvent::Finished {
                output: Some("sticky".into()),
            })],
        ]));
        let router = router(primary.clone(), fallback.clone());

        let first = router
            .stream(routed_request(), Steering::empty())
            .collect::<Vec<_>>()
            .await;
        assert!(matches!(first.get(1), Some(Ok(ModelEvent::TextReset))));
        assert!(matches!(first.get(2), Some(Ok(ModelEvent::TextDelta(text))) if text == "kept"));
        let second = router
            .stream(routed_request(), Steering::empty())
            .collect::<Vec<_>>()
            .await;
        assert!(second.iter().all(Result::is_ok));
        assert_eq!(primary.calls.load(Ordering::Acquire), 1);
        assert_eq!(fallback.calls.load(Ordering::Acquire), 2);
        assert_eq!(
            fallback.selected.lock().unwrap()[0]
                .as_ref()
                .unwrap()
                .parameters,
            serde_json::json!({"temperature": 1})
        );
    }

    #[tokio::test]
    async fn fallback_never_retries_after_tool_activity() {
        let primary = Arc::new(ScriptModel::new([vec![
            Ok(ModelEvent::ToolCall {
                call_id: CallId::from("call"),
                name: "read".into(),
                arguments: "{}".into(),
            }),
            Err(ModelError::new("failed after tool activity")),
        ]]));
        let fallback = Arc::new(ScriptModel::new([vec![Ok(ModelEvent::Finished {
            output: None,
        })]]));
        let router = router(primary, fallback.clone());
        let events = router
            .stream(routed_request(), Steering::empty())
            .collect::<Vec<_>>()
            .await;
        assert!(matches!(events.last(), Some(Err(_))));
        assert_eq!(fallback.calls.load(Ordering::Acquire), 0);
    }
}
