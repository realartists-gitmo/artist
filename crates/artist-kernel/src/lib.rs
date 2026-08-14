//! The resource kernel: URI routing, universal operations, and handler calls.
//!
//! This crate intentionally contains no concrete harness integrations yet.
//! Handlers are the extension point for files, sessions, repositories, and
//! future virtual resources.

mod address;
mod anchors;
mod claims;
mod dynamic;
mod error;
mod filesystem;
mod handler;
mod operation;
mod process;
mod registry;
mod repository;
mod request;
mod resolver;
mod result;
mod routing;
mod search;
mod session;
mod structure;
mod typed;
mod uri;
mod verbs;

pub use address::ResourceAddress;
pub use anchors::{AddressedItem, Anchor, AnchorError, AnchorInput, AnchorSet};
pub use claims::{ClaimRegistry, DynamicClaimProvider};
pub use dynamic::{DynamicType, DynamicValue, DynamicVerbCall, DynamicVerbResult};
pub use error::KernelError;
pub use filesystem::FileHandler;
pub use handler::{
    BoxFuture, ClaimDecision, Handler, HandlerDescriptor, KernelHandle, ResourceCatalogDoc,
    ResourceCatalogEntry, ResourceCatalogProvider, ToolDefinition, ToolProvider, TypedHandler,
};
pub use operation::{Verb, VerbId};
pub use process::{ProcessManager, ProcessSnapshot};
pub use registry::Kernel;
pub use repository::RepositoryHandler;
pub use request::{BatchRequest, Request};
pub use resolver::{is_file_uri, normalize};
pub use result::{BatchResult, ItemResult};
pub use routing::{DynamicRouteExtractor, RouteRegistry};
pub use search::{Pattern, SearchService};
pub use session::SessionHandler;
pub use structure::{
    CstError, CstProvider, LineFallbackProvider, RustCstProvider, StructuralAnalyzer,
    StructuralLine,
};
pub use typed::*;
pub use uri::ResourceUri;
pub use verbs::{
    ActiveVerb, DynamicVerbExecutor, VerbDefinition, VerbLease, VerbPackageManifest, VerbRegistry,
    VerbToolDescriptor, discover_verb_packages,
};
