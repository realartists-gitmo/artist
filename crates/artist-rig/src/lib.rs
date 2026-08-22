//! Rig adapter for the Artist streaming kernel.

use std::{collections::HashMap, sync::Arc};

use artist_core::{CallId, TokenUsage};
use artist_kernel::{
    ModelError, ModelEvent, ModelHistoryItem, ModelMessage, ModelRequest, ModelStream, Steering,
    StreamingModel,
};
use artist_resource::ToolRegistry;
use futures::StreamExt;
use rig_agent::{
    Agent, AgentBuilder, AgentHook, HookContext,
    agent::{CompletionCallAction, CompletionCallEvent, RequestPatch},
    completion::Document,
    prelude::MultiTurnStreamItem,
    tool::{DynamicTool, ToolExecutionError, ToolOutput},
};
use rig_core::{
    completion::{AssistantContent, CompletionModel, Message},
    message::{ToolCall, ToolCallId, ToolFunction, ToolResultContent},
    streaming::{StreamedAssistantContent, StreamedUserContent},
};
use rig_memory::{
    Compactor, HeuristicTokenCounter, MemoryPolicy, NoopMemoryPolicy, SlidingWindowMemory,
    TemplateCompactor, TokenWindowMemory,
};

pub struct RigModel {
    agent: Agent,
    policy: Arc<dyn MemoryPolicy>,
    compact: bool,
}

impl RigModel {
    pub fn new<M>(model: M) -> Self
    where
        M: CompletionModel + 'static,
    {
        Self::from_agent(AgentBuilder::new(model).build())
    }

    /// Build a streaming Rig model with every current registry entry exposed
    /// through Rig's runtime-defined tool surface.
    pub async fn with_registry<M>(model: M, registry: ToolRegistry) -> Self
    where
        M: CompletionModel + 'static,
    {
        let tools = registry
            .definitions()
            .await
            .into_iter()
            .map(|definition| {
                let registry = registry.clone();
                let name = definition.name.clone();
                DynamicTool::new(
                    definition.name,
                    definition.description,
                    definition.input_schema,
                    move |_context, arguments| {
                        let registry = registry.clone();
                        let name = name.clone();
                        Box::pin(async move {
                            registry
                                .call(&name, arguments)
                                .await
                                .map(ToolOutput::json)
                                .map_err(ToolExecutionError::from_error)
                        })
                    },
                )
            })
            .collect();
        Self::from_agent(
            AgentBuilder::new(model)
                .dynamic_tools(tools)
                .default_max_turns(32)
                .build(),
        )
    }

    pub fn from_agent(agent: Agent) -> Self {
        Self {
            agent,
            policy: Arc::new(NoopMemoryPolicy),
            compact: false,
        }
    }

    pub fn last_messages(mut self, count: usize) -> Self {
        self.policy = Arc::new(SlidingWindowMemory::last_messages(count));
        self
    }

    pub fn token_window(mut self, tokens: usize) -> Self {
        self.policy = Arc::new(TokenWindowMemory::new(
            tokens,
            HeuristicTokenCounter::default(),
        ));
        self
    }

    pub fn with_policy(mut self, policy: impl MemoryPolicy + 'static) -> Self {
        self.policy = Arc::new(policy);
        self
    }

    pub fn compact_last_messages(mut self, count: usize) -> Self {
        self.policy = Arc::new(SlidingWindowMemory::last_messages(count));
        self.compact = true;
        self
    }

    pub fn compact_token_window(mut self, tokens: usize) -> Self {
        self.policy = Arc::new(TokenWindowMemory::new(
            tokens,
            HeuristicTokenCounter::default(),
        ));
        self.compact = true;
        self
    }
}

impl StreamingModel for RigModel {
    fn stream(&self, request: ModelRequest, steering: Steering) -> ModelStream {
        let agent = self.agent.clone();
        let policy = self.policy.clone();
        let compact = self.compact;
        Box::pin(async_stream::stream! {
            let sequences: Vec<_> = request.history.iter().map(|item| item.sequence).collect();
            let (mut history, demoted) = match to_rig_history(request.history).and_then(|history| {
                policy.apply_with_demoted(history).map_err(|error| ModelError(error.to_string()))
            }) {
                Ok(history) => history,
                Err(error) => {
                    yield Err(error);
                    return;
                }
            };
            if compact && !demoted.is_empty() {
                let evicted_count = demoted.len();
                let evicted_bytes = serde_json::to_vec(&demoted).map_or(0, |bytes| bytes.len());
                let artifact = match TemplateCompactor::new()
                    .compact(request.session_id.as_str(), &demoted, None)
                    .await
                {
                    Ok(artifact) => artifact,
                    Err(error) => {
                        yield Err(ModelError(error.to_string()));
                        return;
                    }
                };
                let Some(through_sequence) = sequences.get(evicted_count - 1).copied() else {
                    yield Err(ModelError("memory policy returned an invalid demoted prefix".into()));
                    return;
                };
                history.insert(0, artifact.clone().into());
                yield Ok(ModelEvent::ContextCompacted {
                    through_sequence,
                    evicted_count,
                    evicted_bytes,
                    artifact: artifact.into_string(),
                });
            }
            let mut stream = agent
                .runner(Message::user(request.prompt))
                .preamble(request.context)
                .history(history)
                .add_hook(SteeringHook(steering))
                .stream()
                .await;

            while let Some(item) = stream.next().await {
                match item {
                    Ok(item) => {
                        for event in translate(item) {
                            yield Ok(event);
                        }
                    }
                    Err(error) => {
                        yield Err(ModelError(error.to_string()));
                        return;
                    }
                }
            }
        })
    }
}

struct SteeringHook(Steering);

impl AgentHook for SteeringHook {
    fn on_completion_call(
        &self,
        _ctx: &HookContext,
        _event: CompletionCallEvent<'_>,
    ) -> impl Future<Output = CompletionCallAction> + Send {
        let steering = self.0.clone();
        async move {
            let notices = steering.take().await;
            if notices.is_empty() {
                CompletionCallAction::Continue
            } else {
                CompletionCallAction::Patch(RequestPatch::new().extra_context(
                    notices.into_iter().map(|notice| Document {
                        id: notice.message_id.to_string(),
                        text: format!("Notification: {}", notice.content),
                        additional_props: HashMap::from([(
                            "source".into(),
                            format!("{:?}", notice.source).to_lowercase(),
                        )]),
                    }),
                ))
            }
        }
    }
}

fn to_rig_history(history: Vec<ModelHistoryItem>) -> Result<Vec<Message>, ModelError> {
    let mut names = HashMap::<CallId, String>::new();
    history
        .into_iter()
        .map(|item| match item.message {
            ModelMessage::User(text) => Ok(Message::user(text)),
            ModelMessage::Assistant(text) => Ok(Message::assistant(text)),
            ModelMessage::Notification(text) => Ok(Message::user(format!("[Notification] {text}"))),
            ModelMessage::ToolCall {
                call_id,
                name,
                arguments,
            } => {
                let arguments = serde_json::from_str(&arguments).map_err(|error| {
                    ModelError(format!("invalid stored tool arguments: {error}"))
                })?;
                names.insert(call_id.clone(), name.clone());
                Ok(Message::Assistant {
                    id: None,
                    content: vec![AssistantContent::ToolCall(ToolCall::new(
                        ToolCallId::new_or_mint(call_id.to_string()),
                        ToolFunction { name, arguments },
                    ))],
                })
            }
            ModelMessage::ToolResult { call_id, result } => {
                let name = names.get(&call_id).cloned().ok_or_else(|| {
                    ModelError(format!(
                        "stored tool result has no matching call: {call_id}"
                    ))
                })?;
                Ok(Message::tool_result(call_id.to_string(), name, result))
            }
        })
        .collect()
}

fn translate(item: MultiTurnStreamItem) -> Vec<ModelEvent> {
    match item {
        MultiTurnStreamItem::StreamAssistantItem(content) => match content {
            StreamedAssistantContent::Text(text) => vec![ModelEvent::TextDelta(text.text)],
            StreamedAssistantContent::ToolCall {
                tool_call,
                internal_call_id,
            } => vec![ModelEvent::ToolCall {
                call_id: CallId::new(internal_call_id),
                name: tool_call.function.name,
                arguments: tool_call.function.arguments.to_string(),
            }],
            StreamedAssistantContent::ToolCallDelta {
                internal_call_id,
                content,
            } => vec![ModelEvent::ToolCallDelta {
                call_id: CallId::new(internal_call_id),
                delta: serde_json::to_string(&content).unwrap_or_default(),
            }],
            StreamedAssistantContent::Reasoning { .. }
            | StreamedAssistantContent::ReasoningDelta { .. }
            | StreamedAssistantContent::Final(_)
            | StreamedAssistantContent::Unknown(_) => vec![],
        },
        MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
            tool_result,
            internal_call_id,
        }) => vec![ModelEvent::ToolResult {
            call_id: CallId::new(internal_call_id),
            result: tool_result
                .content
                .into_iter()
                .map(|content| match content {
                    ToolResultContent::Text(text) => text.text,
                    ToolResultContent::Json { value } => value.to_string(),
                    ToolResultContent::Image(image) => {
                        serde_json::to_string(&image).unwrap_or_default()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }],
        MultiTurnStreamItem::ModelTurnRetried { .. } => vec![ModelEvent::TextReset],
        MultiTurnStreamItem::FinalResponse(response) => vec![
            ModelEvent::Usage(TokenUsage {
                input: response.usage.input_tokens,
                output: response.usage.output_tokens,
                cached_input: response.usage.cached_input_tokens,
                reasoning: response.usage.reasoning_tokens,
            }),
            ModelEvent::Finished {
                output: Some(response.output),
            },
        ],
        MultiTurnStreamItem::ToolExecutionCommitted { .. }
        | MultiTurnStreamItem::CompletionCall(_) => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_core::{
        InitialContext, InterruptionCause, RunId, RunOutcome, SessionId, Source, StreamEvent,
        StreamEventKind, TranscriptEntryKind,
    };
    use artist_kernel::SessionHandle;
    use artist_resource::{
        InvocationContext, ToolDefinition as RegistryDefinition, ToolError, ToolHandler,
    };
    use artist_store::{MemoryStore, SessionStore};
    use async_trait::async_trait;
    use rig_agent::test_utils::{MockCompletionModel, MockError, MockStreamEvent, mock_final};
    use serde_json::{Value, json};
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };
    use tokio::sync::{Semaphore, broadcast};

    #[test]
    fn text_history_converts_without_loss() {
        let history = vec![
            ModelHistoryItem {
                sequence: 0,
                message: ModelMessage::User("hello".into()),
            },
            ModelHistoryItem {
                sequence: 1,
                message: ModelMessage::Assistant("hi".into()),
            },
        ];
        assert_eq!(
            to_rig_history(history).unwrap(),
            vec![Message::user("hello"), Message::assistant("hi")]
        );
    }

    #[test]
    fn tool_history_preserves_call_result_pairing() {
        let history = vec![
            ModelHistoryItem {
                sequence: 0,
                message: ModelMessage::ToolCall {
                    call_id: CallId::from("call"),
                    name: "read".into(),
                    arguments: r#"{"uri":"file:///tmp/a"}"#.into(),
                },
            },
            ModelHistoryItem {
                sequence: 1,
                message: ModelMessage::ToolResult {
                    call_id: CallId::from("call"),
                    result: "body".into(),
                },
            },
        ];
        let converted = to_rig_history(history).unwrap();
        assert!(matches!(converted[0], Message::Assistant { .. }));
        assert!(matches!(converted[1], Message::User { .. }));

        let orphan = vec![ModelHistoryItem {
            sequence: 0,
            message: ModelMessage::ToolResult {
                call_id: CallId::from("missing"),
                result: "body".into(),
            },
        }];
        assert!(
            to_rig_history(orphan)
                .unwrap_err()
                .0
                .contains("no matching call")
        );
    }

    #[test]
    fn memory_policies_shape_a_copy_of_history() {
        let canonical = vec![
            Message::user("one"),
            Message::assistant("two"),
            Message::user("three"),
        ];

        assert_eq!(
            NoopMemoryPolicy.apply(canonical.clone()).unwrap(),
            canonical
        );
        assert_eq!(
            SlidingWindowMemory::last_messages(2)
                .apply(canonical.clone())
                .unwrap(),
            canonical[1..]
        );
        assert!(
            TokenWindowMemory::new(8, HeuristicTokenCounter::default())
                .apply(canonical.clone())
                .unwrap()
                .len()
                < canonical.len()
        );
        assert_eq!(canonical.len(), 3);
    }

    #[test]
    fn usage_is_emitted_before_completion() {
        let mut usage = rig_core::completion::Usage::new();
        usage.input_tokens = 10;
        usage.output_tokens = 4;
        usage.cached_input_tokens = 3;
        usage.reasoning_tokens = 2;
        let item = MultiTurnStreamItem::final_response(vec![AssistantContent::text("done")], usage);
        let events = translate(item);
        assert!(matches!(events[0], ModelEvent::Usage(_)));
        assert!(matches!(events[1], ModelEvent::Finished { .. }));
    }

    struct Echo(Arc<AtomicBool>);
    #[async_trait]
    impl ToolHandler for Echo {
        async fn call(&self, arguments: Value, _: InvocationContext) -> Result<Value, ToolError> {
            self.0.store(true, Ordering::Release);
            Ok(arguments)
        }
    }

    struct Gate {
        started: Arc<Semaphore>,
        release: Arc<Semaphore>,
    }

    #[async_trait]
    impl ToolHandler for Gate {
        async fn call(&self, arguments: Value, _: InvocationContext) -> Result<Value, ToolError> {
            self.started.add_permits(1);
            self.release
                .acquire()
                .await
                .map_err(|_| ToolError::Failed("gate closed".into()))?
                .forget();
            Ok(arguments)
        }
    }

    async fn wait_for(
        events: &mut broadcast::Receiver<StreamEvent>,
        predicate: impl Fn(&StreamEventKind) -> bool,
    ) -> StreamEvent {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = events.recv().await.unwrap();
                if predicate(&event.kind) {
                    return event;
                }
            }
        })
        .await
        .expect("timed out waiting for stream event")
    }

    fn final_event() -> MockStreamEvent {
        MockStreamEvent::FinalResponse(mock_final(rig_core::completion::Usage::new()))
    }

    #[tokio::test]
    async fn rig_executes_model_selected_registry_tools() {
        let registry = ToolRegistry::new();
        let called = Arc::new(AtomicBool::new(false));
        registry
            .register(
                RegistryDefinition {
                    name: "echo".into(),
                    description: "echo JSON".into(),
                    input_schema: json!({"type": "object"}),
                },
                Arc::new(Echo(called.clone())),
            )
            .await
            .unwrap();
        let model = RigModel::with_registry(
            MockCompletionModel::from_stream_turns([
                vec![
                    MockStreamEvent::tool_call("call-1", "echo", json!({"value": 42})),
                    final_event(),
                ],
                vec![MockStreamEvent::text("done"), final_event()],
            ]),
            registry,
        )
        .await;
        let events = model
            .stream(
                ModelRequest {
                    session_id: SessionId::from("session"),
                    run_id: RunId::from("run"),
                    context: String::new(),
                    prompt: "echo".into(),
                    history: Vec::new(),
                },
                Steering::empty(),
            )
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        assert!(called.load(Ordering::Acquire));
        assert!(events.iter().any(|event| matches!(
            event,
            Ok(ModelEvent::ToolResult { result, .. }) if result.contains("42")
        )));
    }

    #[tokio::test]
    async fn kernel_controls_hold_through_the_rig_streaming_adapter() {
        let registry = ToolRegistry::new();
        let started = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        registry
            .register(
                RegistryDefinition {
                    name: "gate".into(),
                    description: "pause between model request boundaries".into(),
                    input_schema: json!({"type":"object"}),
                },
                Arc::new(Gate {
                    started: started.clone(),
                    release: release.clone(),
                }),
            )
            .await
            .unwrap();
        let mock = MockCompletionModel::from_stream_turns([
            vec![
                MockStreamEvent::tool_call("gate-call", "gate", json!({"ok":true})),
                final_event(),
            ],
            vec![MockStreamEvent::text("done"), final_event()],
        ]);
        let model = Arc::new(RigModel::with_registry(mock.clone(), registry).await);
        let store = Arc::new(MemoryStore::default());
        let session = SessionHandle::create(
            SessionId::from("rig-controls"),
            InitialContext { fragments: vec![] },
            store.clone(),
            model,
        )
        .await
        .unwrap();
        let mut events = session.subscribe();

        session.input(Source::User, "use the gate").await.unwrap();
        started.acquire().await.unwrap().forget();
        session
            .steer(Source::Harness, "new boundary context")
            .await
            .unwrap();
        release.add_permits(1);
        wait_for(&mut events, |kind| {
            matches!(kind, StreamEventKind::SteeringDelivered { .. })
        })
        .await;
        wait_for(&mut events, |kind| {
            matches!(kind, StreamEventKind::Completed { .. })
        })
        .await;

        let record = store.load(&SessionId::from("rig-controls")).await.unwrap();
        assert!(
            record
                .entries()
                .iter()
                .any(|entry| matches!(entry.kind, TranscriptEntryKind::ToolCall { .. }))
        );
        assert!(
            record
                .entries()
                .iter()
                .any(|entry| matches!(entry.kind, TranscriptEntryKind::ToolResult { .. }))
        );
        assert!(
            record
                .entries()
                .iter()
                .any(|entry| matches!(entry.kind, TranscriptEntryKind::SteeringDelivered { .. }))
        );
        assert!(matches!(
            record.entries()[record.entries().len() - 2].kind,
            TranscriptEntryKind::AssistantMessage {
                ref content,
                ..
            } if content == "done"
        ));
        let requests = mock.requests();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1]
                .documents
                .iter()
                .any(|document| document.text.contains("new boundary context"))
        );
    }

    #[tokio::test]
    async fn abort_cancels_a_rig_tool_turn_and_records_an_interruption() {
        let registry = ToolRegistry::new();
        let started = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        registry
            .register(
                RegistryDefinition {
                    name: "gate".into(),
                    description: "never released during this test".into(),
                    input_schema: json!({"type":"object"}),
                },
                Arc::new(Gate {
                    started: started.clone(),
                    release,
                }),
            )
            .await
            .unwrap();
        let mock = MockCompletionModel::from_stream_turns([vec![
            MockStreamEvent::tool_call("gate-call", "gate", json!({})),
            final_event(),
        ]]);
        let model = Arc::new(RigModel::with_registry(mock, registry).await);
        let store = Arc::new(MemoryStore::default());
        let session = SessionHandle::create(
            SessionId::from("rig-abort"),
            InitialContext { fragments: vec![] },
            store.clone(),
            model,
        )
        .await
        .unwrap();
        let mut events = session.subscribe();
        session.input(Source::User, "wait").await.unwrap();
        started.acquire().await.unwrap().forget();
        session.abort(InterruptionCause::User).await.unwrap();
        wait_for(&mut events, |kind| {
            matches!(kind, StreamEventKind::Interrupted { .. })
        })
        .await;

        let record = store.load(&SessionId::from("rig-abort")).await.unwrap();
        let tail = &record.entries()[record.entries().len() - 2..];
        assert!(matches!(
            tail[0].kind,
            TranscriptEntryKind::AssistantMessage { .. }
        ));
        assert!(matches!(
            tail[1].kind,
            TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Interrupted {
                    cause: InterruptionCause::User,
                    ..
                },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn rig_provider_failure_preserves_partial_output() {
        let mock = MockCompletionModel::from_stream_turns([vec![
            MockStreamEvent::text("partial"),
            MockStreamEvent::Error(MockError::provider("provider failed")),
        ]]);
        let model = Arc::new(RigModel::new(mock));
        let store = Arc::new(MemoryStore::default());
        let session = SessionHandle::create(
            SessionId::from("rig-failure"),
            InitialContext { fragments: vec![] },
            store.clone(),
            model,
        )
        .await
        .unwrap();
        let mut events = session.subscribe();
        session.input(Source::User, "fail").await.unwrap();
        wait_for(&mut events, |kind| {
            matches!(kind, StreamEventKind::Failed { .. })
        })
        .await;

        let record = store.load(&SessionId::from("rig-failure")).await.unwrap();
        let tail = &record.entries()[record.entries().len() - 2..];
        assert!(matches!(
            tail[0].kind,
            TranscriptEntryKind::AssistantMessage {
                ref content,
                ..
            } if content == "partial"
        ));
        assert!(matches!(
            tail[1].kind,
            TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Failed { .. },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn compaction_appends_an_artifact_without_rewriting_canonical_history() {
        let mock = MockCompletionModel::from_stream_turns([
            vec![MockStreamEvent::text("first answer"), final_event()],
            vec![MockStreamEvent::text("second answer"), final_event()],
        ]);
        let model = Arc::new(RigModel::new(mock.clone()).compact_last_messages(1));
        let store = Arc::new(MemoryStore::default());
        let context = InitialContext {
            fragments: vec![artist_core::ContextFragment {
                source: "component://prompt".into(),
                content: "stable prefix".into(),
            }],
        };
        let session = SessionHandle::create(
            SessionId::from("rig-compaction"),
            context.clone(),
            store.clone(),
            model,
        )
        .await
        .unwrap();
        let mut events = session.subscribe();

        session.input(Source::User, "first").await.unwrap();
        wait_for(&mut events, |kind| {
            matches!(kind, StreamEventKind::Completed { .. })
        })
        .await;
        let before = store
            .load(&SessionId::from("rig-compaction"))
            .await
            .unwrap();
        let frozen_bytes = serde_json::to_vec(&before).unwrap();

        session.input(Source::User, "second").await.unwrap();
        wait_for(&mut events, |kind| {
            matches!(kind, StreamEventKind::ContextCompacted { .. })
        })
        .await;
        wait_for(&mut events, |kind| {
            matches!(kind, StreamEventKind::Completed { .. })
        })
        .await;

        let after = store
            .load(&SessionId::from("rig-compaction"))
            .await
            .unwrap();
        assert_eq!(after.initial_context(), &context);
        assert_eq!(&after.entries()[..before.entries().len()], before.entries());
        assert_eq!(serde_json::to_vec(&before).unwrap(), frozen_bytes);
        assert!(
            after
                .entries()
                .iter()
                .any(|entry| matches!(entry.kind, TranscriptEntryKind::Compaction { .. }))
        );
        let requests = mock.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].preamble, requests[1].preamble);
        let system = |request: &rig_core::completion::CompletionRequest| {
            request
                .chat_history
                .iter()
                .find_map(|message| match message {
                    Message::System { content } => Some(content.clone()),
                    _ => None,
                })
        };
        assert_eq!(system(&requests[0]).as_deref(), Some("stable prefix"));
        assert_eq!(system(&requests[1]).as_deref(), Some("stable prefix"));
    }
}
