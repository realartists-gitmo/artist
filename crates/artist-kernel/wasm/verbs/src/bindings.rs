//! Generated bindings for the verb contract (`artist:verbs@1`).
//!
//! Generated bindings for the shared verb types and the first concrete `read`
//! verb interface.

mod generated {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "verbs-extension",
        imports: { default: async | trappable },
    });
}

mod tool_generated {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "tool-extension",
        imports: { default: async | trappable },
        exports: { default: async },
    });
}

pub use generated::artist::verbs::registry;
pub use generated::artist::verbs::types as ty;
pub use generated::exports::artist::verbs::find as find_bindings;
pub use generated::exports::artist::verbs::move_ as move_bindings;
pub use generated::exports::artist::verbs::read;
pub use generated::exports::artist::verbs::write as write_bindings;
pub use tool_generated::artist::verbs::resource_api as resource;
pub use tool_generated::exports::artist::verbs::tool;
