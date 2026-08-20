//! Generated host bindings for the universal URI resource contract.

mod generated {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "resource-provider",
        exports: { default: async },
    });
}

pub use generated::*;
