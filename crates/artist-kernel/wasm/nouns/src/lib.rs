//! artist-wasm-nouns — the noun contract host glue.
//!
//! A component exporting `artist:nouns/namespace` is a noun: an addressable,
//! filesystem-shaped resource. [`WasmNamespace`] adapts such a component into
//! the kernel's ino-based [`artist_kernel::Namespace`] surface.

pub mod bindings;
pub mod host;

pub use host::{NamespaceGuest, WasmNamespace};
