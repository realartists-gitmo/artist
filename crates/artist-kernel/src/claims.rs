//! Dynamic claim arbitration keyed by versioned verb identity and URI.

use crate::{ClaimDecision, KernelError, ResourceUri, VerbId};
use std::sync::Arc;

pub trait DynamicClaimProvider: Send + Sync {
    fn claim(&self, verb: &VerbId, uri: &ResourceUri) -> ClaimDecision;
}

#[derive(Clone, Default)]
pub struct ClaimRegistry {
    providers: Arc<std::sync::RwLock<Vec<Arc<dyn DynamicClaimProvider>>>>,
}

impl ClaimRegistry {
    pub fn register(&self, provider: Arc<dyn DynamicClaimProvider>) -> Result<(), KernelError> {
        self.providers
            .write()
            .map_err(|_| KernelError::Handler {
                message: "claim registry lock poisoned".into(),
            })?
            .push(provider);
        Ok(())
    }

    pub fn arbitrate(
        &self,
        verb: &VerbId,
        uri: &ResourceUri,
    ) -> Result<ClaimDecision, KernelError> {
        let providers = self.providers.read().map_err(|_| KernelError::Handler {
            message: "claim registry lock poisoned".into(),
        })?;
        let mut handler = false;
        let mut reserve = false;
        for provider in providers.iter() {
            match provider.claim(verb, uri) {
                ClaimDecision::Pass => {}
                ClaimDecision::Handle => handler = true,
                ClaimDecision::Reserve => reserve = true,
            }
        }
        match (handler, reserve) {
            (true, true) => Err(KernelError::Conflict {
                uri: uri.to_string(),
            }),
            (true, false) => Ok(ClaimDecision::Handle),
            (false, true) => Ok(ClaimDecision::Reserve),
            (false, false) => Ok(ClaimDecision::Pass),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Provider(ClaimDecision);
    impl DynamicClaimProvider for Provider {
        fn claim(&self, _: &VerbId, _: &ResourceUri) -> ClaimDecision {
            self.0
        }
    }

    #[test]
    fn dynamic_claims_arbitrate_handle_reserve_and_conflict() {
        let registry = ClaimRegistry::default();
        registry
            .register(Arc::new(Provider(ClaimDecision::Handle)))
            .unwrap();
        let verb = VerbId::new("example:text/read@1.0.0").unwrap();
        let uri = ResourceUri::parse("file:///tmp/a").unwrap();
        assert_eq!(
            registry.arbitrate(&verb, &uri).unwrap(),
            ClaimDecision::Handle
        );
        registry
            .register(Arc::new(Provider(ClaimDecision::Reserve)))
            .unwrap();
        assert!(matches!(
            registry.arbitrate(&verb, &uri),
            Err(KernelError::Conflict { .. })
        ));
    }
}
