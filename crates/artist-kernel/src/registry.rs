use crate::{
    BatchRequest, BatchResult, Handler, HandlerDescriptor, ItemResult, KernelError, KernelHandle,
    Request, ToolDefinition, ToolProvider, Verb,
};
use std::sync::Arc;
use tokio::sync::RwLock;

struct Inner {
    handlers: RwLock<Vec<Arc<dyn Handler>>>,
    tool_providers: RwLock<Vec<Arc<dyn ToolProvider>>>,
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
                tool_providers: RwLock::new(Vec::new()),
            }),
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
        let providers = self.inner.tool_providers.read().await;
        let host = self.handle();
        for provider in providers.iter() {
            let definitions = provider.tool_definitions();
            if definitions.iter().any(|definition| definition.name == name) {
                return provider.execute_tool(name, args, host).await;
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
        KernelHandle::new(Arc::new(move |request| {
            let kernel = kernel.clone();
            Box::pin(async move { kernel.execute(request).await })
        }))
    }

    pub async fn execute(&self, request: Request) -> ItemResult {
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
