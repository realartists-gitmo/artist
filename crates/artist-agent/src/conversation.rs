//! Per-attempt Rig memory adapter for Artist's external rule-retry loop.
//!
//! Rig persists only successful runs. When a stream rule aborts an attempt,
//! Artist carries Rig's committed messages into the retry and this adapter
//! appends that accepted prefix together with the eventual successful delta.

use std::sync::{Arc, Mutex};

use rig_core::OneOrMany;
use rig_core::completion::message::{AssistantContent, Message, ReasoningContent};
use rig_core::memory::{ConversationMemory, MemoryError};

#[derive(Clone, Default)]
pub(crate) struct PersistenceStatus(Arc<Mutex<Option<Result<(), String>>>>);

impl PersistenceStatus {
    pub(crate) fn result(&self) -> Result<(), String> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or_else(|| Err("Rig completed without persisting conversation memory".into()))
    }

    fn record(&self, result: &Result<(), MemoryError>) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(result.as_ref().map(|_| ()).map_err(ToString::to_string));
    }
}

#[derive(Clone)]
pub(crate) struct AttemptMemory {
    durable: Arc<dyn ConversationMemory>,
    conversation_id: String,
    history: Vec<Message>,
    durable_len: usize,
    status: PersistenceStatus,
}

impl AttemptMemory {
    pub(crate) fn new(
        durable: Arc<dyn ConversationMemory>,
        conversation_id: String,
        history: Vec<Message>,
        durable_len: usize,
        status: PersistenceStatus,
    ) -> Self {
        Self {
            durable,
            conversation_id,
            history,
            durable_len,
            status,
        }
    }

    fn check_id(&self, id: &str) -> Result<(), MemoryError> {
        if id == self.conversation_id {
            Ok(())
        } else {
            Err(MemoryError::Policy(format!(
                "attempt for {} cannot serve conversation {id}",
                self.conversation_id
            )))
        }
    }
}

fn without_display_summaries(messages: Vec<Message>) -> Vec<Message> {
    messages
        .into_iter()
        .filter_map(|message| match message {
            Message::Assistant { id, content } => {
                let content = content.into_iter().filter_map(|item| match item {
                    AssistantContent::Reasoning(mut reasoning) => {
                        reasoning
                            .content
                            .retain(|block| !matches!(block, ReasoningContent::Summary(_)));
                        (!reasoning.content.is_empty())
                            .then_some(AssistantContent::Reasoning(reasoning))
                    }
                    other => Some(other),
                });
                OneOrMany::many(content)
                    .ok()
                    .map(|content| Message::Assistant { id, content })
            }
            other => Some(other),
        })
        .collect()
}

impl ConversationMemory for AttemptMemory {
    fn load<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> rig_core::wasm_compat::WasmBoxedFuture<'a, Result<Vec<Message>, MemoryError>> {
        Box::pin(async move {
            self.check_id(conversation_id)?;
            Ok(without_display_summaries(self.history.clone()))
        })
    }

    fn append<'a>(
        &'a self,
        conversation_id: &'a str,
        messages: Vec<Message>,
    ) -> rig_core::wasm_compat::WasmBoxedFuture<'a, Result<(), MemoryError>> {
        Box::pin(async move {
            self.check_id(conversation_id)?;
            let mut delta = self.history[self.durable_len.min(self.history.len())..].to_vec();
            delta.extend(messages);
            let result = self
                .durable
                .append(conversation_id, without_display_summaries(delta))
                .await;
            self.status.record(&result);
            result
        })
    }

    fn clear<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> rig_core::wasm_compat::WasmBoxedFuture<'a, Result<(), MemoryError>> {
        Box::pin(async move {
            self.check_id(conversation_id)?;
            self.durable.clear(conversation_id).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig_core::completion::message::{Reasoning, Text};
    use rig_core::memory::InMemoryConversationMemory;

    #[tokio::test]
    async fn display_summaries_are_excluded_from_context_and_persistence() {
        let mut reasoning = Reasoning::new("");
        reasoning.id = Some("reasoning".into());
        reasoning.content = vec![
            ReasoningContent::Encrypted("opaque".into()),
            ReasoningContent::Summary("Shown only in the UI".into()),
        ];
        let source = vec![Message::Assistant {
            id: Some("response".into()),
            content: OneOrMany::many([
                AssistantContent::Reasoning(reasoning),
                AssistantContent::Text(Text::new("answer")),
            ])
            .unwrap(),
        }];
        let durable = Arc::new(InMemoryConversationMemory::new());
        let attempt = AttemptMemory::new(
            durable.clone(),
            "s".into(),
            source,
            0,
            PersistenceStatus::default(),
        );

        let messages = attempt.load("s").await.unwrap();
        attempt.append("s", Vec::new()).await.unwrap();

        assert_eq!(messages.len(), 1);
        let Message::Assistant { content, .. } = &messages[0] else {
            panic!("expected assistant message");
        };
        let reasoning = content.iter().find_map(|item| match item {
            AssistantContent::Reasoning(reasoning) => Some(reasoning),
            _ => None,
        });
        assert_eq!(
            reasoning.unwrap().content,
            vec![ReasoningContent::Encrypted("opaque".into())]
        );
        assert!(content.iter().any(|item| matches!(
            item,
            AssistantContent::Text(text) if text.text == "answer"
        )));
        assert_eq!(durable.load("s").await.unwrap(), messages);
    }

    #[tokio::test]
    async fn successful_retry_persists_committed_prefix_and_final_delta_once() {
        let durable = Arc::new(InMemoryConversationMemory::new());
        durable
            .append("s", vec![Message::user("old")])
            .await
            .unwrap();
        let attempt = AttemptMemory::new(
            durable.clone(),
            "s".into(),
            vec![Message::user("old"), Message::user("accepted retry state")],
            1,
            PersistenceStatus::default(),
        );

        attempt
            .append("s", vec![Message::assistant("final")])
            .await
            .unwrap();

        assert_eq!(
            durable.load("s").await.unwrap(),
            vec![
                Message::user("old"),
                Message::user("accepted retry state"),
                Message::assistant("final"),
            ]
        );
    }
}
