//! Dynamic verb-owned route extraction.

use crate::{DynamicValue, KernelError, ResourceUri, VerbId};
use std::sync::Arc;

pub trait DynamicRouteExtractor: Send + Sync {
    fn extract(&self, input: &DynamicValue) -> Result<Vec<ResourceUri>, KernelError>;
}

#[derive(Clone, Default)]
pub struct RouteRegistry {
    extractors:
        Arc<std::sync::RwLock<std::collections::BTreeMap<VerbId, Arc<dyn DynamicRouteExtractor>>>>,
}

impl RouteRegistry {
    pub fn register(
        &self,
        identity: VerbId,
        extractor: Arc<dyn DynamicRouteExtractor>,
    ) -> Result<(), KernelError> {
        self.extractors
            .write()
            .map_err(|_| KernelError::Handler {
                message: "route registry lock poisoned".into(),
            })?
            .insert(identity, extractor);
        Ok(())
    }

    pub fn extract(
        &self,
        identity: &VerbId,
        input: &DynamicValue,
    ) -> Result<Vec<ResourceUri>, KernelError> {
        let extractor = self
            .extractors
            .read()
            .map_err(|_| KernelError::Handler {
                message: "route registry lock poisoned".into(),
            })?
            .get(identity)
            .cloned()
            .ok_or_else(|| KernelError::UnsupportedVerb {
                verb: identity.to_string(),
                uri: "<dynamic-route>".into(),
            })?;
        extractor.extract(input)
    }

    pub fn extract_and_arbitrate(
        &self,
        identity: &VerbId,
        input: &DynamicValue,
        claims: &crate::ClaimRegistry,
    ) -> Result<Vec<(ResourceUri, crate::ClaimDecision)>, KernelError> {
        self.extract(identity, input)?
            .into_iter()
            .map(|uri| {
                let decision = claims.arbitrate(identity, &uri)?;
                Ok((uri, decision))
            })
            .collect()
    }

    pub fn extract_and_arbitrate_for_lease(
        &self,
        lease: &crate::VerbLease,
        input: &DynamicValue,
        claims: &crate::ClaimRegistry,
    ) -> Result<Vec<crate::ClaimedResource>, KernelError> {
        self.extract(&lease.definition().identity, input)?
            .into_iter()
            .map(|uri| claims.arbitrate_for_lease(lease, uri))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DynamicValue, ResourceUri};

    struct UriExtractor;
    impl DynamicRouteExtractor for UriExtractor {
        fn extract(&self, input: &DynamicValue) -> Result<Vec<ResourceUri>, KernelError> {
            let DynamicValue::String(uri) = input else {
                return Err(KernelError::InvalidRequest {
                    message: "expected URI".into(),
                });
            };
            Ok(vec![ResourceUri::parse(uri)?])
        }
    }

    #[test]
    fn dynamic_route_extractor_is_ordered_and_name_agnostic() {
        let registry = RouteRegistry::default();
        let identity = VerbId::new("example:custom/read@1.0.0").unwrap();
        registry
            .register(identity.clone(), Arc::new(UriExtractor))
            .unwrap();
        let routes = registry
            .extract(&identity, &DynamicValue::String("file:///tmp/a".into()))
            .unwrap();
        assert_eq!(routes[0].to_string(), "file:///tmp/a");
    }
}
