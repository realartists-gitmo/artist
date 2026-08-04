//! Durable provider-private conversation context.
use crate::{Envelope, ProviderContext, Recorder, SessionEvent};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

pub const PROVIDER_CONTEXT_SCHEMA: u32 = 2;

#[derive(Clone, Debug, Default)]
struct Snapshot {
    items: Vec<Value>,
    input_fingerprints: Vec<String>,
}

/// Concurrency-safe sidecar backed by versioned append-only snapshot events.
/// Clones share one lock, making update plus durable append atomic to callers.
#[derive(Clone)]
pub struct ProviderContextHandle {
    inner: Arc<Mutex<HashMap<(String, String), Snapshot>>>,
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
        // Rewinds mask provider snapshots just like model-facing history. A
        // snapshot from a divergent future must never be resurrected.
        let mut masks = Vec::new();
        for envelope in events.iter().rev() {
            if let SessionEvent::HistoryRewind(rewind) = envelope.event()
                && !masks
                    .iter()
                    .any(|(start, end)| *start <= envelope.seq && envelope.seq <= *end)
            {
                masks.push((rewind.to_seq.saturating_add(1), envelope.seq));
            }
        }
        for envelope in events {
            if masks
                .iter()
                .any(|(start, end)| *start <= envelope.seq && envelope.seq <= *end)
            {
                continue;
            }
            if let SessionEvent::ProviderContext(context) = envelope.event()
                && (context.schema == 1 || context.schema == PROVIDER_CONTEXT_SCHEMA)
            {
                contexts.insert(
                    (context.conversation_id, context.provider),
                    Snapshot {
                        items: context.items,
                        input_fingerprints: context.input_fingerprints,
                    },
                );
            }
        }
        Self {
            inner: Arc::new(Mutex::new(contexts)),
            recorder,
        }
    }

    /// Restore the currently visible durable snapshots into this shared handle.
    /// This mutates the shared map so every existing clone sees the restored state.
    pub async fn restore_from_events(&self, events: &[Envelope]) {
        let restored = Self::from_events(events, self.recorder.clone());
        let snapshots = restored.inner.lock().await.clone();
        *self.inner.lock().await = snapshots;
    }

    pub async fn items(&self, conversation_id: &str, provider: &str) -> Vec<Value> {
        self.inner
            .lock()
            .await
            .get(&(conversation_id.to_owned(), provider.to_owned()))
            .map(|snapshot| snapshot.items.clone())
            .unwrap_or_default()
    }

    pub async fn snapshot(
        &self,
        conversation_id: &str,
        provider: &str,
    ) -> (Vec<Value>, Vec<String>) {
        self.inner
            .lock()
            .await
            .get(&(conversation_id.to_owned(), provider.to_owned()))
            .map(|snapshot| (snapshot.items.clone(), snapshot.input_fingerprints.clone()))
            .unwrap_or_default()
    }

    pub async fn commit(&self, conversation_id: &str, provider: &str, items: Vec<Value>) {
        self.commit_checkpoint(conversation_id, provider, items, Vec::new())
            .await;
    }

    /// Replace the canonical provider suffix and its framework-history checkpoint.
    /// If a compaction item exists, only the latest compaction and its suffix survive.
    pub async fn commit_checkpoint(
        &self,
        conversation_id: &str,
        provider: &str,
        mut items: Vec<Value>,
        input_fingerprints: Vec<String>,
    ) {
        if let Some(index) = items.iter().rposition(is_compaction) {
            items.drain(..index);
        }
        let mut contexts = self.inner.lock().await;
        contexts.insert(
            (conversation_id.to_owned(), provider.to_owned()),
            Snapshot {
                items: items.clone(),
                input_fingerprints: input_fingerprints.clone(),
            },
        );
        self.recorder.record(ProviderContext {
            conversation_id: conversation_id.to_owned(),
            provider: provider.to_owned(),
            schema: PROVIDER_CONTEXT_SCHEMA,
            items,
            input_fingerprints,
        });
        self.recorder.flush().await;
    }
}

fn is_compaction(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("compaction")
}
