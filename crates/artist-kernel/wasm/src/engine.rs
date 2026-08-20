//! Engine construction for the wasm host.
//!
//! The host is async-first (locked in `kernel-wasm-scaffold.md`): the kernel is
//! tokio + async-trait, and the events contract is inherently async. The
//! component model is compiled with async support enabled.

use wasmtime::{Config, Engine};

/// Build the host engine with component-model async support.
pub fn build_engine() -> anyhow::Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    // Every component store receives a bounded fuel budget from Runtime. This
    // prevents a guest from spinning forever independently of host I/O.
    config.consume_fuel(true);
    Ok(Engine::new(&config)?)
}
