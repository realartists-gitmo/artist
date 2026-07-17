//! Event-sourced session store for Artist.
//!
//! The canonical record of a session is an append-only JSONL event log
//! (`events.jsonl`); everything else — the markdown transcript, the
//! model-facing history, the TUI replay — is a projection. Rewind and
//! compaction are *events* that mask ranges in projections; nothing is ever
//! deleted, which is what makes retroactive rule evaluation and session
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
mod convert;
mod event;
mod history;
mod log;
mod recorder;
mod replay;

pub use attachments::AttachmentStore;
pub use convert::{assistant_to_blocks, blocks_to_assistant, blocks_to_user, user_to_blocks};
pub use event::{
    ContentBlock, DelegateFinished, DelegateStarted, Envelope, HistoryCompact, HistoryRewind,
    LegacyTurn, MAIN_LINEAGE, ModelTurn, RuleFired, RuleInjection, RuleRetroFindings, RunFinished,
    RunStarted, SCHEMA_VERSION, SessionCreated, SessionEvent, SteeringDelivered, ToolOutcomeRecord,
    ToolResultEvent, TurnUser,
};
pub use history::{HistoryOptions, build as build_history};
pub use log::{EVENTS_FILE, EventLogReader, EventLogWriter};
pub use recorder::{Recorder, WriterTask, spawn_writer};
pub use replay::{ReplayItem, markdown_fragment, render_markdown, replay_for_ui, user_prompts};
