//! Kernel-owned dynamic verb invocation.
//!
//! The kernel does not know a verb's request or response schema. It owns only
//! the name-to-handler registry and transports the caller's opaque payload.
//! The default tool layer uses TOON for that payload, but the kernel remains
//! format-neutral.

use async_trait::async_trait;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerbInvocationError {
    InvalidArgument(String),
    NotFound(String),
    Unsupported(String),
    PermissionDenied(String),
    Conflict(String),
    Aborted(String),
    Internal(String),
}

#[async_trait]
pub trait VerbHandler: Send + Sync {
    fn name(&self) -> &str;

    async fn invoke(&self, request: &[u8]) -> Result<Vec<u8>, VerbInvocationError>;
}
