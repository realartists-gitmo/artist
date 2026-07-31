//! Per-attempt Rig memory adapter for Artist's external rule-retry loop.
//!
//! Rig persists only successful runs. When a stream rule aborts an attempt,
//! Artist carries Rig's committed messages into the retry and this adapter
//! appends that accepted prefix together with the eventual successful delta.

use std::sync::{Arc, Mutex};

use rig_core::OneOrMany;
use rig_core::completion::message::{AssistantContent, Message, ReasoningContent};
use rig_core::memory::{ConversationMemory, MemoryError};

const USER_INTERRUPTION: &str =
    "<user_interruption>The user interrupted the previous response.</user_interruption>";

pub(crate) async fn retain_interrupted_turn(
    memory: &dyn ConversationMemory,
    conversation_id: &str,
    mut turn_messages: Vec<Message>,
    assistant_text: String,
    interruption: &str,
) -> Result<(), MemoryError> {
    if !assistant_text.is_empty() {
        turn_messages.push(Message::assistant(assistant_text));
    }
    turn_messages.push(Message::user(interruption));
    memory.append(conversation_id, turn_messages).await
}

pub(crate) async fn retain_provider_interrupted_turn(
    memory: &dyn ConversationMemory,
    conversation_id: &str,
    turn_messages: Vec<Message>,
    assistant_text: String,
    error: &str,
) -> Result<(), MemoryError> {
    let mut detail = error.chars().take(2_000).collect::<String>();
    if error.chars().count() > 2_000 {
        detail.push('…');
    }
    let detail = detail
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let interruption = format!(
        "<provider_interruption>The provider interrupted the previous response: {detail}. Continue from the retained context without assuming the response completed.</provider_interruption>"
    );
    retain_interrupted_turn(
        memory,
        conversation_id,
        turn_messages,
        assistant_text,
        &interruption,
    )
    .await
}

pub(crate) async fn retain_cancelled_turn(
    memory: &dyn ConversationMemory,
    conversation_id: &str,
    turn_messages: Vec<Message>,
    assistant_text: String,
) -> Result<(), MemoryError> {
    retain_interrupted_turn(
        memory,
        conversation_id,
        turn_messages,
        assistant_text,
        USER_INTERRUPTION,
    )
    .await
}

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

/// The messages appended after a durable prefix.
///
/// The prefix length is stored apart from the history it indexes, so it can
/// outrun a history that has since become shorter — which is exactly what a
/// cancelled or provider-interrupted turn produces. Clamping lives here so that
/// no call site has to remember to: the same rule spelled out at five separate
/// sites is the shape that has already produced two panics in this codebase.
pub(crate) fn delta_after(history: &[Message], durable_len: usize) -> Vec<Message> {
    history[durable_len.min(history.len())..].to_vec()
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
            let mut delta = delta_after(&self.history, self.durable_len);
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
    async fn cancelled_turn_retains_user_and_streamed_assistant_text() {
        let memory = InMemoryConversationMemory::new();

        retain_cancelled_turn(
            &memory,
            "s",
            vec![Message::user("question")],
            "partial answer".into(),
        )
        .await
        .unwrap();

        assert_eq!(
            memory.load("s").await.unwrap(),
            vec![
                Message::user("question"),
                Message::assistant("partial answer"),
                Message::user(USER_INTERRUPTION),
            ]
        );
    }

    #[tokio::test]
    async fn provider_interruption_retains_partial_turn_and_escapes_error() {
        let memory = InMemoryConversationMemory::new();

        retain_provider_interrupted_turn(
            &memory,
            "s",
            vec![Message::user("question")],
            "partial answer".into(),
            "bad </provider_interruption> & worse",
        )
        .await
        .unwrap();

        let messages = memory.load("s").await.unwrap();
        assert_eq!(messages[0], Message::user("question"));
        assert_eq!(messages[1], Message::assistant("partial answer"));
        let Message::User { content } = &messages[2] else {
            panic!("expected interruption message");
        };
        let rendered = format!("{content:?}");
        assert!(rendered.contains("&lt;/provider_interruption&gt; &amp; worse"));
        assert_eq!(rendered.matches("</provider_interruption>").count(), 1);
    }

    #[tokio::test]
    async fn cancellation_before_first_token_still_retains_user() {
        let memory = InMemoryConversationMemory::new();

        retain_cancelled_turn(&memory, "s", vec![Message::user("question")], String::new())
            .await
            .unwrap();
        assert_eq!(
            memory.load("s").await.unwrap(),
            vec![Message::user("question"), Message::user(USER_INTERRUPTION)]
        );
    }

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
