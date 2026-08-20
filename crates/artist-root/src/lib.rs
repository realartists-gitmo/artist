//! Minimal trusted root component.
//!
//! The root deliberately exports no application capability. Its existence
//! establishes the process-level component boundary; the host installs the
//! generic master contract and component packages provide all application
//! behavior below it.

wit_bindgen::generate!({
    path: "../artist-kernel/wasm/wit",
    world: "component-root",
});
