//! WIPE FLAG: this crate may need to be wiped and rebuilt. The kernel and
//! component crates were wiped on 2026-08-17 because the refactor ignored
//! substantial prior art; the session store has not been audited yet against
//! that decision. Audit before rebuilding the VFS layer.
//!
//! Event-sourced session store for Artist.
//!
//! The canonical record of a session is an append-only JSONL event log
//! (`events.jsonl`); everything else — the markdown transcript, the
//! model-facing history, the TUI replay — is a projection. Rewind events mask
//! ranges in projections; nothing is ever deleted, which is what makes session
//! forking possible.
//!
//! Layout on disk, one directory per session:
//!
//! ```text
//! <config_root>/sessions/<project-key>/<session-id>/
//!   events.jsonl      # canonical log (this crate)
//!   transcript.md     # derived, regenerable
//!   attachments/<sha> # content-addressed image blobs
//!   writer.lock       # exclusive while a process owns the session
//! ```

mod attachments;
mod conversation_replay;
mod convert;
mod event;
mod history;
mod log;
mod memory;
mod provider_context;
mod recorder;
mod replay;

pub use attachments::AttachmentStore;
pub use convert::{assistant_to_blocks, blocks_to_assistant, blocks_to_user, user_to_blocks};
pub use event::{
    ContentBlock, ConversationMessages, Envelope, HistoryRewind, LegacyTurn, MAIN_LINEAGE,
    ModelTurn, ProviderContext, RunFinished, RunStarted, SCHEMA_VERSION, SessionCreated,
    SessionEvent, SteeringDelivered, ToolOutcomeRecord, ToolResultEvent, TurnUser,
};
pub use history::{HistoryOptions, build as build_history};
pub use log::{EVENTS_FILE, EventLogReader, EventLogWriter};
pub use memory::SessionMemory;
pub use provider_context::{PROVIDER_CONTEXT_SCHEMA, ProviderContextHandle};
pub use recorder::{Recorder, WriterTask, spawn_writer};
pub use replay::{
    ReplayItem, markdown_fragment, render_markdown, replay_for_ui, rewind_targets, user_prompts,
    visible_events,
};
