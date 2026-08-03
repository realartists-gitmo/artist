//! Event-sourced session store for Artist.
//!
//! The canonical record of a session is an append-only JSONL event log
//! (`events.jsonl`); everything else — the markdown transcript, the
//! model-facing history, the TUI replay — is a projection. Rewind events mask
//! ranges in projections; nothing is ever deleted, which is what makes
//! retroactive rule evaluation and session forking possible.
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

pub mod ask;
mod ask_outbox;
mod attachments;
pub mod capabilities;
pub mod chain;
pub mod compaction;
mod conversation_replay;
mod convert;
pub mod decay;
mod event;
mod file_handles;
mod history;
pub mod inline_images;
mod log;
mod memory;
mod provider_context;
mod recorder;
mod replay;
mod store;

pub use ask::{Answer, AskRegistry, Question, QuestionOption};
pub use ask_outbox::AskOutbox;
pub use attachments::{AttachmentStore, content_digest};
pub use capabilities::ProviderCapabilities;
pub use chain::{ChainState, Send as ChainSend};
pub use convert::{
    assistant_to_blocks, blocks_to_assistant, blocks_to_user, externalize_images,
    referenced_attachments, rehydrate_images, store_tool_image, tool_image_from_block,
    user_to_blocks,
};
pub use event::{
    AskAnswered, AskPosted, CanvasCreated, CanvasOpened, CanvasState, ChangeRecorded,
    ComputerActed, ComputerElided, ComputerLaunched, ComputerObserved, ComputerStageClosed,
    ComputerStageOpened, ComputerStep, ContentBlock, ConversationCompacted, ConversationMessages,
    DelegateFinished, DelegateStarted, Envelope, HandoffPerformed, HistoryRewind, LegacyTurn,
    MAIN_LINEAGE, MemoryWritten, ModelTurn, ProviderContext, RuleFired, RuleInjection,
    RuleRetroFindings, RunFinished, RunStarted, RunUsage, SCHEMA_VERSION, SessionCreated,
    SessionEvent, SteeringDelivered, TaskFinished, TaskStarted, TaskUpdated, TodoItem, TodoStatus,
    TodoUpdated, ToolOutcomeRecord, ToolResultEvent, TurnUser,
};
pub use file_handles::HandleLedger;
pub use history::{HistoryOptions, build as build_history};
pub use inline_images::InlineImage;
pub use log::{EVENTS_FILE, EventLogReader, EventLogWriter};
pub use memory::SessionMemory;
pub use provider_context::{PROVIDER_CONTEXT_SCHEMA, ProviderContextHandle};
pub use recorder::{Recorder, WriterTask, spawn_writer};
pub use replay::{
    ReplayItem, active_profile, handoff_depth, markdown_fragment, render_markdown, replay_for_ui,
    rewind_targets, user_prompts, visible_events,
};
pub use store::{ActiveSession, Session, SessionStore};
