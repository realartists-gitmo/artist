//! Open-ended typed resource-provider registry.
use crate::{ClaimDecision, DynamicValue, DynamicVerbResult, KernelError, ResourceUri, VerbId};
use std::{future::Future, pin::Pin, sync::Arc};

pub type ResourceFuture<'a> =
    Pin<Box<dyn Future<Output = Result<DynamicVerbResult, KernelError>> + Send + 'a>>;

pub trait DynamicResourceProvider: Send + Sync {
    fn claim(&self, verb: &VerbId, uri: &ResourceUri) -> ClaimDecision;
    fn invoke<'a>(
        &'a self,
        verb: &'a VerbId,
        uri: &'a ResourceUri,
        input: DynamicValue,
    ) -> ResourceFuture<'a>;
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

    pub async fn invoke(
        &self,
        verb: &VerbId,
        uri: &ResourceUri,
        input: DynamicValue,
    ) -> Result<DynamicVerbResult, KernelError> {
        let providers = self.providers.read().map_err(|_| KernelError::Handler {
            message: "resource registry lock poisoned".into(),
        })?;
        let mut handler = None;
        let mut reserved = false;
        for provider in providers.iter() {
            match provider.claim(verb, uri) {
                ClaimDecision::Handle => handler = Some(provider.clone()),
                ClaimDecision::Reserve => reserved = true,
                ClaimDecision::Pass => {}
            }
        }
        if reserved && handler.is_some() {
            return Err(KernelError::Conflict {
                uri: uri.to_string(),
            });
        }
        let provider = handler.ok_or_else(|| KernelError::UnsupportedVerb {
            verb: verb.to_string(),
            uri: uri.to_string(),
        })?;
        drop(providers);
        provider.invoke(verb, uri, input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct UnknownVerbProvider {
        identity: VerbId,
    }

    impl DynamicResourceProvider for UnknownVerbProvider {
        fn claim(&self, verb: &VerbId, _: &ResourceUri) -> ClaimDecision {
            if verb == &self.identity {
                ClaimDecision::Handle
            } else {
                ClaimDecision::Pass
            }
        }

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

    #[tokio::test]
    async fn invokes_a_resource_for_a_verb_unknown_to_artist() {
        let identity = VerbId::new("example:composition/uppercase@1.0.0").unwrap();
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
}
