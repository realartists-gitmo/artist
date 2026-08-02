//! Generalized computer use.
//!
//! One observation contract over every application surface, selecting the
//! cheapest abstraction each one supports rather than clicking everything with
//! equal enthusiasm.
//!
//! The design is documented in `docs/computer-use.md`. The short version:
//!
//! * **Observation is the problem**, not action. The verb list is trivial; what
//!   is hard is keeping a hundred-step session inside a context window while
//!   still noticing what changed. Hence [`anchors`] (identity-only naming,
//!   deltas by default) and [`render`] (a budget, and no coordinates).
//! * **The model names things, never places.** It emits an anchor from an
//!   observation plus its own echo of the element's label; the harness resolves
//!   both, and a mismatch is a loud error rather than a click on whatever moved
//!   into that position.
//! * **Backends differ in capability, not in contract.** A [`surface::Surface`]
//!   emits nodes and consumes steps; everything model-facing lives above it.

pub mod anchors;
pub mod doctor;
pub mod extract;
pub mod host;
pub mod keys;
pub mod ladder;
pub mod macros;
pub mod model;
#[cfg(feature = "ocr")]
pub mod ocr;
pub mod program;
pub mod render;
pub mod search;
pub mod stage;
pub mod surface;
pub mod tool;

pub use anchors::{AnchorBook, AnchorError, Change, Entry, Observation};
pub use model::{Binding, Caps, Frame, Node, NodeState, Rect, Role, Rung, Snapshot, SurfaceId};
pub use program::{Program, Settle, SettleKind, SettleOutcome, Step, StepError, Target};
pub use surface::{ProgramReport, StepReport, Surface, run_program};
pub use tool::{ComputerArgs, ComputerTool, Restorable, SurfaceRegistry};
