//! artist-wasm-verbs — the verb contract host side.
//!
//! A component exporting verb interfaces is a tool. Verbs are typed operations
//! over resources, batch-native at the ABI, scalar at the model surface. No
//! specific verbs are defined yet; this crate pins the family shape and the
//! dispatch path.

pub mod bindings;
pub mod host;

pub use host::{VerbDispatcher, VerbError, VerbTool};

/// Link the capability-scoped WASI filesystem imports required by verb
/// components. The embedder supplies the store projection and preopens.
pub use artist_wasm_filesystem::{FilesystemView, add_to_linker as add_filesystem_to_linker};
