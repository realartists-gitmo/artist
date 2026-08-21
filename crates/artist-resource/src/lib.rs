//! URI-addressed text resources and the shared Artist tool registry.

mod filesystem;
mod registry;
mod resource;
mod router;
mod search;
mod streaming;
mod uri;

#[cfg(target_os = "linux")]
mod fabric;
#[cfg(target_os = "linux")]
pub mod fuse;

#[cfg(target_os = "linux")]
pub use fabric::*;
pub use filesystem::*;
pub use registry::*;
pub use resource::*;
pub use router::*;
pub use search::*;
pub use streaming::*;
pub use uri::*;
