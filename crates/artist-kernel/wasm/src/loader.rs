//! Loading and classification of extensions.
//!
//! A loaded extension is a compiled [`wasmtime::component::Component`] plus its
//! [`ExtensionClass`]. Lifecycle is minimal (locked in
//! `kernel-wasm-scaffold.md`): load-on-demand, one live instance per extension.
//! Hot-reload is deferred to its own subcrate.

use wasmtime::Engine;
use wasmtime::component::Component;

use crate::classify::{ExtensionClass, classify};

/// A compiled extension component with its classified contract families.
pub struct Extension {
    pub component: Component,
    pub class: ExtensionClass,
}

impl Extension {
    /// Compile and classify an extension from wasm bytes.
    pub fn load(engine: &Engine, bytes: &[u8]) -> anyhow::Result<Self> {
        let component = Component::new(engine, bytes)?;
        let class = classify(engine, &component);
        Ok(Self { component, class })
    }
}
