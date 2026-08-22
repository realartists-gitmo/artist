//! Rig adapter for the Artist streaming kernel.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use artist_core::{
    CallId, CompletionCallMetadata, ContentPart, FailureClass, FinishReason as ArtistFinishReason,
    ModelFailure, TokenUsage,
};
use artist_kernel::{
    ModelError, ModelEvent, ModelHistoryItem, ModelMessage, ModelRequest, ModelStream, Steering,
    StreamingModel,
};
use artist_resource::ToolRegistry;
use futures::StreamExt;
use rig_agent::{
    Agent, AgentBuilder, AgentHook, HookContext,
    agent::{CompletionCallAction, CompletionCallEvent, RequestPatch, StreamingError},
    completion::{Document, PromptError},
    prelude::MultiTurnStreamItem,
    tool::{DynamicTool, ToolExecutionError, ToolOutput},
};
use rig_core::{
    completion::{AssistantContent, CompletionError, CompletionModel, FinishReason, Message},
    message::{
        Image, Reasoning, ToolCall, ToolCallId, ToolFunction, ToolResultContent, UserContent,
    },
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
    tool_concurrency: usize,
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
    pub fn with_registry<M>(model: M, registry: ToolRegistry) -> Self
    where
        M: CompletionModel + 'static,
    {
        let tools = registry
            .definitions()
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
            tool_concurrency: 1,
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

    /// Opt in to parallel execution of a turn's tool calls. The default is
    /// one so side-effecting tools remain serial unless explicitly configured.
    pub fn tool_concurrency(mut self, concurrency: usize) -> Self {
        self.tool_concurrency = concurrency.max(1);
        self
    }
}

impl StreamingModel for RigModel {
    fn stream(&self, request: ModelRequest, steering: Steering) -> ModelStream {
        let agent = self.agent.clone();
        let policy = self.policy.clone();
        let compact = self.compact;
        let tool_concurrency = self.tool_concurrency;
        Box::pin(async_stream::stream! {
            let sequences: Vec<_> = request.history.iter().map(|item| item.sequence).collect();
            let history = match to_rig_history(request.history) {
                Ok(history) => history,
                Err(error) => {
                    yield Err(error);
                    return;
                }
            };
            let (memory_events, mut memory_rx) = tokio::sync::mpsc::unbounded_channel();
            let mut stream = agent
                .runner(Message::user(request.prompt))
                .tool_concurrency(tool_concurrency)
                .preamble(request.context)
                .history(history)
                .add_hook(HistoryHook {
                    policy,
                    compact,
                    session_id: request.session_id.to_string(),
                    sequences,
                    emitted_compaction: Arc::new(AtomicBool::new(false)),
                    events: memory_events,
                })
                .add_hook(SteeringHook(steering))
                .stream()
                .await;

            loop {
                tokio::select! {
                    biased;
                    Some(event) = memory_rx.recv() => yield event,
                    item = stream.next() => match item {
                        Some(Ok(item)) => {
                            for event in translate(item) {
                                yield Ok(event);
                            }
                        }
                        Some(Err(error)) => {
                            // Rig 0.42's AgentRunner emits errors only at
                            // terminal sites (each yield is followed by a
                            // return), so no recoverable stream item remains
                            // to drain after this boundary.
                            yield Err(model_error(&error));
                            return;
                        }
                        None => break,
                    },
                }
            }
            while let Ok(event) = memory_rx.try_recv() {
                yield event;
            }
        })
    }
}

struct HistoryHook {
    policy: Arc<dyn MemoryPolicy>,
    compact: bool,
    session_id: String,
    sequences: Vec<u64>,
    emitted_compaction: Arc<AtomicBool>,
    events: tokio::sync::mpsc::UnboundedSender<Result<ModelEvent, ModelError>>,
}

impl AgentHook for HistoryHook {
    fn on_completion_call(
        &self,
        _ctx: &HookContext,
        event: CompletionCallEvent<'_>,
    ) -> impl Future<Output = CompletionCallAction> + Send {
        let policy = self.policy.clone();
        let compact = self.compact;
        let session_id = self.session_id.clone();
        let sequences = self.sequences.clone();
        let emitted = self.emitted_compaction.clone();
        let events = self.events.clone();
        let history = event.history.to_vec();
        async move {
            let (mut retained, demoted) = match policy.apply_with_demoted(history) {
                Ok(shaped) => shaped,
                Err(error) => return CompletionCallAction::stop(error.to_string()),
            };
            if compact && !demoted.is_empty() {
                let evicted_count = demoted.len();
                let evicted_bytes = serde_json::to_vec(&demoted).map_or(0, |bytes| bytes.len());
                let artifact = match TemplateCompactor::new()
                    .compact(&session_id, &demoted, None)
                    .await
                {
                    Ok(artifact) => artifact,
                    Err(error) => return CompletionCallAction::stop(error.to_string()),
                };
                retained.insert(0, artifact.clone().into());
                if !emitted.swap(true, Ordering::AcqRel) {
                    let through_sequence = sequences
                        .get(evicted_count.saturating_sub(1))
                        .copied()
                        .or_else(|| sequences.last().copied())
                        .unwrap_or(0);
                    let _ = events.send(Ok(ModelEvent::ContextCompacted {
                        through_sequence,
                        evicted_count,
                        evicted_bytes,
                        artifact: artifact.into_string(),
                    }));
                }
            }
            CompletionCallAction::patch(RequestPatch::new().history(retained))
        }
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
            ModelMessage::Assistant(content) => Ok(Message::Assistant {
                id: None,
                content: content
                    .into_iter()
                    .map(to_rig_assistant_content)
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            ModelMessage::Notification(text) => Ok(Message::user(format!("[Notification] {text}"))),
            ModelMessage::ToolCall {
                call_id,
                name,
                arguments,
            } => {
                let arguments = serde_json::from_str(&arguments).map_err(|error| {
                    ModelError::new(format!("invalid stored tool arguments: {error}"))
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
            ModelMessage::ToolResult { call_id, content } => {
                let name = names.get(&call_id).cloned().ok_or_else(|| {
                    ModelError::new(format!(
                        "stored tool result has no matching call: {call_id}"
                    ))
                })?;
                let content = content
                    .into_iter()
                    .map(to_rig_tool_result)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Message::User {
                    content: vec![UserContent::tool_result(call_id.to_string(), name, content)],
                })
            }
        })
        .collect()
}

fn to_rig_assistant_content(part: ContentPart) -> Result<AssistantContent, ModelError> {
    match part {
        ContentPart::Text { text } => Ok(AssistantContent::text(text)),
        ContentPart::Image { value } => serde_json::from_value::<Image>(value)
            .map(AssistantContent::Image)
            .map_err(|error| ModelError::new(format!("invalid stored assistant image: {error}"))),
        ContentPart::Reasoning { value } => serde_json::from_value::<Reasoning>(value)
            .map(AssistantContent::Reasoning)
            .map_err(|error| ModelError::new(format!("invalid stored reasoning: {error}"))),
        ContentPart::Json { .. } | ContentPart::Opaque { .. } => Err(ModelError::new(
            "stored assistant content cannot be represented by Rig 0.42",
        )),
    }
}

fn to_rig_tool_result(part: ContentPart) -> Result<ToolResultContent, ModelError> {
    match part {
        ContentPart::Text { text } => Ok(ToolResultContent::text(text)),
        ContentPart::Json { value } => Ok(ToolResultContent::Json { value }),
        ContentPart::Image { value } => serde_json::from_value::<Image>(value)
            .map(ToolResultContent::Image)
            .map_err(|error| ModelError::new(format!("invalid stored image content: {error}"))),
        ContentPart::Reasoning { value } => Ok(ToolResultContent::Json {
            value: serde_json::json!({ "artist_content_type": "reasoning", "value": value }),
        }),
        ContentPart::Opaque { kind, value } => Ok(ToolResultContent::Json {
            value: serde_json::json!({ "artist_content_type": "opaque", "kind": kind, "value": value }),
        }),
    }
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
            StreamedAssistantContent::Reasoning { reasoning, .. } => {
                vec![ModelEvent::Content(ContentPart::Reasoning {
                    value: serde_json::to_value(reasoning).unwrap_or(serde_json::Value::Null),
                })]
            }
            StreamedAssistantContent::ReasoningDelta {
                id,
                provider_id,
                reasoning,
            } => vec![ModelEvent::Content(ContentPart::Opaque {
                kind: "reasoning_delta".into(),
                value: serde_json::json!({
                    "id": id,
                    "provider_id": provider_id,
                    "text": reasoning,
                }),
            })],
            StreamedAssistantContent::Unknown(payload) => {
                vec![ModelEvent::Content(ContentPart::Opaque {
                    kind: "provider_native".into(),
                    value: payload.value().clone(),
                })]
            }
            StreamedAssistantContent::Final(_) => vec![],
        },
        MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
            tool_result,
            internal_call_id,
        }) => vec![ModelEvent::ToolResult {
            call_id: CallId::new(internal_call_id),
            content: tool_result
                .content
                .into_iter()
                .map(|content| match content {
                    ToolResultContent::Text(text) => ContentPart::Text { text: text.text },
                    ToolResultContent::Json { value } => ContentPart::Json { value },
                    ToolResultContent::Image(image) => ContentPart::Image {
                        value: serde_json::to_value(image).unwrap_or(serde_json::Value::Null),
                    },
                })
                .collect(),
        }],
        MultiTurnStreamItem::ModelTurnRetried { .. } => vec![ModelEvent::TextReset],
        MultiTurnStreamItem::FinalResponse(response) => vec![
            ModelEvent::Usage(TokenUsage {
                input: response.usage.input_tokens,
                output: response.usage.output_tokens,
                cached_input: response.usage.cached_input_tokens,
                reasoning: response.usage.reasoning_tokens,
            }),
            ModelEvent::CompletionMetadata(
                response
                    .completion_calls
                    .into_iter()
                    .map(|call| CompletionCallMetadata {
                        call_index: call.call_index,
                        finish_reason: call.finish_reason.map(finish_reason),
                        message_id: call.message_id,
                        response_id: call.response_id,
                        provider_request_id: call.provider_request_id,
                    })
                    .collect(),
            ),
            ModelEvent::Finished {
                output: Some(response.output),
            },
        ],
        MultiTurnStreamItem::ToolExecutionCommitted {
            tool_call,
            internal_call_id,
        } => vec![ModelEvent::ToolExecutionCommitted {
            call_id: CallId::new(internal_call_id),
            name: tool_call.function.name,
            arguments: tool_call.function.arguments.to_string(),
        }],
        MultiTurnStreamItem::CompletionCall(_) => vec![],
    }
}

fn finish_reason(reason: FinishReason) -> ArtistFinishReason {
    match reason {
        FinishReason::Stop => ArtistFinishReason::Stop,
        FinishReason::Length => ArtistFinishReason::Length,
        FinishReason::ToolCalls => ArtistFinishReason::ToolCalls,
        FinishReason::ContentFilter => ArtistFinishReason::ContentFilter,
        FinishReason::Other(reason) => ArtistFinishReason::Other(reason),
    }
}

fn model_error(error: &StreamingError) -> ModelError {
    let completion = match error {
        StreamingError::Completion(error) => Some(error),
        StreamingError::Prompt(error) => match error.as_ref() {
            PromptError::CompletionError(error) => Some(error),
            _ => None,
        },
    };
    let status = completion
        .and_then(CompletionError::provider_response_status)
        .map(|status| status.as_u16());
    let provider_request_id = match error {
        StreamingError::Completion(error) => error.provider_request_id(),
        StreamingError::Prompt(error) => error.provider_request_id(),
    }
    .map(str::to_owned);
    let provider_code = completion
        .and_then(|error| error.provider_response_json().ok().flatten())
        .and_then(|body| {
            body.pointer("/error/code")
                .or_else(|| body.get("code"))
                .and_then(|code| match code {
                    serde_json::Value::String(code) => Some(code.clone()),
                    serde_json::Value::Number(code) => Some(code.to_string()),
                    _ => None,
                })
        });
    let class = match error {
        StreamingError::Completion(error) => completion_error_class(error),
        StreamingError::Prompt(error) => match error.as_ref() {
            PromptError::CompletionError(error) => completion_error_class(error),
            PromptError::MemoryError(_) => FailureClass::Memory,
            PromptError::MaxTurnsError { .. } => FailureClass::Limit,
            PromptError::PromptCancelled { .. } => FailureClass::Cancelled,
            PromptError::UnknownToolCall { .. } => FailureClass::Tool,
        },
    };
    let retriable = matches!(class, FailureClass::Transport)
        || status.is_some_and(|status| {
            status == 408 || status == 409 || status == 425 || status == 429 || status >= 500
        });
    ModelError(ModelFailure {
        message: error.to_string(),
        class,
        retriable,
        provider_code,
        http_status: status,
        provider_request_id,
    })
}

fn completion_error_class(error: &CompletionError) -> FailureClass {
    match error {
        CompletionError::HttpError(_) => {
            if error.provider_response_status().is_some() {
                FailureClass::Provider
            } else {
                FailureClass::Transport
            }
        }
        CompletionError::JsonError(_) | CompletionError::ResponseError(_) => {
            FailureClass::InvalidResponse
        }
        CompletionError::UrlError(_) | CompletionError::RequestError(_) => {
            FailureClass::InvalidRequest
        }
        CompletionError::ProviderError(_) | CompletionError::ProviderResponse(_) => {
            FailureClass::Provider
        }
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
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
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
                message: ModelMessage::Assistant(vec![ContentPart::text("hi")]),
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
                    content: vec![ContentPart::text("body")],
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
                content: vec![ContentPart::text("body")],
            },
        }];
        assert!(
            to_rig_history(orphan)
                .unwrap_err()
                .0
                .message
                .contains("no matching call")
        );
    }

    #[test]
    fn rich_content_round_trips_through_rig_history_adaptation() {
        let image = Image::default();
        let parts = vec![
            ContentPart::text("literal"),
            ContentPart::Json {
                value: json!({"answer": 42}),
            },
            ContentPart::Image {
                value: serde_json::to_value(&image).unwrap(),
            },
            ContentPart::Opaque {
                kind: "provider_native".into(),
                value: json!({"id": "native"}),
            },
        ];
        let converted = parts
            .clone()
            .into_iter()
            .map(to_rig_tool_result)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(matches!(&converted[0], ToolResultContent::Text(text) if text.text == "literal"));
        assert!(
            matches!(&converted[1], ToolResultContent::Json { value } if value == &json!({"answer": 42}))
        );
        assert!(matches!(&converted[2], ToolResultContent::Image(value) if value == &image));
        assert!(matches!(
            &converted[3],
            ToolResultContent::Json { value }
                if value == &json!({
                    "artist_content_type": "opaque",
                    "kind": "provider_native",
                    "value": {"id": "native"}
                })
        ));

        let reasoning = Reasoning::new_with_signature("thinking", Some("signature".into()));
        let assistant = to_rig_assistant_content(ContentPart::Reasoning {
            value: serde_json::to_value(&reasoning).unwrap(),
        })
        .unwrap();
        assert!(matches!(assistant, AssistantContent::Reasoning(value) if value == reasoning));
    }

    #[test]
    fn reasoning_and_unknown_stream_parts_are_not_discarded() {
        let reasoning = Reasoning::new("thinking");
        assert!(matches!(
            translate(MultiTurnStreamItem::StreamAssistantItem(
                StreamedAssistantContent::Reasoning {
                    reasoning,
                    id: "reasoning-part".into(),
                }
            ))
            .as_slice(),
            [ModelEvent::Content(ContentPart::Reasoning { .. })]
        ));
        assert!(matches!(
            translate(MultiTurnStreamItem::StreamAssistantItem(
                StreamedAssistantContent::Unknown(rig_core::streaming::UnknownPayload::new(
                    json!({"type": "hosted_tool", "id": "native"})
                ))
            )).as_slice(),
            [ModelEvent::Content(ContentPart::Opaque { kind, value })]
                if kind == "provider_native" && value["id"] == "native"
        ));
    }

    #[test]
    fn tool_concurrency_is_serial_by_default_and_explicitly_configurable() {
        let mock = MockCompletionModel::from_stream_turns([vec![MockStreamEvent::FinalResponse(
            mock_final(rig_core::completion::Usage::new()),
        )]]);
        let model = RigModel::new(mock);
        assert_eq!(model.tool_concurrency, 1);
        assert_eq!(model.tool_concurrency(3).tool_concurrency, 3);
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
        assert!(matches!(events[1], ModelEvent::CompletionMetadata(_)));
        assert!(matches!(events[2], ModelEvent::Finished { .. }));
    }

    #[test]
    fn every_normalized_finish_reason_round_trips() {
        assert_eq!(finish_reason(FinishReason::Stop), ArtistFinishReason::Stop);
        assert_eq!(
            finish_reason(FinishReason::Length),
            ArtistFinishReason::Length
        );
        assert_eq!(
            finish_reason(FinishReason::ToolCalls),
            ArtistFinishReason::ToolCalls
        );
        assert_eq!(
            finish_reason(FinishReason::ContentFilter),
            ArtistFinishReason::ContentFilter
        );
        assert_eq!(
            finish_reason(FinishReason::Other("provider_reason".into())),
            ArtistFinishReason::Other("provider_reason".into())
        );
    }

    #[test]
    fn structured_provider_failure_fields_survive_adapter_projection() {
        let response = rig_core::ProviderResponseError::new(
            "429".parse().unwrap(),
            r#"{"error":{"code":"rate_limited","message":"slow down"}}"#,
        )
        .with_provider_request_id(Some("request-7".into()));
        let error = StreamingError::Completion(CompletionError::ProviderResponse(response));
        let failure = model_error(&error).0;

        assert_eq!(failure.class, FailureClass::Provider);
        assert!(failure.retriable);
        assert_eq!(failure.provider_code.as_deref(), Some("rate_limited"));
        assert_eq!(failure.http_status, Some(429));
        assert_eq!(failure.provider_request_id.as_deref(), Some("request-7"));
    }

    #[test]
    fn execution_commits_and_terminal_metadata_are_not_discarded() {
        let call = ToolCall::new(
            ToolCallId::new_or_mint("provider-call"),
            ToolFunction {
                name: "read".into(),
                arguments: json!({"uri":"file:///tmp/a"}),
            },
        );
        assert!(matches!(
            translate(MultiTurnStreamItem::ToolExecutionCommitted {
                tool_call: call,
                internal_call_id: "internal-call".into(),
            })
            .as_slice(),
            [ModelEvent::ToolExecutionCommitted { call_id, name, .. }]
                if call_id.as_str() == "internal-call" && name == "read"
        ));

        let mut response =
            rig_agent::agent::PromptResponse::new("done", rig_core::completion::Usage::new());
        let mut metadata =
            rig_agent::agent::CompletionCall::new(0, rig_core::completion::Usage::new());
        metadata.finish_reason = Some(FinishReason::Other("provider_reason".into()));
        metadata.message_id = Some("message".into());
        metadata.response_id = Some("response".into());
        metadata.provider_request_id = Some("request".into());
        response.completion_calls.push(metadata);
        let events = translate(MultiTurnStreamItem::FinalResponse(response));
        assert!(matches!(
            &events[1],
            ModelEvent::CompletionMetadata(calls)
                if matches!(calls.as_slice(), [CompletionCallMetadata {
                    finish_reason: Some(ArtistFinishReason::Other(reason)),
                    response_id: Some(response_id),
                    provider_request_id: Some(request_id),
                    ..
                }] if reason == "provider_reason" && response_id == "response" && request_id == "request")
        ));
    }

    struct Echo(Arc<AtomicBool>);

    struct CountingPolicy(Arc<AtomicUsize>);

    impl MemoryPolicy for CountingPolicy {
        fn apply(&self, messages: Vec<Message>) -> Result<Vec<Message>, rig_memory::MemoryError> {
            self.0.fetch_add(1, Ordering::AcqRel);
            Ok(messages)
        }
    }

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
        let policy_calls = Arc::new(AtomicUsize::new(0));
        registry
            .register(
                RegistryDefinition {
                    name: "echo".into(),
                    description: "echo JSON".into(),
                    input_schema: json!({"type": "object"}),
                },
                Arc::new(Echo(called.clone())),
            )
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
        .with_policy(CountingPolicy(policy_calls.clone()));
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
        assert_eq!(policy_calls.load(Ordering::Acquire), 2);
        assert!(events.iter().any(|event| matches!(
            event,
            Ok(ModelEvent::ToolResult { content, .. })
                if content.iter().any(|part| match part {
                    ContentPart::Text { text } => text.contains("42"),
                    ContentPart::Json { value } => value.to_string().contains("42"),
                    _ => false,
                })
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
            .unwrap();
        let mock = MockCompletionModel::from_stream_turns([
            vec![
                MockStreamEvent::tool_call("gate-call", "gate", json!({"ok":true})),
                final_event(),
            ],
            vec![MockStreamEvent::text("done"), final_event()],
        ]);
        let model = Arc::new(RigModel::with_registry(mock.clone(), registry));
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
            } if content == &vec![ContentPart::text("done")]
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
            .unwrap();
        let mock = MockCompletionModel::from_stream_turns([vec![
            MockStreamEvent::tool_call("gate-call", "gate", json!({})),
            final_event(),
        ]]);
        let model = Arc::new(RigModel::with_registry(mock, registry));
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
    async fn rig_agent_runner_error_is_terminal_and_preserves_partial_output() {
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
            } if content == &vec![ContentPart::text("partial")]
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
