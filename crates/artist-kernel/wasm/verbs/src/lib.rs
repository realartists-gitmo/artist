//! artist-wasm-verbs — the verb contract host side.
//!
//! A component exporting verb interfaces is a tool. Verbs are typed operations
//! over resources, batch-native at the ABI, scalar at the model surface. The
//! base package currently provides `read`, `write`, `move`, `edit`, `find`, and `grep`.

pub mod bindings;
pub mod component;
pub mod edit;
pub mod find;
pub mod grep;
pub mod host;
pub mod move_;
pub mod process;
pub mod read;
pub mod write;

pub use host::{ToonVerbHandler, VerbDispatcher, VerbError, VerbTool};

/// Link the capability-scoped WASI filesystem imports required by verb
/// components. The embedder supplies the store projection and preopens.
pub use artist_wasm_filesystem::{FilesystemView, add_to_linker as add_filesystem_to_linker};
