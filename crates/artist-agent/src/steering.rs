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

impl SteeringHandle {
    pub fn replace_pending(&self, messages: &[String]) {
        let mut state = self.lock();
        let delivered = state.delivered.iter().cloned().collect::<Vec<_>>();
        state.pending = messages
            .iter()
            .filter(|message| !delivered.contains(message))
            .cloned()
            .collect();
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
    fn replaces_and_drains_steering() {
        let handle = SteeringHandle::default();
        handle.replace_pending(&["first".into(), "second".into()]);
        let mut state = handle.lock();
        assert_eq!(state.pending.pop_front().as_deref(), Some("first"));
        assert_eq!(state.pending.pop_front().as_deref(), Some("second"));
        state.delivered.push_back("first".into());
        drop(state);
        handle.replace_pending(&["first".into(), "third".into()]);
        let state = handle.lock();
        assert_eq!(state.pending.iter().cloned().collect::<Vec<_>>(), ["third"]);
    }
}
