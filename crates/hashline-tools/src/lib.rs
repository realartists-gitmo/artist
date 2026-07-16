//! # hashline-tools
//!
//! Excised from [RealArtist](https://github.com/): stable **mnemonic line anchors**
//! backed by hidden content hashes, plus a multi-agent coordinator with SQLite
//! persistence and cross-process path locking.
//!
//! ## Layers
//!
//! | Layer | Type | Role |
//! |-------|------|------|
//! | Core | [`FileToolManager`] | In-process read / write / edit with anchors |
//! | Persist | [`StateStore`] | Per-agent anchor map in SQLite |
//! | Coord | [`FileCoordinator`] | Multi-agent + path locks + whole-file BLAKE3 |
//!
//! Most harnesses should start with [`FileCoordinator::open`].

mod agent;
mod coordinator;
mod error;
mod file_tools;
mod mnemonic_anchors;
mod state;

pub use agent::{AgentId, AgentIdentity};
pub use coordinator::{
    content_hash, CoordinatedEditResult, CoordinatedReadResult, FileCoordinator, WriteCondition,
    ANCHOR_USAGE,
};
pub use error::{HashlineError, HashlineErrorCode};
pub use file_tools::{
    AnchoredLine, ConfirmationRequired, EditOperation, EditRequest, EditResult, FileToolConfig,
    FileToolManager, ReadFileRequest, ReadFileResult, WriteFileRequest, WriteFileResult,
};
pub use state::StateStore;
