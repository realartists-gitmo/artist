//! # hashline-tools
//!
//! Deterministic semantic line anchors plus multi-agent file coordination.
//!
//! Anchor identities and v1 addresses are stateless. SQLite is used only for
//! coordination metadata such as writer attribution, never anchor allocation.
mod agent;
mod anchor_address_v1 {
    include!("anchor_address_v1.rs");
}
mod anchor_table;
mod coordinator;
mod error;
mod file_tools;
mod semantic_anchors;
mod state;

pub use agent::{AgentId, AgentIdentity};
pub use anchor_table::{not_issued_message, stale_anchor_message, AnchorTable};
pub use coordinator::{
    content_hash, CoordinatedEditResult, CoordinatedReadResult, FileCoordinator, WriteCondition,
    ANCHOR_USAGE,
};
pub use error::{HashlineError, HashlineErrorCode};
pub use file_tools::{
    AnchoredLine, Drift, DriftCandidate, DriftKind, EditOperation, EditRequest, EditResult,
    FileToolConfig, FileToolManager, ReadFileRequest, ReadFileResult, WriteFileRequest,
    WriteFileResult,
};
pub use semantic_anchors::ANCHOR_ABI_VERSION;
pub use state::StateStore;
