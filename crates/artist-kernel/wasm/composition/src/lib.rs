//! Host bindings for the replaceable model-context composition contract.

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "composition-extension",
        imports: { default: async | trappable },
        exports: { default: async },
    });
}

pub use bindings::artist::composition::resource_host;
pub use bindings::artist::composition::types;
pub use bindings::exports::artist::composition::extension;
