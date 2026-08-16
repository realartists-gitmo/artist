//! Open-ended typed resource-provider registry.
use crate::{
    ClaimDecision, DynamicClaimProvider, DynamicValue, DynamicVerbResult, KernelError, ResourceUri,
    VerbDefinition, VerbId,
};
use futures::future::join_all;
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

pub type ResourceFuture<'a> =
    Pin<Box<dyn Future<Output = Result<DynamicVerbResult, KernelError>> + Send + 'a>>;
pub type ResourceBatchFuture<'a> =
    Pin<Box<dyn Future<Output = Vec<Result<DynamicVerbResult, KernelError>>> + Send + 'a>>;

#[derive(Clone)]
pub struct ResourceRequest {
    pub uri: ResourceUri,
    pub input: DynamicValue,
    pub scope: Option<crate::InvocationScope>,
}

#[derive(Clone, Debug)]
pub struct MixedResourceRequest {
    pub verb: VerbId,
    pub uri: ResourceUri,
    pub input: DynamicValue,
}

pub trait DynamicResourceProvider: DynamicClaimProvider + Send + Sync {
    /// Optional model-facing documentation owned by this provider.
    fn resource_catalog(&self) -> Vec<crate::ResourceCatalogEntry> {
        Vec::new()
    }
    /// Concrete verb identities published together with this provider. The
    /// kernel uses these definitions when resolving open universal calls;
    /// claims and executable identities therefore cannot drift apart.
    fn verb_definitions(&self) -> Vec<VerbDefinition> {
        Vec::new()
    }

    fn invoke<'a>(
        &'a self,
        verb: &'a VerbId,
        uri: &'a ResourceUri,
        input: DynamicValue,
    ) -> ResourceFuture<'a>;

    fn invoke_with_host<'a>(
        &'a self,
        verb: &'a VerbId,
        uri: &'a ResourceUri,
        input: DynamicValue,
        _host: crate::KernelHandle,
        _scope: crate::InvocationScope,
    ) -> ResourceFuture<'a> {
        self.invoke(verb, uri, input)
    }

    fn invoke_batch_with_host<'a>(
        &'a self,
        verb: &'a VerbId,
        requests: Vec<ResourceRequest>,
        host: crate::KernelHandle,
        scope: crate::InvocationScope,
    ) -> ResourceBatchFuture<'a> {
        Box::pin(async move {
            join_all(requests.into_iter().map(|request| {
                let request_scope = request.scope.unwrap_or_else(|| scope.child());
                let uri = request.uri;
                let input = request.input;
                let host = host.clone();
                async move {
                    if request_scope.cancellation.is_cancelled() {
                        Err(KernelError::Aborted {
                            message: "resource batch cancelled".to_owned(),
                        })
                    } else {
                        self.invoke_with_host(verb, &uri, input, host, request_scope)
                            .await
                    }
                }
            }))
            .await
        })
    }

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

    fn invoke_batch<'a>(
        &'a self,
        verb: &'a VerbId,
        requests: Vec<ResourceRequest>,
        scope: crate::InvocationScope,
    ) -> ResourceBatchFuture<'a> {
        Box::pin(async move {
            join_all(requests.into_iter().map(|request| {
                if scope.cancellation.is_cancelled() {
                    futures::future::Either::Left(async {
                        Err(KernelError::Aborted {
                            message: "resource batch cancelled".to_owned(),
                        })
                    })
                } else {
                    let uri = request.uri;
                    let input = request.input;
                    futures::future::Either::Right(
                        async move { self.invoke(verb, &uri, input).await },
                    )
                }
            }))
            .await
        })
    }

    fn invoke_mixed_batch<'a>(
        &'a self,
        requests: Vec<MixedResourceRequest>,
        scope: crate::InvocationScope,
    ) -> ResourceBatchFuture<'a> {
        Box::pin(async move {
            let mut results = Vec::with_capacity(requests.len());
            for request in requests {
                if scope.cancellation.is_cancelled() {
                    results.push(Err(KernelError::Aborted {
                        message: "resource batch cancelled".to_owned(),
                    }));
                } else {
                    results.push(
                        self.invoke(&request.verb, &request.uri, request.input)
                            .await,
                    );
                }
            }
            results
        })
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
        let providers = self
            .providers
            .read()
            .map_err(|_| KernelError::Handler {
                message: "resource registry lock poisoned".into(),
            })?
            .clone();
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
        let mut result = provider.invoke(verb, uri, input).await?;
        // `invoke` is the historical direct/native API. Keep its compact
        // compatibility shape while the host-facing path below carries the
        // canonical universal typed shape across WASM.
        if let DynamicValue::Variant(_, Some(value)) = result.output {
            result.output = *value;
        } else if matches!(verb.function(), "delete" | "abort") {
            if let DynamicValue::Record(fields) = &result.output {
                if let Some(DynamicValue::ResourceUri(uri)) = fields.get("uri") {
                    result.output = DynamicValue::ResourceUri(uri.clone());
                }
            }
        }
        Ok(result)
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

    pub async fn invoke_with_host(
        &self,
        verb: &VerbId,
        uri: &ResourceUri,
        input: DynamicValue,
        host: crate::KernelHandle,
        scope: crate::InvocationScope,
    ) -> Result<DynamicVerbResult, KernelError> {
        let providers = self
            .providers
            .read()
            .map_err(|_| KernelError::Handler {
                message: "resource registry lock poisoned".into(),
            })?
            .clone();
        let mut selected = None;
        let mut count = 0;
        for (index, provider) in providers.iter().enumerate() {
            if matches!(provider.claim(verb, uri), ClaimDecision::Handle) {
                selected = Some(index);
                count += 1;
            }
        }
        match (count, selected) {
            (1, Some(index)) => {
                providers[index]
                    .invoke_with_host(verb, uri, input, host, scope)
                    .await
            }
            (count, _) if count > 1 => {
                for provider in providers.iter() {
                    if !matches!(provider.claim(verb, uri), ClaimDecision::Handle) {
                        continue;
                    }
                    match provider
                        .invoke_with_host(verb, uri, input.clone(), host.clone(), scope.child())
                        .await
                    {
                        Err(KernelError::UnsupportedVerb { .. }) => continue,
                        result => return result,
                    }
                }
                Err(KernelError::Conflict {
                    uri: uri.to_string(),
                })
            }
            _ => Err(KernelError::UnsupportedVerb {
                verb: verb.to_string(),
                uri: uri.to_string(),
            }),
        }
    }

    pub async fn invoke_batch(
        &self,
        verb: &VerbId,
        requests: Vec<ResourceRequest>,
        scope: crate::InvocationScope,
    ) -> Vec<Result<DynamicVerbResult, KernelError>> {
        let mut groups: BTreeMap<usize, (Arc<dyn DynamicResourceProvider>, Vec<ResourceRequest>)> =
            BTreeMap::new();
        let providers = match self.providers.read() {
            Ok(providers) => providers.clone(),
            Err(_) => {
                return requests
                    .into_iter()
                    .map(|_| {
                        Err(KernelError::Handler {
                            message: "resource registry lock poisoned".into(),
                        })
                    })
                    .collect();
            }
        };
        let mut assignments = Vec::with_capacity(requests.len());
        for request in requests {
            let mut selected = None;
            let mut count = 0;
            for (index, provider) in providers.iter().enumerate() {
                if matches!(provider.claim(verb, &request.uri), ClaimDecision::Handle) {
                    selected = Some(index);
                    count += 1;
                }
            }
            if count == 1 {
                let Some(index) = selected else {
                    assignments.push(Err(KernelError::Handler {
                        message: "resource arbitration selected no provider".to_owned(),
                    }));
                    continue;
                };
                assignments.push(Ok(index));
                groups
                    .entry(index)
                    .or_insert_with(|| (Arc::clone(&providers[index]), Vec::new()))
                    .1
                    .push(request);
            } else if count > 1 {
                assignments.push(Err(KernelError::Conflict {
                    uri: request.uri.to_string(),
                }));
            } else {
                assignments.push(Err(KernelError::UnsupportedVerb {
                    verb: verb.to_string(),
                    uri: request.uri.to_string(),
                }));
            }
        }
        let mut grouped = join_all(groups.into_iter().map(|(index, (provider, requests))| {
            let scope = scope.clone();
            async move {
                (
                    index,
                    provider.invoke_batch(verb, requests, scope.child()).await,
                )
            }
        }))
        .await
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let mut offsets = BTreeMap::<usize, usize>::new();
        assignments
            .into_iter()
            .map(|assignment| match assignment {
                Err(error) => Err(error),
                Ok(index) => {
                    let offset = offsets.entry(index).or_insert(0);
                    let result = grouped.get_mut(&index).and_then(|results| {
                        let result = results.get(*offset).cloned();
                        *offset += 1;
                        result
                    });
                    result.unwrap_or_else(|| {
                        Err(KernelError::Handler {
                            message: "resource provider returned the wrong batch length".to_owned(),
                        })
                    })
                }
            })
            .collect()
    }

    pub async fn invoke_batch_with_host(
        &self,
        verb: &VerbId,
        requests: Vec<ResourceRequest>,
        host: crate::KernelHandle,
        scope: crate::InvocationScope,
    ) -> Vec<Result<DynamicVerbResult, KernelError>> {
        let providers = match self.providers.read() {
            Ok(providers) => providers.clone(),
            Err(_) => {
                return requests
                    .into_iter()
                    .map(|_| {
                        Err(KernelError::Handler {
                            message: "resource registry lock poisoned".into(),
                        })
                    })
                    .collect();
            }
        };
        let mut groups: BTreeMap<usize, Vec<ResourceRequest>> = BTreeMap::new();
        enum Assignment {
            Group(usize),
            Immediate(Result<DynamicVerbResult, KernelError>),
        }
        let mut assignments = Vec::with_capacity(requests.len());
        for request in requests {
            if scope.cancellation.is_cancelled() {
                assignments.push(Assignment::Immediate(Err(KernelError::Aborted {
                    message: "resource batch cancelled".into(),
                })));
            } else {
                let mut selected = None;
                let mut count = 0;
                for (index, provider) in providers.iter().enumerate() {
                    if matches!(provider.claim(verb, &request.uri), ClaimDecision::Handle) {
                        selected = Some(index);
                        count += 1;
                    }
                }
                let result = match (count, selected) {
                    (1, Some(index)) => {
                        groups.entry(index).or_default().push(request);
                        Assignment::Group(index)
                    }
                    (count, _) if count > 1 => {
                        let mut fallback = None;
                        for provider in providers.iter() {
                            if !matches!(provider.claim(verb, &request.uri), ClaimDecision::Handle)
                            {
                                continue;
                            }
                            match provider
                                .invoke_with_host(
                                    verb,
                                    &request.uri,
                                    request.input.clone(),
                                    host.clone(),
                                    scope.child(),
                                )
                                .await
                            {
                                Err(KernelError::UnsupportedVerb { .. }) => continue,
                                result => {
                                    fallback = Some(result);
                                    break;
                                }
                            }
                        }
                        Assignment::Immediate(fallback.unwrap_or_else(|| {
                            Err(KernelError::Conflict {
                                uri: request.uri.to_string(),
                            })
                        }))
                    }
                    _ => Assignment::Immediate(Err(KernelError::UnsupportedVerb {
                        verb: verb.to_string(),
                        uri: request.uri.to_string(),
                    })),
                };
                assignments.push(result);
            }
        }
        let mut grouped = join_all(groups.into_iter().map(|(index, requests)| {
            let provider = providers[index].clone();
            let host = host.clone();
            let aligned = requests
                .iter()
                .filter_map(|request| request.scope.clone())
                .collect::<Vec<_>>();
            let scope = requests
                .first()
                .and_then(|request| request.scope.clone())
                .unwrap_or_else(|| scope.child())
                .with_batch_scopes(aligned);
            async move {
                (
                    index,
                    provider
                        .invoke_batch_with_host(verb, requests, host, scope)
                        .await,
                )
            }
        }))
        .await
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let mut offsets = BTreeMap::<usize, usize>::new();
        assignments
            .into_iter()
            .map(|assignment| match assignment {
                Assignment::Immediate(result) => result,
                Assignment::Group(index) => {
                    let offset = offsets.entry(index).or_insert(0);
                    let result = grouped.get_mut(&index).and_then(|results| {
                        let result = results.get(*offset).cloned();
                        *offset += 1;
                        result
                    });
                    result.unwrap_or_else(|| {
                        Err(KernelError::Handler {
                            message: "resource provider returned the wrong batch length".into(),
                        })
                    })
                }
            })
            .collect()
    }

    pub async fn invoke_mixed_batch(
        &self,
        requests: Vec<MixedResourceRequest>,
        scope: crate::InvocationScope,
    ) -> Vec<Result<DynamicVerbResult, KernelError>> {
        let providers = match self.providers.read() {
            Ok(providers) => providers.clone(),
            Err(_) => {
                return requests
                    .into_iter()
                    .map(|_| {
                        Err(KernelError::Handler {
                            message: "resource registry lock poisoned".into(),
                        })
                    })
                    .collect();
            }
        };
        let mut assignments = Vec::with_capacity(requests.len());
        let mut groups: BTreeMap<
            usize,
            (Arc<dyn DynamicResourceProvider>, Vec<MixedResourceRequest>),
        > = BTreeMap::new();
        for request in requests {
            let selected = providers
                .iter()
                .enumerate()
                .filter(|(_, provider)| {
                    matches!(
                        provider.claim(&request.verb, &request.uri),
                        ClaimDecision::Handle
                    )
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if selected.len() == 1 {
                let index = selected[0];
                assignments.push(Ok(index));
                groups
                    .entry(index)
                    .or_insert_with(|| (Arc::clone(&providers[index]), Vec::new()))
                    .1
                    .push(request);
            } else if selected.len() > 1 {
                assignments.push(Err(KernelError::Conflict {
                    uri: request.uri.to_string(),
                }));
            } else {
                assignments.push(Err(KernelError::UnsupportedVerb {
                    verb: request.verb.to_string(),
                    uri: request.uri.to_string(),
                }));
            }
        }
        let mut grouped = join_all(groups.into_iter().map(|(index, (provider, requests))| {
            let scope = scope.clone();
            async move {
                (
                    index,
                    provider.invoke_mixed_batch(requests, scope.child()).await,
                )
            }
        }))
        .await
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let mut offsets = BTreeMap::<usize, usize>::new();
        assignments
            .into_iter()
            .map(|assignment| match assignment {
                Err(error) => Err(error),
                Ok(index) => {
                    let offset = offsets.entry(index).or_insert(0);
                    let result = grouped.get_mut(&index).and_then(|results| {
                        let result = results.get(*offset).cloned();
                        *offset += 1;
                        result
                    });
                    result.unwrap_or_else(|| {
                        Err(KernelError::Handler {
                            message: "resource provider returned the wrong mixed batch length"
                                .into(),
                        })
                    })
                }
            })
            .collect()
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
