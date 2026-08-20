//! artist-kernel/wasm — the wasm extensibility surface host.
//!
//! Per `extension-contracts.md`, extensions are wasm components; their class is
//! which of the three contract families their exported interfaces satisfy:
//!
//! * nouns (`artist:nouns/namespace`) — addressable, filesystem-shaped resources
//! * verbs (`artist:verbs/*`) — typed operations over resources
//! * events (`artist:events/subscriber`) — long-lived, reactive services
//!
//! Per `kernel-wasm-scaffold.md` this is a horizontal slice: the entire host
//! surface, no guest. The host glue for each family lives in the sibling
//! subcrates (`nouns/`, `verbs/`, `events/`); this crate owns engine, loading,
//! and classification.

pub mod classify;
pub mod engine;
pub mod loader;
pub mod runtime;

pub use classify::{ExtensionClass, names};
pub use engine::build_engine;
pub use loader::{Extension, Extension as LoadedExtension};
pub use runtime::{
    ComponentRuntime, ExtensionMetadata, GenerationHandle, GenerationLease, HostEnvironment,
    KernelHostEnvironment, PreparedGeneration, ResourceCapabilities, Runtime, RuntimeStore,
    ScopedHostEnvironment,
};
