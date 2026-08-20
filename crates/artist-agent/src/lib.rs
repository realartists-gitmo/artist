//! Provider- and tool-neutral agent orchestration.
//!
//! This crate owns the turn state machine. Concrete tools, resources, model
//! adapters, and frontends are supplied through traits or protocol adapters.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use artist_component::{
    ComponentToolRegistry, CompositionInput, CompositionUpdate, CompositionUpdater,
    PermissionRegistry,
};
use artist_kernel::{Kernel, ResourceError};
use artist_session::Snapshot;
use artist_session::{ContextController, ContextEvent, EventLog, EventLogTranscript, LogError};
use futures::StreamExt;
use llm_provider::{
    Message, ModelEvent, ModelProvider, ModelRequest, ModelResponse, Role, ToolCall,
    ToolDefinition, ToolResult, Usage,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use observability::AgentMetrics;

pub mod acp;
pub mod daemon;
pub mod observability;
pub mod protocol;
pub mod rpc;
pub mod session;
pub mod toolsurface;

pub use toolsurface::{ToolSurface, ToolSurfaceError, ToolSurfaceEvent};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    UserMessage { message: Message },
    ModelEvent { event: ModelEvent },
    ToolRequested { call: ToolCall },
    ToolCompleted { result: ToolResult },
    TurnCompleted { response: ModelResponse },
    TurnAborted { reason: String },
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("tool execution failed: {message}")]
    Failed { message: String },
}

/// The only tool boundary required by the agent loop.
#[async_trait::async_trait]
pub trait ToolInvoker: Send + Sync {
    async fn invoke(&self, call: ToolCall) -> Result<ToolResult, ToolError>;
}

/// Agent-loop adapter for the session-local component tool registry. The
/// registry owns TOON and component dispatch; the agent loop only translates
/// its provider-neutral `ToolCall`/`ToolResult` records.
pub struct ComponentToolInvoker {
    registry: ComponentToolRegistry,
    permissions: Option<(String, PermissionRegistry)>,
}

impl ComponentToolInvoker {
    pub fn new(registry: ComponentToolRegistry) -> Self {
        Self {
            registry,
            permissions: None,
        }
    }
    pub fn with_permissions(
        mut self,
        profile: impl Into<String>,
        permissions: PermissionRegistry,
    ) -> Self {
        self.permissions = Some((profile.into(), permissions));
        self
    }
    pub fn registry(&self) -> &ComponentToolRegistry {
        &self.registry
    }
}

#[async_trait::async_trait]
impl ToolInvoker for ComponentToolInvoker {
    async fn invoke(&self, call: ToolCall) -> Result<ToolResult, ToolError> {
        if let Some((profile, permissions)) = &self.permissions {
            let resource = call
                .arguments
                .get("uri")
                .or_else(|| call.arguments.get("source"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if !permissions.authorize(profile, &call.name, resource) {
                return Ok(ToolResult {
                    call_id: call.id,
                    content: vec![llm_provider::ContentPart::Text {
                        text: format!("permission denied: {}", call.name),
                    }],
                    is_error: true,
                });
            }
        }
        let request =
            toon_format::encode_default(&call.arguments).map_err(|error| ToolError::Failed {
                message: format!("encode tool request: {error}"),
            })?;
        match self.registry.invoke(&call.name, request.as_bytes()).await {
            Ok(response) => {
                let response = std::str::from_utf8(&response)
                    .map_err(|error| ToolError::Failed {
                        message: error.to_string(),
                    })?
                    .to_owned();
                Ok(ToolResult {
                    call_id: call.id,
                    content: vec![llm_provider::ContentPart::Text { text: response }],
                    is_error: false,
                })
            }
            Err(error) => Ok(ToolResult {
                call_id: call.id,
                content: vec![llm_provider::ContentPart::Text {
                    text: error.to_string(),
                }],
                is_error: true,
            }),
        }
    }
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error(transparent)]
    Provider(#[from] llm_provider::ProviderError),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error(transparent)]
    Log(#[from] LogError),
    #[error("composition update failed: {0}")]
    Composition(String),
    #[error("tool surface update failed: {0}")]
    ToolSurface(String),
    #[error("context update failed: {0}")]
    Context(String),
    #[error("tool-call continuation limit reached ({limit})")]
    ToolRoundLimit { limit: usize },
    #[error("agent turn cancelled")]
    Cancelled,
    #[error("agent turn deadline exceeded")]
    DeadlineExceeded,
    #[error("could not replay agent event at sequence {sequence}: {message}")]
    Replay { sequence: u64, message: String },
}

#[derive(Clone, Debug)]
pub struct TurnControl {
    cancelled: Arc<AtomicBool>,
    deadline: Option<Instant>,
    steering: Arc<Mutex<Vec<Message>>>,
}

impl Default for TurnControl {
    fn default() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: None,
            steering: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl TurnControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            deadline: Some(Instant::now() + timeout),
            ..Self::default()
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn deadline_exceeded(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    pub fn steer(&self, message: Message) {
        self.steering.lock().unwrap().push(message);
    }

    fn take_steering(&self) -> Vec<Message> {
        std::mem::take(&mut *self.steering.lock().unwrap())
    }
}

#[derive(Clone)]
pub struct TurnRequest {
    pub model: String,
    pub user: Message,
    pub tools: Vec<ToolDefinition>,
    pub tool_surface: Option<Arc<ToolSurface>>,
    pub composition_updater: Option<Arc<dyn CompositionUpdater>>,
    pub composition_input: Option<CompositionInput>,
    pub context: Option<Snapshot>,
    pub control: TurnControl,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnOutcome {
    pub response: Option<ModelResponse>,
    pub tool_rounds: usize,
    pub usage: Usage,
}

pub struct AgentEngine<P, T> {
    provider: Arc<P>,
    tools: Arc<T>,
    log: Arc<EventLog>,
    max_tool_rounds: usize,
    metrics: Arc<AgentMetrics>,
}

impl<P, T> AgentEngine<P, T>
where
    P: ModelProvider + 'static,
    T: ToolInvoker + 'static,
{
    pub fn new(provider: Arc<P>, tools: Arc<T>, log: Arc<EventLog>) -> Self {
        Self {
            provider,
            tools,
            log,
            max_tool_rounds: 32,
            metrics: Arc::new(AgentMetrics::default()),
        }
    }

    pub fn with_max_tool_rounds(mut self, max_tool_rounds: usize) -> Self {
        self.max_tool_rounds = max_tool_rounds;
        self
    }

    pub fn with_metrics(mut self, metrics: Arc<AgentMetrics>) -> Self {
        self.metrics = metrics;
        self
    }

    pub fn metrics(&self) -> Arc<AgentMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Publish this engine's existing durable event log through the kernel's
    /// `agent://<agent>/transcript` resource.
    pub fn register_transcript(
        &self,
        kernel: &Kernel,
        agent: impl Into<String>,
    ) -> Result<(), ResourceError> {
        kernel.register_agent_transcript(agent, EventLogTranscript::new(Arc::clone(&self.log)))
    }

    /// Reconstruct the canonical agent event stream without invoking a model
    /// or a tool. Projections and resume logic can build on this stable log
    /// boundary while the concrete context policy remains higher-level.
    pub fn replay_events(&self) -> Result<Vec<AgentEvent>, AgentError> {
        self.log
            .records()?
            .into_iter()
            .map(|record| {
                serde_json::from_value(record.payload).map_err(|error| AgentError::Replay {
                    sequence: record.sequence,
                    message: error.to_string(),
                })
            })
            .collect()
    }

    pub async fn run_turn(&self, request: TurnRequest) -> Result<TurnOutcome, AgentError> {
        self.metrics.turn_started();
        self.check_control(&request.control)?;
        self.record(AgentEvent::UserMessage {
            message: request.user.clone(),
        })?;
        let context = ContextController::new(
            request.context.clone().unwrap_or_else(|| Snapshot::new([])),
            Arc::clone(&self.log),
        )
        .map_err(|error| AgentError::Context(error.to_string()))?;
        let mut messages = crate::session::project_snapshot(&context.snapshot());
        messages.push(request.user);
        let mut tool_rounds = 0;
        let mut usage = Usage::default();

        loop {
            self.check_control(&request.control)?;
            if let (Some(updater), Some(input), Some(surface)) = (
                &request.composition_updater,
                &request.composition_input,
                &request.tool_surface,
            ) {
                let updates = updater
                    .update(input.clone())
                    .await
                    .map_err(|error| AgentError::Composition(error.to_string()))?;
                for update in updates {
                    match update {
                        CompositionUpdate::Context(event) => {
                            let model_message = match &event {
                                ContextEvent::Replace { contribution }
                                | ContextEvent::Append { contribution } => {
                                    Some(Message::text(Role::System, contribution.content.clone()))
                                }
                                ContextEvent::Remove { .. } => None,
                            };
                            context
                                .apply(event)
                                .map_err(|error| AgentError::Context(error.to_string()))?;
                            if let Some(message) = model_message {
                                messages.push(message);
                            }
                        }
                        tool_update => surface
                            .apply_composition_update(tool_update)
                            .map_err(|error| AgentError::ToolSurface(error.to_string()))?,
                    }
                }
            }
            if let Some(surface) = &request.tool_surface {
                messages.extend(surface.take_messages());
            }
            let tools = request
                .tool_surface
                .as_ref()
                .map(|surface| surface.formal_definitions())
                .unwrap_or_else(|| request.tools.clone());
            self.provider
                .register_formal_tools(&request.model, &tools)?;
            let model_request = ModelRequest {
                model: request.model.clone(),
                messages: messages.clone(),
                tools,
                temperature: None,
                max_output_tokens: None,
                metadata: serde_json::json!({}),
            };
            let mut stream = self.provider.stream(model_request);
            let mut response = None;
            while let Some(event) = stream.next().await {
                self.check_control(&request.control)?;
                let event = event.map_err(|error| {
                    self.abort(format!("provider: {error}"));
                    AgentError::Provider(error)
                })?;
                if let ModelEvent::Completed {
                    response: completed,
                } = &event
                {
                    response = Some(completed.clone());
                }
                self.record(AgentEvent::ModelEvent { event })?;
            }
            let response = match response {
                Some(response) => response,
                None => {
                    let error = llm_provider::ProviderError::InvalidResponse {
                        message: "provider stream ended without a completed response".into(),
                    };
                    self.abort(error.to_string());
                    return Err(error.into());
                }
            };
            usage = add_usage(&usage, &response.usage);

            if response.tool_calls.is_empty() {
                self.check_control(&request.control)?;
                self.record(AgentEvent::TurnCompleted {
                    response: response.clone(),
                })?;
                self.metrics.turn_completed();
                return Ok(TurnOutcome {
                    response: Some(response),
                    tool_rounds,
                    usage,
                });
            }
            if tool_rounds >= self.max_tool_rounds {
                self.abort(format!(
                    "tool round limit reached: {}",
                    self.max_tool_rounds
                ));
                return Err(AgentError::ToolRoundLimit {
                    limit: self.max_tool_rounds,
                });
            }

            tool_rounds += 1;
            messages.push(Message {
                role: Role::Assistant,
                content: response.content.clone(),
                name: None,
                tool_calls: response.tool_calls.clone(),
                tool_results: Vec::new(),
            });
            for call in response.tool_calls {
                self.check_control(&request.control)?;
                self.record(AgentEvent::ToolRequested { call: call.clone() })?;
                self.metrics.tool_call();
                let result = if let Some(surface) = &request.tool_surface {
                    surface.invoke(call).await
                } else {
                    self.tools.invoke(call).await
                }
                .map_err(|error| {
                    self.abort(format!("tool: {error}"));
                    AgentError::Tool(error)
                })?;
                self.check_control(&request.control)?;
                self.record(AgentEvent::ToolCompleted {
                    result: result.clone(),
                })?;
                messages.push(Message {
                    role: Role::Tool,
                    content: result.content.clone(),
                    name: None,
                    tool_calls: Vec::new(),
                    tool_results: vec![result],
                });
            }
            messages.extend(request.control.take_steering());
        }
    }

    fn record(&self, event: AgentEvent) -> Result<(), AgentError> {
        let event_type = match &event {
            AgentEvent::UserMessage { .. } => "agent.user_message",
            AgentEvent::ModelEvent { .. } => "agent.model_event",
            AgentEvent::ToolRequested { .. } => "agent.tool_requested",
            AgentEvent::ToolCompleted { .. } => "agent.tool_completed",
            AgentEvent::TurnCompleted { .. } => "agent.turn_completed",
            AgentEvent::TurnAborted { .. } => "agent.turn_aborted",
        };
        self.log.append(
            event_type,
            serde_json::to_value(event).expect("AgentEvent is serializable"),
        )?;
        Ok(())
    }

    fn abort(&self, reason: impl Into<String>) {
        let _ = self.record(AgentEvent::TurnAborted {
            reason: reason.into(),
        });
    }

    fn check_control(&self, control: &TurnControl) -> Result<(), AgentError> {
        if control.is_cancelled() {
            self.abort("cancelled");
            return Err(AgentError::Cancelled);
        }
        if control.deadline_exceeded() {
            self.abort("deadline_exceeded");
            return Err(AgentError::DeadlineExceeded);
        }
        Ok(())
    }
}

fn add_usage(current: &Usage, next: &Usage) -> Usage {
    fn add(left: Option<u64>, right: Option<u64>) -> Option<u64> {
        match (left, right) {
            (Some(left), Some(right)) => Some(left.saturating_add(right)),
            (Some(value), None) | (None, Some(value)) => Some(value),
            (None, None) => None,
        }
    }
    Usage {
        input_tokens: add(current.input_tokens, next.input_tokens),
        output_tokens: add(current.output_tokens, next.output_tokens),
        total_tokens: add(current.total_tokens, next.total_tokens),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_stream::stream;
    use futures::stream::BoxStream;
    use llm_provider::{ContentPart, ModelCapabilities, ProviderError};
    use tempfile::tempdir;

    struct FakeProvider {
        responses: std::sync::Mutex<Vec<ModelResponse>>,
    }

    impl ModelProvider for FakeProvider {
        fn provider_name(&self) -> &str {
            "fake"
        }
        fn capabilities(&self, _model: &str) -> ModelCapabilities {
            ModelCapabilities {
                streaming: true,
                tool_calls: true,
                ..Default::default()
            }
        }
        fn stream<'a>(&'a self, _request: ModelRequest) -> llm_provider::ModelEventStream<'a> {
            let response = self.responses.lock().unwrap().remove(0);
            Box::pin(stream! {
                yield Ok(ModelEvent::Started);
                yield Ok(ModelEvent::Completed { response });
            })
        }
    }

    struct FakeTools;

    #[async_trait::async_trait]
    impl ToolInvoker for FakeTools {
        async fn invoke(&self, call: ToolCall) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                call_id: call.id,
                content: vec![ContentPart::Text {
                    text: "tool result".into(),
                }],
                is_error: false,
            })
        }
    }

    fn response(text: &str, tool_calls: Vec<ToolCall>) -> ModelResponse {
        ModelResponse {
            content: vec![ContentPart::Text { text: text.into() }],
            tool_calls,
            finish_reason: Some("stop".into()),
            usage: Default::default(),
        }
    }

    #[tokio::test]
    async fn runs_model_tool_model_and_records_the_turn() {
        let dir = tempdir().unwrap();
        let log = Arc::new(EventLog::open(dir.path().join("session.jsonl"), "s1").unwrap());
        let call = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: serde_json::json!({}),
        };
        let provider = Arc::new(FakeProvider {
            responses: std::sync::Mutex::new(vec![
                response("use tool", vec![call]),
                response("done", vec![]),
            ]),
        });
        let engine = AgentEngine::new(provider, Arc::new(FakeTools), log.clone());
        let outcome = engine
            .run_turn(TurnRequest {
                model: "fake".into(),
                user: Message::text(Role::User, "go"),
                tools: vec![],
                tool_surface: None,
                composition_updater: None,
                composition_input: None,
                context: None,
                control: TurnControl::default(),
            })
            .await
            .unwrap();
        assert_eq!(outcome.tool_rounds, 1);
        assert_eq!(
            outcome.response.unwrap().content,
            vec![ContentPart::Text {
                text: "done".into()
            }]
        );
        assert!(
            log.records()
                .unwrap()
                .iter()
                .any(|record| record.event_type == "agent.tool_completed")
        );
        assert!(
            engine
                .replay_events()
                .unwrap()
                .iter()
                .any(|event| matches!(event, AgentEvent::TurnCompleted { .. }))
        );
        assert_eq!(
            engine.metrics().snapshot(),
            observability::MetricsSnapshot {
                turns_started: 1,
                turns_completed: 1,
                tool_calls: 1,
            }
        );
    }

    #[tokio::test]
    async fn rejects_a_provider_stream_without_completion() {
        struct EmptyProvider;
        impl ModelProvider for EmptyProvider {
            fn provider_name(&self) -> &str {
                "empty"
            }
            fn capabilities(&self, _model: &str) -> ModelCapabilities {
                Default::default()
            }
            fn stream<'a>(&'a self, _request: ModelRequest) -> llm_provider::ModelEventStream<'a> {
                let output: BoxStream<'a, Result<ModelEvent, ProviderError>> =
                    Box::pin(futures::stream::empty());
                output
            }
        }
        let dir = tempdir().unwrap();
        let log = Arc::new(EventLog::open(dir.path().join("session.jsonl"), "s1").unwrap());
        let result = AgentEngine::new(Arc::new(EmptyProvider), Arc::new(FakeTools), log)
            .run_turn(TurnRequest {
                model: "empty".into(),
                user: Message::text(Role::User, "go"),
                tools: vec![],
                tool_surface: None,
                composition_updater: None,
                composition_input: None,
                context: None,
                control: TurnControl::default(),
            })
            .await;
        assert!(matches!(
            result,
            Err(AgentError::Provider(ProviderError::InvalidResponse { .. }))
        ));
    }

    #[tokio::test]
    async fn cancellation_is_checked_and_recorded() {
        let dir = tempdir().unwrap();
        let log = Arc::new(EventLog::open(dir.path().join("session.jsonl"), "s1").unwrap());
        let control = TurnControl::new();
        control.cancel();
        let result = AgentEngine::new(
            Arc::new(FakeProvider {
                responses: std::sync::Mutex::new(vec![response("never", vec![])]),
            }),
            Arc::new(FakeTools),
            log.clone(),
        )
        .run_turn(TurnRequest {
            model: "fake".into(),
            user: Message::text(Role::User, "go"),
            tools: vec![],
            tool_surface: None,
            composition_updater: None,
            composition_input: None,
            context: None,
            control,
        })
        .await;
        assert!(matches!(result, Err(AgentError::Cancelled)));
        assert!(
            log.records()
                .unwrap()
                .iter()
                .any(|record| record.event_type == "agent.turn_aborted")
        );
    }
}
