//! Canvas: agent-authored, hot-reloading local apps.
//!
//! A canvas is a React app the model writes with the ordinary `write`/`edit`
//! tools, served from the user's machine and reloaded as it changes. Unlike a
//! rendered artifact it is durable and project-local, and it can call back into
//! the running agent.
//!
//! The toolchain is entirely in-process: no Node, no bundler, no `npm install`.
//! Modules are served as unbundled ESM and transformed on the way out, and the
//! dependency set is compiled into the binary.

pub mod assets;
pub mod bridge;
pub mod deps;
pub mod docs;
pub mod drift;
pub mod editor;
pub mod highlight;
pub mod manifest;
pub mod markdown;
pub mod palette;
pub mod registry;
pub mod server;
pub mod state;
pub mod templates;
pub mod window;
pub mod transform;

pub use bridge::{CanvasHost, Denied, SendMode};
pub use manifest::Manifest;
pub use state::{Snapshot, StateStore};
pub use templates::{TEMPLATES, Template};
pub use registry::{Canvas, Registry, slugify};
pub use transform::{Diagnostic, TransformError, Transformed, transform};
