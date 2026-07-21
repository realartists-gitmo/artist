//! Per-attempt Rig memory adapter for Artist's external rule-retry loop.
//!
//! Rig persists only successful runs. When a stream rule aborts an attempt,
//! Artist carries Rig's committed messages into the retry and this adapter
//! appends that accepted prefix together with the eventual successful delta.

use std::sync::{Arc, Mutex};

use rig_core::completion::Message;
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

impl ConversationMemory for AttemptMemory {
    fn load<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> rig_core::wasm_compat::WasmBoxedFuture<'a, Result<Vec<Message>, MemoryError>> {
        Box::pin(async move {
            self.check_id(conversation_id)?;
            Ok(self.history.clone())
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
            let result = self.durable.append(conversation_id, delta).await;
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
    use rig_core::memory::InMemoryConversationMemory;

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
