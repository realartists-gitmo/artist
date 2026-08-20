//! Generated bindings for the URI-aware noun contract (`artist:nouns@2`).
//!
//! The world `nouns-extension` exports the URI-aware `provider` interface.
//! On the host side bindgen produces a `Guest` trait per exported interface,
//! bound to an instantiated component, which the [`crate::host`] glue adapts
//! into the kernel's URI-facing resource-provider surface.

mod generated {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "nouns-extension",
        imports: { default: async | trappable },
        exports: { default: async },
    });
}

pub use generated::artist::nouns::resource_host;
pub use generated::artist::nouns::types;
pub use generated::exports::artist::nouns::provider as ns;
