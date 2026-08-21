//! Provider- and tool-neutral agent orchestration.
//!
//! This crate owns the turn state machine. Concrete tools, resources, model
//! adapters, and frontends are supplied through traits or protocol adapters.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use artist_component::{
    CompactionComponent, CompactionRequest, ComponentToolRegistry, CompositionInput,
    CompositionUpdate, CompositionUpdater, PermissionRegistry,
};
use artist_kernel::{Kernel, ResourceError};
use artist_session::Snapshot;
use artist_session::{ContextController, EventLog, EventLogTranscript, LogError};
use futures::StreamExt;
use llm_provider::{
    Message, ModelEvent, ModelProvider, ModelRequest, ModelResponse, ToolCall, ToolDefinition,
    ToolResult, Usage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use observability::AgentMetrics;

pub mod acp;
pub mod daemon;
pub mod observability;
pub mod protocol;
pub mod rpc;
pub mod session;
pub mod toolsurface;

pub use toolsurface::{HarnessPolicy, ToolSurface, ToolSurfaceError, ToolSurfaceEvent};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    TurnStarted {
        turn_id: String,
        model: String,
    },
    UserMessage {
        message: Message,
    },
    SteeringAccepted {
        message: Message,
    },
    SurfaceMessage {
        message: Message,
    },
    ModelEvent {
        event: ModelEvent,
    },
    Usage {
        usage: Usage,
    },
    ToolRequested {
        call: ToolCall,
    },
    ToolCompleted {
        result: ToolResult,
    },
    ProviderObserved {
        turn_id: String,
        provider_call_id: String,
        provider: String,
        latency_ms: u64,
        ok: bool,
        error_class: Option<String>,
    },
    ToolObserved {
        turn_id: String,
        tool_call_id: String,
        tool: String,
        latency_ms: u64,
        ok: bool,
        error_class: Option<String>,
    },
    ContextCompacted {
        context: Vec<Message>,
        metadata: Value,
    },
    ProfileChanged {
        profile_id: String,
        provider_id: Option<String>,
        model: Option<String>,
    },
    Yielded {
        payload: Value,
    },
    ForkStarted {
        fork_id: String,
        tasks: Vec<String>,
    },
    ForkCompleted {
        fork_id: String,
        payload: Value,
    },
    Handoff {
        profile_id: String,
        brief: String,
    },
    TurnCompleted {
        response: ModelResponse,
    },
    TurnAborted {
        reason: String,
    },
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("tool execution failed: {message}")]
    Failed { message: String },
    #[error("tool execution cancelled")]
    Cancelled,
}

/// The only tool boundary required by the agent loop.
#[async_trait::async_trait]
pub trait ToolInvoker: Send + Sync {
    async fn invoke(&self, call: ToolCall) -> Result<ToolResult, ToolError>;

    async fn invoke_with_control(
        &self,
        call: ToolCall,
        control: &TurnControl,
    ) -> Result<ToolResult, ToolError> {
        tokio::select! {
            result = self.invoke(call) => result,
            _ = control.cancelled() => Err(ToolError::Cancelled),
        }
    }
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
                let envelope = artist_component::ToolResultEnvelope {
                    ok: false,
                    output: None,
                    error: Some(artist_component::ToolFailure {
                        code: "permission_denied".into(),
                        message: format!("permission denied: {}", call.name),
                        details: None,
                    }),
                };
                return Ok(ToolResult {
                    call_id: call.id,
                    content: vec![llm_provider::ContentPart::Text {
                        text: serde_json::to_string(&envelope)
                            .expect("tool envelope is serializable"),
                    }],
                    is_error: true,
                });
            }
        }
        let request =
            toon_format::encode_default(&call.arguments).map_err(|error| ToolError::Failed {
                message: format!("encode tool request: {error}"),
            })?;
        let response = self
            .registry
            .invoke_enveloped(&call.name, request.as_bytes())
            .await;
        let response = std::str::from_utf8(&response)
            .map_err(|error| ToolError::Failed {
                message: error.to_string(),
            })?
            .to_owned();
        let envelope: artist_component::ToolResultEnvelope = serde_json::from_str(&response)
            .map_err(|error| ToolError::Failed {
                message: error.to_string(),
            })?;
        Ok(ToolResult {
            call_id: call.id,
            content: vec![llm_provider::ContentPart::Text { text: response }],
            is_error: !envelope.ok,
        })
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
    #[error("provider selection failed: {0}")]
    ProviderSelection(String),
    #[error("compaction failed: {0}")]
    Compaction(String),
    #[error("tool surface update failed: {0}")]
    ToolSurface(String),
    #[error("context update failed: {0}")]
    Context(String),
    #[error("tool-call continuation limit reached ({limit})")]
    ToolRoundLimit { limit: usize },
    #[error("another turn is already active for this session")]
    TurnAlreadyActive,
    #[error("agent turn cancelled")]
    Cancelled,
    #[error("agent turn deadline exceeded")]
    DeadlineExceeded,
    #[error("could not replay agent event at sequence {sequence}: {message}")]
    Replay { sequence: u64, message: String },
}

/// A fork receives one immutable model-context ingress and its own execution
/// identity. `snapshot` is the active composed context; `messages` is the
/// complete provider-neutral request prefix captured at the fork boundary.
/// Each child may append only its own task to that copy.
#[derive(Clone, Debug, PartialEq)]
pub struct ForkIngress {
    pub snapshot: Snapshot,
    pub messages: Vec<Message>,
}

/// The executor owns the provider/profile choices for each child; the parent
/// only waits for the structured terminal results.
#[async_trait::async_trait]
pub trait ForkExecutor: Send + Sync {
    async fn execute(
        &self,
        fork_id: String,
        context: Snapshot,
        task: String,
        control: TurnControl,
    ) -> Result<Value, AgentError>;

    /// New executors can consume the complete captured model context. The
    /// default preserves the narrow snapshot-only ABI for embedders that do
    /// not need to reconstruct a child conversation.
    async fn execute_with_ingress(
        &self,
        fork_id: String,
        ingress: ForkIngress,
        task: String,
        control: TurnControl,
    ) -> Result<Value, AgentError> {
        self.execute(fork_id, ingress.snapshot, task, control).await
    }
}

#[derive(Clone, Debug)]
pub struct HandoffPlan {
    pub profile_id: String,
    pub context: Snapshot,
    /// The next live composition input. Handoff must replace the input used
    /// by the composition socket as well as replacing the already-projected
    /// snapshot; otherwise the next boundary could re-apply the old profile.
    pub composition_input: Option<CompositionInput>,
    pub model: Option<String>,
    pub provider_config_id: Option<String>,
    pub tool_definitions: Option<Vec<ToolDefinition>>,
    pub permissions: Option<(String, PermissionRegistry)>,
    pub harness: Option<HarnessPolicy>,
}

#[async_trait::async_trait]
pub trait HandoffHandler: Send + Sync {
    async fn resolve(&self, profile_id: &str, brief: &str) -> Result<HandoffPlan, AgentError>;
}

/// Resolves a stable provider configuration identity during a handoff. The
/// session host owns this router; provider-specific auth/configuration stays
/// behind the provider component catalog.
#[async_trait::async_trait]
pub trait ProviderResolver: Send + Sync {
    async fn resolve(&self, provider_config_id: &str)
    -> Result<Arc<dyn ModelProvider>, AgentError>;
}

async fn execute_forks(
    executor: Arc<dyn ForkExecutor>,
    ingress: ForkIngress,
    tasks: Vec<String>,
    control: TurnControl,
) -> Result<Vec<(String, Value)>, AgentError> {
    let jobs = tasks.into_iter().enumerate().map(|(index, task)| {
        let executor = Arc::clone(&executor);
        let ingress = ingress.clone();
        let control = control.child();
        async move {
            let fork_id = format!("fork-{}", uuid::Uuid::new_v4());
            let value = executor
                .execute_with_ingress(fork_id.clone(), ingress, task, control)
                .await?;
            let _ = index;
            Ok::<_, AgentError>((fork_id, value))
        }
    });
    futures::future::try_join_all(jobs).await
}

#[derive(Clone, Debug)]
pub struct TurnControl {
    cancelled: CancellationToken,
    deadline: Option<Instant>,
    steering: Arc<Mutex<Vec<Message>>>,
    aborted: Arc<AtomicBool>,
}

impl Default for TurnControl {
    fn default() -> Self {
        Self {
            cancelled: CancellationToken::new(),
            deadline: None,
            steering: Arc::new(Mutex::new(Vec::new())),
            aborted: Arc::new(AtomicBool::new(false)),
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
        self.cancelled.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.is_cancelled()
    }

    pub fn cancelled(&self) -> impl std::future::Future<Output = ()> + '_ {
        self.cancelled.cancelled()
    }

    /// Create an execution-local control view for a fork. Cancellation and
    /// the parent deadline are shared, while steering and abort journaling
    /// remain private to the child so concurrent forks cannot consume one
    /// another's stdin.
    pub fn child(&self) -> Self {
        Self {
            cancelled: self.cancelled.clone(),
            deadline: self.deadline,
            steering: Arc::new(Mutex::new(Vec::new())),
            aborted: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn deadline_exceeded(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    pub fn steer(&self, message: Message) {
        // Steering is an input stream, not a set. Two identical messages can
        // be meaningful (and two durable stdin records must not collapse into
        // one in-memory input merely because their JSON is equal).
        self.steering.lock().unwrap().push(message);
    }

    fn take_steering(&self) -> Vec<Message> {
        std::mem::take(&mut *self.steering.lock().unwrap())
    }

    fn mark_abort_needed(&self) -> bool {
        self.aborted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn reset_abort_marker(&self) {
        self.aborted.store(false, Ordering::Release);
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
    /// Optional complete provider-neutral ingress captured by a fork. The
    /// ordinary session path leaves this unset and rebuilds from its log.
    pub initial_messages: Option<Vec<Message>>,
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
    turn_gate: Arc<tokio::sync::Mutex<()>>,
    fork_executor: Option<Arc<dyn ForkExecutor>>,
    handoff_handler: Option<Arc<dyn HandoffHandler>>,
    provider_resolver: Option<Arc<dyn ProviderResolver>>,
    compaction: Option<Arc<dyn CompactionComponent>>,
    compaction_context_limit: Option<u64>,
    compaction_metadata: Value,
    fork_execution: bool,
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
            turn_gate: Arc::new(tokio::sync::Mutex::new(())),
            fork_executor: None,
            handoff_handler: None,
            provider_resolver: None,
            compaction: None,
            compaction_context_limit: None,
            compaction_metadata: Value::Null,
            fork_execution: false,
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

    pub fn with_fork_executor(mut self, executor: Arc<dyn ForkExecutor>) -> Self {
        self.fork_executor = Some(executor);
        self
    }

    pub fn with_handoff_handler(mut self, handler: Arc<dyn HandoffHandler>) -> Self {
        self.handoff_handler = Some(handler);
        self
    }

    pub fn with_provider_resolver(mut self, resolver: Arc<dyn ProviderResolver>) -> Self {
        self.provider_resolver = Some(resolver);
        self
    }

    pub fn with_compaction(
        mut self,
        component: Arc<dyn CompactionComponent>,
        context_limit: Option<u64>,
        metadata: Value,
    ) -> Self {
        self.compaction = Some(component);
        self.compaction_context_limit = context_limit;
        self.compaction_metadata = metadata;
        self
    }

    /// Construct a child-engine marker for fork executors that delegate back
    /// to this orchestration layer. Handoff is rejected for such executions.
    pub fn as_fork(mut self) -> Self {
        self.fork_execution = true;
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
            .filter(|record| record.event_type.starts_with("agent."))
            .map(|record| {
                serde_json::from_value(record.payload).map_err(|error| AgentError::Replay {
                    sequence: record.sequence,
                    message: error.to_string(),
                })
            })
            .collect()
    }

    /// Rehydrate stdin records that crossed the durable acceptance boundary
    /// but were not yet acknowledged by the turn state machine. This closes
    /// the crash window between `session.stdin` and `agent.steering_accepted`
    /// without replaying already-accepted steering messages.
    fn restore_pending_steering(&self, control: &TurnControl) -> Result<(), AgentError> {
        let records = self.log.records()?;
        let mut pending = Vec::<Message>::new();
        for record in records {
            match record.event_type.as_str() {
                "agent.steering_accepted" => {
                    let event: AgentEvent =
                        serde_json::from_value(record.payload).map_err(|error| {
                            AgentError::Replay {
                                sequence: record.sequence,
                                message: error.to_string(),
                            }
                        })?;
                    if let AgentEvent::SteeringAccepted { message } = event {
                        // Accepted steering is paired with the oldest still
                        // pending stdin record, rather than with a global
                        // value multiset. A repeated message in a later turn
                        // is a new input and must not be swallowed by an
                        // identically-shaped message accepted earlier.
                        if let Some(position) =
                            pending.iter().position(|candidate| candidate == &message)
                        {
                            pending.remove(position);
                        }
                    }
                }
                "session.stdin" => {
                    let Some(message) = record.payload.get("message") else {
                        continue;
                    };
                    pending.push(serde_json::from_value(message.clone()).map_err(|error| {
                        AgentError::Replay {
                            sequence: record.sequence,
                            message: error.to_string(),
                        }
                    })?);
                }
                _ => {}
            }
        }
        for message in pending {
            control.steer(message);
        }
        Ok(())
    }

    pub async fn run_turn(&self, request: TurnRequest) -> Result<TurnOutcome, AgentError> {
        let control = request.control.clone();
        let result = self.run_turn_inner(request).await;
        if let Err(error) = &result {
            // Provider cancellation/failure paths already journal their abort
            // at the point where the in-flight operation is interrupted. The
            // outer guard covers composition, context, surface, and log
            // errors too, so every started turn has an explicit terminal
            // record. A rejected concurrent turn never acquired the gate and
            // must not create a phantom abort record.
            if !matches!(error, AgentError::TurnAlreadyActive) {
                self.abort(&control, format!("turn failed: {error}"))?;
            }
        }
        result
    }

    async fn run_turn_inner(&self, request: TurnRequest) -> Result<TurnOutcome, AgentError> {
        let _turn_guard = self
            .turn_gate
            .try_lock()
            .map_err(|_| AgentError::TurnAlreadyActive)?;
        self.metrics.turn_started();
        self.check_control(&request.control)?;
        self.restore_pending_steering(&request.control)?;
        let turn_id = uuid::Uuid::new_v4().to_string();
        self.record(AgentEvent::TurnStarted {
            turn_id: turn_id.clone(),
            model: request.model.clone(),
        })?;
        self.record(AgentEvent::UserMessage {
            message: request.user.clone(),
        })?;
        let context = ContextController::restore(
            request.context.clone().unwrap_or_else(|| Snapshot::new([])),
            Arc::clone(&self.log),
        )
        .map_err(|error| AgentError::Context(error.to_string()))?;
        let mut composition_input = request.composition_input.clone();
        let mut active_provider: Arc<dyn ModelProvider> = self.provider.clone();
        let mut active_model = request.model.clone();
        let mut tool_rounds = 0;
        let mut usage = Usage::default();
        let mut compaction_attempted = false;

        'turn: loop {
            self.check_control(&request.control)?;
            self.restore_pending_steering(&request.control)?;
            if let (Some(updater), Some(input), Some(surface)) = (
                &request.composition_updater,
                &composition_input,
                &request.tool_surface,
            ) {
                let updates = updater
                    .update(input.clone())
                    .await
                    .map_err(|error| AgentError::Composition(error.to_string()))?;
                for update in updates {
                    match update {
                        CompositionUpdate::Context(event) => {
                            context
                                .apply(event)
                                .map_err(|error| AgentError::Context(error.to_string()))?;
                        }
                        tool_update => surface
                            .apply_composition_update(tool_update)
                            .map_err(|error| AgentError::ToolSurface(error.to_string()))?,
                    }
                }
            }
            if let Some(surface) = &request.tool_surface {
                for message in surface.take_messages() {
                    self.record(AgentEvent::SurfaceMessage { message })?;
                }
            }
            let events = self.replay_events()?;
            let messages = if let Some(initial_messages) = &request.initial_messages {
                let mut messages = initial_messages.clone();
                messages.extend(crate::session::project_conversation(&events));
                messages
            } else {
                crate::session::project_request(&context.snapshot(), &events)
            };
            // Capture the exact provider-neutral request prefix before the
            // provider adds the current assistant/tool response. Forks all
            // receive this same immutable ingress and append only their own
            // task.
            let fork_messages = messages.clone();
            let tools = request
                .tool_surface
                .as_ref()
                .map(|surface| surface.formal_definitions())
                .unwrap_or_else(|| request.tools.clone());
            active_provider.register_formal_tools(&active_model, &tools)?;
            let model_request = ModelRequest {
                model: active_model.clone(),
                messages,
                tools,
                temperature: None,
                max_output_tokens: None,
                metadata: serde_json::json!({}),
                structured_output: None,
            };
            let provider_call_id = uuid::Uuid::new_v4().to_string();
            let provider_started = Instant::now();
            let mut stream = active_provider.stream(model_request);
            let mut response = None;
            let mut streamed_usage = Usage::default();
            loop {
                let next = if let Some(deadline) = request.control.deadline {
                    tokio::select! {
                        biased;
                        _ = request.control.cancelled() => {
                            let _ = self.record_provider_observation(
                                &turn_id,
                                &provider_call_id,
                                active_provider.provider_name(),
                                provider_started.elapsed(),
                                false,
                                Some("cancelled".into()),
                            );
                            return Err(self.cancelled(&request.control));
                        }
                        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                            let _ = self.record_provider_observation(
                                &turn_id,
                                &provider_call_id,
                                active_provider.provider_name(),
                                provider_started.elapsed(),
                                false,
                                Some("deadline".into()),
                            );
                            return Err(self.deadline_exceeded(&request.control));
                        }
                        event = stream.next() => event,
                    }
                } else {
                    tokio::select! {
                        biased;
                        _ = request.control.cancelled() => {
                            let _ = self.record_provider_observation(
                                &turn_id,
                                &provider_call_id,
                                active_provider.provider_name(),
                                provider_started.elapsed(),
                                false,
                                Some("cancelled".into()),
                            );
                            return Err(self.cancelled(&request.control));
                        }
                        event = stream.next() => event,
                    }
                };
                let Some(event) = next else { break };
                self.check_control(&request.control)?;
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        if let (true, false, Some(compaction)) = (
                            matches!(error, llm_provider::ProviderError::ContextLimit { .. }),
                            compaction_attempted,
                            self.compaction.as_ref(),
                        ) {
                            self.record_provider_observation(
                                &turn_id,
                                &provider_call_id,
                                active_provider.provider_name(),
                                provider_started.elapsed(),
                                false,
                                Some(provider_error_class(&error)),
                            )?;
                            let compacted = compaction
                                .compact(CompactionRequest {
                                    context: crate::session::project_conversation(&events),
                                    model: active_model.clone(),
                                    context_limit: self.compaction_context_limit,
                                    metadata: self.compaction_metadata.clone(),
                                })
                                .await
                                .map_err(|error| AgentError::Compaction(error.to_string()))?;
                            compaction_attempted = true;
                            self.record(AgentEvent::ContextCompacted {
                                context: compacted.context,
                                metadata: compacted.metadata,
                            })?;
                            continue 'turn;
                        }
                        self.record_provider_observation(
                            &turn_id,
                            &provider_call_id,
                            active_provider.provider_name(),
                            provider_started.elapsed(),
                            false,
                            Some(provider_error_class(&error)),
                        )?;
                        return Err(self.provider_failure(&request.control, error));
                    }
                };
                if let ModelEvent::Usage { usage: event_usage } = &event {
                    streamed_usage = add_usage(&streamed_usage, event_usage);
                }
                if let ModelEvent::Completed {
                    response: completed,
                } = &event
                {
                    response = Some(completed.clone());
                }
                self.record(AgentEvent::ModelEvent { event })?;
            }
            drop(stream);
            let response = match response {
                Some(response) => response,
                None => {
                    self.record_provider_observation(
                        &turn_id,
                        &provider_call_id,
                        active_provider.provider_name(),
                        provider_started.elapsed(),
                        false,
                        Some("invalid_response".into()),
                    )?;
                    let error = llm_provider::ProviderError::InvalidResponse {
                        message: "provider stream ended without a completed response".into(),
                    };
                    return Err(self.provider_failure(&request.control, error));
                }
            };
            self.record_provider_observation(
                &turn_id,
                &provider_call_id,
                active_provider.provider_name(),
                provider_started.elapsed(),
                true,
                None,
            )?;
            let call_usage = if response.usage != Usage::default() {
                response.usage.clone()
            } else {
                streamed_usage
            };
            usage = add_usage(&usage, &call_usage);
            self.metrics
                .usage(call_usage.input_tokens, call_usage.output_tokens);
            self.record(AgentEvent::Usage { usage: call_usage })?;

            if response.tool_calls.is_empty() {
                self.restore_pending_steering(&request.control)?;
                self.check_control(&request.control)?;
                let steering = {
                    // Hold the input mutex through the durable completion
                    // append. A concurrent stdin write may either be seen
                    // here and force another provider boundary, or wait
                    // until completion and remain pending for the next turn;
                    // it cannot slip between the final check and the
                    // turn-completed record.
                    let mut pending = request.control.steering.lock().unwrap();
                    if pending.is_empty() {
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
                    std::mem::take(&mut *pending)
                };
                for message in steering {
                    self.record(AgentEvent::SteeringAccepted { message })?;
                }
                continue;
            }
            if tool_rounds >= self.max_tool_rounds {
                self.abort(
                    &request.control,
                    format!("tool round limit reached: {}", self.max_tool_rounds),
                )?;
                return Err(AgentError::ToolRoundLimit {
                    limit: self.max_tool_rounds,
                });
            }

            tool_rounds += 1;
            for call in &response.tool_calls {
                let harness_arguments = call.arguments.clone();
                self.check_control(&request.control)?;
                self.record(AgentEvent::ToolRequested { call: call.clone() })?;
                self.metrics.tool_call();
                let tool_started = Instant::now();
                let result = if let Some(surface) = &request.tool_surface {
                    surface
                        .invoke_with_control(call.clone(), &request.control)
                        .await
                } else {
                    self.tools
                        .invoke_with_control(call.clone(), &request.control)
                        .await
                };
                let result = match result {
                    Ok(result) => result,
                    Err(ToolError::Cancelled) => {
                        let _ = self.record_tool_observation(
                            &turn_id,
                            call,
                            tool_started.elapsed(),
                            false,
                            Some("cancelled".into()),
                        );
                        return Err(self.cancelled(&request.control));
                    }
                    Err(error) => ToolResult {
                        call_id: call.id.clone(),
                        content: vec![llm_provider::ContentPart::Text {
                            text: error.to_string(),
                        }],
                        is_error: true,
                    },
                };
                self.record_tool_observation(
                    &turn_id,
                    call,
                    tool_started.elapsed(),
                    !result.is_error,
                    result.is_error.then_some("tool_error".into()),
                )?;
                self.check_control(&request.control)?;
                self.record(AgentEvent::ToolCompleted {
                    result: result.clone(),
                })?;
                match call.name.as_str() {
                    "yield" => {
                        self.record(AgentEvent::Yielded {
                            payload: harness_arguments.clone(),
                        })?;
                        if harness_arguments
                            .get("complete")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                        {
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
                    }
                    "fork" => {
                        let tasks = harness_arguments
                            .get("tasks")
                            .and_then(Value::as_array)
                            .ok_or_else(|| {
                                AgentError::Context("fork.tasks must be an array".into())
                            })?
                            .iter()
                            .map(|task| {
                                task.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                                    AgentError::Context("fork tasks must be strings".into())
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        if self.fork_execution {
                            return Err(AgentError::Context(
                                "fork is not available inside a fork".into(),
                            ));
                        }
                        let fork_id = format!("fork-{}", uuid::Uuid::new_v4());
                        self.record(AgentEvent::ForkStarted {
                            fork_id: fork_id.clone(),
                            tasks: tasks.clone(),
                        })?;
                        let payload = if let Some(executor) = &self.fork_executor {
                            let results = execute_forks(
                                Arc::clone(executor),
                                ForkIngress {
                                    snapshot: context.snapshot(),
                                    messages: fork_messages.clone(),
                                },
                                tasks,
                                request.control.clone(),
                            )
                            .await
                            .map_err(|error| match error {
                                AgentError::Cancelled => self.cancelled(&request.control),
                                AgentError::DeadlineExceeded => {
                                    self.deadline_exceeded(&request.control)
                                }
                                other => other,
                            })?;
                            serde_json::json!({
                                "forks": results
                                    .into_iter()
                                    .map(|(id, value)| serde_json::json!({
                                        "id": id,
                                        "result": value
                                    }))
                                    .collect::<Vec<_>>()
                            })
                        } else {
                            serde_json::json!({"error": "fork executor is not installed"})
                        };
                        self.record(AgentEvent::ForkCompleted { fork_id, payload })?;
                    }
                    "handoff" => {
                        if self.fork_execution {
                            return Err(AgentError::Context(
                                "handoff is not available inside a fork".into(),
                            ));
                        }
                        let profile_id = harness_arguments
                            .get("profile")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        let brief = harness_arguments
                            .get("brief")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        let plan = if let Some(handler) = &self.handoff_handler {
                            handler.resolve(&profile_id, &brief).await?
                        } else {
                            HandoffPlan {
                                profile_id: profile_id.clone(),
                                context: Snapshot::new([]),
                                composition_input: None,
                                model: None,
                                provider_config_id: None,
                                tool_definitions: None,
                                permissions: None,
                                harness: None,
                            }
                        };
                        // Resolve the new provider before changing the active
                        // context. A missing provider must not leave a
                        // half-applied handoff persisted in the session.
                        let next_provider = if let Some(provider_config_id) =
                            &plan.provider_config_id
                        {
                            let resolver = self.provider_resolver.as_ref().ok_or_else(|| {
                                AgentError::ProviderSelection(format!(
                                    "provider configuration {provider_config_id:?} cannot be resolved"
                                ))
                            })?;
                            Some(resolver.resolve(provider_config_id).await?)
                        } else {
                            None
                        };
                        let next_provider_id = plan.provider_config_id.clone();
                        context
                            .reset(plan.context)
                            .map_err(|error| AgentError::Context(error.to_string()))?;
                        if let Some(next_input) = plan.composition_input {
                            composition_input = Some(next_input);
                        }
                        if let Some(model) = plan.model {
                            active_model = model;
                        }
                        if let Some(provider) = next_provider {
                            active_provider = provider;
                        }
                        if let (Some(surface), Some(definitions)) =
                            (&request.tool_surface, plan.tool_definitions)
                        {
                            surface
                                .replace_static_definitions(definitions)
                                .map_err(|error| AgentError::ToolSurface(error.to_string()))?;
                        }
                        if let (Some(surface), Some((profile, permissions))) =
                            (&request.tool_surface, plan.permissions)
                        {
                            surface.set_permissions(profile, permissions);
                        }
                        if let (Some(surface), Some(harness)) =
                            (&request.tool_surface, plan.harness)
                        {
                            surface.configure_harness(harness);
                        }
                        self.record(AgentEvent::ProfileChanged {
                            profile_id: plan.profile_id,
                            provider_id: next_provider_id,
                            model: Some(active_model.clone()),
                        })?;
                        self.record(AgentEvent::Handoff { profile_id, brief })?;
                    }
                    _ => {}
                }
            }
            for message in request.control.take_steering() {
                self.record(AgentEvent::SteeringAccepted { message })?;
            }
        }
    }

    fn record(&self, event: AgentEvent) -> Result<(), AgentError> {
        let event_type = match &event {
            AgentEvent::TurnStarted { .. } => "agent.turn_started",
            AgentEvent::UserMessage { .. } => "agent.user_message",
            AgentEvent::SteeringAccepted { .. } => "agent.steering_accepted",
            AgentEvent::SurfaceMessage { .. } => "agent.surface_message",
            AgentEvent::ModelEvent { .. } => "agent.model_event",
            AgentEvent::Usage { .. } => "agent.usage",
            AgentEvent::ToolRequested { .. } => "agent.tool_requested",
            AgentEvent::ToolCompleted { .. } => "agent.tool_completed",
            AgentEvent::ProviderObserved { .. } => "agent.provider_observed",
            AgentEvent::ToolObserved { .. } => "agent.tool_observed",
            AgentEvent::ContextCompacted { .. } => "agent.context_compacted",
            AgentEvent::ProfileChanged { .. } => "agent.profile_changed",
            AgentEvent::Yielded { .. } => "agent.yielded",
            AgentEvent::ForkStarted { .. } => "agent.fork_started",
            AgentEvent::ForkCompleted { .. } => "agent.fork_completed",
            AgentEvent::Handoff { .. } => "agent.handoff",
            AgentEvent::TurnCompleted { .. } => "agent.turn_completed",
            AgentEvent::TurnAborted { .. } => "agent.turn_aborted",
        };
        let payload = serde_json::to_value(event).map_err(|error| {
            AgentError::Log(LogError::InvalidRecord {
                line: 0,
                source: error,
            })
        })?;
        self.log.append(event_type, payload)?;
        Ok(())
    }

    fn abort(&self, control: &TurnControl, reason: impl Into<String>) -> Result<(), AgentError> {
        if !control.mark_abort_needed() {
            return Ok(());
        }
        if let Err(error) = self.record(AgentEvent::TurnAborted {
            reason: reason.into(),
        }) {
            control.reset_abort_marker();
            return Err(error);
        }
        Ok(())
    }

    fn check_control(&self, control: &TurnControl) -> Result<(), AgentError> {
        if control.is_cancelled() {
            return Err(self.cancelled(control));
        }
        if control.deadline_exceeded() {
            return Err(self.deadline_exceeded(control));
        }
        Ok(())
    }

    fn cancelled(&self, control: &TurnControl) -> AgentError {
        self.metrics.cancelled();
        match self.abort(control, "cancelled") {
            Ok(()) => AgentError::Cancelled,
            Err(error) => error,
        }
    }

    fn deadline_exceeded(&self, control: &TurnControl) -> AgentError {
        match self.abort(control, "deadline_exceeded") {
            Ok(()) => AgentError::DeadlineExceeded,
            Err(error) => error,
        }
    }

    fn provider_failure(
        &self,
        control: &TurnControl,
        error: llm_provider::ProviderError,
    ) -> AgentError {
        match self.abort(control, format!("provider: {error}")) {
            Ok(()) => AgentError::Provider(error),
            Err(log_error) => log_error,
        }
    }

    fn record_provider_observation(
        &self,
        turn_id: &str,
        provider_call_id: &str,
        provider: &str,
        elapsed: Duration,
        ok: bool,
        error_class: Option<String>,
    ) -> Result<(), AgentError> {
        let latency_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
        self.metrics.provider_call(latency_ms, ok);
        self.record(AgentEvent::ProviderObserved {
            turn_id: turn_id.into(),
            provider_call_id: provider_call_id.into(),
            provider: provider.into(),
            latency_ms,
            ok,
            error_class,
        })
    }

    fn record_tool_observation(
        &self,
        turn_id: &str,
        call: &ToolCall,
        elapsed: Duration,
        ok: bool,
        error_class: Option<String>,
    ) -> Result<(), AgentError> {
        let latency_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
        self.metrics.tool_result(latency_ms, ok);
        self.record(AgentEvent::ToolObserved {
            turn_id: turn_id.into(),
            tool_call_id: call.id.clone(),
            tool: call.name.clone(),
            latency_ms,
            ok,
            error_class,
        })
    }
}

fn provider_error_class(error: &llm_provider::ProviderError) -> String {
    match error {
        llm_provider::ProviderError::Authentication { .. } => "authentication",
        llm_provider::ProviderError::RateLimited { .. } => "rate_limited",
        llm_provider::ProviderError::Request { .. } => "request",
        llm_provider::ProviderError::InvalidResponse { .. } => "invalid_response",
        llm_provider::ProviderError::ContextLimit { .. } => "context_limit",
        llm_provider::ProviderError::Cancelled => "cancelled",
        llm_provider::ProviderError::Unsupported { .. } => "unsupported",
    }
    .into()
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
    use artist_session::Contribution;
    use async_stream::stream;
    use futures::stream::BoxStream;
    use llm_provider::{ContentPart, ModelCapabilities, ProviderError, Role};
    use tempfile::tempdir;
    use tokio::sync::Barrier;

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
            refusal: None,
            incomplete: false,
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
                initial_messages: None,
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
        let metrics = engine.metrics().snapshot();
        assert_eq!(metrics.turns_started, 1);
        assert_eq!(metrics.turns_completed, 1);
        assert_eq!(metrics.tool_calls, 1);
        assert_eq!(metrics.provider_calls, 2);
        assert_eq!(metrics.provider_failures, 0);
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
                initial_messages: None,
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
            initial_messages: None,
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

    struct CapturingProvider {
        responses: std::sync::Mutex<Vec<ModelResponse>>,
        requests: Arc<std::sync::Mutex<Vec<ModelRequest>>>,
    }

    impl ModelProvider for CapturingProvider {
        fn provider_name(&self) -> &str {
            "capturing"
        }

        fn capabilities(&self, _model: &str) -> ModelCapabilities {
            ModelCapabilities {
                streaming: true,
                tool_calls: true,
                ..Default::default()
            }
        }

        fn stream<'a>(&'a self, request: ModelRequest) -> llm_provider::ModelEventStream<'a> {
            self.requests.lock().unwrap().push(request);
            let response = self.responses.lock().unwrap().remove(0);
            Box::pin(stream! {
                yield Ok(ModelEvent::Started);
                yield Ok(ModelEvent::Completed { response });
            })
        }
    }

    struct TestHandoff {
        plan: std::sync::Mutex<Option<HandoffPlan>>,
    }

    #[async_trait::async_trait]
    impl HandoffHandler for TestHandoff {
        async fn resolve(
            &self,
            _profile_id: &str,
            _brief: &str,
        ) -> Result<HandoffPlan, AgentError> {
            self.plan
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| AgentError::Context("handoff resolved more than once".into()))
        }
    }

    struct ConcurrentForks {
        barrier: Arc<Barrier>,
        ingresses: Arc<std::sync::Mutex<Vec<ForkIngress>>>,
    }

    #[async_trait::async_trait]
    impl ForkExecutor for ConcurrentForks {
        async fn execute(
            &self,
            _fork_id: String,
            _context: Snapshot,
            _task: String,
            _control: TurnControl,
        ) -> Result<Value, AgentError> {
            Err(AgentError::Context(
                "snapshot-only fork path was used".into(),
            ))
        }

        async fn execute_with_ingress(
            &self,
            _fork_id: String,
            ingress: ForkIngress,
            task: String,
            _control: TurnControl,
        ) -> Result<Value, AgentError> {
            self.ingresses.lock().unwrap().push(ingress);
            // A sequential implementation deadlocks here. This makes the
            // test prove that all children are live before any child returns.
            self.barrier.wait().await;
            Ok(serde_json::json!({"task": task, "status": "yielded"}))
        }
    }

    #[tokio::test]
    async fn forks_share_one_ingress_and_wake_the_parent_concurrently() {
        let dir = tempdir().unwrap();
        let log = Arc::new(EventLog::open(dir.path().join("session.jsonl"), "session").unwrap());
        let fork_call = ToolCall {
            id: "fork-call".into(),
            name: "fork".into(),
            arguments: serde_json::json!({"tasks": ["one", "two"]}),
        };
        let provider = Arc::new(CapturingProvider {
            responses: std::sync::Mutex::new(vec![
                response("forking", vec![fork_call]),
                response("parent complete", vec![]),
            ]),
            requests: Arc::new(std::sync::Mutex::new(Vec::new())),
        });
        let ingresses = Arc::new(std::sync::Mutex::new(Vec::new()));
        let executor = Arc::new(ConcurrentForks {
            barrier: Arc::new(Barrier::new(2)),
            ingresses: Arc::clone(&ingresses),
        });
        let engine = AgentEngine::new(provider, Arc::new(FakeTools), Arc::clone(&log))
            .with_fork_executor(executor);

        let outcome = tokio::time::timeout(
            Duration::from_millis(500),
            engine.run_turn(TurnRequest {
                model: "fake".into(),
                user: Message::text(Role::User, "delegate"),
                tools: vec![],
                tool_surface: None,
                composition_updater: None,
                composition_input: None,
                context: None,
                initial_messages: None,
                control: TurnControl::default(),
            }),
        )
        .await
        .expect("forks should not deadlock")
        .unwrap();
        assert_eq!(
            outcome.response.unwrap().content,
            response("parent complete", vec![]).content
        );

        let ingresses = ingresses.lock().unwrap();
        assert_eq!(ingresses.len(), 2);
        assert_eq!(ingresses[0], ingresses[1]);
        assert_eq!(
            ingresses[0].messages,
            vec![Message::text(Role::User, "delegate")]
        );
        assert!(engine.replay_events().unwrap().iter().any(|event| {
            matches!(
                event,
                AgentEvent::ForkCompleted { payload, .. }
                    if payload["forks"]
                        .as_array()
                        .is_some_and(|forks| forks.len() == 2)
            )
        }));
    }

    #[tokio::test]
    async fn handoff_replaces_context_and_restarts_the_provider_conversation() {
        let dir = tempdir().unwrap();
        let log = Arc::new(EventLog::open(dir.path().join("session.jsonl"), "session").unwrap());
        let old_context = Snapshot::new([Contribution::new("old", "profile", "old system")]);
        let new_context = Snapshot::new([Contribution::new("new", "profile", "new system")]);
        let handoff_call = ToolCall {
            id: "handoff-call".into(),
            name: "handoff".into(),
            arguments: serde_json::json!({"profile": "reviewer", "brief": "review this"}),
        };
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let provider = Arc::new(CapturingProvider {
            responses: std::sync::Mutex::new(vec![
                response("switching", vec![handoff_call]),
                response("review complete", vec![]),
            ]),
            requests: Arc::clone(&requests),
        });
        let handoff = Arc::new(TestHandoff {
            plan: std::sync::Mutex::new(Some(HandoffPlan {
                profile_id: "reviewer".into(),
                context: new_context,
                composition_input: None,
                model: None,
                provider_config_id: None,
                tool_definitions: None,
                permissions: None,
                harness: None,
            })),
        });
        let engine = AgentEngine::new(provider, Arc::new(FakeTools), Arc::clone(&log))
            .with_handoff_handler(handoff);
        engine
            .run_turn(TurnRequest {
                model: "fake".into(),
                user: Message::text(Role::User, "start"),
                tools: vec![],
                tool_surface: None,
                composition_updater: None,
                composition_input: None,
                context: Some(old_context),
                initial_messages: None,
                control: TurnControl::default(),
            })
            .await
            .unwrap();

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let second = &requests[1].messages;
        assert!(second.iter().any(|message| {
            message
                .content
                .iter()
                .any(|part| matches!(part, ContentPart::Text { text } if text == "new system"))
        }));
        assert!(!second.iter().any(|message| {
            message
                .content
                .iter()
                .any(|part| matches!(part, ContentPart::Text { text } if text == "old system"))
        }));
        assert_eq!(
            second
                .iter()
                .filter(|message| message.content.iter().any(|part| {
                    matches!(part, ContentPart::Text { text } if text == "review this")
                }))
                .count(),
            1
        );
        let records = log.records().unwrap();
        assert!(
            records
                .iter()
                .any(|record| record.event_type == "context.reset")
        );
        assert!(
            records
                .iter()
                .any(|record| record.event_type == "agent.handoff")
        );
        assert!(
            records
                .iter()
                .any(|record| record.event_type == "agent.profile_changed")
        );
    }
}
