//! URI-addressed text resources and the shared Artist tool registry.

mod anchor;
mod filesystem;
mod profiles;
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

pub use anchor::*;
#[cfg(target_os = "linux")]
pub use fabric::*;
pub use filesystem::*;
pub use profiles::*;
pub use registry::*;
pub use resource::*;
pub use router::*;
pub use search::*;
pub use streaming::*;
pub use uri::*;
