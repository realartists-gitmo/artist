//! The platform-neutral Artist resource kernel.
//!
//! The kernel owns the `url://` namespace-registration root, canonical URI routing, and
//! the VFS projection. Provider implementations own the behavior and source of
//! truth of the resources they serve.

pub mod contracts;
pub mod kernel;
pub mod namespace;
pub mod native;
pub mod provider;
pub mod resources;
pub mod uri;
pub mod verbs;
pub mod vfs;

pub use contracts::{ContractRegistry, ExtensionContract};
pub use kernel::Kernel;
pub use native::{EmptyNamespace, FilesNamespace};
pub use provider::{
    LayeredResourceProvider, ProviderAttrs, ProviderEntry, ResourceError, ResourceErrorCode,
    ResourceProvider, ResourceSignal,
};
pub use resources::UrlNamespace;
pub use uri::{ResourceUri, UriError};
pub use verbs::{VerbHandler, VerbInvocationError};
pub use vfs::{Attrs, DirEntry, Ino, NodeKind, Vfs, VfsError};
