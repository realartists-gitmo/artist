//! Per-session rule state shared between the TUI, the TTSR hook, and the
//! retry driver — the `SteeringHandle` pattern: a clonable handle over
//! `Arc<Mutex<..>>` with short critical sections.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use artist_session::{Envelope, SessionEvent};

use crate::types::{FirePolicy, Firing, Persistence, RuleId};

/// Default cap on TTSR retries per user prompt. `fire: once` rules bound
/// retries naturally; the cap is the backstop for per-turn and wasm rules.
pub const DEFAULT_RETRY_BUDGET: u32 = 4;

#[derive(Default)]
struct SessionState {
    /// Rules that have fired (once-per-session bookkeeping). Holds both
    /// `Once` rules (permanent) and `PerTurn` rules (cleared each prompt).
    fired: HashSet<RuleId>,
    per_turn_fired: HashSet<RuleId>,
    /// `(rule, reminder)` pairs re-injected on every completion call.
    active_injections: Vec<(RuleId, String)>,
    retries_this_prompt: u32,
    retry_budget: u32,
    /// Rules disabled at runtime from `/rules` (session-scoped).
    disabled: HashSet<RuleId>,
    /// Per-rule hit counts for the `/rules` panel.
    hits: Vec<(RuleId, u32)>,
}

/// Clonable handle to one session's rule state. Shared by the CLI, the TTSR
/// hook inside the agent run, and delegate subagent runs (making
/// once-per-session global across main + delegates).
#[derive(Clone)]
pub struct RulesHandle {
    state: Arc<Mutex<SessionState>>,
}

impl Default for RulesHandle {
    fn default() -> Self {
        Self::new(DEFAULT_RETRY_BUDGET)
    }
}

impl RulesHandle {
    pub fn new(retry_budget: u32) -> Self {
        Self {
            state: Arc::new(Mutex::new(SessionState {
                retry_budget,
                ..SessionState::default()
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SessionState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A new user prompt began: re-arm `per-turn` rules, reset the retry
    /// budget.
    pub fn note_user_turn(&self) {
        let mut state = self.lock();
        state.retries_this_prompt = 0;
        let per_turn = std::mem::take(&mut state.per_turn_fired);
        for rule in per_turn {
            state.fired.remove(&rule);
        }
    }

    /// Is this rule armed (not fired, not disabled)?
    pub fn is_armed(&self, rule: &RuleId) -> bool {
        let state = self.lock();
        !state.fired.contains(rule) && !state.disabled.contains(rule)
    }

    /// Record a firing: marks the rule fired, tallies the hit, and activates
    /// a persistent injection when applicable. Returns `true` when the retry
    /// budget allows an abort-and-retry; `false` when exhausted (the firing
    /// degrades to inject-only). The firing itself is NOT stashed here — the
    /// abort slot is per-run (in the TTSR hook state), because delegates
    /// share this handle and concurrent runs must not race one slot.
    pub fn record_firing(&self, firing: &Firing, fire: FirePolicy) -> bool {
        let mut state = self.lock();
        state.fired.insert(firing.rule.clone());
        if fire == FirePolicy::PerTurn {
            state.per_turn_fired.insert(firing.rule.clone());
        }
        match state.hits.iter_mut().find(|(rule, _)| *rule == firing.rule) {
            Some((_, count)) => *count += 1,
            None => state.hits.push((firing.rule.clone(), 1)),
        }
        if firing.persistence == Persistence::Session {
            let already = state
                .active_injections
                .iter()
                .any(|(rule, _)| *rule == firing.rule);
            if !already {
                state
                    .active_injections
                    .push((firing.rule.clone(), firing.reminder.clone()));
            }
        }
        if state.retries_this_prompt >= state.retry_budget {
            return false;
        }
        state.retries_this_prompt += 1;
        true
    }

    /// Active session-persistent reminders, for per-turn re-injection.
    pub fn injections(&self) -> Vec<(RuleId, String)> {
        self.lock().active_injections.clone()
    }

    /// Add a persistent injection without a time-travel abort (inject-only
    /// mode: retro `inject`, budget-exhausted firings).
    pub fn add_injection(&self, rule: RuleId, reminder: String) {
        let mut state = self.lock();
        state.fired.insert(rule.clone());
        if !state.active_injections.iter().any(|(id, _)| *id == rule) {
            state.active_injections.push((rule, reminder));
        }
    }

    /// Runtime enable/disable from `/rules` (session-scoped).
    pub fn set_disabled(&self, rule: RuleId, disabled: bool) {
        let mut state = self.lock();
        if disabled {
            state.disabled.insert(rule);
        } else {
            state.disabled.remove(&rule);
        }
    }

    /// Rules currently disabled from `/rules` (session-scoped).
    pub fn disabled(&self) -> Vec<RuleId> {
        let state = self.lock();
        let mut disabled: Vec<RuleId> = state.disabled.iter().cloned().collect();
        disabled.sort();
        disabled
    }

    /// `(rule, hits)` tallies for the `/rules` panel.
    pub fn hits(&self) -> Vec<(RuleId, u32)> {
        self.lock().hits.clone()
    }

    pub fn fired(&self) -> Vec<RuleId> {
        let state = self.lock();
        let mut fired: Vec<RuleId> = state.fired.iter().cloned().collect();
        fired.sort();
        fired
    }

    /// Rebuild fired/injection state from a session's event log so
    /// once-semantics and injections survive process restarts (`-r`).
    pub fn restore_from_log(&self, events: &[Envelope]) {
        let mut state = self.lock();
        for envelope in events {
            match envelope.event() {
                SessionEvent::RuleFired(fired) => {
                    state.fired.insert(RuleId(fired.rule.clone()));
                    match state.hits.iter_mut().find(|(rule, _)| rule.0 == fired.rule) {
                        Some((_, count)) => *count += 1,
                        None => state.hits.push((RuleId(fired.rule), 1)),
                    }
                }
                SessionEvent::RuleInjection(injection) => {
                    let rule = RuleId(injection.rule.clone());
                    if !state.active_injections.iter().any(|(id, _)| *id == rule) {
                        state
                            .active_injections
                            .push((rule, injection.reminder.clone()));
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MatchTarget;

    fn firing(rule: &str, persistence: Persistence) -> Firing {
        Firing {
            rule: RuleId(rule.into()),
            target: MatchTarget::AssistantText,
            matched: "x".into(),
            reminder: format!("reminder for {rule}"),
            persistence,
        }
    }

    #[test]
    fn once_rule_fires_once_per_session() {
        let handle = RulesHandle::default();
        let rule = RuleId("r".into());
        assert!(handle.is_armed(&rule));
        assert!(handle.record_firing(&firing("r", Persistence::Session), FirePolicy::Once));
        assert!(!handle.is_armed(&rule));
        handle.note_user_turn();
        assert!(
            !handle.is_armed(&rule),
            "once rules stay fired across turns"
        );
    }

    #[test]
    fn per_turn_rule_rearms_on_user_turn() {
        let handle = RulesHandle::default();
        let rule = RuleId("r".into());
        assert!(handle.record_firing(&firing("r", Persistence::Message), FirePolicy::PerTurn));
        assert!(!handle.is_armed(&rule));
        handle.note_user_turn();
        assert!(handle.is_armed(&rule));
    }

    #[test]
    fn retry_budget_degrades_to_inject_only() {
        let handle = RulesHandle::new(2);
        assert!(handle.record_firing(&firing("a", Persistence::Session), FirePolicy::Once));
        assert!(handle.record_firing(&firing("b", Persistence::Session), FirePolicy::Once));
        // Budget exhausted: recorded but no abort.
        assert!(!handle.record_firing(&firing("c", Persistence::Session), FirePolicy::Once));
        // The reminder still becomes an active injection.
        assert_eq!(handle.injections().len(), 3);
    }

    #[test]
    fn session_persistence_injections_deduplicate() {
        let handle = RulesHandle::default();
        handle.record_firing(&firing("a", Persistence::Session), FirePolicy::PerTurn);
        handle.note_user_turn();
        handle.record_firing(&firing("a", Persistence::Session), FirePolicy::PerTurn);
        assert_eq!(handle.injections().len(), 1);
        assert_eq!(handle.hits(), vec![(RuleId("a".into()), 2)]);
    }

    #[test]
    fn message_persistence_does_not_linger() {
        let handle = RulesHandle::default();
        handle.record_firing(&firing("a", Persistence::Message), FirePolicy::Once);
        assert!(handle.injections().is_empty());
    }

    #[test]
    fn disabled_rules_are_not_armed() {
        let handle = RulesHandle::default();
        let rule = RuleId("r".into());
        handle.set_disabled(rule.clone(), true);
        assert!(!handle.is_armed(&rule));
        handle.set_disabled(rule.clone(), false);
        assert!(handle.is_armed(&rule));
    }

    #[test]
    fn restore_from_log_rebuilds_fired_and_injections() {
        use artist_session::{RuleFired, RuleInjection, SCHEMA_VERSION};
        let events = vec![
            Envelope {
                v: SCHEMA_VERSION,
                seq: 0,
                ts: 0,
                session: "s".into(),
                run: None,
                lineage: "main".into(),
                kind: "rule.fired".into(),
                payload: serde_json::to_value(RuleFired {
                    rule: "r".into(),
                    target: "assistant-text".into(),
                    matched: "x".into(),
                    turn: 1,
                })
                .unwrap(),
            },
            Envelope {
                v: SCHEMA_VERSION,
                seq: 1,
                ts: 0,
                session: "s".into(),
                run: None,
                lineage: "main".into(),
                kind: "rule.injection".into(),
                payload: serde_json::to_value(RuleInjection {
                    rule: "r".into(),
                    reminder: "don't".into(),
                })
                .unwrap(),
            },
        ];
        let handle = RulesHandle::default();
        handle.restore_from_log(&events);
        assert!(!handle.is_armed(&RuleId("r".into())));
        assert_eq!(handle.injections().len(), 1);
    }
}
