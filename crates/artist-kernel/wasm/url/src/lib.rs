//! URL claim registration for the trusted URL root component.
//!
//! This is deliberately only a claim table. It does not impose a universal
//! namespace hierarchy or decide what a claim means; the component that owns
//! a claim interprets the complete URI. Duplicate canonical claims are hard
//! errors.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use anyhow::anyhow;
use wasmtime::component::{HasData, Linker};

pub mod bindings;

pub use bindings::artist::url::registry;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimToken {
    id: u64,
    uri: String,
}

impl ClaimToken {
    pub fn id(&self) -> u64 { self.id }
    pub fn uri(&self) -> &str { &self.uri }
}

#[derive(Default)]
struct Claims {
    next_id: u64,
    by_uri: BTreeMap<String, ClaimToken>,
}

/// Process-local registry state. A separate instance is supplied to each
/// session/root, so claims are scoped to that VFS view while still being
/// visible to every component in the session.
#[derive(Clone, Default)]
pub struct UrlClaimRegistry {
    claims: Arc<RwLock<Claims>>,
}

impl UrlClaimRegistry {
    pub fn register(&self, uri: &str) -> anyhow::Result<ClaimToken> {
        let canonical = url::Url::parse(uri)
            .map(|value| value.to_string())
            .map_err(|error| anyhow!("invalid URI claim {uri}: {error}"))?;
        let mut claims = self.claims.write().unwrap();
        if claims.by_uri.contains_key(&canonical) {
            return Err(anyhow!("URI claim already registered: {canonical}"));
        }
        claims.next_id += 1;
        let token = ClaimToken { id: claims.next_id, uri: canonical.clone() };
        claims.by_uri.insert(canonical, token.clone());
        Ok(token)
    }

    pub fn unregister(&self, token: &ClaimToken) -> anyhow::Result<()> {
        let mut claims = self.claims.write().unwrap();
        match claims.by_uri.get(&token.uri) {
            Some(current) if current == token => {
                claims.by_uri.remove(&token.uri);
                Ok(())
            }
            _ => Err(anyhow!("URI claim is not registered by this component")),
        }
    }

    pub fn claims(&self) -> Vec<String> {
        self.claims.read().unwrap().by_uri.keys().cloned().collect()
    }
}

pub struct UrlRegistryContext {
    pub registry: UrlClaimRegistry,
    registrations: BTreeMap<u32, ClaimToken>,
    next_registration: u32,
}

impl UrlRegistryContext {
    pub fn new(registry: UrlClaimRegistry) -> Self {
        Self { registry, registrations: BTreeMap::new(), next_registration: 0 }
    }
}

pub struct UrlRegistryMarker;

impl HasData for UrlRegistryMarker {
    type Data<'a> = UrlRegistryContext;
}

impl registry::HostRegistration for UrlRegistryContext {
    async fn drop(&mut self, value: wasmtime::component::Resource<registry::Registration>) -> wasmtime::Result<()> {
        if let Some(token) = self.registrations.remove(&value.rep()) {
            let _ = self.registry.unregister(&token);
        }
        Ok(())
    }
}

impl registry::Host for UrlRegistryContext {
    async fn register(
        &mut self,
        uri: String,
    ) -> Result<wasmtime::component::Resource<registry::Registration>, registry::Error> {
        match self.registry.register(&uri) {
            Ok(token) => {
                self.next_registration += 1;
                let id = self.next_registration;
                self.registrations.insert(id, token);
                Ok(wasmtime::component::Resource::new_own(id))
            }
            Err(error) if error.to_string().starts_with("invalid URI") => Err(registry::Error::InvalidUri),
            Err(_) => Err(registry::Error::Conflict),
        }
    }

    async fn unregister(
        &mut self,
        value: wasmtime::component::Resource<registry::Registration>,
    ) -> Result<(), registry::Error> {
        let Some(token) = self.registrations.remove(&value.rep()) else {
            return Err(registry::Error::NotFound);
        };
        match self.registry.unregister(&token) {
            Ok(()) => Ok(()),
            Err(_) => Err(registry::Error::NotFound),
        }
    }
}

pub fn add_to_linker<T>(linker: &mut Linker<T>, registry: fn(&mut T) -> UrlRegistryContext) -> wasmtime::Result<()>
where
    T: Send + 'static,
{
    bindings::UrlRoot::add_to_linker::<T, UrlRegistryMarker>(linker, registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_claims_are_hard_conflicts_and_release_cleanly() {
        let registry = UrlClaimRegistry::default();
        let token = registry.register("ast://file.rs/symbols").unwrap();
        assert!(registry.register("ast://file.rs/symbols").is_err());
        registry.unregister(&token).unwrap();
        assert_eq!(registry.register("ast://file.rs/symbols").unwrap().uri, "ast://file.rs/symbols");
    }
}
