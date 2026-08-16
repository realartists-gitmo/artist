//! Captures structured tool outcomes for Artist's live display stream.
//!
//! Conversation persistence is owned by Rig's `ConversationMemory`; this hook
//! only supplies metadata absent from Rig's streamed user items.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use artist_session::ToolOutcomeRecord;
use rig_agent::agent::{
    AgentHook, HookContext, StepEventKind, ToolCall, ToolCallAction, ToolResultAction,
    ToolResultEvent,
};

/// Structured tool metadata keyed by `internal_call_id`, shared with the
/// stream loop so `PromptEvent::ToolResult` can carry outcome + timing
/// (the `StreamUserItem` the display path sees has neither).
#[derive(Clone, Default)]
pub(crate) struct ToolMeta {
    inner: Arc<Mutex<HashMap<String, (ToolOutcomeRecord, u64)>>>,
}

impl ToolMeta {
    pub fn record(
        &self,
        internal_call_id: impl Into<String>,
        outcome: ToolOutcomeRecord,
        duration_ms: u64,
    ) {
        self.lock()
            .insert(internal_call_id.into(), (outcome, duration_ms));
    }

    pub fn take(&self, internal_call_id: &str) -> Option<(ToolOutcomeRecord, u64)> {
        self.lock().remove(internal_call_id)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (ToolOutcomeRecord, u64)>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub(crate) struct CaptureHook {
    meta: ToolMeta,
    starts: Mutex<HashMap<String, Instant>>,
}

impl CaptureHook {
    pub fn new(meta: ToolMeta) -> Self {
        Self {
            meta,
            starts: Mutex::new(HashMap::new()),
        }
    }
}

impl AgentHook for CaptureHook {
    fn observes(&self, kind: StepEventKind) -> bool {
        matches!(kind, StepEventKind::ToolCall | StepEventKind::ToolResult)
    }

    async fn on_tool_call(&self, _context: &HookContext, event: ToolCall<'_>) -> ToolCallAction {
        self.starts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(event.internal_call_id.to_owned(), Instant::now());
        ToolCallAction::run()
    }

    async fn on_tool_result(
        &self,
        _context: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> ToolResultAction {
        let duration_ms = self
            .starts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(event.internal_call_id)
            .map(|start| start.elapsed().as_millis() as u64)
            .unwrap_or(0);
        let result = event.presentation.as_text().unwrap_or_default();
        let raw = event.raw_result;
        let record = if raw.is_success() {
            ToolOutcomeRecord::Success
        } else if let Some(error) = raw.error() {
            ToolOutcomeRecord::Error {
                kind: Some(format!("{:?}", error.kind())),
                message: result.to_owned(),
            }
        } else if raw.is_skipped() {
            ToolOutcomeRecord::Skipped {
                reason: result.to_owned(),
            }
        } else if raw.is_refused() {
            ToolOutcomeRecord::Denied {
                reason: result.to_owned(),
            }
        } else {
            ToolOutcomeRecord::Success
        };
        self.meta
            .lock()
            .insert(event.internal_call_id.to_owned(), (record, duration_ms));
        ToolResultAction::keep()
    }
}
