//! URI-addressed text resources and the shared Artist tool registry.

mod anchor;
mod filesystem;
mod profiles;
mod registry;
mod resource;
mod router;
mod search;
mod storage;
pub mod storage_provider;
mod streaming;
mod uri;

mod fabric;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod fuse;
mod mount;
#[cfg(target_os = "windows")]
mod winfsp;

pub use anchor::*;
pub use fabric::*;
pub use filesystem::*;
pub use mount::*;
pub use profiles::*;
pub use registry::*;
pub use resource::*;
pub use router::*;
pub use search::*;
pub use storage::*;
pub use streaming::*;
pub use uri::*;
