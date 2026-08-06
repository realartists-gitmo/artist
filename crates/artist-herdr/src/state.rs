use crate::HerdrState;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct Activity {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<ActivityState>,
    on_change: Arc<dyn Fn(HerdrState) + Send + Sync>,
}

#[derive(Default)]
struct ActivityState {
    claimed: bool,
    next_generation: u64,
    current_turn: Option<u64>,
    closed_through: u64,
    tools: HashSet<(u64, String)>,
    subagents: HashSet<(u64, String)>,
    blockers: HashSet<String>,
    last_emitted: Option<HerdrState>,
}

impl ActivityState {
    fn derived(&self) -> Option<HerdrState> {
        self.claimed.then(|| {
            if !self.blockers.is_empty() {
                HerdrState::Blocked
            } else if self.current_turn.is_some()
                || !self.tools.is_empty()
                || !self.subagents.is_empty()
            {
                HerdrState::Working
            } else {
                HerdrState::Idle
            }
        })
    }
}

impl Activity {
    pub fn new(on_change: impl Fn(HerdrState) + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(ActivityState::default()),
                on_change: Arc::new(on_change),
            }),
        }
    }

    pub fn claim_idle(&self) {
        self.update(|state| state.claimed = true);
    }

    pub fn start_turn(&self) -> TurnActivity {
        let generation = self.update_with(|state| {
            if let Some(previous) = state.current_turn.take() {
                state
                    .tools
                    .retain(|(generation, _)| *generation != previous);
                state.closed_through = state.closed_through.max(previous);
            }
            state.claimed = true;
            state.next_generation += 1;
            state.current_turn = Some(state.next_generation);
            state.next_generation
        });
        TurnActivity {
            activity: self.clone(),
            generation,
        }
    }

    pub fn set_blocked(&self, reason: impl Into<String>, blocked: bool) {
        let reason = reason.into();
        self.update(|state| {
            if blocked {
                state.claimed = true;
                state.blockers.insert(reason);
            } else {
                state.blockers.remove(&reason);
            }
        });
    }

    fn update(&self, change: impl FnOnce(&mut ActivityState)) {
        self.update_with(|state| change(state));
    }

    fn update_with<T>(&self, change: impl FnOnce(&mut ActivityState) -> T) -> T {
        let (output, changed) = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let output = change(&mut state);
            let derived = state.derived();
            let changed = (derived != state.last_emitted).then_some(derived).flatten();
            if changed.is_some() {
                state.last_emitted = derived;
            }
            (output, changed)
        };
        if let Some(state) = changed {
            (self.inner.on_change)(state);
        }
        output
    }
}

#[derive(Clone)]
pub struct TurnActivity {
    activity: Activity,
    generation: u64,
}

impl TurnActivity {
    pub fn tool_started(&self, id: impl Into<String>) {
        self.start_item(id.into(), true);
    }

    pub fn tool_finished(&self, id: &str) {
        self.finish_item(id, true);
    }

    pub fn subagent_started(&self, id: impl Into<String>) {
        self.start_item(id.into(), false);
    }

    pub fn subagent_finished(&self, id: &str) {
        self.finish_item(id, false);
    }

    pub fn finish(&self) {
        self.close(false);
    }

    pub fn cancel(&self) {
        self.close(true);
    }

    fn start_item(&self, id: String, tool: bool) {
        self.activity.update(|state| {
            if self.generation <= state.closed_through {
                return;
            }
            let items = if tool {
                &mut state.tools
            } else {
                &mut state.subagents
            };
            items.insert((self.generation, id));
        });
    }

    fn finish_item(&self, id: &str, tool: bool) {
        self.activity.update(|state| {
            let items = if tool {
                &mut state.tools
            } else {
                &mut state.subagents
            };
            items.remove(&(self.generation, id.to_owned()));
        });
    }

    fn close(&self, cancelled: bool) {
        self.activity.update(|state| {
            if state.current_turn == Some(self.generation) {
                state.current_turn = None;
            }
            state.closed_through = state.closed_through.max(self.generation);
            state
                .tools
                .retain(|(generation, _)| *generation != self.generation);
            if cancelled {
                state
                    .subagents
                    .retain(|(generation, _)| *generation != self.generation);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Activity, Arc<Mutex<Vec<HerdrState>>>) {
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let capture = emitted.clone();
        let activity = Activity::new(move |state| capture.lock().unwrap().push(state));
        (activity, emitted)
    }

    fn states(emitted: &Arc<Mutex<Vec<HerdrState>>>) -> Vec<HerdrState> {
        emitted.lock().unwrap().clone()
    }

    #[test]
    fn aggregates_turn_tools_and_background_subagents() {
        let (activity, emitted) = fixture();
        activity.claim_idle();
        let turn = activity.start_turn();
        turn.tool_started("tool");
        turn.subagent_started("child");
        turn.tool_finished("tool");
        turn.finish();
        assert_eq!(
            states(&emitted),
            vec![HerdrState::Idle, HerdrState::Working]
        );

        turn.subagent_finished("child");
        assert_eq!(
            states(&emitted),
            vec![HerdrState::Idle, HerdrState::Working, HerdrState::Idle]
        );
    }

    #[test]
    fn blockers_take_precedence_and_resolve_to_working() {
        let (activity, emitted) = fixture();
        let turn = activity.start_turn();
        activity.set_blocked("approval", true);
        activity.set_blocked("question", true);
        activity.set_blocked("approval", false);
        assert_eq!(
            states(&emitted),
            vec![HerdrState::Working, HerdrState::Blocked]
        );
        activity.set_blocked("question", false);
        turn.finish();
        assert_eq!(
            states(&emitted),
            vec![
                HerdrState::Working,
                HerdrState::Blocked,
                HerdrState::Working,
                HerdrState::Idle,
            ]
        );
    }

    #[test]
    fn cancellation_clears_children_and_rejects_late_starts() {
        let (activity, emitted) = fixture();
        let turn = activity.start_turn();
        turn.tool_started("tool");
        turn.subagent_started("child");
        turn.cancel();
        turn.tool_started("late-tool");
        turn.subagent_started("late-child");
        turn.tool_finished("tool");
        turn.subagent_finished("child");
        assert_eq!(
            states(&emitted),
            vec![HerdrState::Working, HerdrState::Idle]
        );
    }

    #[test]
    fn identical_states_are_deduplicated() {
        let (activity, emitted) = fixture();
        activity.claim_idle();
        activity.claim_idle();
        activity.set_blocked("auth", false);
        assert_eq!(states(&emitted), vec![HerdrState::Idle]);
    }
}
