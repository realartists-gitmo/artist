//! artist-wasm-events — the event contract host side.
//!
//! A component exporting `artist:events/subscriber` is a service: long-lived,
//! reactive, and addressable. The kernel hosts the broker that subscribes,
//! emits, assigns sequence numbers, and fans events out to subscribers.

pub mod bindings;
pub mod host;

pub use host::{Event, EventBroker, EventError, Subscriber};
