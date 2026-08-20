//! Generated bindings for the noun contract (`artist:nouns@1`).
//!
//! The world `nouns-extension` has a single export: the `namespace` interface.
//! On the host side bindgen produces a `Guest` trait per exported interface,
//! bound to an instantiated component, which the [`crate::host`] glue adapts
//! into the kernel's ino-based [`artist_kernel::Namespace`] surface.

mod generated {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "nouns-extension",
        exports: { default: async },
    });
}

pub use generated::exports::artist::nouns::namespace as ns;
