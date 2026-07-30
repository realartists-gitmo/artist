//! Captures structured tool outcomes for Artist's live display stream.
//!
//! Conversation persistence is owned by Rig's `ConversationMemory`; this hook
//! only supplies metadata absent from Rig's streamed user items.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use artist_session::ToolOutcomeRecord;
use rig_core::agent::{AgentHook, Flow, HookContext, StepEvent, StepEventKind};
use rig_core::completion::CompletionModel;
use rig_core::tool::ToolOutcome;

/// Structured tool metadata keyed by `internal_call_id`, shared with the
/// stream loop so `PromptEvent::ToolResult` can carry outcome + timing
/// (the `StreamUserItem` the display path sees has neither).
#[derive(Clone, Default)]
pub(crate) struct ToolMeta {
    inner: Arc<Mutex<HashMap<String, (ToolOutcomeRecord, u64)>>>,
}

impl ToolMeta {
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

fn outcome_record(outcome: &ToolOutcome, result: &str) -> ToolOutcomeRecord {
    match outcome {
        ToolOutcome::Success => ToolOutcomeRecord::Success,
        ToolOutcome::Error(failure) => ToolOutcomeRecord::Error {
            kind: Some(format!("{failure:?}")),
            message: result.to_owned(),
        },
        ToolOutcome::Skipped => ToolOutcomeRecord::Skipped {
            reason: result.to_owned(),
        },
        ToolOutcome::Denied => ToolOutcomeRecord::Denied {
            reason: result.to_owned(),
        },
        // ToolOutcome is #[non_exhaustive].
        _ => ToolOutcomeRecord::Success,
    }
}

impl<M: CompletionModel> AgentHook<M> for CaptureHook {
    fn observes(&self, kind: StepEventKind) -> bool {
        matches!(kind, StepEventKind::ToolCall | StepEventKind::ToolResult)
    }

    async fn on_event(&self, _context: &HookContext, event: StepEvent<'_, M>) -> Flow {
        match event {
            StepEvent::ToolCall {
                internal_call_id, ..
            } => {
                self.starts
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(internal_call_id.to_owned(), Instant::now());
            }
            StepEvent::ToolResult {
                tool_name,
                tool_call_id,
                internal_call_id,
                args,
                result,
                outcome,
                ..
            } => {
                let duration_ms = self
                    .starts
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(internal_call_id)
                    .map(|start| start.elapsed().as_millis() as u64);
                let record = outcome_record(outcome, result);
                self.meta.lock().insert(
                    internal_call_id.to_owned(),
                    (record.clone(), duration_ms.unwrap_or(0)),
                );
                let _ = (tool_name, tool_call_id, args);
            }
            _ => {}
        }
        Flow::cont()
    }
}
