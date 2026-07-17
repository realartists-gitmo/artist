//! The capture hook: records committed model turns and tool results into
//! the session event log, and stashes structured tool outcomes for the
//! display stream.
//!
//! Registered AFTER `SteeringHook` so the recorded tool-result text is the
//! model-visible version (steering rewrites chain through the hook stack);
//! `outcome` is rig's raw structured result regardless of rewrites.
//! `observes` excludes all delta events — zero hot-path cost.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use artist_session::{AttachmentStore, ModelTurn, Recorder, ToolOutcomeRecord, ToolResultEvent};
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
    recorder: Recorder,
    attachments: AttachmentStore,
    meta: ToolMeta,
    starts: Mutex<HashMap<String, Instant>>,
}

impl CaptureHook {
    pub fn new(recorder: Recorder, attachments: AttachmentStore, meta: ToolMeta) -> Self {
        Self {
            recorder,
            attachments,
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
        matches!(
            kind,
            StepEventKind::ModelTurnFinished | StepEventKind::ToolCall | StepEventKind::ToolResult
        )
    }

    async fn on_event(&self, _context: &HookContext, event: StepEvent<'_, M>) -> Flow {
        match event {
            // The canonical commit point: exactly what rig accepts into the
            // run, including reasoning items and tool calls.
            StepEvent::ModelTurnFinished {
                turn,
                content,
                usage,
            } => {
                let blocks = content
                    .iter()
                    .flat_map(|item| artist_session::assistant_to_blocks(item, &self.attachments))
                    .collect::<Vec<_>>();
                self.recorder.record(ModelTurn {
                    turn: turn as u32,
                    content: blocks,
                    total_tokens: usage.total_tokens,
                    partial: false,
                });
            }
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
                self.recorder.record(ToolResultEvent {
                    internal_call_id: internal_call_id.to_owned(),
                    tool_call_id: tool_call_id.map(str::to_owned),
                    name: tool_name.to_owned(),
                    arguments: serde_json::from_str(args)
                        .unwrap_or_else(|_| serde_json::Value::String(args.to_owned())),
                    result: result.to_owned(),
                    outcome: record,
                    duration_ms,
                });
            }
            _ => {}
        }
        Flow::cont()
    }
}
