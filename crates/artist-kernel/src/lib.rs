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
mod invocation;
mod operation;
mod process;
mod registry;
mod repository;
mod resolver;
mod resources;
mod routing;
mod search;
mod session;
mod structure;
mod typed;
mod uri;
mod verbs;

pub use address::ResourceAddress;
pub use anchors::{AddressedItem, Anchor, AnchorError, AnchorInput, AnchorSet};
pub use claims::{ClaimRegistry, ClaimedResource, DynamicClaimProvider};
pub use dynamic::{DynamicType, DynamicValue, DynamicVerbCall, DynamicVerbResult};
pub use error::KernelError;
pub use filesystem::{FileHandler, FileResourceProvider, FileVerbBindings};
pub use handler::{
    BoxFuture, ClaimDecision, KernelHandle, ResourceCatalogDoc, ResourceCatalogEntry,
    ResourceCatalogProvider, ToolDefinition, ToolModelResult, ToolProvider,
};
pub use invocation::{Invocation, InvocationResourceProvider, InvocationStatus, InvocationStore};
pub use operation::VerbId;
pub use process::{ProcessManager, ProcessResourceProvider, ProcessSnapshot, ProcessVerbBindings};
pub use registry::Kernel;
pub use repository::{RepositoryHandler, RepositoryResourceProvider, RepositoryVerbBindings};
pub use resolver::{is_file_uri, normalize};
pub use resources::{
    DynamicResourceProvider, MixedResourceRequest, ResourceBatchFuture, ResourceFuture,
    ResourceRegistry, ResourceRequest,
};
pub use routing::{DynamicRouteExtractor, ResourceUriValueExtractor, RouteRegistry};
pub use search::{Pattern, SearchService};
pub use session::{SessionHandler, SessionResourceProvider, SessionVerbBindings};
pub use structure::{
    CstError, CstProvider, LineFallbackProvider, RustCstProvider, StructuralAnalyzer,
    StructuralLine,
};
pub use typed::*;
pub use uri::ResourceUri;
pub use verbs::{
    ActiveVerb, DynamicVerbExecutor, VerbDefinition, VerbLease, VerbPackageManifest, VerbRegistry,
    VerbToolDescriptor, discover_verb_packages, dynamic_contract_from_wit, dynamic_type_from_wit,
};

#[derive(Clone, Debug, PartialEq)]
pub struct DynamicResourceResult {
    pub uri: ResourceUri,
    pub result: DynamicVerbResult,
}
