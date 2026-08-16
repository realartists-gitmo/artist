use crate::{
    ClaimDecision, ClaimRegistry, InvocationContext, InvocationResourceProvider, InvocationScope,
    InvocationStore, KernelError, KernelHandle, ProcessManager, ResourceCatalogEntry,
    ResourceCatalogProvider, ResourceRegistry, RouteRegistry, ToolDefinition, ToolProvider,
    VerbDefinition, VerbRegistry,
};
use std::any::Any;
use std::collections::BTreeMap;
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
    invocations: InvocationStore,
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
        let invocations = InvocationStore::default();
        let resources = ResourceRegistry::default();
        let claims = ClaimRegistry::default();
        let invocation_provider = Arc::new(InvocationResourceProvider::new(invocations.clone()));
        let resource_catalog_providers: RwLock<Vec<Arc<dyn ResourceCatalogProvider>>> =
            RwLock::new(vec![Arc::new(invocations.clone())]);
        let kernel = Self {
            inner: Arc::new(Inner {
                tool_providers: RwLock::new(Vec::new()),
                resource_catalog_providers,
                verbs: VerbRegistry::new(),
                processes: ProcessManager::new(),
                routes: RouteRegistry::default(),
                claims,
                resources,
                invocations,
                background: Mutex::new(Vec::new()),
            }),
        };
        kernel
            .register_dynamic_resource_provider(invocation_provider)
            .expect("invocation provider publication");
        kernel
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

    pub fn invocation_store(&self) -> InvocationStore {
        self.inner.invocations.clone()
    }

    pub fn register_dynamic_resource_provider(
        &self,
        provider: Arc<dyn crate::DynamicResourceProvider>,
    ) -> Result<(), KernelError> {
        let mut definitions = provider.verb_definitions();
        let existing = self.active_verbs()?;
        for definition in &mut definitions {
            if definition.input_type.is_none() {
                definition.input_type = canonical_universal_input_type(&definition.function);
            }
            if definition.output_type.is_none() {
                definition.output_type = canonical_universal_output_type(&definition.function);
            }
            if let Some(active) = existing
                .iter()
                .find(|active| active.definition.identity == definition.identity)
            {
                if definition.input_type.is_none() {
                    definition.input_type = active.definition.input_type.clone();
                }
                if definition.output_type.is_none() {
                    definition.output_type = active.definition.output_type.clone();
                }
            }
        }
        if !definitions.is_empty() {
            self.inner.verbs.activate_packages(definitions)?;
        }
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

    pub fn reconcile_owned_verbs(
        &self,
        definitions: Vec<VerbDefinition>,
        owned: std::collections::BTreeSet<crate::VerbId>,
    ) -> Result<Vec<crate::VerbId>, KernelError> {
        self.inner
            .verbs
            .reconcile_owned_packages(definitions, owned)
    }

    pub fn active_verbs(&self) -> Result<Vec<Arc<crate::ActiveVerb>>, KernelError> {
        self.inner.verbs.definitions()
    }

    /// Resolve an open universal function against the active verb catalog and
    /// claim registry. Resource schemes are deliberately opaque here: the
    /// provider that claims `(function, uri)` determines the implementation.
    pub fn resolve_resource_verb(
        &self,
        function: &str,
        uri: &crate::ResourceUri,
    ) -> Result<crate::VerbId, KernelError> {
        let candidates = self
            .active_verbs()?
            .into_iter()
            .filter(|active| active.definition.function == function)
            .filter(|active| {
                matches!(
                    self.inner
                        .claims
                        .arbitrate(&active.definition.identity, uri),
                    Ok(crate::ClaimDecision::Handle)
                )
            })
            .map(|active| active.definition.identity.clone())
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [verb] => Ok(verb.clone()),
            [] => Err(crate::KernelError::UnsupportedVerb {
                verb: function.to_owned(),
                uri: uri.to_string(),
            }),
            _ => Err(crate::KernelError::Conflict {
                uri: uri.to_string(),
            }),
        }
    }

    pub fn new_mutation_transaction(&self, expected: usize) -> Arc<crate::MutationTransaction> {
        crate::MutationTransaction::new(expected)
    }

    async fn execute_transaction_request(
        &self,
        transaction: Arc<crate::MutationTransaction>,
        verb: crate::VerbId,
        uri: crate::ResourceUri,
        input: crate::DynamicValue,
        scope: InvocationScope,
    ) -> Result<crate::DynamicResourceResult, KernelError> {
        let participant = scope
            .context
            .correlation_id
            .clone()
            .unwrap_or_else(|| uri.to_string());
        let (slot, execute) = transaction.register((verb, uri.clone(), input), participant)?;
        if let Some(requests) = execute {
            let mixed = requests
                .into_iter()
                .map(|(verb, uri, input)| crate::MixedResourceRequest { verb, uri, input })
                .collect();
            transaction.finish(
                self.invoke_mixed_dynamic_resources(mixed, scope.without_mutation_transaction())
                    .await,
            );
        }
        let result = tokio::select! {
            result = transaction.result(slot) => result?,
            _ = scope.cancellation.cancelled() => return Err(KernelError::Aborted { message: "mutation transaction cancelled".to_owned() }),
        };
        Ok(crate::DynamicResourceResult { uri, result })
    }

    pub async fn execute_universal_function_with_scope(
        &self,
        function: String,
        uri: crate::ResourceUri,
        input: crate::DynamicValue,
        scope: crate::InvocationScope,
    ) -> Result<Vec<crate::DynamicResourceResult>, KernelError> {
        let verb = self.resolve_resource_verb(&function, &uri)?;
        if let Some(transaction) = scope.mutation_transaction()
            && matches!(function.as_str(), "write" | "edit" | "insert")
        {
            let result = self
                .execute_transaction_request(transaction, verb, uri, input, scope)
                .await
                .map(|result| vec![result]);
            return result;
        }
        let result = self
            .inner
            .resources
            .invoke_with_host(&verb, &uri, input, self.handle(), scope.clone())
            .await?;
        let values = vec![crate::DynamicResourceResult { uri, result }];
        Ok(values)
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
        if uri.scheme() == "invocations" {
            return self.inner.resources.invoke(&verb, &uri, input).await;
        }
        self.inner
            .resources
            .invoke_with_host(
                &verb,
                &uri,
                input,
                self.handle(),
                InvocationScope::new(InvocationContext::default()),
            )
            .await
    }

    pub async fn invoke_mixed_dynamic_resources(
        &self,
        requests: Vec<crate::MixedResourceRequest>,
        scope: InvocationScope,
    ) -> Vec<Result<crate::DynamicVerbResult, KernelError>> {
        self.inner
            .resources
            .invoke_mixed_batch(requests, scope)
            .await
    }

    pub async fn invoke_dynamic_resource_batch(
        &self,
        verb: crate::VerbId,
        requests: Vec<crate::ResourceRequest>,
        scope: InvocationScope,
    ) -> Vec<Result<crate::DynamicVerbResult, KernelError>> {
        self.inner
            .resources
            .invoke_batch_with_host(&verb, requests, self.handle(), scope)
            .await
    }

    pub async fn execute_dynamic_resources(
        &self,
        call: crate::DynamicVerbCall,
    ) -> Result<Vec<crate::DynamicResourceResult>, KernelError> {
        self.execute_dynamic_resources_with_scope(
            call,
            InvocationScope::new(InvocationContext::default()),
        )
        .await
    }

    pub async fn execute_dynamic_resources_with_scope(
        &self,
        call: crate::DynamicVerbCall,
        scope: InvocationScope,
    ) -> Result<Vec<crate::DynamicResourceResult>, KernelError> {
        if scope.cancellation.is_cancelled() {
            return Err(KernelError::Aborted {
                message: "resource dispatch cancelled".to_owned(),
            });
        }
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
            if scope.cancellation.is_cancelled() {
                return Err(KernelError::Aborted {
                    message: "resource dispatch cancelled".to_owned(),
                });
            }
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
        let invocation = self.inner.invocations.begin(args.clone());
        let mut context = context;
        context.correlation_id = Some(invocation.uri.to_string());
        let scope = InvocationScope::with_cancellation(
            context.clone(),
            tokio_util::sync::CancellationToken::new(),
        )
        .with_invocation_uri(invocation.uri.clone());
        let scope = self
            .inner
            .invocations
            .subscribe_stdin(&invocation.uri)
            .map(|receiver| scope.clone().with_stdin_receiver(receiver))
            .unwrap_or(scope);
        let providers = self.inner.tool_providers.read().await;
        let host = self.handle();
        for provider in providers.iter() {
            if provider.can_execute_tool(name) {
                let result = provider
                    .execute_tool_for_model_result(name, args, host, scope)
                    .await;
                return match result {
                    Ok(result) => {
                        let stdout = result.stdout.clone();
                        let _ = self.inner.invocations.complete(
                            &invocation.uri,
                            stdout.clone(),
                            result.stdobs,
                            result.stderr,
                        );
                        stdout
                    }
                    Err(error) => {
                        let _ = self.inner.invocations.complete(
                            &invocation.uri,
                            Err(error.clone()),
                            "",
                            error.to_string(),
                        );
                        Err(error)
                    }
                };
            }
        }
        let result = Err(KernelError::Handler {
            message: format!("no named tool registered: {name}"),
        });
        let _ = self.inner.invocations.complete(
            &invocation.uri,
            result.clone(),
            "",
            result
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default(),
        );
        result
    }

    pub async fn execute_tool_with_scope(
        &self,
        name: &str,
        args: crate::DynamicValue,
        scope: InvocationScope,
    ) -> Result<crate::DynamicValue, KernelError> {
        let invocation = self.inner.invocations.begin(args.clone());
        let scope = scope.with_invocation_uri(invocation.uri.clone());
        let scope = self
            .inner
            .invocations
            .subscribe_stdin(&invocation.uri)
            .map(|receiver| scope.clone().with_stdin_receiver(receiver))
            .unwrap_or(scope);
        let providers = self.inner.tool_providers.read().await;
        let host = self.handle();
        for provider in providers.iter() {
            if provider.can_execute_tool(name) {
                let result = provider
                    .execute_tool_for_model_result(name, args, host, scope)
                    .await;
                return match result {
                    Ok(result) => {
                        let stdout = result.stdout.clone();
                        let _ = self.inner.invocations.complete(
                            &invocation.uri,
                            stdout.clone(),
                            result.stdobs,
                            result.stderr,
                        );
                        stdout
                    }
                    Err(error) => {
                        let _ = self.inner.invocations.complete(
                            &invocation.uri,
                            Err(error.clone()),
                            "",
                            error.to_string(),
                        );
                        Err(error)
                    }
                };
            }
        }
        let result = Err(KernelError::Handler {
            message: format!("no named tool registered: {name}"),
        });
        let _ = self.inner.invocations.complete(
            &invocation.uri,
            result.clone(),
            "",
            result
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default(),
        );
        result
    }

    pub async fn execute_tool_for_model(
        &self,
        name: &str,
        args: crate::DynamicValue,
        scope: InvocationScope,
    ) -> Result<crate::DynamicValue, KernelError> {
        let invocation = self.inner.invocations.begin(args.clone());
        let scope = scope.with_invocation_uri(invocation.uri.clone());
        let scope = self
            .inner
            .invocations
            .subscribe_stdin(&invocation.uri)
            .map(|receiver| scope.clone().with_stdin_receiver(receiver))
            .unwrap_or(scope);
        let providers = self.inner.tool_providers.read().await;
        let host = self.handle();
        for provider in providers.iter() {
            if provider.can_execute_tool(name) {
                let model_result = provider
                    .execute_tool_for_model_result(name, args, host, scope)
                    .await;
                let (stdout, observation) = match model_result {
                    Ok(value) => (value.stdout, value.stdobs),
                    Err(error) => (Err(error), String::new()),
                };
                let result = stdout.clone();
                let stderr = stdout
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                let _ =
                    self.inner
                        .invocations
                        .complete(&invocation.uri, stdout, observation, stderr);
                return result;
            }
        }
        let result = Err(KernelError::Handler {
            message: format!("no named tool registered: {name}"),
        });
        let _ = self.inner.invocations.complete(
            &invocation.uri,
            result.clone(),
            "",
            result
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default(),
        );
        result
    }

    pub async fn execute_tools_for_model(
        &self,
        name: &str,
        args: Vec<crate::DynamicValue>,
        scope: InvocationScope,
    ) -> Vec<Result<crate::DynamicValue, KernelError>> {
        let invocations = args
            .iter()
            .cloned()
            .map(|arg| self.inner.invocations.begin(arg))
            .collect::<Vec<_>>();
        let providers = self.inner.tool_providers.read().await;
        let host = self.handle();
        for provider in providers.iter() {
            if provider.can_execute_tool(name) {
                let model_results = provider
                    .execute_tools_for_model_results(name, args, host, scope)
                    .await;
                let mut results = Vec::with_capacity(invocations.len());
                for (index, invocation) in invocations.iter().enumerate() {
                    let model_result = model_results.get(index).cloned().unwrap_or_else(|| {
                        Err(KernelError::Handler {
                            message: format!(
                                "tool provider returned {} results for {} requests",
                                model_results.len(),
                                invocations.len()
                            ),
                        })
                    });
                    match model_result {
                        Ok(value) => {
                            let observation = value.stdobs.clone();
                            let stdout = value.stdout;
                            let result = stdout.clone();
                            let stderr = value.stderr.clone();
                            let stderr = if stderr.is_empty() {
                                stdout
                                    .as_ref()
                                    .err()
                                    .map(ToString::to_string)
                                    .unwrap_or_default()
                            } else {
                                stderr
                            };
                            let _ = self.inner.invocations.complete(
                                &invocation.uri,
                                stdout,
                                observation,
                                stderr,
                            );
                            results.push(result);
                        }
                        Err(error) => {
                            let result = Err(error.clone());
                            let _ = self.inner.invocations.complete(
                                &invocation.uri,
                                result.clone(),
                                "",
                                error.to_string(),
                            );
                            results.push(result);
                        }
                    }
                }
                return results;
            }
        }
        let result = vec![Err(KernelError::Handler {
            message: format!("no named tool registered: {name}"),
        })];
        for (invocation, value) in invocations.iter().zip(result.iter()) {
            let _ = self
                .inner
                .invocations
                .complete(&invocation.uri, value.clone(), "", "");
        }
        result
    }

    pub async fn execute_tool_models_for_model(
        &self,
        name: &str,
        args: Vec<crate::DynamicValue>,
        scope: InvocationScope,
    ) -> Vec<Result<crate::ToolModelResult, KernelError>> {
        let invocations = args
            .iter()
            .cloned()
            .map(|arg| self.inner.invocations.begin(arg))
            .collect::<Vec<_>>();
        let providers = self.inner.tool_providers.read().await;
        let host = self.handle();
        let scopes = invocations
            .iter()
            .map(|invocation| {
                let scoped = scope.clone().with_invocation_uri(invocation.uri.clone());
                self.inner
                    .invocations
                    .subscribe_stdin(&invocation.uri)
                    .map(|receiver| scoped.clone().with_stdin_receiver(receiver))
                    .unwrap_or(scoped)
            })
            .collect::<Vec<_>>();
        for provider in providers.iter() {
            if provider
                .tool_definitions()
                .iter()
                .any(|definition| definition.name == name)
            {
                let values = provider
                    .execute_tools_for_model_results_with_scopes(name, args, host, scopes)
                    .await;
                for (invocation, value) in invocations.iter().zip(values.iter()) {
                    match value {
                        Ok(value) => {
                            let stdout = value.stdout.clone();
                            let _ = self.inner.invocations.complete(
                                &invocation.uri,
                                stdout,
                                value.stdobs.clone(),
                                value.stderr.clone(),
                            );
                        }
                        Err(error) => {
                            let _ = self.inner.invocations.complete(
                                &invocation.uri,
                                Err(error.clone()),
                                "",
                                error.to_string(),
                            );
                        }
                    }
                }
                return values;
            }
        }
        let error = KernelError::Handler {
            message: format!("no named tool registered: {name}"),
        };
        invocations
            .into_iter()
            .map(|invocation| {
                let _ = self.inner.invocations.complete(
                    &invocation.uri,
                    Err(error.clone()),
                    "",
                    error.to_string(),
                );
                Err(error.clone())
            })
            .collect()
    }

    pub async fn execute_tool_batch_with_scope(
        &self,
        name: &str,
        args: Vec<crate::DynamicValue>,
        scope: InvocationScope,
    ) -> Vec<Result<crate::DynamicVerbResult, KernelError>> {
        let invocations = args
            .iter()
            .map(|arg| self.inner.invocations.begin(arg.clone()))
            .collect::<Vec<_>>();
        let scopes = invocations
            .iter()
            .map(|invocation| {
                let scoped = scope.clone().with_invocation_uri(invocation.uri.clone());
                self.inner
                    .invocations
                    .subscribe_stdin(&invocation.uri)
                    .map(|receiver| scoped.clone().with_stdin_receiver(receiver))
                    .unwrap_or(scoped)
            })
            .collect::<Vec<_>>();
        let providers = self.inner.tool_providers.read().await;
        let host = self.handle();
        for provider in providers.iter() {
            if provider
                .tool_definitions()
                .iter()
                .any(|definition| definition.name == name)
            {
                let values = provider
                    .execute_tools_for_model_results_with_scopes(name, args, host, scopes)
                    .await;
                let results = values
                    .iter()
                    .map(|value| match value {
                        Ok(value) => match &value.stdout {
                            Ok(output) => Ok(crate::DynamicVerbResult {
                                verb: value.verb.clone(),
                                function: name.to_owned(),
                                output: output.clone(),
                            }),
                            Err(error) => Err(error.clone()),
                        },
                        Err(error) => Err(error.clone()),
                    })
                    .collect::<Vec<_>>();
                for (invocation, value) in invocations.iter().zip(values.iter()) {
                    let (stdout, stdobs, stderr) = match value {
                        Ok(value) => (
                            value.stdout.clone(),
                            value.stdobs.clone(),
                            value.stderr.clone(),
                        ),
                        Err(error) => (Err(error.clone()), String::new(), error.to_string()),
                    };
                    let _ =
                        self.inner
                            .invocations
                            .complete(&invocation.uri, stdout, stdobs, stderr);
                }
                return results;
            }
        }
        let results: Vec<Result<crate::DynamicVerbResult, KernelError>> =
            vec![Err(KernelError::Handler {
                message: format!("no named tool registered: {name}"),
            })];
        for (invocation, result) in invocations.iter().zip(results.iter()) {
            let _ = self.inner.invocations.complete(
                &invocation.uri,
                result.clone().map(|value| value.output.clone()),
                "",
                "",
            );
        }
        results
    }

    pub fn handle(&self) -> KernelHandle {
        let dynamic_kernel = self.clone();
        let direct_dynamic_kernel = self.clone();
        let direct_dynamic_batch_kernel = direct_dynamic_kernel.clone();
        let universal_kernel = self.clone();
        let type_kernel = self.clone();
        let universal_batch_kernel = self.clone();
        KernelHandle::with_dispatch(
            Arc::new(move |call, scope| {
                let kernel = dynamic_kernel.clone();
                Box::pin(async move {
                    kernel
                        .execute_dynamic_resources_with_scope(call, scope.child())
                        .await
                })
            }),
            Arc::new(move |verb, uri, input, scope| {
                let kernel = direct_dynamic_kernel.clone();
                Box::pin(async move {
                    if scope.cancellation.is_cancelled() {
                        return Err(KernelError::Aborted {
                            message: "resource dispatch cancelled".to_owned(),
                        });
                    }
                    kernel.invoke_dynamic_resource(verb, uri, input).await
                })
            }),
            Arc::new(move |verb, requests, scope| {
                let kernel = direct_dynamic_batch_kernel.clone();
                Box::pin(async move {
                    if scope.cancellation.is_cancelled() {
                        return requests
                            .into_iter()
                            .map(|_| {
                                Err(KernelError::Aborted {
                                    message: "resource dispatch cancelled".to_owned(),
                                })
                            })
                            .collect();
                    }
                    kernel
                        .invoke_dynamic_resource_batch(verb, requests, scope)
                        .await
                })
            }),
        )
        .with_input_type_dispatch(Arc::new(move |function, uri| {
            let verb = type_kernel.resolve_resource_verb(&function, &uri)?;
            let active = type_kernel.active_verbs()?;
            Ok(active
                .iter()
                .find(|active| active.definition.identity == verb)
                .and_then(|active| active.definition.input_type.clone()))
        }))
        .with_universal_batch_dispatch(Arc::new(move |function, requests, scope| {
            let kernel = universal_batch_kernel.clone();
            Box::pin(async move {
                let mut groups = BTreeMap::<
                    crate::VerbId,
                    Vec<(usize, crate::ResourceUri, crate::DynamicValue)>,
                >::new();
                let mut results = requests
                    .iter()
                    .map(|(uri, _)| {
                        Err(crate::KernelError::UnsupportedVerb {
                            verb: function.clone(),
                            uri: uri.to_string(),
                        })
                    })
                    .collect::<Vec<_>>();
                for (index, (uri, input)) in requests.iter().enumerate() {
                    match kernel.resolve_resource_verb(&function, uri) {
                        Ok(verb) => groups.entry(verb).or_default().push((
                            index,
                            uri.clone(),
                            input.clone(),
                        )),
                        Err(error) => results[index] = Err(error),
                    }
                }
                let group_results =
                    futures::future::join_all(groups.into_iter().map(|(verb, items)| {
                        let kernel = kernel.clone();
                        let scope = scope.child();
                        async move {
                            let requests = items
                                .iter()
                                .map(|(index, uri, input)| crate::ResourceRequest {
                                    uri: uri.clone(),
                                    input: input.clone(),
                                    scope: Some(scope.batch_scope(*index)),
                                })
                                .collect::<Vec<_>>();
                            let values = kernel
                                .invoke_dynamic_resource_batch(verb, requests, scope)
                                .await;
                            (items, values)
                        }
                    }))
                    .await;
                for (items, values) in group_results {
                    for ((index, uri, _), value) in items.into_iter().zip(values) {
                        results[index] =
                            value.map(|result| crate::DynamicResourceResult { uri, result });
                    }
                }
                results
            })
        }))
        .with_universal_dispatch(Arc::new(move |function, input, scope| {
            let kernel = universal_kernel.clone();
            Box::pin(async move {
                let uri = match &input {
                    crate::DynamicValue::Record(fields) => fields
                        .get("uri")
                        .or_else(|| fields.get("root"))
                        .and_then(|value| match value {
                            crate::DynamicValue::ResourceUri(uri) => Some(uri.clone()),
                            crate::DynamicValue::String(uri) => crate::ResourceUri::parse(uri).ok(),
                            _ => None,
                        })
                        .ok_or_else(|| crate::KernelError::InvalidRequest {
                            message: "universal adapter input has no URI".to_owned(),
                        })?,
                    _ => {
                        return Err(crate::KernelError::InvalidRequest {
                            message: "universal adapter input is not a record".to_owned(),
                        });
                    }
                };
                kernel
                    .execute_universal_function_with_scope(function, uri, input, scope)
                    .await
            })
        }))
    }
}

fn canonical_universal_input_type(function: &str) -> Option<crate::DynamicType> {
    use crate::DynamicType;
    let uri = DynamicType::ResourceUri;
    let anchor = DynamicType::String;
    let position = DynamicType::Variant(BTreeMap::from([
        ("top".to_owned(), None),
        ("bottom".to_owned(), None),
        ("at".to_owned(), Some(anchor.clone())),
    ]));
    let optional = |ty| DynamicType::Option(Box::new(ty));
    let record =
        |fields: Vec<(String, DynamicType)>| DynamicType::Record(fields.into_iter().collect());
    Some(match function {
        "read" => record(vec![
            ("uri".to_owned(), uri.clone()),
            ("at".to_owned(), optional(position.clone())),
            ("before".to_owned(), optional(DynamicType::U32)),
            ("after".to_owned(), optional(DynamicType::U32)),
        ]),
        "write" => record(vec![
            ("uri".to_owned(), uri.clone()),
            ("content".to_owned(), DynamicType::String),
        ]),
        "edit" => record(vec![
            ("uri".to_owned(), uri.clone()),
            ("start".to_owned(), anchor.clone()),
            ("end".to_owned(), optional(anchor.clone())),
            ("content".to_owned(), DynamicType::String),
        ]),
        "insert" => record(vec![
            ("uri".to_owned(), uri.clone()),
            ("at".to_owned(), position),
            ("content".to_owned(), DynamicType::String),
        ]),
        "find" => record(vec![
            ("root".to_owned(), uri),
            ("query".to_owned(), DynamicType::String),
        ]),
        "grep" => record(vec![
            ("uri".to_owned(), uri),
            ("pattern".to_owned(), DynamicType::String),
        ]),
        "poll" => record(vec![
            ("uri".to_owned(), uri),
            ("from".to_owned(), optional(position)),
            ("match".to_owned(), optional(DynamicType::String)),
            ("timeout-ms".to_owned(), optional(DynamicType::U64)),
        ]),
        "run" => record(vec![
            ("uri".to_owned(), uri),
            (
                "args".to_owned(),
                DynamicType::List(Box::new(DynamicType::String)),
            ),
        ]),
        "abort" | "delete" => record(vec![("uri".to_owned(), uri)]),
        _ => return None,
    })
}

fn canonical_universal_output_type(function: &str) -> Option<crate::DynamicType> {
    use crate::DynamicType;
    let uri = DynamicType::ResourceUri;
    let line = DynamicType::Record(BTreeMap::from([
        ("anchor".to_owned(), DynamicType::String),
        ("text".to_owned(), DynamicType::String),
        (
            "ending".to_owned(),
            DynamicType::Enum(vec!["lf".into(), "crlf".into(), "cr".into(), "none".into()]),
        ),
    ]));
    let text = DynamicType::Record(BTreeMap::from([
        ("uri".to_owned(), uri.clone()),
        (
            "lines".to_owned(),
            DynamicType::List(Box::new(line.clone())),
        ),
    ]));
    let hunk = DynamicType::Record(BTreeMap::from([
        ("old".to_owned(), DynamicType::List(Box::new(line.clone()))),
        ("new".to_owned(), DynamicType::List(Box::new(line.clone()))),
    ]));
    let diff = DynamicType::Record(BTreeMap::from([
        ("uri".to_owned(), uri.clone()),
        ("hunks".to_owned(), DynamicType::List(Box::new(hunk))),
    ]));
    let record =
        |fields: Vec<(String, DynamicType)>| DynamicType::Record(fields.into_iter().collect());
    Some(match function {
        "read" => DynamicType::Variant(BTreeMap::from([
            (
                "lines".into(),
                Some(record(vec![
                    ("uri".into(), uri.clone()),
                    ("lines".into(), DynamicType::List(Box::new(line.clone()))),
                ])),
            ),
            (
                "entries".into(),
                Some(record(vec![
                    ("uri".into(), uri.clone()),
                    ("entries".into(), DynamicType::List(Box::new(uri.clone()))),
                ])),
            ),
        ])),
        "write" => record(vec![
            ("uri".into(), uri),
            ("text".into(), DynamicType::Option(Box::new(text))),
        ]),
        "edit" | "insert" => record(vec![
            ("uri".into(), uri),
            ("changed".into(), DynamicType::List(Box::new(text))),
            ("diff".into(), diff),
        ]),
        "find" => record(vec![("uris".into(), DynamicType::List(Box::new(uri)))]),
        "grep" => record(vec![("matches".into(), DynamicType::List(Box::new(text)))]),
        "poll" => record(vec![
            ("uri".into(), uri),
            ("text".into(), text),
            (
                "reason".into(),
                DynamicType::Enum(vec![
                    "changed".into(),
                    "matched".into(),
                    "terminated".into(),
                    "timeout".into(),
                ]),
            ),
        ]),
        "run" | "abort" | "delete" => record(vec![("uri".into(), uri)]),
        _ => return None,
    })
}
