//! Reusable session projection contracts.

use llm_provider::Message;

use crate::AgentEvent;

pub trait ContextProjector: Send + Sync {
    fn project(&self, events: &[AgentEvent]) -> Vec<Message>;
}

pub trait Compactor: Send + Sync {
    fn compact(&self, messages: Vec<Message>, max_messages: usize) -> Vec<Message>;
}

#[derive(Default)]
pub struct IdentityCompactor;

impl Compactor for IdentityCompactor {
    fn compact(&self, mut messages: Vec<Message>, max_messages: usize) -> Vec<Message> {
        if messages.len() > max_messages {
            messages.drain(..messages.len() - max_messages);
        }
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_provider::{ContentPart, Role};

    #[test]
    fn identity_compactor_keeps_the_newest_context() {
        let messages = vec![
            Message::text(Role::User, "one"),
            Message::text(Role::User, "two"),
            Message::text(Role::User, "three"),
        ];
        let compacted = IdentityCompactor.compact(messages, 2);
        assert_eq!(compacted.len(), 2);
        assert_eq!(
            compacted[0].content[0],
            ContentPart::Text { text: "two".into() }
        );
    }
}
