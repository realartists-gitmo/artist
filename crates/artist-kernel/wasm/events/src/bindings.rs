//! Generated bindings for the event contract (`artist:events@1`).
//!
//! The world `events-extension` imports `types` and `broker` (the host
//! implements the broker) and exports `subscriber` (the guest receives pushed
//! events). On the host side bindgen produces a `Host` trait for the imported
//! `broker` interface and a `Guest` trait for the exported `subscriber`
//! interface.

mod generated {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "events-extension",
        imports: { default: async | trappable },
        exports: { default: async },
    });
}

pub use generated::artist::events::types as et;
