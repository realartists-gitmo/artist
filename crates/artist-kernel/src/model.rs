use std::{
    collections::{HashMap, VecDeque},
    pin::Pin,
    sync::{Arc, Mutex as SyncMutex, RwLock},
};

use artist_core::{
    CallId, CompletionCallMetadata, ContentPart, FailureClass, MessageId, ModelFailure, ModelRoute,
    ProfileSnapshot, RunId, SessionId, Source, TokenUsage, ToolControl,
};
use futures::{Stream, StreamExt};
use thiserror::Error;
use tokio::sync::{Mutex, mpsc};

pub type ModelStream = Pin<Box<dyn Stream<Item = Result<ModelEvent, ModelError>> + Send>>;

pub trait StreamingModel: Send + Sync + 'static {
    fn stream(&self, request: ModelRequest, steering: Steering) -> ModelStream;
}

/// Resolves a profile's ordered model routes to already-constructed model
/// adapters and retries a failed, side-effect-free attempt on the next route.
pub struct ProfileModelRouter {
    default: Arc<dyn StreamingModel>,
    routes: RwLock<HashMap<(String, String), Arc<dyn StreamingModel>>>,
    sticky: Arc<SyncMutex<HashMap<(SessionId, u64), usize>>>,
}

impl ProfileModelRouter {
    pub fn new(default: Arc<dyn StreamingModel>) -> Self {
        Self {
            default,
            routes: RwLock::new(HashMap::new()),
            sticky: Arc::new(SyncMutex::new(HashMap::new())),
        }
    }

    pub fn register(
        &self,
        provider: impl Into<String>,
        model: impl Into<String>,
        implementation: Arc<dyn StreamingModel>,
    ) {
        self.routes
            .write()
            .expect("model route lock poisoned")
            .insert((provider.into(), model.into()), implementation);
    }
}

impl StreamingModel for ProfileModelRouter {
    fn stream(&self, request: ModelRequest, steering: Steering) -> ModelStream {
        let configured = request
            .profile
            .as_ref()
            .map(|profile| profile.models.clone())
            .unwrap_or_default();
        let routes = self.routes.read().expect("model route lock poisoned");
        let candidates = if configured.is_empty() {
            vec![(None, Some(self.default.clone()))]
        } else {
            configured
                .into_iter()
                .map(|route| {
                    let implementation = routes
                        .get(&(route.provider.clone(), route.model.clone()))
                        .cloned();
                    (Some(route), implementation)
                })
                .collect::<Vec<_>>()
        };
        drop(routes);
        let sticky = self.sticky.clone();
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
            let mut failures = Vec::new();
            for index in start..candidates.len() {
                let (route, implementation) = &candidates[index];
                let Some(implementation) = implementation else {
                    let route = route.as_ref().expect("default route always has an implementation");
                    failures.push(format!("{}/{} is not registered", route.provider, route.model));
                    continue;
                };
                let mut attempt = request.clone();
                attempt.selected_model = route.clone();
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelRequest {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub context: String,
    pub prompt: String,
    pub history: Vec<ModelHistoryItem>,
    pub profile: Option<Arc<ProfileSnapshot>>,
    pub profile_epoch: Option<u64>,
    pub selected_model: Option<ModelRoute>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelHistoryItem {
    pub sequence: u64,
    pub message: ModelMessage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelMessage {
    User(String),
    Assistant(Vec<ContentPart>),
    Notification(String),
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

#[derive(Clone, Debug, Eq, PartialEq)]
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
    Content(ContentPart),
    ContextCompacted {
        through_sequence: u64,
        evicted_count: usize,
        evicted_bytes: usize,
        artifact: String,
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
    pub content: String,
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
            prompt: "work".into(),
            history: Vec::new(),
            profile: Some(Arc::new(ProfileSnapshot {
                name: "worker".into(),
                instructions: String::new(),
                yield_schema: default_yield_schema(),
                policy: ProfilePolicy::default(),
                models: vec![
                    ModelRoute {
                        provider: "provider".into(),
                        model: "primary".into(),
                        parameters: serde_json::json!({"temperature": 0}),
                    },
                    ModelRoute {
                        provider: "provider".into(),
                        model: "fallback".into(),
                        parameters: serde_json::json!({"temperature": 1}),
                    },
                ],
                catalog: vec!["worker".into()],
            })),
            profile_epoch: Some(4),
            selected_model: None,
        }
    }

    #[tokio::test]
    async fn fallback_resets_partial_text_and_sticks_to_the_working_route() {
        let default = Arc::new(ScriptModel::new([]));
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
        let router = ProfileModelRouter::new(default);
        router.register("provider", "primary", primary.clone());
        router.register("provider", "fallback", fallback.clone());

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
        let default = Arc::new(ScriptModel::new([]));
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
        let router = ProfileModelRouter::new(default);
        router.register("provider", "primary", primary);
        router.register("provider", "fallback", fallback.clone());
        let events = router
            .stream(routed_request(), Steering::empty())
            .collect::<Vec<_>>()
            .await;
        assert!(matches!(events.last(), Some(Err(_))));
        assert_eq!(fallback.calls.load(Ordering::Acquire), 0);
    }
}
