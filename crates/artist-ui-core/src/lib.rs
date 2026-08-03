//! The frontend-agnostic core shared by Artist's TUI and GUI.
//!
//! Artist has two frontends — ratatui in `artist-cli` and gpui in `artist-gpui`
//! — and neither is the primary. This crate is the seam that keeps that true:
//! it turns the session event log into a semantic document, and both views are
//! projections of that document.
//!
//! Four rules keep the arrangement honest, in descending order of how
//! mechanically they are enforced:
//!
//! 1. **This crate must not depend on a rendering toolkit.** Enforced by
//!    `tests/no_toolkit_dependency.rs`, which fails the build on a ratatui or
//!    gpui edge in the dependency graph.
//! 2. **No width crosses into this crate.** Wrapping to a column count is a
//!    terminal concept; gpui shapes proportional text where columns are
//!    meaningless. If a function here takes a width, it is in the wrong crate.
//! 3. **Semantic style, not visual style.** [`inline::InlineKind`] says what a
//!    run of text *is*. Each view owns the palette.
//! 4. **No runtime.** Nothing here spawns. tokio drives the TUI and gpui drives
//!    the GUI with its own executor, so the core is a reducer plus channels.

pub mod inline;
pub mod markdown;
mod syntax;
pub mod transcript;
pub mod workspace;

pub use inline::{Inline, InlineKind, InlineLine, TokenKind};
pub use markdown::MarkdownStream;
pub use transcript::{
    Block, Notice, NoticeKind, PromptEvent, Role, ToolCard, ToolImage, ToolStatus, Transcript,
    Usage,
};
pub use workspace::{
    FocusContext, FocusStack, FocusableObject, FocusableObjectId, ObjectKind, ObjectState,
    SessionNode, WorkspaceProjection,
};
