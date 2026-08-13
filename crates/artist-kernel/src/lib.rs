//! The resource kernel: URI routing, universal operations, and handler calls.
//!
//! This crate intentionally contains no concrete harness integrations yet.
//! Handlers are the extension point for files, sessions, repositories, and
//! future virtual resources.

mod address;
mod anchors;
mod error;
mod filesystem;
mod handler;
mod operation;
mod registry;
mod repository;
mod request;
mod resolver;
mod result;
mod session;
mod structure;
mod uri;

pub use address::ResourceAddress;
pub use anchors::{AddressedItem, Anchor, AnchorError, AnchorInput, AnchorSet};
pub use error::KernelError;
pub use filesystem::FileHandler;
pub use handler::{
    BoxFuture, Handler, HandlerDescriptor, KernelHandle, ToolDefinition, ToolProvider,
};
pub use operation::Verb;
pub use registry::Kernel;
pub use repository::RepositoryHandler;
pub use request::{BatchRequest, Request};
pub use resolver::{has_projection, is_file_uri, normalize};
pub use result::{BatchResult, ItemResult};
pub use session::SessionHandler;
pub use structure::{
    CstError, CstProvider, LineFallbackProvider, RustCstProvider, StructuralAnalyzer,
    StructuralLine,
};
pub use uri::ResourceUri;
