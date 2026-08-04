//! Rig conversation memory backed by Artist's append-only session log.
//!
//! New sessions persist the exact `rig_core::Message` batches Rig commits.
//! Older event-only sessions are projected once and replaced by a native
//! conversation snapshot on their next successful turn.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rig_core::completion::Message;
use rig_core::memory::{ConversationMemory, MemoryError};
type CachedConversation = Option<(Vec<Message>, bool)>;
use crate::{
    AttachmentStore, ConversationCompacted, ConversationMessages, EventLogReader, HistoryOptions,
    Recorder, build_history,
};

/// A durable [`ConversationMemory`] scoped to one Artist session.
#[derive(Clone)]
pub struct SessionMemory {
    session_id: String,
    lineage: String,
    session_dir: PathBuf,
    recorder: Recorder,
    attachments: AttachmentStore,
    cache: Arc<Mutex<CachedConversation>>,
}
impl SessionMemory {
    pub fn new(
        session_id: impl Into<String>,
        session_dir: impl AsRef<Path>,
        recorder: Recorder,
        attachments: AttachmentStore,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            lineage: crate::MAIN_LINEAGE.into(),
            session_dir: session_dir.as_ref().to_owned(),
            recorder,
            attachments,
            cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a conversation memory projected from one exact agent lineage.
    pub fn for_lineage(
        conversation_id: impl Into<String>,
        lineage: impl Into<String>,
        session_dir: impl AsRef<Path>,
        recorder: Recorder,
        attachments: AttachmentStore,
    ) -> Self {
        Self {
            session_id: conversation_id.into(),
            lineage: lineage.into(),
            session_dir: session_dir.as_ref().to_owned(),
            recorder,
            attachments,
            cache: Arc::new(Mutex::new(None)),
        }
    }

    fn check_id(&self, conversation_id: &str) -> Result<(), MemoryError> {
        if conversation_id == self.session_id {
            Ok(())
        } else {
            Err(MemoryError::Policy(format!(
                "session memory {} cannot serve conversation {conversation_id}",
                self.session_id
            )))
        }
    }

    fn read(&self) -> Result<(Vec<Message>, bool), MemoryError> {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(state) = cache.as_ref() {
            return Ok(state.clone());
        }
        let events = EventLogReader::new(&self.session_dir)
            .read_all()
            .map_err(memory_error)?;
        let native =
            crate::history::has_native_conversation_for_lineage(&events, &self.lineage, None);
        let mut messages = build_history(
            &events,
            &self.attachments,
            &HistoryOptions {
                lineage: &self.lineage,
                ..HistoryOptions::default()
            },
        )
        .map_err(memory_error)?;
        crate::convert::rehydrate_images(&mut messages, &self.attachments);
        let messages = normalize(messages)?;
        *cache = Some((messages.clone(), native));
        Ok((messages, native))
    }

    /// Rebuild the shared in-memory projection from the currently visible events.
    /// All clones observe the replacement immediately.
    pub fn reload_from_events(&self, events: &[crate::Envelope]) -> Result<(), MemoryError> {
        let native =
            crate::history::has_native_conversation_for_lineage(events, &self.lineage, None);
        let mut messages = build_history(
            events,
            &self.attachments,
            &HistoryOptions {
                lineage: &self.lineage,
                ..HistoryOptions::default()
            },
        )
        .map_err(memory_error)?;
        crate::convert::rehydrate_images(&mut messages, &self.attachments);
        *self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((messages, native));
        Ok(())
    }

    /// Image payloads bound for the log, replaced by attachment references.
    ///
    /// The caller's copy stays inline: the RAM projection feeds the next turn
    /// directly, so rehydrating it again on the way out would be wasted work.
    /// Only what is written to `events.jsonl` is externalized, which is what
    /// keeps the log proportional to distinct images rather than to turn count.
    fn for_log(&self, messages: &[Message]) -> Vec<Message> {
        let mut messages = messages.to_vec();
        crate::convert::externalize_images(&mut messages, &self.attachments);
        messages
    }

    fn cache(&self, messages: Vec<Message>) -> Result<(), MemoryError> {
        let messages = normalize(messages)?;
        *self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((messages, true));
        Ok(())
    }

    /// Replace the active conversation while retaining prior log records.
    pub async fn replace(&self, messages: Vec<Message>) -> Result<(), MemoryError> {
        let event_messages = self.for_log(&messages);
        self.cache(messages)?;
        self.recorder.record(ConversationMessages {
            messages: event_messages,
            reset: true,
            display_from: 0,
        });
        self.recorder.flush().await;
        self.health()
    }

    /// Replace only model context after compaction. The reset snapshot is
    /// hidden from display projections so the append-only transcript remains
    /// intact while future turns use the summary and retained suffix.
    pub async fn compact(
        &self,
        messages: Vec<Message>,
        event: ConversationCompacted,
    ) -> Result<(), MemoryError> {
        let display_from = messages.len();
        let event_messages = self.for_log(&messages);
        self.cache(messages)?;
        self.recorder.record(event);
        self.recorder.record(ConversationMessages {
            messages: event_messages,
            reset: true,
            display_from,
        });
        self.recorder.flush().await;
        self.health()
    }

    /// Rewrite model context in place without touching the transcript.
    ///
    /// Deliberately *not* [`Self::replace`]: that records `display_from: 0`,
    /// which every display projection reads as "clear the scrollback". A decay
    /// pass runs before turns, so using `replace` would blank the user's
    /// transcript each time a screenshot aged out. This uses the `compact`
    /// shape — the reset is hidden from display — but records no
    /// `ConversationCompacted`, because decay is not a compaction and the
    /// status line should not claim one happened.
    pub async fn revise(&self, messages: Vec<Message>) -> Result<(), MemoryError> {
        let display_from = messages.len();
        let event_messages = self.for_log(&messages);
        self.cache(messages)?;
        self.recorder.record(ConversationMessages {
            messages: event_messages,
            reset: true,
            display_from,
        });
        self.recorder.flush().await;
        self.health()
    }

    fn health(&self) -> Result<(), MemoryError> {
        if self.recorder.is_healthy() {
            Ok(())
        } else {
            Err(MemoryError::backend(std::io::Error::other(
                "Artist session writer failed",
            )))
        }
    }
}

impl ConversationMemory for SessionMemory {
    fn load<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> rig_core::wasm_compat::WasmBoxedFuture<'a, Result<Vec<Message>, MemoryError>> {
        Box::pin(async move {
            self.check_id(conversation_id)?;
            self.read().map(|(messages, _)| messages)
        })
    }

    fn append<'a>(
        &'a self,
        conversation_id: &'a str,
        messages: Vec<Message>,
    ) -> rig_core::wasm_compat::WasmBoxedFuture<'a, Result<(), MemoryError>> {
        Box::pin(async move {
            self.check_id(conversation_id)?;
            let (mut existing, native) = self.read()?;
            let reset = !native;
            let display_from = if reset { existing.len() } else { 0 };
            let event_messages = if reset {
                existing.extend(messages);
                existing.clone()
            } else {
                existing.extend(messages.clone());
                messages
            };
            self.recorder.record(ConversationMessages {
                messages: self.for_log(&event_messages),
                reset,
                display_from,
            });
            self.cache(existing)?;
            // Recorder writes on its background task. The RAM projection above is
            // immediately visible to the next turn without forcing an fsync here.
            self.health()
        })
    }

    fn clear<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> rig_core::wasm_compat::WasmBoxedFuture<'a, Result<(), MemoryError>> {
        Box::pin(async move {
            self.check_id(conversation_id)?;
            self.replace(Vec::new()).await
        })
    }
}

/// Apply the JSONL round trip a freshly loaded session performs, so a live
/// projection and a restored one carry identical Rig parameter defaults.
///
/// rig's `additional_params` are `#[serde(flatten)]`, so a `None` written to
/// disk reads back as `Some({})`. Both encodings are wire-identical; what
/// matters is that every path agrees on one of them, which is why this runs on
/// the read side as well as on every cached write.
fn normalize(messages: Vec<Message>) -> Result<Vec<Message>, MemoryError> {
    serde_json::from_value(serde_json::to_value(messages).map_err(memory_error)?)
        .map_err(memory_error)
}

fn memory_error(error: impl std::fmt::Display) -> MemoryError {
    MemoryError::backend(std::io::Error::other(error.to_string()))
}
