//! Durable account descriptors. Accounts hold non-secret metadata plus opaque
//! credential *references*; secret material never leaves the credential store.

use std::sync::Arc;

use async_trait::async_trait;

use crate::{AccountDescriptor, ProviderError};

/// Durable home for account descriptors. Backends may be file-backed, a
/// remote vault of metadata, or in-memory for tests.
#[async_trait]
pub trait AccountStore: Send + Sync {
    async fn put(&self, account: &AccountDescriptor) -> Result<(), ProviderError>;
    async fn get(&self, id: &str) -> Result<Option<AccountDescriptor>, ProviderError>;
    async fn delete(&self, id: &str) -> Result<bool, ProviderError>;
    async fn list(&self) -> Result<Vec<AccountDescriptor>, ProviderError>;
}

/// `ScopedResourceStore`-backed account persistence. One flat scope holds one
/// JSON document per account id; restart-safe when backed by the file store.
pub struct ScopedAccountStore {
    store: Arc<dyn artist_resource::ScopedResourceStore>,
    scope: String,
}

impl ScopedAccountStore {
    pub const SCOPE: &'static str = "accounts";

    pub fn shared(
        store: Arc<dyn artist_resource::ScopedResourceStore>,
        scope: impl Into<String>,
    ) -> Self {
        Self {
            store,
            scope: scope.into(),
        }
    }

    /// File-backed default under the host working directory.
    pub fn open(root: impl Into<std::path::PathBuf>) -> Result<Self, String> {
        Ok(Self::shared(
            Arc::new(
                artist_resource::FileResourceStore::open(root)
                    .map_err(|error| error.to_string())?,
            ),
            Self::SCOPE,
        ))
    }
}

fn encode(account: &AccountDescriptor) -> Result<Vec<u8>, ProviderError> {
    serde_json::to_vec(account)
        .map_err(|error| ProviderError::State(format!("account is not serializable: {error}")))
}

fn decode(bytes: &[u8]) -> Result<AccountDescriptor, ProviderError> {
    serde_json::from_slice(bytes)
        .map_err(|error| ProviderError::State(format!("stored account is invalid: {error}")))
}

#[async_trait]
impl AccountStore for ScopedAccountStore {
    async fn put(&self, account: &AccountDescriptor) -> Result<(), ProviderError> {
        self.store
            .write(&self.scope, &account.id, encode(account)?, None)
            .await
            .map_err(|error| ProviderError::State(format!("account put failed: {error}")))?;
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<AccountDescriptor>, ProviderError> {
        match self.store.read(&self.scope, id).await {
            Ok(Some(document)) => Ok(Some(decode(&document.value)?)),
            Ok(None) => Ok(None),
            Err(error) => Err(ProviderError::State(format!("account get failed: {error}"))),
        }
    }

    async fn delete(&self, id: &str) -> Result<bool, ProviderError> {
        self.store
            .delete(&self.scope, id, None)
            .await
            .map_err(|error| ProviderError::State(format!("account delete failed: {error}")))
    }

    async fn list(&self) -> Result<Vec<AccountDescriptor>, ProviderError> {
        let documents = self
            .store
            .list(&self.scope)
            .await
            .map_err(|error| ProviderError::State(format!("account list failed: {error}")))?;
        documents
            .iter()
            .map(|document| decode(&document.value))
            .collect()
    }
}

/// In-memory implementation for tests.
#[derive(Default)]
pub struct MemoryAccountStore {
    accounts: tokio::sync::Mutex<std::collections::BTreeMap<String, AccountDescriptor>>,
}

#[async_trait]
impl AccountStore for MemoryAccountStore {
    async fn put(&self, account: &AccountDescriptor) -> Result<(), ProviderError> {
        self.accounts
            .lock()
            .await
            .insert(account.id.clone(), account.clone());
        Ok(())
    }
    async fn get(&self, id: &str) -> Result<Option<AccountDescriptor>, ProviderError> {
        Ok(self.accounts.lock().await.get(id).cloned())
    }
    async fn delete(&self, id: &str) -> Result<bool, ProviderError> {
        Ok(self.accounts.lock().await.remove(id).is_some())
    }
    async fn list(&self) -> Result<Vec<AccountDescriptor>, ProviderError> {
        Ok(self.accounts.lock().await.values().cloned().collect())
    }
}

/// Hydrate the registry's in-memory accounts from durable state at startup.
/// Accounts whose provider is not installed are skipped and reported.
pub async fn hydrate_accounts(
    registry: &crate::ProviderRegistry,
    store: &dyn AccountStore,
) -> Result<(usize, usize), ProviderError> {
    let accounts = store.list().await?;
    let mut restored = 0;
    let mut skipped = 0;
    for account in accounts {
        if registry.upsert_account(account).is_ok() {
            restored += 1;
        } else {
            skipped += 1;
        }
    }
    Ok((restored, skipped))
}

/// Write-through helper: persist then register.
pub async fn register_account_durable(
    registry: &crate::ProviderRegistry,
    store: &dyn AccountStore,
    account: AccountDescriptor,
) -> Result<(), ProviderError> {
    store.put(&account).await?;
    registry.upsert_account(account)
}

/// Deletion helper: remove from both stores so retries stay consistent.
pub async fn delete_account_durable(
    registry: &crate::ProviderRegistry,
    store: &dyn AccountStore,
    id: &str,
) -> Result<bool, ProviderError> {
    let removed = store.delete(id).await?;
    registry.delete_account(id);
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderId;

    fn account(id: &str) -> AccountDescriptor {
        AccountDescriptor {
            id: id.into(),
            provider: ProviderId("openai".into()),
            credential_ref: "keychain:openai/work".into(),
            credential_kind: "api-key".into(),
            api_variant: "responses".into(),
            default_model: "gpt-5".into(),
            default_reasoning: None,
            metadata: Default::default(),
        }
    }

    #[tokio::test]
    async fn accounts_round_trip_and_survive_restart() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("accounts");
        {
            let store = ScopedAccountStore::open(&path).unwrap();
            store.put(&account("work")).await.unwrap();
            store.put(&account("personal")).await.unwrap();
            assert_eq!(store.list().await.unwrap().len(), 2);
        }
        let reopened = ScopedAccountStore::open(&path).unwrap();
        let work = reopened.get("work").await.unwrap().expect("persisted");
        assert_eq!(work.provider.0, "openai");
        assert_eq!(reopened.list().await.unwrap().len(), 2);
        assert!(reopened.delete("personal").await.unwrap());
        assert!(reopened.get("personal").await.unwrap().is_none());
    }
}
