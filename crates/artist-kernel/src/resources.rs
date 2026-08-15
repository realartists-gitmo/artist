//! Open-ended typed resource-provider registry.
use crate::{
    ClaimDecision, DynamicClaimProvider, DynamicValue, DynamicVerbResult, KernelError, ResourceUri,
    VerbId,
};
use std::{future::Future, pin::Pin, sync::Arc};

pub type ResourceFuture<'a> =
    Pin<Box<dyn Future<Output = Result<DynamicVerbResult, KernelError>> + Send + 'a>>;

pub trait DynamicResourceProvider: DynamicClaimProvider + Send + Sync {
    fn invoke<'a>(
        &'a self,
        verb: &'a VerbId,
        uri: &'a ResourceUri,
        input: DynamicValue,
    ) -> ResourceFuture<'a>;

    /// Invoke a resource after the caller has pinned the active verb
    /// generation. Providers which keep their own replaceable resource
    /// implementation can override this hook to reject a stale generation;
    /// the default keeps existing providers source-compatible.
    fn invoke_at_generation<'a>(
        &'a self,
        verb: &'a VerbId,
        uri: &'a ResourceUri,
        generation: u64,
        input: DynamicValue,
    ) -> ResourceFuture<'a> {
        let _ = generation;
        self.invoke(verb, uri, input)
    }
}

#[derive(Clone, Default)]
pub struct ResourceRegistry {
    providers: Arc<std::sync::RwLock<Vec<Arc<dyn DynamicResourceProvider>>>>,
}

impl ResourceRegistry {
    pub fn register(&self, provider: Arc<dyn DynamicResourceProvider>) -> Result<(), KernelError> {
        self.providers
            .write()
            .map_err(|_| KernelError::Handler {
                message: "resource registry lock poisoned".into(),
            })?
            .push(provider);
        Ok(())
    }

    pub fn providers_for(
        &self,
        verb: &VerbId,
        uri: &ResourceUri,
    ) -> Result<Vec<Arc<dyn DynamicResourceProvider>>, KernelError> {
        Ok(self
            .providers
            .read()
            .map_err(|_| KernelError::Handler {
                message: "resource registry lock poisoned".into(),
            })?
            .iter()
            .filter(|provider| matches!(provider.claim(verb, uri), ClaimDecision::Handle))
            .cloned()
            .collect())
    }

    pub fn arbitrate_for_lease(
        &self,
        verb: &VerbId,
        uri: &ResourceUri,
        generation: u64,
    ) -> Result<crate::ClaimedResource, KernelError> {
        let providers = self.providers.read().map_err(|_| KernelError::Handler {
            message: "resource registry lock poisoned".into(),
        })?;
        let mut handles = 0usize;
        let mut reserved = false;
        for provider in providers.iter() {
            match provider.claim(verb, uri) {
                ClaimDecision::Handle => handles += 1,
                ClaimDecision::Reserve => reserved = true,
                ClaimDecision::Pass => {}
            }
        }
        let decision = if handles > 1 || (reserved && handles > 0) {
            return Err(KernelError::Conflict {
                uri: uri.to_string(),
            });
        } else if handles == 1 {
            ClaimDecision::Handle
        } else if reserved {
            ClaimDecision::Reserve
        } else {
            ClaimDecision::Pass
        };
        Ok(crate::ClaimedResource {
            uri: uri.clone(),
            decision,
            generation,
        })
    }

    pub async fn invoke(
        &self,
        verb: &VerbId,
        uri: &ResourceUri,
        input: DynamicValue,
    ) -> Result<DynamicVerbResult, KernelError> {
        let provider = {
            let providers = self.providers.read().map_err(|_| KernelError::Handler {
                message: "resource registry lock poisoned".into(),
            })?;
            let mut handler = None;
            let mut handlers = 0usize;
            let mut reserved = false;
            for provider in providers.iter() {
                match provider.claim(verb, uri) {
                    ClaimDecision::Handle => {
                        handlers += 1;
                        handler = Some(provider.clone());
                    }
                    ClaimDecision::Reserve => reserved = true,
                    ClaimDecision::Pass => {}
                }
            }
            if handlers > 1 || (reserved && handler.is_some()) {
                return Err(KernelError::Conflict {
                    uri: uri.to_string(),
                });
            }
            handler.ok_or_else(|| KernelError::UnsupportedVerb {
                verb: verb.to_string(),
                uri: uri.to_string(),
            })?
        };
        provider.invoke(verb, uri, input).await
    }

    pub async fn invoke_at_generation(
        &self,
        verb: &VerbId,
        uri: &ResourceUri,
        generation: u64,
        input: DynamicValue,
    ) -> Result<DynamicVerbResult, KernelError> {
        let provider = {
            let providers = self.providers.read().map_err(|_| KernelError::Handler {
                message: "resource registry lock poisoned".into(),
            })?;
            let mut handler = None;
            let mut handlers = 0usize;
            let mut reserved = false;
            for provider in providers.iter() {
                match provider.claim(verb, uri) {
                    ClaimDecision::Handle => {
                        handlers += 1;
                        handler = Some(provider.clone());
                    }
                    ClaimDecision::Reserve => reserved = true,
                    ClaimDecision::Pass => {}
                }
            }
            if handlers > 1 || (reserved && handler.is_some()) {
                return Err(KernelError::Conflict {
                    uri: uri.to_string(),
                });
            }
            handler.ok_or_else(|| KernelError::UnsupportedVerb {
                verb: verb.to_string(),
                uri: uri.to_string(),
            })?
        };
        provider
            .invoke_at_generation(verb, uri, generation, input)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct UnknownVerbProvider {
        identity: VerbId,
    }

    impl DynamicClaimProvider for UnknownVerbProvider {
        fn claim(&self, verb: &VerbId, _: &ResourceUri) -> ClaimDecision {
            if verb == &self.identity {
                ClaimDecision::Handle
            } else {
                ClaimDecision::Pass
            }
        }
    }

    impl DynamicResourceProvider for UnknownVerbProvider {
        fn invoke<'a>(
            &'a self,
            verb: &'a VerbId,
            _: &'a ResourceUri,
            input: DynamicValue,
        ) -> ResourceFuture<'a> {
            Box::pin(async move {
                Ok(DynamicVerbResult {
                    verb: verb.clone(),
                    function: "compose".into(),
                    output: input,
                })
            })
        }
    }

    struct ReservedProvider;

    impl DynamicClaimProvider for ReservedProvider {
        fn claim(&self, _: &VerbId, _: &ResourceUri) -> ClaimDecision {
            ClaimDecision::Reserve
        }
    }

    impl DynamicResourceProvider for ReservedProvider {
        fn invoke<'a>(
            &'a self,
            verb: &'a VerbId,
            uri: &'a ResourceUri,
            _: DynamicValue,
        ) -> ResourceFuture<'a> {
            Box::pin(async move {
                Err(KernelError::UnsupportedVerb {
                    verb: verb.to_string(),
                    uri: uri.to_string(),
                })
            })
        }
    }

    #[tokio::test]
    async fn invokes_a_resource_for_a_verb_unknown_to_artist() {
        let identity = VerbId::new("example:composition/transform@1.0.0").unwrap();
        let registry = ResourceRegistry::default();
        registry
            .register(Arc::new(UnknownVerbProvider {
                identity: identity.clone(),
            }))
            .unwrap();
        let uri = ResourceUri::parse("memory://composition").unwrap();
        let result = registry
            .invoke(&identity, &uri, DynamicValue::String("hello".into()))
            .await
            .unwrap();
        assert_eq!(result.output, DynamicValue::String("hello".into()));
    }

    #[tokio::test]
    async fn multiple_dynamic_handlers_conflict_instead_of_last_wins() {
        let identity = VerbId::new("example:composition/transform@1.0.0").unwrap();
        let registry = ResourceRegistry::default();
        registry
            .register(Arc::new(UnknownVerbProvider {
                identity: identity.clone(),
            }))
            .unwrap();
        registry
            .register(Arc::new(UnknownVerbProvider {
                identity: identity.clone(),
            }))
            .unwrap();
        let uri = ResourceUri::parse("memory://composition").unwrap();
        assert!(matches!(
            registry
                .invoke(&identity, &uri, DynamicValue::String("hello".into()))
                .await,
            Err(KernelError::Conflict { .. })
        ));
    }

    #[tokio::test]
    async fn dynamic_reserve_blocks_invocation_without_a_handler() {
        let identity = VerbId::new("example:composition/transform@1.0.0").unwrap();
        let registry = ResourceRegistry::default();
        registry.register(Arc::new(ReservedProvider)).unwrap();
        let uri = ResourceUri::parse("memory://composition").unwrap();
        assert!(matches!(
            registry
                .invoke(&identity, &uri, DynamicValue::String("hello".into()))
                .await,
            Err(KernelError::UnsupportedVerb { .. })
        ));
    }
}
