//! Rig conversation memory backed by Artist's append-only session log.
//!
//! New sessions persist the exact `rig_core::Message` batches Rig commits.
//! Older event-only sessions are projected once and replaced by a native
//! conversation snapshot on their next successful turn.

use std::path::{Path, PathBuf};

use rig_core::completion::Message;
use rig_core::memory::{ConversationMemory, MemoryError};

use crate::{
    AttachmentStore, ConversationCompacted, ConversationMessages, EventLogReader, HistoryOptions,
    Recorder, build_history,
};

/// A durable [`ConversationMemory`] scoped to one Artist session.
#[derive(Clone)]
pub struct SessionMemory {
    session_id: String,
    session_dir: PathBuf,
    recorder: Recorder,
    attachments: AttachmentStore,
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
            session_dir: session_dir.as_ref().to_owned(),
            recorder,
            attachments,
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
        let events = EventLogReader::new(&self.session_dir)
            .read_all()
            .map_err(memory_error)?;
        let native = crate::history::has_native_conversation(&events, None);
        let messages = build_history(&events, &self.attachments, &HistoryOptions::default())
            .map_err(memory_error)?;
        Ok((messages, native))
    }

    /// Replace the active conversation while retaining prior log records.
    pub async fn replace(&self, messages: Vec<Message>) -> Result<(), MemoryError> {
        self.recorder.record(ConversationMessages {
            messages,
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
        self.recorder.record(event);
        self.recorder.record(ConversationMessages {
            messages,
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
        mut messages: Vec<Message>,
    ) -> rig_core::wasm_compat::WasmBoxedFuture<'a, Result<(), MemoryError>> {
        Box::pin(async move {
            self.check_id(conversation_id)?;
            let (mut existing, native) = self.read()?;
            let reset = !native;
            let display_from = if reset { existing.len() } else { 0 };
            if reset {
                existing.append(&mut messages);
                messages = existing;
            }
            self.recorder.record(ConversationMessages {
                messages,
                reset,
                display_from,
            });
            self.recorder.flush().await;
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

fn memory_error(error: impl std::fmt::Display) -> MemoryError {
    MemoryError::backend(std::io::Error::other(error.to_string()))
}
