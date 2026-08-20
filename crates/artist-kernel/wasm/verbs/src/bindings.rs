//! Generated bindings for the verb contract (`artist:verbs@1`).
//!
//! Specific verb interfaces are not defined yet. This crate provides the
//! shared `types` interface (the `error` algebra) and the batch-native family
//! shape that future verb interfaces must follow.

mod generated {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "verbs-extension",
        imports: { default: async | trappable },
    });
}

pub use generated::artist::verbs::types as ty;
