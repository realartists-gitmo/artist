//! Durable provider-private conversation context.
use crate::{Envelope, ProviderContext, Recorder, SessionEvent};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

pub const PROVIDER_CONTEXT_SCHEMA: u32 = 1;

/// Concurrency-safe sidecar backed by versioned append-only snapshot events.
/// Clones share one lock, making update plus durable append atomic to callers.
#[derive(Clone)]
pub struct ProviderContextHandle {
    inner: Arc<Mutex<HashMap<(String, String), Vec<Value>>>>,
    recorder: Recorder,
}

impl Default for ProviderContextHandle {
    fn default() -> Self {
        Self::noop()
    }
}

impl ProviderContextHandle {
    pub fn noop() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            recorder: Recorder::noop(),
        }
    }

    pub fn from_events(events: &[Envelope], recorder: Recorder) -> Self {
        let mut contexts = HashMap::new();
        for envelope in events {
            if let SessionEvent::ProviderContext(context) = envelope.event() {
                if context.schema == PROVIDER_CONTEXT_SCHEMA {
                    contexts.insert((context.conversation_id, context.provider), context.items);
                }
            }
        }
        Self {
            inner: Arc::new(Mutex::new(contexts)),
            recorder,
        }
    }

    pub async fn items(&self, conversation_id: &str, provider: &str) -> Vec<Value> {
        self.inner
            .lock()
            .await
            .get(&(conversation_id.to_owned(), provider.to_owned()))
            .cloned()
            .unwrap_or_default()
    }

    /// Replace the canonical provider suffix and durably append its snapshot.
    /// If a compaction item exists, only the latest compaction and its suffix survive.
    pub async fn commit(&self, conversation_id: &str, provider: &str, mut items: Vec<Value>) {
        if let Some(index) = items.iter().rposition(is_compaction) {
            items.drain(..index);
        }
        let mut contexts = self.inner.lock().await;
        contexts.insert(
            (conversation_id.to_owned(), provider.to_owned()),
            items.clone(),
        );
        self.recorder.record(ProviderContext {
            conversation_id: conversation_id.to_owned(),
            provider: provider.to_owned(),
            schema: PROVIDER_CONTEXT_SCHEMA,
            items,
        });
        self.recorder.flush().await;
    }
}

fn is_compaction(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("compaction")
}
