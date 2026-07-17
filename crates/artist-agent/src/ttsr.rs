//! The TTSR rig hook and shared run state.
//!
//! `TtsrShared` is created per `stream_chat` call and shared between the
//! [`TtsrHook`] registered on the agent (which sees hook-visible streams:
//! text deltas, tool-call deltas, completion calls) and the outer drive loop
//! in `lib.rs` (which sees reasoning deltas — rig has no reasoning hook
//! event — and performs the abort/inject/retry).
//!
//! Abort mechanics: a match returns `Flow::terminate("ttsr:<rule>")`, which
//! makes rig yield `PromptCancelled { chat_history }` — the committed turns
//! *minus* the offending partial turn. The driver seeds the retry from that
//! history with the rule reminder as the new prompt: the mistake never
//! entered context. Reasoning matches abort from the driver instead, seeded
//! from the history captured here at each `CompletionCall`.

use std::sync::{Arc, Mutex};

use artist_rules::matcher::{RuleSet, StreamMatcher};
use artist_rules::state::RulesHandle;
use artist_rules::types::{Firing, MatchTarget, RuleId};
use rig_core::agent::{AgentHook, Flow, HookContext, RequestPatch, StepEvent, StepEventKind};
use rig_core::completion::message::Message;
use rig_core::completion::{CompletionModel, Document};

pub(crate) struct TtsrShared {
    handle: RulesHandle,
    rules: Arc<RuleSet>,
    /// True when this run is a delegate subagent (rule scope filtering).
    delegate: bool,
    inner: Mutex<TtsrInner>,
}

struct TtsrInner {
    matcher: StreamMatcher,
    /// history + prompt captured at the latest `CompletionCall`: the
    /// committed conversation at the start of the current model turn. The
    /// retry seed for driver-side (reasoning) aborts.
    committed: Vec<Message>,
    /// One-based model-call index within the run, for rule.fired events.
    turn: u32,
    /// The firing that aborted this run, taken by the driver. Per-run (not
    /// on the shared `RulesHandle`) so concurrent delegate runs never race.
    pending: Option<Firing>,
}

impl TtsrShared {
    pub fn new(handle: RulesHandle, rules: Arc<RuleSet>, delegate: bool) -> Arc<Self> {
        Arc::new(Self {
            handle,
            rules: Arc::clone(&rules),
            delegate,
            inner: Mutex::new(TtsrInner {
                matcher: StreamMatcher::new(rules),
                committed: Vec::new(),
                turn: 0,
                pending: None,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TtsrInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Is this rule armed for this run (not fired, not disabled, in scope)?
    fn armed(&self, rule: &RuleId) -> bool {
        if !self.handle.is_armed(rule) {
            return false;
        }
        self.rules
            .get(rule)
            .is_some_and(|compiled| match self.delegate {
                true => compiled.rule.scope.delegate,
                false => compiled.rule.scope.main,
            })
    }

    /// Record a firing. Returns true when the run should abort and retry
    /// (false = budget exhausted, degraded to inject-only). On abort the
    /// firing lands in this run's pending slot for the driver.
    fn fire(&self, firing: Firing) -> bool {
        let policy = self
            .rules
            .get(&firing.rule)
            .map(|compiled| compiled.rule.fire)
            .unwrap_or_default();
        if !self.handle.record_firing(&firing, policy) {
            return false;
        }
        self.lock().pending = Some(firing);
        true
    }

    /// The firing that aborted this run, if any. Distinguishes a TTSR
    /// termination from a real stream failure.
    pub fn take_pending(&self) -> Option<Firing> {
        self.lock().pending.take()
    }

    /// Feed a reasoning-summary delta (driver side — rig has no hook event
    /// for reasoning). Returns true when the driver must abort and retry.
    pub fn push_reasoning(&self, delta: &str) -> bool {
        let firing = {
            let mut inner = self.lock();
            inner
                .matcher
                .push_reasoning(delta, &|rule| self.armed(rule))
        };
        match firing {
            Some(firing) => self.fire(firing),
            None => false,
        }
    }

    /// The retry seed for driver-side aborts, and the turn index.
    pub fn committed(&self) -> (Vec<Message>, u32) {
        let inner = self.lock();
        (inner.committed.clone(), inner.turn)
    }

    pub fn turn(&self) -> u32 {
        self.lock().turn
    }
}

/// The user-role reminder message a fired rule injects as the retry prompt.
/// Never `Message::System` — the ChatGPT provider hoists system messages
/// into `instructions`, away from the failure point.
pub(crate) fn reminder_message(firing: &Firing) -> Message {
    Message::user(reminder_text(&firing.rule, &firing.reminder))
}

fn reminder_text(rule: &RuleId, reminder: &str) -> String {
    format!("<system-reminder rule=\"{rule}\">\n{reminder}\n</system-reminder>")
}

/// The aggregated "active stream rules" context document re-applied on
/// every completion call — `persistence: session` reminders survive any
/// history manipulation (including future compaction) by construction.
fn injection_document(injections: &[(RuleId, String)]) -> Document {
    let mut text = String::from(
        "Active stream rules (previously triggered this session; they still apply):\n",
    );
    for (rule, reminder) in injections {
        text.push_str(&reminder_text(rule, reminder));
        text.push('\n');
    }
    Document {
        id: "artist-stream-rules".to_owned(),
        text,
        additional_props: Default::default(),
    }
}

pub(crate) struct TtsrHook(pub Arc<TtsrShared>);

impl<M: CompletionModel> AgentHook<M> for TtsrHook {
    fn observes(&self, kind: StepEventKind) -> bool {
        match kind {
            StepEventKind::CompletionCall => true,
            StepEventKind::TextDelta => self.0.rules.observes(MatchTarget::AssistantText),
            StepEventKind::ToolCallDelta | StepEventKind::ToolCall => {
                self.0.rules.observes(MatchTarget::ToolArgs)
            }
            _ => false,
        }
    }

    async fn on_event(&self, _context: &HookContext, event: StepEvent<'_, M>) -> Flow {
        let shared = &*self.0;
        match event {
            StepEvent::CompletionCall {
                prompt,
                history,
                turn,
            } => {
                {
                    let mut inner = shared.lock();
                    inner.matcher.reset_turn();
                    inner.committed = history.to_vec();
                    inner.committed.push(prompt.clone());
                    inner.turn = turn as u32;
                }
                let injections = shared.handle.injections();
                if injections.is_empty() {
                    Flow::cont()
                } else {
                    Flow::patch_request(
                        RequestPatch::new().extra_context([injection_document(&injections)]),
                    )
                }
            }
            StepEvent::TextDelta { delta, .. } => {
                let firing = shared
                    .lock()
                    .matcher
                    .push_text(delta, &|rule| shared.armed(rule));
                terminate_if_fired(shared, firing)
            }
            StepEvent::ToolCallDelta {
                internal_call_id,
                tool_name,
                delta,
                ..
            } => {
                let firing = shared.lock().matcher.push_tool_arg_delta(
                    internal_call_id,
                    tool_name,
                    delta,
                    &|rule| shared.armed(rule),
                );
                terminate_if_fired(shared, firing)
            }
            // Final check on complete arguments — fires BEFORE the tool
            // executes (rig honors Terminate on ToolCall).
            StepEvent::ToolCall {
                tool_name,
                internal_call_id,
                args,
                ..
            } => {
                let firing = shared.lock().matcher.tool_call_complete(
                    internal_call_id,
                    tool_name,
                    args,
                    &|rule| shared.armed(rule),
                );
                terminate_if_fired(shared, firing)
            }
            _ => Flow::cont(),
        }
    }
}

fn terminate_if_fired(shared: &TtsrShared, firing: Option<Firing>) -> Flow {
    match firing {
        Some(firing) => {
            let rule = firing.rule.clone();
            if shared.fire(firing) {
                Flow::terminate(format!("ttsr:{rule}"))
            } else {
                Flow::cont()
            }
        }
        None => Flow::cont(),
    }
}
