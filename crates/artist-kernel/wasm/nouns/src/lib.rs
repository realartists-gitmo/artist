//! artist-wasm-nouns — the noun contract host glue.
//!
//! A component exporting `artist:nouns/provider` is a noun: an addressable,
//! URI-aware resource provider. [`RoutedNoun`] adapts such a component into
//! the kernel's provider surface.

pub mod bindings;
pub mod host;

pub use host::{
    KernelResourceHost, ResourceHost, ResourceHostContext, ResourceHostView, RoutedNoun,
    RoutedNounGuest, WasmRoutedNoun, add_resource_host_to_linker,
};
