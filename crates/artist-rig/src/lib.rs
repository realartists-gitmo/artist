//! Rig adapter for the Artist streaming kernel.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use artist_core::{
    Attachment, BlobRef, CallId, CompletionCallMetadata, ContentPart, CorrelationId, FailureClass,
    FinishReason as ArtistFinishReason, InvocationScope, ModelFailure, ProjectionArtifact,
    TokenUsage, ToolControl,
};
use artist_kernel::{
    ModelError, ModelEvent, ModelHistoryItem, ModelMessage, ModelRequest, ModelStream, Steering,
    StreamingModel,
};
use artist_resource::{InvocationContext, ToolProgressSink, ToolRegistry};
use futures::StreamExt;
use rig_agent::{
    AgentBuilder, AgentHook, HookContext, ModelHandle,
    agent::{
        CompletionCallAction, CompletionCallEvent, RequestPatch, StreamingError,
        ToolCall as HookToolCall, ToolCallAction, ToolResultAction, ToolResultEvent,
    },
    completion::PromptError,
    prelude::MultiTurnStreamItem,
    tool::{DynamicTool, ToolExecutionError, ToolOutput},
};
use rig_core::{
    completion::{AssistantContent, CompletionError, CompletionModel, FinishReason, Message},
    message::{
        Audio, Document as RigDocument, DocumentSourceKind, Image, MediaType, MimeType, Reasoning,
        Text, ToolCall, ToolCallId, ToolFunction, ToolResultContent, UserContent, Video,
    },
    streaming::{StreamedAssistantContent, StreamedUserContent},
};
use rig_memory::{
    Compactor, HeuristicTokenCounter, MemoryPolicy, NoopMemoryPolicy, SlidingWindowMemory,
    TemplateCompactor, TokenWindowMemory,
};

#[async_trait::async_trait]
pub trait AttachmentResolver: Send + Sync + 'static {
    async fn read(&self, blob: &BlobRef) -> Result<Vec<u8>, String>;
    async fn write(
        &self,
        bytes: Vec<u8>,
        media_type: String,
        logical_name: Option<String>,
    ) -> Result<BlobRef, String>;
}

#[async_trait::async_trait]
impl<T> AttachmentResolver for T
where
    T: artist_resource::BlobStore + Send + Sync + 'static,
{
    async fn read(&self, blob: &BlobRef) -> Result<Vec<u8>, String> {
        blob.validate()?;
        let id = artist_resource::BlobId::parse(&blob.digest).map_err(|error| error.to_string())?;
        self.get(&id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("blob {} was not found", blob.digest))
    }

    async fn write(
        &self,
        bytes: Vec<u8>,
        media_type: String,
        logical_name: Option<String>,
    ) -> Result<BlobRef, String> {
        self.put_ref(bytes, media_type, logical_name)
            .await
            .map_err(|error| error.to_string())
    }
}

pub struct RigModel {
    model: ModelHandle,
    registry: Option<ToolRegistry>,
    attachments: Option<Arc<dyn AttachmentResolver>>,
    policy: Arc<dyn MemoryPolicy>,
    compact: bool,
    tool_concurrency: usize,
}

impl RigModel {
    pub fn new<M>(model: M) -> Self
    where
        M: CompletionModel + 'static,
    {
        Self::from_model(ModelHandle::new(model), None)
    }

    /// Build a streaming Rig model with every current registry entry exposed
    /// through Rig's runtime-defined tool surface.
    pub fn with_registry<M>(model: M, registry: ToolRegistry) -> Self
    where
        M: CompletionModel + 'static,
    {
        Self::from_model(ModelHandle::new(model), Some(registry))
    }

    fn from_model(model: ModelHandle, registry: Option<ToolRegistry>) -> Self {
        Self {
            model,
            registry,
            attachments: None,
            policy: Arc::new(NoopMemoryPolicy),
            compact: false,
            tool_concurrency: 1,
        }
    }

    pub fn with_attachment_resolver(mut self, resolver: Arc<dyn AttachmentResolver>) -> Self {
        self.attachments = Some(resolver);
        self
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
        let controls = Arc::new(Mutex::new(HashMap::<String, ToolControl>::new()));
        let attachments = self.attachments.clone();
        let (progress_events, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let (tools, has_terminal_control) = self.registry.as_ref().map_or_else(
            || (Vec::new(), false),
            |registry| {
                let definitions = request.profile.as_deref().map_or_else(
                    || registry.definitions(),
                    |profile| registry.definitions_for(profile),
                );
                let has_terminal_control = definitions.iter().any(|definition| {
                    definition
                        .effects
                        .contains(&artist_core::ToolEffect::SessionControl)
                });
                let tools = definitions
                    .into_iter()
                    .map(|definition| {
                        let registry = registry.clone();
                        let name = definition.name.clone();
                        let profile = request.profile.clone();
                        let session_id = request.session_id.clone();
                        let run_id = request.run_id.clone();
                        let extensions = request.extensions.clone();
                        let progress = Arc::new(ProgressEvents(progress_events.clone()));
                        let attachments = attachments.clone();
                        let description =
                            with_metadata_suffix(&definition.description, &definition);
                        DynamicTool::new(
                            definition.name,
                            description,
                            definition.input_schema,
                            move |tool_context, arguments| {
                                let registry = registry.clone();
                                let name = name.clone();
                                let profile = profile.clone();
                                let session_id = session_id.clone();
                                let run_id = run_id.clone();
                                let extensions = extensions.clone();
                                let progress = progress.clone();
                                let attachments = attachments.clone();
                                Box::pin(async move {
                                    let correlation = uuid::Uuid::new_v4();
                                    let invocation = InvocationContext::for_execution(
                                        profile,
                                        InvocationScope {
                                            session_id,
                                            run_id: Some(run_id),
                                            call_id: None,
                                            correlation_id: CorrelationId::new(
                                                correlation.to_string(),
                                            ),
                                            parent_correlation_id: None,
                                        },
                                        extensions,
                                        Some(progress),
                                    );
                                    let output = match registry
                                        .call_output_with_context(&name, arguments, invocation)
                                        .await
                                    {
                                        Ok(output) => output,
                                        // Structured failures reach the model as
                                        // tool-result content with stable code,
                                        // retryability, and hints instead of
                                        // flattening into an opaque terminal error.
                                        Err(error) => {
                                            let (code, message, retriable, failure) = match &error {
                                                artist_resource::ToolError::Failed(failure) => (
                                                    failure.code.clone(),
                                                    failure.message.clone(),
                                                    failure.retriable,
                                                    serde_json::json!({
                                                        "artist_tool_failure": {
                                                            "code": failure.code,
                                                            "message": failure.message,
                                                            "retriable": failure.retriable,
                                                            "details": failure.details,
                                                            "violations": failure.violations,
                                                            "next_actions": failure.next_actions,
                                                        }
                                                    }),
                                                ),
                                                other => (
                                                    "tool-failed".to_owned(),
                                                    other.to_string(),
                                                    false,
                                                    serde_json::json!({
                                                        "artist_tool_failure": {
                                                            "code": "tool-failed",
                                                            "message": other.to_string(),
                                                            "retriable": false,
                                                        }
                                                    }),
                                                ),
                                            };
                                            return Err(ToolExecutionError::other(message)
                                                .with_model_output(
                                                    ToolOutput::content(vec![
                                                        ToolResultContent::Json { value: failure },
                                                    ])
                                                    .expect("failure feedback is non-empty"),
                                                )
                                                .with_retryable(retriable)
                                                .with_code(code));
                                        }
                                    };
                                    if let Some(control) = output.control {
                                        tool_context.insert_result(control);
                                    }
                                    let mut content = Vec::new();
                                    for part in output.content {
                                        content.extend(
                                            to_rig_tool_result(part, attachments.as_deref())
                                                .await
                                                .map_err(ToolExecutionError::from_error)?,
                                        );
                                    }
                                    if !output.next_actions.is_empty() {
                                        content.push(ToolResultContent::Json {
                                            value: serde_json::json!({
                                                "artist_next_actions": output.next_actions,
                                            }),
                                        });
                                    }
                                    ToolOutput::content(content)
                                })
                            },
                        )
                    })
                    .collect();
                (tools, has_terminal_control)
            },
        );
        let agent = if tools.is_empty() {
            AgentBuilder::new(self.model.clone())
                .default_max_turns(32)
                .build()
        } else {
            AgentBuilder::new(self.model.clone())
                .dynamic_tools(tools)
                .default_max_turns(32)
                .build()
        };
        let policy = self.policy.clone();
        let compact = self.compact;
        let tool_concurrency = if has_terminal_control {
            1
        } else {
            self.tool_concurrency
        };
        let model_parameters = request
            .selected_model
            .as_ref()
            .map(|route| route.parameters.clone());
        Box::pin(async_stream::stream! {
            let original_history = request.history.clone();
            let sequences: Vec<_> = original_history.iter().map(|item| item.sequence).collect();
            let history = match to_rig_history(request.history, attachments.as_deref()).await {
                Ok(history) => history,
                Err(error) => {
                    yield Err(error);
                    return;
                }
            };
            let prompt = match to_rig_user_message(request.prompt, attachments.as_deref()).await {
                Ok(prompt) => prompt,
                Err(error) => {
                    yield Err(error);
                    return;
                }
            };
            let (memory_events, mut memory_rx) = tokio::sync::mpsc::unbounded_channel();
            let mut stream = agent
                .runner(prompt)
                .tool_concurrency(tool_concurrency)
                .preamble(request.context)
                .history(history)
                .add_hook(HistoryHook {
                    policy,
                    compact,
                    session_id: request.session_id.to_string(),
                    sequences,
                    original_history,
                    scope: InvocationScope {
                        session_id: request.session_id.clone(),
                        run_id: Some(request.run_id.clone()),
                        call_id: None,
                        correlation_id: CorrelationId::new(format!("{}:compaction", request.run_id)),
                        parent_correlation_id: None,
                    },
                    extensions: request.extensions.clone(),
                    attachments: attachments.clone(),
                    emitted_compaction: Arc::new(AtomicBool::new(false)),
                    events: memory_events,
                })
                .add_hook(TerminalControlHook(controls.clone()))
                .add_hook(ModelParametersHook(model_parameters))
                .add_hook(SteeringHook {
                    steering,
                    attachments: attachments.clone(),
                })
                .stream()
                .await;

            loop {
                tokio::select! {
                    biased;
                    Some(event) = memory_rx.recv() => yield event,
                    Some(progress) = progress_rx.recv() => yield Ok(ModelEvent::ToolProgress(progress)),
                    item = stream.next() => match item {
                        Some(Ok(item)) => {
                            let translated = match translate(item, attachments.as_deref()).await {
                                Ok(events) => events,
                                Err(error) => {
                                    yield Err(error);
                                    return;
                                }
                            };
                            for event in translated {
                                let terminal = match &event {
                                    ModelEvent::ToolResult { call_id, .. } => controls
                                        .lock()
                                        .expect("terminal control lock poisoned")
                                        .remove(&call_id.to_string())
                                        .map(|control| (call_id.clone(), control)),
                                    _ => None,
                                };
                                yield Ok(event);
                                if let Some((call_id, control)) = terminal {
                                    yield Ok(ModelEvent::Control { call_id, control });
                                    return;
                                }
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
            while let Ok(progress) = progress_rx.try_recv() {
                yield Ok(ModelEvent::ToolProgress(progress));
            }
        })
    }
}

struct ProgressEvents(tokio::sync::mpsc::UnboundedSender<artist_core::ToolProgress>);

impl ToolProgressSink for ProgressEvents {
    fn emit(&self, progress: artist_core::ToolProgress) {
        let _ = self.0.send(progress);
    }
}

struct HistoryHook {
    policy: Arc<dyn MemoryPolicy>,
    compact: bool,
    session_id: String,
    sequences: Vec<u64>,
    original_history: Vec<ModelHistoryItem>,
    scope: InvocationScope,
    extensions: Arc<dyn artist_kernel::ExecutionExtensions>,
    attachments: Option<Arc<dyn AttachmentResolver>>,
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
        let original_history = self.original_history.clone();
        let scope = self.scope.clone();
        let extensions = self.extensions.clone();
        let attachments = self.attachments.clone();
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
                let through_sequence = sequences
                    .get(evicted_count.saturating_sub(1))
                    .copied()
                    .or_else(|| sequences.last().copied())
                    .unwrap_or(0);
                let source = original_history
                    .iter()
                    .filter(|item| item.sequence <= through_sequence)
                    .cloned()
                    .collect::<Vec<_>>();
                let custom = match extensions.compact_context(&scope, &source).await {
                    Ok(custom) => custom,
                    Err(error) => return CompletionCallAction::stop(error.to_string()),
                };
                let artifact = if let Some(artifact) = custom {
                    if artifact.earliest_changed_sequence > through_sequence
                        || artifact.projection_digest.len() != 64
                        || artifact.content.is_empty()
                    {
                        return CompletionCallAction::stop(
                            "context plugin returned an invalid projection artifact",
                        );
                    }
                    let message =
                        match to_rig_user_message(artifact.content.clone(), attachments.as_deref())
                            .await
                        {
                            Ok(message) => message,
                            Err(error) => return CompletionCallAction::stop(error.to_string()),
                        };
                    retained.insert(0, message);
                    artifact
                } else {
                    let summary = match TemplateCompactor::new()
                        .compact(&session_id, &demoted, None)
                        .await
                    {
                        Ok(artifact) => artifact,
                        Err(error) => return CompletionCallAction::stop(error.to_string()),
                    };
                    retained.insert(0, summary.clone().into());
                    ProjectionArtifact::text(summary.into_string(), through_sequence)
                };
                if !emitted.swap(true, Ordering::AcqRel) {
                    let _ = events.send(Ok(ModelEvent::ContextCompacted {
                        through_sequence,
                        evicted_count,
                        evicted_bytes,
                        artifact,
                    }));
                }
            }
            CompletionCallAction::patch(RequestPatch::new().history(retained))
        }
    }
}

struct SteeringHook {
    steering: Steering,
    attachments: Option<Arc<dyn AttachmentResolver>>,
}

impl AgentHook for SteeringHook {
    fn on_completion_call(
        &self,
        _ctx: &HookContext,
        _event: CompletionCallEvent<'_>,
    ) -> impl Future<Output = CompletionCallAction> + Send {
        let steering = self.steering.clone();
        let attachments = self.attachments.clone();
        let mut history = _event.history.to_vec();
        async move {
            let notices = steering.take().await;
            if notices.is_empty() {
                return CompletionCallAction::Continue;
            }
            for notice in notices {
                let mut content = vec![ContentPart::text(format!(
                    "[Notification from {:?}]",
                    notice.source
                ))];
                content.extend(notice.content);
                match to_rig_user_message(content, attachments.as_deref()).await {
                    Ok(message) => history.push(message),
                    Err(error) => return CompletionCallAction::stop(error.to_string()),
                }
            }
            CompletionCallAction::patch(RequestPatch::new().history(history))
        }
    }
}

struct TerminalControlHook(Arc<Mutex<HashMap<String, ToolControl>>>);

impl AgentHook for TerminalControlHook {
    fn on_tool_call(
        &self,
        _ctx: &HookContext,
        _event: HookToolCall<'_>,
    ) -> impl Future<Output = ToolCallAction> + Send {
        let terminal = !self
            .0
            .lock()
            .expect("terminal control lock poisoned")
            .is_empty();
        async move {
            if terminal {
                ToolCallAction::skip("a terminal control tool already completed")
            } else {
                ToolCallAction::Run
            }
        }
    }

    fn on_tool_result(
        &self,
        _ctx: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> impl Future<Output = ToolResultAction> + Send {
        if let Some(control) = event.tool_context.result::<ToolControl>() {
            self.0
                .lock()
                .expect("terminal control lock poisoned")
                .insert(event.internal_call_id.to_owned(), control.clone());
        }
        async { ToolResultAction::Keep }
    }
}

struct ModelParametersHook(Option<serde_json::Value>);

impl AgentHook for ModelParametersHook {
    fn on_completion_call(
        &self,
        _ctx: &HookContext,
        _event: CompletionCallEvent<'_>,
    ) -> impl Future<Output = CompletionCallAction> + Send {
        let parameters = self.0.clone();
        async move {
            parameters.map_or(CompletionCallAction::Continue, |parameters| {
                CompletionCallAction::patch(RequestPatch::new().additional_params(parameters))
            })
        }
    }
}

async fn to_rig_history(
    history: Vec<ModelHistoryItem>,
    resolver: Option<&dyn AttachmentResolver>,
) -> Result<Vec<Message>, ModelError> {
    let mut names = HashMap::<CallId, String>::new();
    let mut converted = Vec::with_capacity(history.len());
    for item in history {
        let message = match item.message {
            ModelMessage::User(content) => to_rig_user_message(content, resolver).await?,
            ModelMessage::Assistant(content) => {
                let mut converted = Vec::with_capacity(content.len());
                for part in content {
                    converted.push(to_rig_assistant_content(part, resolver).await?);
                }
                Message::Assistant {
                    id: None,
                    content: converted,
                }
            }
            ModelMessage::Notification(mut content) => {
                content.insert(0, ContentPart::text("[Notification]"));
                to_rig_user_message(content, resolver).await?
            }
            ModelMessage::ToolCall {
                call_id,
                name,
                arguments,
            } => {
                let arguments = serde_json::from_str(&arguments).map_err(|error| {
                    ModelError::new(format!("invalid stored tool arguments: {error}"))
                })?;
                names.insert(call_id.clone(), name.clone());
                Message::Assistant {
                    id: None,
                    content: vec![AssistantContent::ToolCall(ToolCall::new(
                        ToolCallId::new_or_mint(call_id.to_string()),
                        ToolFunction { name, arguments },
                    ))],
                }
            }
            ModelMessage::ToolResult { call_id, content } => {
                let name = names.get(&call_id).cloned().ok_or_else(|| {
                    ModelError::new(format!(
                        "stored tool result has no matching call: {call_id}"
                    ))
                })?;
                let mut converted = Vec::new();
                for part in content {
                    converted.extend(to_rig_tool_result(part, resolver).await?);
                }
                Message::User {
                    content: vec![UserContent::tool_result(
                        call_id.to_string(),
                        name,
                        converted,
                    )],
                }
            }
        };
        converted.push(message);
    }
    Ok(converted)
}

async fn to_rig_user_message(
    content: Vec<ContentPart>,
    resolver: Option<&dyn AttachmentResolver>,
) -> Result<Message, ModelError> {
    let mut converted = Vec::new();
    for part in content {
        match part {
            ContentPart::Text { text } => converted.push(UserContent::Text(Text::new(text))),
            ContentPart::Json { value } => converted.push(UserContent::Text(Text::new(
                serde_json::to_string(&value)
                    .map_err(|error| ModelError::new(error.to_string()))?,
            ))),
            ContentPart::Reasoning { value } => converted.push(UserContent::Text(Text::new(
                serde_json::json!({"artist_content_type": "reasoning", "value": value}).to_string(),
            ))),
            ContentPart::Opaque { kind, value } => converted.push(UserContent::Text(Text::new(
                serde_json::json!({"artist_content_type": "opaque", "kind": kind, "value": value})
                    .to_string(),
            ))),
            ContentPart::Attachment { attachment } => {
                if let Some(alternate) = attachment.alternate_text.as_ref() {
                    converted.push(UserContent::Text(Text::new(alternate.clone())));
                }
                converted.push(to_rig_user_attachment(&attachment, resolver).await?);
            }
        }
    }
    if converted.is_empty() {
        return Err(ModelError::new("model user content must not be empty"));
    }
    Ok(Message::User { content: converted })
}

async fn attachment_bytes(
    attachment: &Attachment,
    resolver: Option<&dyn AttachmentResolver>,
) -> Result<Vec<u8>, ModelError> {
    let resolver = resolver.ok_or_else(|| {
        ModelError::new(format!(
            "attachment {} requires a configured blob resolver",
            attachment.blob.digest
        ))
    })?;
    let bytes = resolver
        .read(&attachment.blob)
        .await
        .map_err(ModelError::new)?;
    if bytes.len() as u64 != attachment.blob.byte_length {
        return Err(ModelError::new(format!(
            "attachment {} length does not match its canonical reference",
            attachment.blob.digest
        )));
    }
    Ok(bytes)
}

async fn to_rig_user_attachment(
    attachment: &Attachment,
    resolver: Option<&dyn AttachmentResolver>,
) -> Result<UserContent, ModelError> {
    let bytes = attachment_bytes(attachment, resolver).await?;
    let media = MediaType::from_mime_type(&attachment.blob.media_type).ok_or_else(|| {
        ModelError::new(format!(
            "Rig 0.42 cannot represent attachment media type {}",
            attachment.blob.media_type
        ))
    })?;
    Ok(match media {
        MediaType::Image(media_type) => UserContent::Image(Image {
            data: DocumentSourceKind::Raw(bytes),
            media_type: Some(media_type),
            detail: None,
            additional_params: None,
        }),
        MediaType::Audio(media_type) => UserContent::Audio(Audio {
            data: DocumentSourceKind::Raw(bytes),
            media_type: Some(media_type),
            additional_params: None,
        }),
        MediaType::Video(media_type) => UserContent::Video(Video {
            data: DocumentSourceKind::Raw(bytes),
            media_type: Some(media_type),
            additional_params: None,
        }),
        MediaType::Document(media_type) => UserContent::Document(RigDocument {
            data: DocumentSourceKind::Raw(bytes),
            media_type: Some(media_type),
            additional_params: None,
        }),
    })
}

async fn to_rig_assistant_content(
    part: ContentPart,
    resolver: Option<&dyn AttachmentResolver>,
) -> Result<AssistantContent, ModelError> {
    match part {
        ContentPart::Text { text } => Ok(AssistantContent::text(text)),
        ContentPart::Attachment { attachment } => {
            let bytes = attachment_bytes(&attachment, resolver).await?;
            let MediaType::Image(media_type) =
                MediaType::from_mime_type(&attachment.blob.media_type).ok_or_else(|| {
                    ModelError::new("assistant attachment has an unsupported media type")
                })?
            else {
                return Err(ModelError::new(
                    "Rig 0.42 assistant history supports image attachments only",
                ));
            };
            Ok(AssistantContent::Image(Image {
                data: DocumentSourceKind::Raw(bytes),
                media_type: Some(media_type),
                detail: None,
                additional_params: None,
            }))
        }
        ContentPart::Reasoning { value } => serde_json::from_value::<Reasoning>(value)
            .map(AssistantContent::Reasoning)
            .map_err(|error| ModelError::new(format!("invalid stored reasoning: {error}"))),
        ContentPart::Json { .. } | ContentPart::Opaque { .. } => Err(ModelError::new(
            "stored assistant content cannot be represented by Rig 0.42",
        )),
    }
}

/// Rig's dynamic-tool surface carries only name/description/input schema, so
/// the rest of the registry contract rides along in a deterministic,
/// model-visible suffix. This keeps annotations, category, and output-schema
/// identity intact across the WIT -> registry -> Rig hop.
fn with_metadata_suffix(description: &str, definition: &artist_resource::ToolDefinition) -> String {
    let output_digest = artist_resource::sha256(
        serde_json::to_string(&definition.output_schema)
            .unwrap_or_default()
            .as_bytes(),
    );
    let a = &definition.annotations;
    format!(
        "{description}\n\n[artist category={} read-only={} destructive={} idempotent={} open-world={} output-schema={}]",
        definition.category,
        a.read_only,
        a.destructive,
        a.idempotent,
        a.open_world,
        &output_digest[..12]
    )
}

async fn to_rig_tool_result(
    part: ContentPart,
    resolver: Option<&dyn AttachmentResolver>,
) -> Result<Vec<ToolResultContent>, ModelError> {
    Ok(match part {
        ContentPart::Text { text } => vec![ToolResultContent::text(text)],
        ContentPart::Json { value } => vec![ToolResultContent::Json { value }],
        ContentPart::Attachment { attachment } => {
            let bytes = attachment_bytes(&attachment, resolver).await?;
            let MediaType::Image(media_type) =
                MediaType::from_mime_type(&attachment.blob.media_type).ok_or_else(|| {
                    ModelError::new("tool-result attachment has an unsupported media type")
                })?
            else {
                return Err(ModelError::new(
                    "Rig 0.42 tool-result history supports image attachments only",
                ));
            };
            let mut content = Vec::new();
            if let Some(alternate) = attachment.alternate_text {
                content.push(ToolResultContent::text(alternate));
            }
            content.push(ToolResultContent::Image(Image {
                data: DocumentSourceKind::Raw(bytes),
                media_type: Some(media_type),
                detail: None,
                additional_params: None,
            }));
            content
        }
        ContentPart::Reasoning { value } => vec![ToolResultContent::Json {
            value: serde_json::json!({ "artist_content_type": "reasoning", "value": value }),
        }],
        ContentPart::Opaque { kind, value } => vec![ToolResultContent::Json {
            value: serde_json::json!({ "artist_content_type": "opaque", "kind": kind, "value": value }),
        }],
    })
}

async fn translate(
    item: MultiTurnStreamItem,
    attachments: Option<&dyn AttachmentResolver>,
) -> Result<Vec<ModelEvent>, ModelError> {
    Ok(match item {
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
        }) => {
            let mut content = Vec::with_capacity(tool_result.content.len());
            for item in tool_result.content {
                match item {
                    ToolResultContent::Text(text) => {
                        content.push(ContentPart::Text { text: text.text });
                    }
                    ToolResultContent::Json { value } => {
                        content.push(ContentPart::Json { value });
                    }
                    ToolResultContent::Image(image) => {
                        content.push(canonicalize_rig_image(image, attachments).await?);
                    }
                }
            }
            vec![ModelEvent::ToolResult {
                call_id: CallId::new(internal_call_id),
                content,
            }]
        }
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
    })
}

async fn canonicalize_rig_image(
    image: Image,
    attachments: Option<&dyn AttachmentResolver>,
) -> Result<ContentPart, ModelError> {
    use base64::Engine as _;
    let media_type = image
        .media_type
        .as_ref()
        .map(MimeType::to_mime_type)
        .unwrap_or("application/octet-stream")
        .to_owned();
    let bytes = match image.data {
        DocumentSourceKind::Raw(bytes) => bytes,
        DocumentSourceKind::Base64(encoded) => base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| ModelError::new(format!("invalid provider image base64: {error}")))?,
        DocumentSourceKind::Url(_)
        | DocumentSourceKind::FileId(_)
        | DocumentSourceKind::String(_)
        | DocumentSourceKind::Unknown => {
            return Err(ModelError::new(
                "provider image cannot enter canonical history without materialized bytes",
            ));
        }
    };
    let resolver = attachments.ok_or_else(|| {
        ModelError::new("provider image requires a configured canonical attachment repository")
    })?;
    let blob = resolver
        .write(bytes, media_type, None)
        .await
        .map_err(ModelError::new)?;
    Ok(ContentPart::attachment(blob, "artist.tool-result"))
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
        InitialContext, InterruptionCause, ProfilePolicy, ProfileSnapshot, RunId, RunOutcome,
        SessionId, Source, StreamEvent, StreamEventKind, ToolEffect, TranscriptEntryKind,
        default_yield_schema,
    };
    use artist_kernel::{
        CreateSession, ProfileSource, SessionDependencies, SessionHandle, no_extensions,
    };
    use artist_resource::{
        BlobStore, InvocationContext, MemoryBlobStore, ToolDefinition as RegistryDefinition,
        ToolError, ToolHandler,
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

    #[tokio::test]
    async fn text_history_converts_without_loss() {
        let history = vec![
            ModelHistoryItem {
                sequence: 0,
                message: ModelMessage::User(vec![ContentPart::text("hello")]),
            },
            ModelHistoryItem {
                sequence: 1,
                message: ModelMessage::Assistant(vec![ContentPart::text("hi")]),
            },
        ];
        assert_eq!(
            to_rig_history(history, None).await.unwrap(),
            vec![Message::user("hello"), Message::assistant("hi")]
        );
    }

    #[tokio::test]
    async fn tool_history_preserves_call_result_pairing() {
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
        let converted = to_rig_history(history, None).await.unwrap();
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
            to_rig_history(orphan, None)
                .await
                .unwrap_err()
                .0
                .message
                .contains("no matching call")
        );
    }

    #[tokio::test]
    async fn rich_content_round_trips_through_rig_history_adaptation() {
        let store = MemoryBlobStore::default();
        let bytes = vec![137, 80, 78, 71, 1, 2, 3];
        let blob = store
            .put_ref(
                bytes.clone(),
                "image/png".into(),
                Some("fixture.png".into()),
            )
            .await
            .unwrap();
        let parts = vec![
            ContentPart::text("literal"),
            ContentPart::Json {
                value: json!({"answer": 42}),
            },
            ContentPart::attachment(blob, "artist.input"),
            ContentPart::Opaque {
                kind: "provider_native".into(),
                value: json!({"id": "native"}),
            },
        ];
        let mut converted = Vec::new();
        for part in parts {
            converted.extend(to_rig_tool_result(part, Some(&store)).await.unwrap());
        }
        assert!(matches!(&converted[0], ToolResultContent::Text(text) if text.text == "literal"));
        assert!(
            matches!(&converted[1], ToolResultContent::Json { value } if *value == json!({"answer": 42}))
        );
        assert!(matches!(&converted[2], ToolResultContent::Image(value)
            if matches!(&value.data, DocumentSourceKind::Raw(actual) if actual == &bytes)));
        assert!(matches!(&converted[3], ToolResultContent::Json { value }
            if *value == json!({"artist_content_type":"opaque","kind":"provider_native","value":{"id":"native"}})));

        let reasoning = Reasoning::new_with_signature("thinking", Some("signature".into()));
        let assistant = to_rig_assistant_content(
            ContentPart::Reasoning {
                value: serde_json::to_value(&reasoning).unwrap(),
            },
            None,
        )
        .await
        .unwrap();
        assert!(matches!(assistant, AssistantContent::Reasoning(value) if value == reasoning));
    }

    #[tokio::test]
    async fn reasoning_and_unknown_stream_parts_are_not_discarded() {
        let reasoning = Reasoning::new("thinking");
        assert!(matches!(
            translate(
                MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Reasoning {
                    reasoning,
                    id: "reasoning-part".into(),
                }),
                None
            )
            .await
            .unwrap()
            .as_slice(),
            [ModelEvent::Content(ContentPart::Reasoning { .. })]
        ));
        assert!(matches!(
            translate(MultiTurnStreamItem::StreamAssistantItem(
                StreamedAssistantContent::Unknown(rig_core::streaming::UnknownPayload::new(
                    json!({"type": "hosted_tool", "id": "native"})
                ))
            ), None).await.unwrap().as_slice(),
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

    #[tokio::test]
    async fn usage_is_emitted_before_completion() {
        let mut usage = rig_core::completion::Usage::new();
        usage.input_tokens = 10;
        usage.output_tokens = 4;
        usage.cached_input_tokens = 3;
        usage.reasoning_tokens = 2;
        let item = MultiTurnStreamItem::final_response(vec![AssistantContent::text("done")], usage);
        let events = translate(item, None).await.unwrap();
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

    #[tokio::test]
    async fn execution_commits_and_terminal_metadata_are_not_discarded() {
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
            }, None).await.unwrap().as_slice(),
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
        let events = translate(MultiTurnStreamItem::FinalResponse(response), None)
            .await
            .unwrap();
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

    struct TestProfiles;

    #[async_trait]
    impl ProfileSource for TestProfiles {
        async fn load(&self, name: &str) -> Result<ProfileSnapshot, String> {
            Ok(ProfileSnapshot {
                name: name.into(),
                instructions: String::new(),
                yield_schema: default_yield_schema(),
                policy: ProfilePolicy::default(),
                models: Vec::new(),
                catalog: vec![name.into()],
            })
        }
    }

    impl MemoryPolicy for CountingPolicy {
        fn apply(&self, messages: Vec<Message>) -> Result<Vec<Message>, rig_memory::MemoryError> {
            self.0.fetch_add(1, Ordering::AcqRel);
            Ok(messages)
        }
    }

    #[async_trait]
    impl ToolHandler for Echo {
        async fn call(
            &self,
            arguments: Value,
            _: InvocationContext,
        ) -> Result<artist_resource::ToolOutput, ToolError> {
            self.0.store(true, Ordering::Release);
            Ok(artist_resource::ToolOutput::value(arguments))
        }
    }

    struct Gate {
        started: Arc<Semaphore>,
        release: Arc<Semaphore>,
    }

    struct TerminalYield;

    #[async_trait]
    impl ToolHandler for TerminalYield {
        async fn call(
            &self,
            arguments: Value,
            _: InvocationContext,
        ) -> Result<artist_resource::ToolOutput, ToolError> {
            Ok(artist_resource::ToolOutput {
                value: arguments.clone(),
                content: vec![ContentPart::Json {
                    value: arguments.clone(),
                }],
                control: Some(ToolControl::Yield { payload: arguments }),
                next_actions: Vec::new(),
            })
        }
    }

    #[async_trait]
    impl ToolHandler for Gate {
        async fn call(
            &self,
            arguments: Value,
            context: InvocationContext,
        ) -> Result<artist_resource::ToolOutput, ToolError> {
            if let (Some(progress), Some(scope)) = (context.progress, context.scope) {
                progress.emit(artist_core::ToolProgress {
                    scope,
                    sequence: 0,
                    fraction: Some(0.5),
                    message: Some("gate waiting".into()),
                    detail: json!({"phase": "blocked"}),
                });
            }
            self.started.add_permits(1);
            self.release
                .acquire()
                .await
                .map_err(|_| ToolError::failed("gate_closed", "gate closed"))?
                .forget();
            Ok(artist_resource::ToolOutput::value(arguments))
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
                    effects: vec![ToolEffect::Observe],
                    category: "test".into(),
                    output_schema: json!({"type": "object"}),
                    annotations: artist_resource::ToolAnnotations {
                        read_only: true,
                        destructive: false,
                        idempotent: true,
                        open_world: false,
                    },
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
                    prompt: vec![ContentPart::text("echo")],
                    history: Vec::new(),
                    extensions: no_extensions(),
                    profile: None,
                    profile_epoch: None,
                    selected_model: None,
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

    /// Gate proof: binary content is stored once by digest, referenced at its
    /// exact canonical positions (user input AND tool result), and resolved
    /// to provider-native raw image parts only while constructing requests.

    #[tokio::test]
    async fn tool_metadata_rides_through_the_rig_hop() {
        let registry = ToolRegistry::new();
        registry
            .register(
                RegistryDefinition {
                    name: "probe".into(),
                    description: "probe things".into(),
                    input_schema: json!({"type": "object"}),
                    effects: vec![ToolEffect::Observe],
                    category: "inspection".into(),
                    output_schema: json!({"type": "object", "required": ["ok"]}),
                    annotations: artist_resource::ToolAnnotations {
                        read_only: true,
                        destructive: false,
                        idempotent: true,
                        open_world: false,
                    },
                },
                Arc::new(Echo(Arc::new(AtomicBool::new(false)))),
            )
            .unwrap();
        let mock = MockCompletionModel::from_stream_turns([vec![
            MockStreamEvent::text("done"),
            final_event(),
        ]]);
        let model = RigModel::with_registry(mock.clone(), registry)
            .with_policy(CountingPolicy(Default::default()));
        let events = model
            .stream(
                ModelRequest {
                    session_id: SessionId::from("session"),
                    run_id: RunId::from("run"),
                    context: String::new(),
                    prompt: vec![ContentPart::text("go")],
                    history: Vec::new(),
                    extensions: no_extensions(),
                    profile: None,
                    profile_epoch: None,
                    selected_model: None,
                },
                Steering::empty(),
            )
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        let requests = mock.requests();
        let tools = &requests[0].tools;
        assert_eq!(tools.len(), 1);
        let description = &tools[0].description;
        assert!(description.starts_with("probe things"), "{description}");
        assert!(
            description.contains("[artist category=inspection"),
            "{description}"
        );
        assert!(description.contains("read-only=true"), "{description}");
        assert!(description.contains("destructive=false"), "{description}");
        assert!(description.contains("idempotent=true"), "{description}");
        assert!(description.contains("open-world=false"), "{description}");
        // The output-schema digest is deterministic for the same schema.
        let expected_digest = artist_resource::sha256(
            serde_json::to_string(&json!({"type": "object", "required": ["ok"]}))
                .unwrap()
                .as_bytes(),
        );
        assert!(
            description.contains(&format!("output-schema={}", &expected_digest[..12])),
            "{description}"
        );
    }

    #[tokio::test]
    async fn binary_content_stored_once_flows_through_tools_and_history() {
        use artist_resource::{BlobStore, MemoryBlobStore};

        let blobs = Arc::new(MemoryBlobStore::default());
        let png = vec![0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 7, 7, 7];
        let reference = blobs
            .put_ref(png.clone(), "image/png".into(), Some("dot.png".into()))
            .await
            .unwrap();
        // Storing the identical bytes again deduplicates to one physical blob.
        let duplicate = blobs
            .put_ref(png.clone(), "image/png".into(), None)
            .await
            .unwrap();
        assert_eq!(reference.digest, duplicate.digest);
        assert_eq!(blobs.list().await.unwrap().len(), 1);

        struct EmitAttachment {
            blob: artist_core::BlobRef,
        }
        #[async_trait]
        impl artist_resource::ToolHandler for EmitAttachment {
            async fn call(
                &self,
                _: serde_json::Value,
                _: artist_resource::InvocationContext,
            ) -> Result<artist_resource::ToolOutput, artist_resource::ToolError> {
                Ok(artist_resource::ToolOutput {
                    value: serde_json::json!({"attached": true}),
                    content: vec![ContentPart::Attachment {
                        attachment: artist_core::Attachment {
                            blob: self.blob.clone(),
                            role: "artist.tool-result".into(),
                            alternate_text: Some("a tiny png".into()),
                            metadata: Default::default(),
                        },
                    }],
                    control: None,
                    next_actions: Vec::new(),
                })
            }
        }

        let registry = ToolRegistry::new();
        registry
            .register(
                RegistryDefinition {
                    name: "attach".into(),
                    description: "return the screenshot".into(),
                    input_schema: json!({"type": "object"}),
                    effects: vec![ToolEffect::Observe],
                    category: "test".into(),
                    output_schema: json!({"type": "object"}),
                    annotations: artist_resource::ToolAnnotations {
                        read_only: true,
                        destructive: false,
                        idempotent: true,
                        open_world: false,
                    },
                },
                Arc::new(EmitAttachment {
                    blob: reference.clone(),
                }),
            )
            .unwrap();

        let mock = MockCompletionModel::from_stream_turns([
            vec![
                MockStreamEvent::tool_call("call-1", "attach", json!({})),
                final_event(),
            ],
            vec![MockStreamEvent::text("done"), final_event()],
        ]);
        let model = RigModel::with_registry(mock.clone(), registry)
            .with_attachment_resolver(blobs as Arc<dyn AttachmentResolver>);

        // The user prompt itself references the SAME digest.
        let events = model
            .stream(
                ModelRequest {
                    session_id: SessionId::from("session"),
                    run_id: RunId::from("run"),
                    context: String::new(),
                    prompt: vec![
                        ContentPart::text("look"),
                        ContentPart::Attachment {
                            attachment: artist_core::Attachment {
                                blob: reference.clone(),
                                role: "artist.input".into(),
                                alternate_text: None,
                                metadata: Default::default(),
                            },
                        },
                    ],
                    history: Vec::new(),
                    extensions: no_extensions(),
                    profile: None,
                    profile_epoch: None,
                    selected_model: None,
                },
                Steering::empty(),
            )
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");

        // The tool result stream event carries the blob reference, not bytes.
        assert!(events.iter().any(|event| matches!(
            event,
            Ok(ModelEvent::ToolResult { content, .. })
                if content.iter().any(|part| matches!(
                    part,
                    ContentPart::Attachment { attachment }
                        if attachment.blob.digest == reference.digest
                ))
        )));

        // Provider request 2 resolves BOTH attachments to raw image parts.
        let requests = mock.requests();
        assert_eq!(requests.len(), 2);
        let serialized = serde_json::to_string(&requests[1]).unwrap();
        // Both attachments resolve to provider-native raw image parts holding
        // the exact stored bytes: input image AND tool-result image.
        let png_array =
            format!("{:?}", png.clone().into_iter().collect::<Vec<u8>>()).replace(" ", "");
        assert_eq!(
            serialized.matches(&png_array).count(),
            2,
            "expected input+tool raw images, got {serialized}"
        );
        // Alternate text rides along as an adjacent text part (it is not a
        // field on the provider image part).
        assert!(!serialized.contains("alternate_text"));
        assert!(serialized.contains("a tiny png"));
    }

    #[tokio::test]
    async fn tool_failures_reach_the_model_with_structured_feedback() {
        struct Failing;
        #[async_trait]
        impl artist_resource::ToolHandler for Failing {
            async fn call(
                &self,
                _arguments: serde_json::Value,
                _: artist_resource::InvocationContext,
            ) -> Result<artist_resource::ToolOutput, artist_resource::ToolError> {
                Err(artist_resource::ToolError::Failed(Box::new(
                    artist_resource::ToolFailure {
                        code: "disk-full".into(),
                        message: "cannot write".into(),
                        retriable: true,
                        details: serde_json::json!({"path": "/tmp/x"}),
                        violations: Vec::new(),
                        next_actions: vec![artist_resource::NextActionHint {
                            kind: "retry".into(),
                            label: "retry".into(),
                            payload: serde_json::json!({"tool": "write"}),
                        }],
                    },
                )))
            }
        }
        let registry = ToolRegistry::new();
        registry
            .register(
                RegistryDefinition {
                    name: "boom".into(),
                    description: "always fails".into(),
                    input_schema: json!({"type": "object"}),
                    effects: vec![ToolEffect::Mutate],
                    category: "test".into(),
                    output_schema: json!({"type": "object"}),
                    annotations: artist_resource::ToolAnnotations {
                        read_only: false,
                        destructive: false,
                        idempotent: false,
                        open_world: false,
                    },
                },
                Arc::new(Failing),
            )
            .unwrap();
        let mock = MockCompletionModel::from_stream_turns([
            vec![
                MockStreamEvent::tool_call("call-1", "boom", json!({})),
                final_event(),
            ],
            vec![MockStreamEvent::text("recovered"), final_event()],
        ]);
        let model = Arc::new(
            RigModel::with_registry(mock.clone(), registry)
                .with_policy(CountingPolicy(Default::default())),
        );
        let events = model
            .stream(
                ModelRequest {
                    session_id: SessionId::from("session"),
                    run_id: RunId::from("run"),
                    context: String::new(),
                    prompt: vec![ContentPart::text("boom")],
                    history: Vec::new(),
                    extensions: no_extensions(),
                    profile: None,
                    profile_epoch: None,
                    selected_model: None,
                },
                Steering::empty(),
            )
            .collect::<Vec<_>>()
            .await;
        // The run completes; the failure did not abort it.
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Ok(ModelEvent::Finished { .. })))
        );
        let requests = mock.requests();
        assert_eq!(requests.len(), 2);
        let serialized = serde_json::to_string(&requests[1]).unwrap();
        assert!(serialized.contains("artist_tool_failure"), "{serialized}");
        assert!(serialized.contains("disk-full"));
        assert!(serialized.contains("\"retriable\":true"));
    }

    #[tokio::test]
    async fn terminal_control_crosses_rig_and_stops_later_tool_calls() {
        let registry = ToolRegistry::new();
        let later_called = Arc::new(AtomicBool::new(false));
        registry
            .register(
                RegistryDefinition {
                    name: "yield".into(),
                    description: "return structured state".into(),
                    input_schema: artist_core::default_yield_schema(),
                    effects: vec![ToolEffect::SessionControl],
                    category: "test".into(),
                    output_schema: json!({"type": "object"}),
                    annotations: artist_resource::ToolAnnotations {
                        read_only: false,
                        destructive: false,
                        idempotent: false,
                        open_world: false,
                    },
                },
                Arc::new(TerminalYield),
            )
            .unwrap();
        registry
            .register(
                RegistryDefinition {
                    name: "later".into(),
                    description: "must not run".into(),
                    input_schema: json!({"type": "object"}),
                    effects: vec![ToolEffect::Mutate],
                    category: "test".into(),
                    output_schema: json!({"type": "object"}),
                    annotations: artist_resource::ToolAnnotations {
                        read_only: false,
                        destructive: false,
                        idempotent: false,
                        open_world: false,
                    },
                },
                Arc::new(Echo(later_called.clone())),
            )
            .unwrap();
        let model = RigModel::with_registry(
            MockCompletionModel::from_stream_turns([vec![
                MockStreamEvent::tool_call("yield-call", "yield", json!({"completed": true})),
                MockStreamEvent::tool_call("later-call", "later", json!({})),
                final_event(),
            ]]),
            registry,
        )
        .tool_concurrency(8);
        let mut request = ModelRequest {
            session_id: SessionId::from("session"),
            run_id: RunId::from("run"),
            context: String::new(),
            prompt: vec![ContentPart::text("finish")],
            history: Vec::new(),
            extensions: no_extensions(),
            profile: None,
            profile_epoch: None,
            selected_model: None,
        };
        request.profile = Some(Arc::new(TestProfiles.load("test").await.unwrap()));
        let events = model
            .stream(request, Steering::empty())
            .collect::<Vec<_>>()
            .await;
        let yielded_call = events.iter().find_map(|event| match event {
            Ok(ModelEvent::ToolResult { call_id, .. }) => Some(call_id),
            _ => None,
        });
        assert!(
            events.iter().any(|event| matches!(
                event,
                Ok(ModelEvent::Control {
                    call_id,
                    control: ToolControl::Yield { payload }
                }) if Some(call_id) == yielded_call && payload == &json!({"completed": true})
            )),
            "{events:?}"
        );
        assert!(!later_called.load(Ordering::Acquire));
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
                    effects: vec![ToolEffect::Execute],
                    category: "test".into(),
                    output_schema: json!({"type": "object"}),
                    annotations: artist_resource::ToolAnnotations {
                        read_only: false,
                        destructive: false,
                        idempotent: false,
                        open_world: false,
                    },
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
            CreateSession {
                session_id: SessionId::from("rig-controls"),
                metadata: artist_core::SessionMetadata::root(0, None),
                context: InitialContext { fragments: vec![] },
                initial_profile: Some("test".into()),
            },
            SessionDependencies {
                store: store.clone(),
                model,
                profiles: Some(Arc::new(TestProfiles)),
                slash_commands: None,
                extensions: no_extensions(),
                resume_queued_work: true,
            },
        )
        .await
        .unwrap();
        let mut events = session.subscribe();

        session.input(Source::User, "use the gate").await.unwrap();
        started.acquire().await.unwrap().forget();
        let progress = wait_for(&mut events, |kind| {
            matches!(kind, StreamEventKind::ToolProgress { .. })
        })
        .await;
        assert!(matches!(
            progress.kind,
            StreamEventKind::ToolProgress { ref progress }
                if progress.message.as_deref() == Some("gate waiting")
                    && progress.fraction == Some(0.5)
        ));
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
        let second_history = serde_json::to_string(&requests[1].chat_history).unwrap();
        assert!(
            second_history.contains("new boundary context"),
            "{second_history}"
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
                    effects: vec![ToolEffect::Execute],
                    category: "test".into(),
                    output_schema: json!({"type": "object"}),
                    annotations: artist_resource::ToolAnnotations {
                        read_only: false,
                        destructive: false,
                        idempotent: false,
                        open_world: false,
                    },
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
            CreateSession {
                session_id: SessionId::from("rig-abort"),
                metadata: artist_core::SessionMetadata::root(0, None),
                context: InitialContext { fragments: vec![] },
                initial_profile: Some("test".into()),
            },
            SessionDependencies {
                store: store.clone(),
                model,
                profiles: Some(Arc::new(TestProfiles)),
                slash_commands: None,
                extensions: no_extensions(),
                resume_queued_work: true,
            },
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
            CreateSession {
                session_id: SessionId::from("rig-failure"),
                metadata: artist_core::SessionMetadata::root(0, None),
                context: InitialContext { fragments: vec![] },
                initial_profile: Some("test".into()),
            },
            SessionDependencies {
                store: store.clone(),
                model,
                profiles: Some(Arc::new(TestProfiles)),
                slash_commands: None,
                extensions: no_extensions(),
                resume_queued_work: true,
            },
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
                role: artist_core::ContextRole::Other,
            }],
        };
        let session = SessionHandle::create(
            CreateSession {
                session_id: SessionId::from("rig-compaction"),
                metadata: artist_core::SessionMetadata::root(0, None),
                context: context.clone(),
                initial_profile: Some("test".into()),
            },
            SessionDependencies {
                store: store.clone(),
                model,
                profiles: Some(Arc::new(TestProfiles)),
                slash_commands: None,
                extensions: no_extensions(),
                resume_queued_work: true,
            },
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
