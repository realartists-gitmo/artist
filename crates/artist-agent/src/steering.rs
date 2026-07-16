use rig_core::{
    agent::{AgentHook, Flow, HookContext, StepEvent},
    completion::CompletionModel,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub struct SteeringHandle {
    inner: Arc<Mutex<SteeringState>>,
}

#[derive(Default)]
struct SteeringState {
    pending: VecDeque<String>,
    delivered: VecDeque<String>,
    original_results: HashMap<String, String>,
}

pub struct SteeringMutation {
    pub applied: bool,
    pub delivered: Vec<String>,
}

impl SteeringHandle {
    pub fn enqueue(&self, message: String) {
        self.lock().pending.push_back(message);
    }

    pub fn edit_pending(&self, index: usize, message: String) -> SteeringMutation {
        let mut state = self.lock();
        let delivered = state.delivered.drain(..).collect();
        let applied = if let Some(pending) = state.pending.get_mut(index) {
            *pending = message;
            true
        } else {
            false
        };
        SteeringMutation { applied, delivered }
    }

    pub fn remove_pending(&self, index: usize) -> SteeringMutation {
        let mut state = self.lock();
        let delivered = state.delivered.drain(..).collect();
        let applied = state.pending.remove(index).is_some();
        SteeringMutation { applied, delivered }
    }

    pub fn take_delivered(&self) -> Vec<String> {
        self.lock().delivered.drain(..).collect()
    }

    pub(crate) fn take_original_result(&self, internal_call_id: &str) -> Option<String> {
        self.lock().original_results.remove(internal_call_id)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SteeringState> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }
}

#[derive(Clone)]
pub(crate) struct SteeringHook(pub SteeringHandle);

impl<M: CompletionModel> AgentHook<M> for SteeringHook {
    async fn on_event(&self, _context: &HookContext, event: StepEvent<'_, M>) -> Flow {
        let StepEvent::ToolResult {
            result,
            internal_call_id,
            ..
        } = event
        else {
            return Flow::cont();
        };
        let messages = {
            let mut state = self.0.lock();
            let messages = state.pending.drain(..).collect::<Vec<_>>();
            state.delivered.extend(messages.iter().cloned());
            if !messages.is_empty() {
                state
                    .original_results
                    .insert(internal_call_id.to_owned(), result.to_owned());
            }
            messages
        };
        if messages.is_empty() {
            return Flow::cont();
        }
        let steering = messages
            .iter()
            .map(|message| format!("<user_steering>\n{message}\n</user_steering>"))
            .collect::<Vec<_>>()
            .join("\n\n");
        Flow::rewrite_result(format!("{result}\n\n{steering}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomically_edits_and_removes_pending_steering() {
        let handle = SteeringHandle::default();
        handle.enqueue("first".into());
        handle.enqueue("second".into());
        assert!(handle.edit_pending(1, "changed".into()).applied);
        assert!(handle.remove_pending(0).applied);
        let mut state = handle.lock();
        assert_eq!(
            state.pending.iter().cloned().collect::<Vec<_>>(),
            ["changed"]
        );
        let delivered = state.pending.pop_front().unwrap();
        state.delivered.push_back(delivered);
        drop(state);
        let mutation = handle.edit_pending(0, "too late".into());
        assert!(!mutation.applied);
        assert_eq!(mutation.delivered, ["changed"]);
    }
}
