//! Dynamic verb-owned route extraction.

use crate::{DynamicValue, KernelError, ResourceUri, VerbId};
use std::sync::Arc;

pub trait DynamicRouteExtractor: Send + Sync {
    fn extract(&self, input: &DynamicValue) -> Result<Vec<ResourceUri>, KernelError>;
}

/// Extract every typed resource URI from a dynamic value in structural order.
///
/// This is a useful package adapter for contracts whose routing fields are
/// represented by the shared `uri` WIT alias. It deliberately does not inspect
/// field names, request verbs, or JSON keys; custom packages can register a
/// more selective `DynamicRouteExtractor` when their contract requires one.
#[derive(Clone, Copy, Debug, Default)]
pub struct ResourceUriValueExtractor;

impl DynamicRouteExtractor for ResourceUriValueExtractor {
    fn extract(&self, input: &DynamicValue) -> Result<Vec<ResourceUri>, KernelError> {
        let mut routes = Vec::new();
        collect_resource_uris(input, &mut routes);
        Ok(routes)
    }
}

fn collect_resource_uris(value: &DynamicValue, routes: &mut Vec<ResourceUri>) {
    match value {
        DynamicValue::ResourceUri(uri) => routes.push(uri.clone()),
        DynamicValue::List(values) | DynamicValue::Tuple(values) => {
            for value in values {
                collect_resource_uris(value, routes);
            }
        }
        DynamicValue::Record(fields) => {
            for value in fields.values() {
                collect_resource_uris(value, routes);
            }
        }
        DynamicValue::Option(Some(value))
        | DynamicValue::Result(Ok(value))
        | DynamicValue::Result(Err(value)) => collect_resource_uris(value, routes),
        DynamicValue::Variant(_, Some(value)) => collect_resource_uris(value, routes),
        DynamicValue::Bool(_)
        | DynamicValue::S8(_)
        | DynamicValue::S16(_)
        | DynamicValue::S32(_)
        | DynamicValue::S64(_)
        | DynamicValue::U8(_)
        | DynamicValue::U16(_)
        | DynamicValue::U32(_)
        | DynamicValue::U64(_)
        | DynamicValue::F32(_)
        | DynamicValue::F64(_)
        | DynamicValue::Char(_)
        | DynamicValue::String(_)
        | DynamicValue::Option(None)
        | DynamicValue::Variant(_, None)
        | DynamicValue::Enum(_)
        | DynamicValue::Flags(_) => {}
    }
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
    use std::collections::BTreeMap;

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

    #[test]
    fn resource_uri_extractor_walks_nested_typed_values_without_field_names() {
        let extractor = ResourceUriValueExtractor;
        let first = ResourceUri::parse("file:///tmp/one").unwrap();
        let second = ResourceUri::parse("session://local/two").unwrap();
        let input = DynamicValue::Record(BTreeMap::from([
            (
                "targets".into(),
                DynamicValue::List(vec![
                    DynamicValue::ResourceUri(first.clone()),
                    DynamicValue::Option(Some(Box::new(DynamicValue::ResourceUri(second.clone())))),
                ]),
            ),
            ("label".into(), DynamicValue::String("ignored".into())),
        ]));
        assert_eq!(extractor.extract(&input).unwrap(), vec![first, second]);
    }
}
