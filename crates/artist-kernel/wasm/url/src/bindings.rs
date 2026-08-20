//! Generated bindings for the component-owned `url://` registry contract.

mod generated {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "url-root",
        imports: { default: async },
    });
}

pub use generated::*;
