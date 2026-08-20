//! The platform-neutral Artist resource kernel.
//!
//! The kernel owns the four native namespace roots, canonical URI routing, and
//! the VFS projection. Provider implementations own the behavior and source of
//! truth of the resources they serve.

pub mod kernel;
pub mod namespace;
pub mod native;
pub mod provider;
pub mod resources;
pub mod uri;
pub mod vfs;

pub use kernel::Kernel;
pub use native::{EmptyNamespace, FilesNamespace};
pub use provider::{
    ProviderAttrs, ProviderEntry, ResourceError, ResourceErrorCode, ResourceProvider,
};
pub use resources::Resources;
pub use uri::{ResourceUri, UriError};
pub use vfs::{Attrs, DirEntry, Ino, NodeKind, Vfs, VfsError};
