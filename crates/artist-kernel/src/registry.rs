use crate::{
    ClaimDecision, ClaimRegistry, InvocationContext, InvocationScope, KernelError, KernelHandle,
    ProcessManager, ResourceCatalogEntry, ResourceCatalogProvider, ResourceRegistry, RouteRegistry,
    ToolDefinition, ToolProvider, VerbDefinition, VerbRegistry,
};
use std::any::Any;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;

struct Inner {
    tool_providers: RwLock<Vec<Arc<dyn ToolProvider>>>,
    resource_catalog_providers: RwLock<Vec<Arc<dyn ResourceCatalogProvider>>>,
    verbs: VerbRegistry,
    processes: ProcessManager,
    routes: RouteRegistry,
    claims: ClaimRegistry,
    resources: ResourceRegistry,
    background: Mutex<Vec<Box<dyn Any + Send>>>,
}

/// The kernel registry for open-ended verb contracts and dynamic resources.
#[derive(Clone)]
pub struct Kernel {
    inner: Arc<Inner>,
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}

impl Kernel {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                tool_providers: RwLock::new(Vec::new()),
                resource_catalog_providers: RwLock::new(Vec::new()),
                verbs: VerbRegistry::new(),
                processes: ProcessManager::new(),
                routes: RouteRegistry::default(),
                claims: ClaimRegistry::default(),
                resources: ResourceRegistry::default(),
                background: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn verb_registry(&self) -> VerbRegistry {
        self.inner.verbs.clone()
    }

    pub fn process_manager(&self) -> ProcessManager {
        self.inner.processes.clone()
    }

    pub fn route_registry(&self) -> RouteRegistry {
        self.inner.routes.clone()
    }

    pub fn claim_registry(&self) -> ClaimRegistry {
        self.inner.claims.clone()
    }

    pub fn resource_registry(&self) -> ResourceRegistry {
        self.inner.resources.clone()
    }

    pub fn register_dynamic_resource_provider(
        &self,
        provider: Arc<dyn crate::DynamicResourceProvider>,
    ) -> Result<(), KernelError> {
        self.inner.resources.register(provider.clone())?;
        self.inner.claims.register(provider)?;
        Ok(())
    }

    pub fn activate_verb(&self, definition: VerbDefinition) -> Result<u64, KernelError> {
        self.inner.verbs.activate(definition)
    }

    pub fn activate_verbs(
        &self,
        definitions: Vec<VerbDefinition>,
    ) -> Result<Vec<u64>, KernelError> {
        self.inner.verbs.activate_packages(definitions)
    }

    pub fn reconcile_verbs(
        &self,
        definitions: Vec<VerbDefinition>,
    ) -> Result<Vec<crate::VerbId>, KernelError> {
        self.inner.verbs.reconcile_packages(definitions)
    }

    pub fn active_verbs(&self) -> Result<Vec<Arc<crate::ActiveVerb>>, KernelError> {
        self.inner.verbs.definitions()
    }

    pub fn active_verb_tools(&self) -> Result<Vec<crate::VerbToolDescriptor>, KernelError> {
        self.inner.verbs.tool_descriptors()
    }

    pub fn execute_dynamic(
        &self,
        call: crate::DynamicVerbCall,
    ) -> Result<crate::DynamicVerbResult, KernelError> {
        self.inner.verbs.execute(&call)
    }

    pub async fn invoke_dynamic_resource(
        &self,
        verb: crate::VerbId,
        uri: crate::ResourceUri,
        input: crate::DynamicValue,
    ) -> Result<crate::DynamicVerbResult, KernelError> {
        self.inner.resources.invoke(&verb, &uri, input).await
    }

    pub async fn execute_dynamic_resources(
        &self,
        call: crate::DynamicVerbCall,
    ) -> Result<Vec<crate::DynamicResourceResult>, KernelError> {
        let lease = self.inner.verbs.acquire(&call.verb)?;
        self.inner.verbs.validate_call(&call)?;
        let claims = self
            .inner
            .routes
            .extract(&lease.definition().identity, &call.input)?
            .into_iter()
            .map(|uri| {
                self.inner
                    .resources
                    .arbitrate_for_lease(&call.verb, &uri, lease.generation())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut results = Vec::with_capacity(claims.len());
        for claimed in claims {
            if claimed.decision != ClaimDecision::Handle {
                return Err(KernelError::UnsupportedVerb {
                    verb: call.verb.to_string(),
                    uri: claimed.uri.to_string(),
                });
            }
            let result = self
                .inner
                .resources
                .invoke_at_generation(
                    &call.verb,
                    &claimed.uri,
                    claimed.generation,
                    call.input.clone(),
                )
                .await?;
            self.inner
                .verbs
                .validate_result_for_lease(&call, &result, &lease)?;
            results.push(crate::DynamicResourceResult {
                uri: claimed.uri,
                result,
            });
        }
        Ok(results)
    }

    pub fn retain_background<T>(&self, service: T)
    where
        T: Send + 'static,
    {
        self.inner
            .background
            .lock()
            .unwrap()
            .push(Box::new(service));
    }

    pub async fn register_tool_provider<P>(&self, provider: P)
    where
        P: ToolProvider + 'static,
    {
        self.inner
            .tool_providers
            .write()
            .await
            .push(Arc::new(provider));
    }

    pub async fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.inner
            .tool_providers
            .read()
            .await
            .iter()
            .flat_map(|provider| provider.tool_definitions())
            .collect()
    }

    pub async fn resource_catalog(&self) -> Vec<ResourceCatalogEntry> {
        self.inner
            .resource_catalog_providers
            .read()
            .await
            .iter()
            .flat_map(|provider| provider.resource_catalog())
            .collect()
    }

    pub async fn execute_tool(
        &self,
        name: &str,
        args: crate::DynamicValue,
    ) -> Result<crate::DynamicValue, KernelError> {
        self.execute_tool_with_context(name, args, InvocationContext::default())
            .await
    }

    pub async fn execute_tool_with_context(
        &self,
        name: &str,
        args: crate::DynamicValue,
        context: InvocationContext,
    ) -> Result<crate::DynamicValue, KernelError> {
        let providers = self.inner.tool_providers.read().await;
        let host = self.handle();
        for provider in providers.iter() {
            if provider
                .tool_definitions()
                .iter()
                .any(|definition| definition.name == name)
            {
                return provider
                    .execute_tool_with_context(name, args, host, context)
                    .await;
            }
        }
        Err(KernelError::Handler {
            message: format!("no named tool registered: {name}"),
        })
    }

    pub async fn execute_tool_with_scope(
        &self,
        name: &str,
        args: crate::DynamicValue,
        scope: InvocationScope,
    ) -> Result<crate::DynamicValue, KernelError> {
        let providers = self.inner.tool_providers.read().await;
        let host = self.handle();
        for provider in providers.iter() {
            if provider
                .tool_definitions()
                .iter()
                .any(|definition| definition.name == name)
            {
                return provider
                    .execute_tool_with_scope(name, args, host, scope)
                    .await;
            }
        }
        Err(KernelError::Handler {
            message: format!("no named tool registered: {name}"),
        })
    }

    pub fn handle(&self) -> KernelHandle {
        let dynamic_kernel = self.clone();
        let direct_dynamic_kernel = self.clone();
        KernelHandle::with_dispatch(
            Arc::new(move |call, scope| {
                let kernel = dynamic_kernel.clone();
                Box::pin(async move {
                    let _scope = scope.child();
                    kernel.execute_dynamic_resources(call).await
                })
            }),
            Arc::new(move |verb, uri, input, _scope| {
                let kernel = direct_dynamic_kernel.clone();
                Box::pin(async move { kernel.inner.resources.invoke(&verb, &uri, input).await })
            }),
        )
    }
}
