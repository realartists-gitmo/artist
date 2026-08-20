//! Kernel-owned dynamic verb invocation.
//!
//! The kernel does not know a verb's request or response schema. It owns only
//! the name-to-handler registry and transports the caller's opaque payload.
//! The default tool layer uses TOON for that payload, but the kernel remains
//! format-neutral.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

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

#[derive(Default)]
pub struct VerbRegistry {
    handlers: RwLock<HashMap<String, Arc<dyn VerbHandler>>>,
}

impl VerbRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register or replace a handler. Names are the stable extension-facing
    /// API; a later registration deliberately replaces an earlier one.
    pub fn register<H: VerbHandler + 'static>(&self, handler: H) {
        self.handlers
            .write()
            .unwrap()
            .insert(handler.name().to_string(), Arc::new(handler));
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.handlers.read().unwrap().keys().cloned().collect();
        names.sort();
        names
    }

    pub async fn invoke(&self, name: &str, request: &[u8]) -> Result<Vec<u8>, VerbInvocationError> {
        let handler = self
            .handlers
            .read()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| VerbInvocationError::NotFound(name.to_string()))?;
        handler.invoke(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait]
    impl VerbHandler for Echo {
        fn name(&self) -> &str {
            "echo"
        }

        async fn invoke(&self, request: &[u8]) -> Result<Vec<u8>, VerbInvocationError> {
            Ok(request.to_vec())
        }
    }

    #[tokio::test]
    async fn registers_and_invokes_opaque_payloads() {
        let registry = VerbRegistry::new();
        registry.register(Echo);
        assert_eq!(registry.names(), vec!["echo"]);
        assert_eq!(
            registry.invoke("echo", b"payload").await.unwrap(),
            b"payload"
        );
        assert!(matches!(
            registry.invoke("missing", b"").await,
            Err(VerbInvocationError::NotFound(name)) if name == "missing"
        ));
    }
}
