use crate::{
    BatchRequest, BatchResult, Handler, HandlerDescriptor, InvocationContext, ItemResult,
    KernelError, KernelHandle, Operation, OperationResult, Request, ResourceUri, ToolDefinition,
    ToolProvider, TypedHandler, Verb,
};
use std::sync::Arc;
use tokio::sync::RwLock;

struct Inner {
    handlers: RwLock<Vec<Arc<dyn Handler>>>,
    typed_handlers: RwLock<Vec<Arc<dyn TypedHandler>>>,
    tool_providers: RwLock<Vec<Arc<dyn ToolProvider>>>,
}

fn operation_uri(operation: &Operation) -> String {
    match operation {
        Operation::Read(items) => items.first().map(|item| item.uri.to_string()),
        Operation::Write(items) => items.first().map(|item| item.uri.to_string()),
        Operation::Edit(items) => items.first().map(|item| item.uri.to_string()),
        Operation::Run(items) => items.first().map(|item| item.uri.to_string()),
        Operation::Send(items) => items.first().map(|item| item.uri.to_string()),
        Operation::Abort(items) | Operation::Delete(items) => {
            items.first().map(ToString::to_string)
        }
        Operation::Find(item) => item.roots.first().map(ToString::to_string),
        Operation::Grep(item) => match &item.source {
            crate::GrepSource::Resources(items) => items.first().map(ToString::to_string),
            crate::GrepSource::Text(items) => items.first().map(|item| item.uri.to_string()),
        },
        Operation::Poll(item) => item.targets.first().map(|item| item.uri.to_string()),
    }
    .unwrap_or_else(|| "<operation>".to_owned())
}

fn poll_timeout(condition: &Option<crate::PollCondition>) -> Option<u64> {
    fn walk(condition: &crate::PollCondition, values: &mut Vec<u64>) {
        match condition {
            crate::PollCondition::Atom(crate::PollAtom::Timeout(milliseconds)) => {
                values.push(*milliseconds)
            }
            crate::PollCondition::Atom(_) => {}
            crate::PollCondition::All(children) | crate::PollCondition::Any(children) => {
                children.iter().for_each(|child| walk(child, values))
            }
        }
    }
    let mut values = Vec::new();
    condition
        .as_ref()
        .into_iter()
        .for_each(|condition| walk(condition, &mut values));
    values.into_iter().min()
}

fn evaluate_poll_condition(
    condition: Option<&crate::PollCondition>,
    results: &[crate::PollResult],
    elapsed: tokio::time::Duration,
) -> (bool, Vec<crate::PollAtom>) {
    fn atom(
        atom: &crate::PollAtom,
        results: &[crate::PollResult],
        elapsed: tokio::time::Duration,
    ) -> (bool, Vec<crate::PollAtom>) {
        let ok = match atom {
            crate::PollAtom::Changed(target) => results.get(*target as usize).is_some_and(|result| result.text.iter().any(|text| !text.lines.is_empty())),
            crate::PollAtom::Regex(regex) => results
                .get(regex.target as usize)
                .is_some_and(|result| {
                    regex::Regex::new(&regex.pattern).is_ok_and(|regex| {
                        result
                            .text
                            .iter()
                            .flat_map(|text| &text.lines)
                            .any(|line| regex.is_match(&line.text))
                    })
                }),
            crate::PollAtom::Terminated(target) => results.get(*target as usize).is_some_and(|result| result.satisfied.iter().any(|item| matches!(item, crate::PollAtom::Terminated(found) if found == target))),
            crate::PollAtom::Timeout(milliseconds) => {
                elapsed >= tokio::time::Duration::from_millis(*milliseconds)
            }
        };
        (ok, ok.then(|| vec![atom.clone()]).unwrap_or_default())
    }
    fn walk(
        condition: &crate::PollCondition,
        results: &[crate::PollResult],
        elapsed: tokio::time::Duration,
    ) -> (bool, Vec<crate::PollAtom>) {
        match condition {
            crate::PollCondition::Atom(atom_value) => atom(atom_value, results, elapsed),
            crate::PollCondition::All(children) => {
                let values = children
                    .iter()
                    .map(|child| walk(child, results, elapsed))
                    .collect::<Vec<_>>();
                (
                    values.iter().all(|(ok, _)| *ok),
                    values.into_iter().flat_map(|(_, atoms)| atoms).collect(),
                )
            }
            crate::PollCondition::Any(children) => {
                let values = children
                    .iter()
                    .map(|child| walk(child, results, elapsed))
                    .collect::<Vec<_>>();
                (
                    values.iter().any(|(ok, _)| *ok),
                    values
                        .into_iter()
                        .filter(|(ok, _)| *ok)
                        .flat_map(|(_, atoms)| atoms)
                        .collect(),
                )
            }
        }
    }
    if condition.is_none() {
        let mut atoms = Vec::new();
        for (index, result) in results.iter().enumerate() {
            if result.text.iter().any(|text| !text.lines.is_empty()) {
                atoms.push(crate::PollAtom::Changed(index as u32));
            }
            if result.satisfied.iter().any(
                |atom| matches!(atom, crate::PollAtom::Terminated(found) if *found == index as u32),
            ) {
                atoms.push(crate::PollAtom::Terminated(index as u32));
            }
        }
        return (!atoms.is_empty(), atoms);
    }
    walk(
        condition.expect("condition checked above"),
        results,
        elapsed,
    )
}

/// URI router and universal operation dispatcher.
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
                handlers: RwLock::new(Vec::new()),
                typed_handlers: RwLock::new(Vec::new()),
                tool_providers: RwLock::new(Vec::new()),
            }),
        }
    }

    pub async fn register_typed<H>(&self, handler: H)
    where
        H: TypedHandler + 'static,
    {
        self.inner
            .typed_handlers
            .write()
            .await
            .push(Arc::new(handler));
    }

    /// Dispatches the typed contract surface. This is the component-native
    /// entry point; `execute(Request)` remains only as an adapter for legacy
    /// callers while they migrate.
    pub async fn execute_operation(
        &self,
        operation: Operation,
    ) -> Result<OperationResult, KernelError> {
        self.execute_operation_with_context(operation, InvocationContext::default())
            .await
    }

    pub async fn execute_operation_with_context(
        &self,
        operation: Operation,
        context: InvocationContext,
    ) -> Result<OperationResult, KernelError> {
        let handlers = self.inner.typed_handlers.read().await;
        let host = self.handle();
        match operation {
            Operation::Read(requests) => {
                let mut output = Vec::with_capacity(requests.len());
                for request in requests {
                    let item = Operation::Read(vec![request.clone()]);
                    let result = self
                        .execute_typed_item(&handlers, item, host.clone(), context.clone())
                        .await;
                    match result {
                        Ok(OperationResult::Read(mut values)) => output.append(&mut values),
                        Ok(_) => output.push(Err(KernelError::Handler {
                            message: "typed read returned the wrong result variant".to_owned(),
                        })),
                        Err(error) => output.push(Err(error)),
                    }
                }
                Ok(OperationResult::Read(output))
            }
            Operation::Write(requests) => {
                let mut output = Vec::with_capacity(requests.len());
                for request in requests {
                    let result = self
                        .execute_typed_item(
                            &handlers,
                            Operation::Write(vec![request]),
                            host.clone(),
                            context.clone(),
                        )
                        .await;
                    match result {
                        Ok(OperationResult::Write(mut values)) => output.append(&mut values),
                        Ok(_) => output.push(Err(KernelError::Handler {
                            message: "typed write returned the wrong result variant".to_owned(),
                        })),
                        Err(error) => output.push(Err(error)),
                    }
                }
                Ok(OperationResult::Write(output))
            }
            Operation::Edit(requests) => {
                let mut output = Vec::with_capacity(requests.len());
                for request in requests {
                    let result = self
                        .execute_typed_item(
                            &handlers,
                            Operation::Edit(vec![request]),
                            host.clone(),
                            context.clone(),
                        )
                        .await;
                    match result {
                        Ok(OperationResult::Edit(mut values)) => output.append(&mut values),
                        Ok(_) => output.push(Err(KernelError::Handler {
                            message: "typed edit returned the wrong result variant".to_owned(),
                        })),
                        Err(error) => output.push(Err(error)),
                    }
                }
                Ok(OperationResult::Edit(output))
            }
            Operation::Run(requests) => Ok(OperationResult::Run(
                self.route_run(&handlers, requests, host, context).await,
            )),
            Operation::Send(requests) => Ok(OperationResult::Send(
                self.route_send(&handlers, requests, host, context).await,
            )),
            Operation::Abort(uris) => Ok(OperationResult::Abort(
                self.route_uris(&handlers, uris, Operation::Abort, host, context)
                    .await,
            )),
            Operation::Delete(uris) => Ok(OperationResult::Delete(
                self.route_uris(&handlers, uris, Operation::Delete, host, context)
                    .await,
            )),
            Operation::Find(request) => self.route_find(&handlers, request, host, context).await,
            Operation::Grep(request) => self.route_grep(&handlers, request, host, context).await,
            Operation::Poll(request) => self.route_poll(&handlers, request, host, context).await,
        }
    }

    async fn execute_typed_item(
        &self,
        handlers: &[Arc<dyn TypedHandler>],
        operation: Operation,
        host: KernelHandle,
        context: InvocationContext,
    ) -> Result<OperationResult, KernelError> {
        let Some(handler) = handlers
            .iter()
            .find(|handler| handler.claims_operation(&operation))
        else {
            return Err(KernelError::NoHandler {
                uri: operation_uri(&operation),
            });
        };
        handler.execute_typed(operation, host, context).await
    }

    async fn route_run(
        &self,
        handlers: &[Arc<dyn TypedHandler>],
        requests: Vec<crate::RunRequest>,
        host: KernelHandle,
        context: InvocationContext,
    ) -> Vec<Result<ResourceUri, KernelError>> {
        let mut output = Vec::with_capacity(requests.len());
        for request in requests {
            match self
                .execute_typed_item(
                    handlers,
                    Operation::Run(vec![request]),
                    host.clone(),
                    context.clone(),
                )
                .await
            {
                Ok(OperationResult::Run(mut values)) => output.push(values.remove(0)),
                Ok(_) => output.push(Err(KernelError::Handler {
                    message: "typed batch returned the wrong result variant".to_owned(),
                })),
                Err(error) => output.push(Err(error)),
            }
        }
        output
    }

    async fn route_send(
        &self,
        handlers: &[Arc<dyn TypedHandler>],
        requests: Vec<crate::SendRequest>,
        host: KernelHandle,
        context: InvocationContext,
    ) -> Vec<Result<ResourceUri, KernelError>> {
        let mut output = Vec::with_capacity(requests.len());
        for request in requests {
            match self
                .execute_typed_item(
                    handlers,
                    Operation::Send(vec![request]),
                    host.clone(),
                    context.clone(),
                )
                .await
            {
                Ok(OperationResult::Send(mut values)) => output.push(values.remove(0)),
                Ok(_) => output.push(Err(KernelError::Handler {
                    message: "typed send returned the wrong result variant".to_owned(),
                })),
                Err(error) => output.push(Err(error)),
            }
        }
        output
    }

    async fn route_uris(
        &self,
        handlers: &[Arc<dyn TypedHandler>],
        uris: Vec<ResourceUri>,
        verb: impl Fn(Vec<ResourceUri>) -> Operation,
        host: KernelHandle,
        context: InvocationContext,
    ) -> Vec<Result<ResourceUri, KernelError>> {
        let mut output = Vec::with_capacity(uris.len());
        for uri in uris {
            match self
                .execute_typed_item(
                    handlers,
                    verb(vec![uri.clone()]),
                    host.clone(),
                    context.clone(),
                )
                .await
            {
                Ok(OperationResult::Abort(mut values))
                | Ok(OperationResult::Delete(mut values)) => output.push(values.remove(0)),
                Ok(_) => output.push(Err(KernelError::Handler {
                    message: "typed URI batch returned the wrong result variant".to_owned(),
                })),
                Err(error) => output.push(Err(error)),
            }
        }
        output
    }

    async fn route_find(
        &self,
        handlers: &[Arc<dyn TypedHandler>],
        request: crate::FindRequest,
        host: KernelHandle,
        context: InvocationContext,
    ) -> Result<OperationResult, KernelError> {
        let mut paths = Vec::new();
        for root in request.roots {
            match self
                .execute_typed_item(
                    handlers,
                    Operation::Find(crate::FindRequest {
                        roots: vec![root],
                        query: request.query.clone(),
                    }),
                    host.clone(),
                    context.clone(),
                )
                .await?
            {
                OperationResult::Find(result) => paths.extend(result?),
                _ => {
                    return Err(KernelError::Handler {
                        message: "typed find returned the wrong result variant".to_owned(),
                    });
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        paths.retain(|path| seen.insert(path.clone()));
        Ok(OperationResult::Find(Ok(paths)))
    }

    async fn route_grep(
        &self,
        handlers: &[Arc<dyn TypedHandler>],
        request: crate::GrepRequest,
        host: KernelHandle,
        context: InvocationContext,
    ) -> Result<OperationResult, KernelError> {
        match request.source {
            crate::GrepSource::Resources(uris) => {
                let mut text = Vec::new();
                for uri in uris {
                    match self
                        .execute_typed_item(
                            handlers,
                            Operation::Grep(crate::GrepRequest {
                                pattern: request.pattern.clone(),
                                source: crate::GrepSource::Resources(vec![uri]),
                            }),
                            host.clone(),
                            context.clone(),
                        )
                        .await?
                    {
                        OperationResult::Grep(result) => text.extend(result?),
                        _ => {
                            return Err(KernelError::Handler {
                                message: "typed grep returned the wrong result variant".to_owned(),
                            });
                        }
                    }
                }
                Ok(OperationResult::Grep(Ok(text)))
            }
            source => {
                self.execute_typed_item(
                    handlers,
                    Operation::Grep(crate::GrepRequest {
                        pattern: request.pattern,
                        source,
                    }),
                    host,
                    context,
                )
                .await
            }
        }
    }

    async fn route_poll(
        &self,
        handlers: &[Arc<dyn TypedHandler>],
        request: crate::PollRequest,
        host: KernelHandle,
        context: InvocationContext,
    ) -> Result<OperationResult, KernelError> {
        if request.targets.is_empty() {
            return Err(KernelError::InvalidRequest {
                message: "poll requires at least one target".to_owned(),
            });
        }
        let owners = request
            .targets
            .iter()
            .map(|target| {
                let operation = Operation::Poll(crate::PollRequest {
                    targets: vec![target.clone()],
                    until: None,
                    before: request.before,
                    after: request.after,
                });
                handlers
                    .iter()
                    .position(|handler| handler.claims_operation(&operation))
                    .ok_or_else(|| KernelError::NoHandler {
                        uri: target.uri.to_string(),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if owners.windows(2).all(|pair| pair[0] == pair[1]) {
            return self
                .execute_typed_item(handlers, Operation::Poll(request), host, context)
                .await;
        }
        let started = tokio::time::Instant::now();
        let timeout_ms = poll_timeout(&request.until);
        loop {
            let slice_ms = timeout_ms
                .map(|ms| {
                    ms.saturating_sub(started.elapsed().as_millis() as u64)
                        .min(50)
                })
                .unwrap_or(50);
            let futures = request.targets.iter().map(|target| {
                let operation = Operation::Poll(crate::PollRequest {
                    targets: vec![target.clone()],
                    until: Some(crate::PollCondition::Atom(crate::PollAtom::Timeout(
                        slice_ms,
                    ))),
                    before: request.before,
                    after: request.after,
                });
                self.execute_typed_item(handlers, operation, host.clone(), context.clone())
            });
            let mut results = Vec::with_capacity(request.targets.len());
            for result in futures::future::join_all(futures).await {
                let OperationResult::Poll(result) = result? else {
                    return Err(KernelError::Handler {
                        message: "typed poll returned the wrong result variant".to_owned(),
                    });
                };
                results.push(result?);
            }
            let elapsed = started.elapsed();
            let (satisfied, atoms) =
                evaluate_poll_condition(request.until.as_ref(), &results, elapsed);
            if satisfied {
                return Ok(OperationResult::Poll(Ok(crate::PollResult {
                    text: results.into_iter().flat_map(|result| result.text).collect(),
                    satisfied: atoms,
                })));
            }
        }
    }

    pub async fn register<H>(&self, handler: H)
    where
        H: Handler + 'static,
    {
        self.inner.handlers.write().await.push(Arc::new(handler));
    }

    /// Register one object as both a URI handler and a named-tool provider.
    /// Keeping the two trait objects backed by the same allocation is
    /// important for self-modifying handlers: their catalog and executor
    /// must observe the same active component generations.
    pub async fn register_tool_handler<H>(&self, handler: H)
    where
        H: Handler + ToolProvider + 'static,
    {
        let handler = Arc::new(handler);
        self.inner.handlers.write().await.push(handler.clone());
        self.inner.tool_providers.write().await.push(handler);
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

    pub async fn execute_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, KernelError> {
        self.execute_tool_with_context(name, args, InvocationContext::default())
            .await
    }

    pub async fn execute_tool_with_context(
        &self,
        name: &str,
        args: serde_json::Value,
        context: InvocationContext,
    ) -> Result<serde_json::Value, KernelError> {
        let providers = self.inner.tool_providers.read().await;
        let host = self.handle();
        for provider in providers.iter() {
            let definitions = provider.tool_definitions();
            if definitions.iter().any(|definition| definition.name == name) {
                return provider
                    .execute_tool_with_context(name, args, host, context)
                    .await;
            }
        }
        Err(KernelError::Handler {
            message: format!("no named tool registered: {name}"),
        })
    }

    pub async fn descriptors(&self) -> Vec<HandlerDescriptor> {
        self.inner
            .handlers
            .read()
            .await
            .iter()
            .map(|handler| handler.descriptor())
            .collect()
    }

    pub fn handle(&self) -> KernelHandle {
        let kernel = self.clone();
        let typed_kernel = kernel.clone();
        KernelHandle::new(
            Arc::new(move |request| {
                let kernel = kernel.clone();
                Box::pin(async move { kernel.execute(request).await })
            }),
            Arc::new(move |operation, context| {
                let kernel = typed_kernel.clone();
                Box::pin(async move {
                    kernel
                        .execute_operation_with_context(operation, context)
                        .await
                })
            }),
        )
    }

    pub async fn execute(&self, request: Request) -> ItemResult {
        let request = match crate::normalize(&request.target) {
            Ok(target) => Request { target, ..request },
            Err(error) => return ItemResult::failure(request.target, error),
        };
        let target = request.target.clone();
        let handlers = self.inner.handlers.read().await;
        let Some(handler) = handlers.iter().find(|handler| handler.claims(&target)) else {
            return ItemResult::failure(
                target.clone(),
                KernelError::NoHandler {
                    uri: target.to_string(),
                },
            );
        };
        if !handler.supports(request.verb) {
            return ItemResult::failure(
                target.clone(),
                KernelError::UnsupportedVerb {
                    verb: request.verb.to_string(),
                    uri: target.to_string(),
                },
            );
        }
        let host = self.handle();
        let result = handler.execute(request, host).await;
        match result {
            Ok(value) => ItemResult::success(target, value),
            Err(error) => ItemResult::failure(target, error),
        }
    }

    pub async fn execute_batch(&self, batch: BatchRequest) -> BatchResult {
        let mut items = Vec::with_capacity(batch.items.len());
        for request in batch.items {
            items.push(self.execute(request).await);
        }
        BatchResult { items }
    }

    pub async fn registered_verbs(&self) -> Vec<Verb> {
        let mut verbs = self
            .descriptors()
            .await
            .into_iter()
            .flat_map(|descriptor| descriptor.verbs)
            .collect::<Vec<_>>();
        verbs.sort_by_key(|verb| verb.to_string());
        verbs.dedup();
        verbs
    }
}
