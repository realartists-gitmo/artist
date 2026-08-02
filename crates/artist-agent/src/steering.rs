use rig_agent::agent::{AgentHook, HookContext, ToolResultAction, ToolResultEvent};
use rig_core::{completion::message::ToolResultContent, tool::ToolOutput};
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
        let delivered = state.delivered.drain(..).collect::<Vec<_>>();
        let pending_index = index.checked_sub(delivered.len());
        let applied =
            if let Some(pending) = pending_index.and_then(|index| state.pending.get_mut(index)) {
                *pending = message;
                true
            } else {
                false
            };
        SteeringMutation { applied, delivered }
    }

    pub fn remove_pending(&self, index: usize) -> SteeringMutation {
        let mut state = self.lock();
        let delivered = state.delivered.drain(..).collect::<Vec<_>>();
        let applied = index
            .checked_sub(delivered.len())
            .and_then(|index| state.pending.remove(index))
            .is_some();
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

/// Injects user steering and inter-agent messages at the same seam.
///
/// One hook rather than two, deliberately. This is where "was it already put in
/// front of the model" is decided, and TTSR's abort-and-reinject makes that a
/// question with a subtle answer — one already pinned by
/// `delivered_steering_survives_abort_without_double_delivery`. A second
/// delivery path would have to rediscover it.
#[derive(Clone)]
pub(crate) struct SteeringHook {
    pub steering: SteeringHandle,
    /// `None` for an agent with no identity, which simply receives no mail.
    pub inbox: Option<crate::messaging::Inbox>,
}

impl AgentHook for SteeringHook {
    async fn on_tool_result(
        &self,
        _context: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> ToolResultAction {
        // `render()` rather than `as_text()`: the latter yields `None` for any
        // multi-block output, so a multimodal result would stash an empty
        // original and the display would lose it on restore.
        let result = event.presentation.render();
        let messages = {
            let mut state = self.steering.lock();
            let messages = state.pending.drain(..).collect::<Vec<_>>();
            state.delivered.extend(messages.iter().cloned());
            if !messages.is_empty() {
                state
                    .original_results
                    .insert(event.internal_call_id.to_owned(), result);
            }
            messages
        };

        let mut blocks: Vec<String> = messages
            .iter()
            .map(|message| format!("<user_steering>\n{message}\n</user_steering>"))
            .collect();
        // Collected here, at the same boundary, so the reply target is bound
        // when the model is actually shown the message — not when it later
        // decides to answer.
        if let Some(agent_mail) = self.inbox.as_ref().and_then(crate::messaging::Inbox::collect) {
            blocks.push(agent_mail);
        }

        if blocks.is_empty() {
            return ToolResultAction::keep();
        }
        ToolResultAction::rewrite_output(append_steering(event.presentation, blocks.join("\n\n")))
    }
}

/// Attach steering to a tool result without discarding any of its content.
///
/// Rewriting through a plain string would collapse a multimodal result to text,
/// silently dropping every image block. Steering merges into the trailing plain
/// text block when there is one — preserving the exact wire form single-text
/// tools produced before — and otherwise becomes its own trailing block.
fn append_steering(presentation: &ToolOutput, steering: String) -> ToolOutput {
    let mut content = presentation.as_content().clone();
    match content.last_mut() {
        ToolResultContent::Text(text) if text.additional_params.is_none() => {
            text.text = format!("{}\n\n{steering}", text.text);
        }
        _ => content.push(ToolResultContent::text(steering)),
    }
    ToolOutput::content(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig_core::{OneOrMany, completion::message::Image};

    fn image_block() -> ToolResultContent {
        ToolResultContent::Image(Image {
            media_type: Some(rig_core::completion::message::ImageMediaType::PNG),
            ..Default::default()
        })
    }

    #[test]
    fn merges_steering_into_a_trailing_text_block() {
        let presentation = ToolOutput::text("tool said this");
        let rewritten = append_steering(&presentation, "<user_steering>go</user_steering>".into());
        assert_eq!(
            rewritten.as_text(),
            Some("tool said this\n\n<user_steering>go</user_steering>"),
            "single-text results must keep the exact pre-existing wire form"
        );
    }

    #[test]
    fn steering_preserves_image_blocks() {
        let presentation = ToolOutput::content(
            OneOrMany::many([ToolResultContent::text("a screenshot"), image_block()]).unwrap(),
        );
        let rewritten = append_steering(&presentation, "<user_steering>go</user_steering>".into());

        let blocks = rewritten.as_content();
        assert_eq!(blocks.len(), 3, "the image block must survive: {blocks:?}");
        assert_eq!(blocks.iter().filter(|b| b.as_text().is_some()).count(), 2);
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, ToolResultContent::Image(_))),
            "steering must never discard image content: {blocks:?}"
        );
        assert_eq!(
            blocks.last_ref().as_text(),
            Some("<user_steering>go</user_steering>"),
            "steering becomes its own trailing block when the last block is not text"
        );
    }

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
        state.pending.push_back("next".into());
        drop(state);
        let mutation = handle.edit_pending(1, "updated next".into());
        assert!(mutation.applied);
        assert_eq!(mutation.delivered, ["changed"]);
        let state = handle.lock();
        assert_eq!(
            state.pending.iter().cloned().collect::<Vec<_>>(),
            ["updated next"]
        );
    }
}
