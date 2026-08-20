//! Generic extension-defined contract dispatch.
//!
//! The kernel knows only that a contract has a name, version, operation, and
//! opaque payload. The extension that owns a noun namespace defines the
//! operation vocabulary and the payload/result encoding.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::provider::{ResourceError, ResourceErrorCode};

#[async_trait]
pub trait ExtensionContract: Send + Sync {
    async fn invoke(&self, operation: &str, input: &[u8]) -> Result<Vec<u8>, ResourceError>;
}

#[derive(Default)]
pub struct ContractRegistry {
    contracts: RwLock<BTreeMap<(String, String), Arc<dyn ExtensionContract>>>,
}

impl ContractRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<C: ExtensionContract + 'static>(
        &self,
        name: impl Into<String>,
        version: impl Into<String>,
        contract: C,
    ) -> Result<(), ResourceError> {
        let key = validate_key(name.into(), version.into())?;
        self.contracts
            .write()
            .unwrap()
            .insert(key, Arc::new(contract));
        Ok(())
    }

    pub fn unregister(&self, name: &str, version: &str) -> bool {
        self.contracts
            .write()
            .unwrap()
            .remove(&(name.to_owned(), version.to_owned()))
            .is_some()
    }

    pub async fn invoke(
        &self,
        name: &str,
        version: &str,
        operation: &str,
        input: &[u8],
    ) -> Result<Vec<u8>, ResourceError> {
        let contract = self
            .contracts
            .read()
            .unwrap()
            .get(&(name.to_owned(), version.to_owned()))
            .cloned()
            .ok_or_else(|| {
                ResourceError::new(
                    ResourceErrorCode::NotFound,
                    format!("extension contract not found: {name}@{version}"),
                )
            })?;
        contract.invoke(operation, input).await
    }
}

fn validate_key(name: String, version: String) -> Result<(String, String), ResourceError> {
    if name.trim().is_empty() || version.trim().is_empty() || name.contains('/') {
        return Err(ResourceError::new(
            ResourceErrorCode::InvalidAddress,
            "extension contract name and version must be non-empty and name-safe",
        ));
    }
    Ok((name, version))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait]
    impl ExtensionContract for Echo {
        async fn invoke(&self, operation: &str, input: &[u8]) -> Result<Vec<u8>, ResourceError> {
            Ok([operation.as_bytes(), b":", input].concat())
        }
    }

    #[tokio::test]
    async fn dispatch_is_opaque_to_the_kernel() {
        let registry = ContractRegistry::new();
        registry.register("rules", "1.0.0", Echo).unwrap();
        assert_eq!(
            registry
                .invoke("rules", "1.0.0", "check", b"payload")
                .await
                .unwrap(),
            b"check:payload"
        );
    }
}
