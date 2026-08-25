use std::sync::Arc;

use artist_core::{
    InitialContext, InvocationScope, ModelRoute, PluginFact, ProjectionArtifact, SessionId,
    StreamEvent,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use crate::ModelRequest;

/// Capacity of the per-session plugin-fact channel. A full channel applies
/// backpressure to the emitting plugin instead of growing without bound.
pub const PLUGIN_FACT_CHANNEL_CAPACITY: usize = 64;

/// One emission batch awaiting canonical append, plus the receipt the emitter
/// awaits. Success is returned to the plugin only after the owning session
/// actor durably appends the facts and publishes their stream events.
#[derive(Debug)]
pub struct FactEnvelope {
    pub session_id: SessionId,
    pub facts: Vec<PluginFact>,
    pub receipt: oneshot::Sender<Result<(), String>>,
}

/// Sender half handed to the extension host for one live session. Bounded:
/// `send().await` applies backpressure to emitting plugins.
pub type FactSink = mpsc::Sender<FactEnvelope>;
pub type FactDrain = mpsc::Receiver<FactEnvelope>;

/// Open a bounded fact channel pair for one session.
pub fn fact_channel() -> (FactSink, FactDrain) {
    mpsc::channel(PLUGIN_FACT_CHANNEL_CAPACITY)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookPhase {
    BeforeModelRequest,
    AfterModelResponse,
    BeforeToolExecution,
    AfterToolResult,
    RunCompleted,
    RunInterrupted,
    RunFailed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleHookEvent {
    pub phase: HookPhase,
    pub scope: InvocationScope,
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "decision", rename_all = "kebab-case")]
pub enum LifecycleHookDecision {
    Proceed,
    Stop { reason: String },
    Rewrite { value: serde_json::Value },
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("execution extension failed during {phase}: {message}")]
pub struct ExecutionExtensionError {
    pub phase: &'static str,
    pub message: String,
}

impl ExecutionExtensionError {
    pub fn new(phase: &'static str, message: impl Into<String>) -> Self {
        Self {
            phase,
            message: message.into(),
        }
    }
}

/// Kernel-owned execution boundary. Implementations may be WASM plugin hosts,
/// native extension chains, or the identity implementation. Critical-path
/// methods are awaited and fail atomically; committed event observation is a
/// non-blocking enqueue and cannot rewrite the canonical outcome.
#[async_trait]
pub trait ExecutionExtensions: Send + Sync + 'static {
    async fn compose_initial_context(
        &self,
        context: InitialContext,
    ) -> Result<InitialContext, ExecutionExtensionError> {
        Ok(context)
    }

    async fn prepare_model_request(
        &self,
        _request: &mut ModelRequest,
    ) -> Result<(), ExecutionExtensionError> {
        Ok(())
    }

    async fn configure_model(
        &self,
        _scope: &InvocationScope,
        route: ModelRoute,
    ) -> Result<ModelRoute, ExecutionExtensionError> {
        Ok(route)
    }

    async fn compact_context(
        &self,
        _scope: &InvocationScope,
        _history: &[crate::ModelHistoryItem],
    ) -> Result<Option<ProjectionArtifact>, ExecutionExtensionError> {
        Ok(None)
    }

    async fn hook(
        &self,
        _event: LifecycleHookEvent,
    ) -> Result<Vec<LifecycleHookDecision>, ExecutionExtensionError> {
        Ok(vec![LifecycleHookDecision::Proceed])
    }

    /// Drain schema registrations and emitted plugin facts produced while
    /// executing this session's extension calls. The session actor appends them
    /// before continuing model execution. Hosts that bind a per-session fact
    /// sink instead deliver synchronously and return an empty drain.
    async fn drain_plugin_facts(&self, _session_id: &SessionId) -> Vec<PluginFact> {
        Vec::new()
    }

    /// Install the canonical fact sink for a live session. Emissions sent to
    /// the sink are durably appended by the owning session actor before the
    /// emitter's receipt resolves. The default implementation ignores it and
    /// hosts fall back to `drain_plugin_facts`.
    fn bind_fact_sink(&self, _session_id: &SessionId, _sink: FactSink) {}

    /// Drop the sink binding when the session actor shuts down.
    fn unbind_fact_sink(&self, _session_id: &SessionId) {}

    /// Enqueue an already committed canonical event for ordered observation.
    /// Implementations own durable cursors, retry, and dead-letter handling.
    fn observe_committed(&self, _event: StreamEvent) {}
}

#[derive(Default)]
pub struct NoopExecutionExtensions;

impl ExecutionExtensions for NoopExecutionExtensions {}

pub fn no_extensions() -> Arc<dyn ExecutionExtensions> {
    Arc::new(NoopExecutionExtensions)
}
