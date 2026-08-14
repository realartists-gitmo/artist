use crate::{
    InvocationContext, InvocationScope, KernelError, Operation, OperationResult, Request,
    ResourceAddress, Verb,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimDecision {
    Pass,
    Handle,
    Reserve,
}

/// Metadata used by the kernel registry and, later, provider adapters.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct HandlerDescriptor {
    pub name: String,
    pub schemes: Vec<String>,
    pub verbs: Vec<Verb>,
}

/// A backend for one or more resource URI families.
pub trait Handler: Send + Sync {
    fn descriptor(&self) -> HandlerDescriptor;

    fn claims(&self, address: &ResourceAddress) -> bool {
        let Some(uri) = address.as_uri() else {
            return false;
        };
        self.descriptor()
            .schemes
            .iter()
            .any(|scheme| scheme == uri.scheme())
    }

    fn supports(&self, verb: Verb) -> bool {
        self.descriptor().verbs.contains(&verb)
    }

    fn execute<'a>(
        &'a self,
        request: Request,
        host: KernelHandle,
    ) -> BoxFuture<'a, Result<serde_json::Value, KernelError>>;
}

/// Typed implementation boundary for universal components. Implementations
/// receive the contract-shaped operation and return the contract-shaped
/// result; JSON adapters must live above this trait.
pub trait TypedHandler: Send + Sync {
    fn descriptor(&self) -> HandlerDescriptor;
    fn claims_operation(&self, operation: &Operation) -> bool;
    fn claim_operation<'a>(
        &'a self,
        operation: &'a Operation,
    ) -> BoxFuture<'a, Result<ClaimDecision, KernelError>> {
        Box::pin(async move {
            Ok(if self.claims_operation(operation) {
                ClaimDecision::Handle
            } else {
                ClaimDecision::Pass
            })
        })
    }
    fn claim_operation_with_scope<'a>(
        &'a self,
        operation: &'a Operation,
        _scope: InvocationScope,
    ) -> BoxFuture<'a, Result<ClaimDecision, KernelError>> {
        self.claim_operation(operation)
    }
    fn execute_typed<'a>(
        &'a self,
        operation: Operation,
        host: KernelHandle,
        context: InvocationContext,
    ) -> BoxFuture<'a, Result<OperationResult, KernelError>>;

    fn execute_typed_with_scope<'a>(
        &'a self,
        operation: Operation,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> BoxFuture<'a, Result<OperationResult, KernelError>> {
        self.execute_typed(operation, host, scope.context)
    }
}

/// Metadata and execution surface for tools exposed directly to the model.
///
/// This is deliberately separate from URI handlers: `tools://` is the
/// mutable definition namespace, while named tools operate on ordinary
/// resource targets.
pub trait ToolProvider: Send + Sync {
    fn tool_definitions(&self) -> Vec<ToolDefinition>;

    fn execute_tool<'a>(
        &'a self,
        name: &'a str,
        args: Value,
        host: KernelHandle,
    ) -> BoxFuture<'a, Result<Value, KernelError>>;

    fn execute_tool_with_context<'a>(
        &'a self,
        name: &'a str,
        args: Value,
        host: KernelHandle,
        _context: InvocationContext,
    ) -> BoxFuture<'a, Result<Value, KernelError>> {
        self.execute_tool(name, args, host)
    }

    fn execute_tool_with_scope<'a>(
        &'a self,
        name: &'a str,
        args: Value,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> BoxFuture<'a, Result<Value, KernelError>> {
        self.execute_tool_with_context(name, args, host, scope.context)
    }
}

/// Compact, model-facing documentation published by active resource
/// extensions. Full prose remains addressable through `resources://`.
pub trait ResourceCatalogProvider: Send + Sync {
    fn resource_catalog(&self) -> Vec<ResourceCatalogEntry>;
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ResourceCatalogEntry {
    pub name: String,
    pub description: String,
    pub docs: Vec<ResourceCatalogDoc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ResourceCatalogDoc {
    pub uri: String,
    pub summary: String,
    pub verbs: Vec<String>,
    pub query: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// The shared kernel surface available to handlers for nested calls.
#[derive(Clone)]
pub struct KernelHandle {
    dispatch: Arc<dyn Fn(Request) -> BoxFuture<'static, crate::ItemResult> + Send + Sync>,
    typed_dispatch: Arc<
        dyn Fn(
                Operation,
                InvocationScope,
            ) -> BoxFuture<'static, Result<OperationResult, KernelError>>
            + Send
            + Sync,
    >,
}

impl KernelHandle {
    pub(crate) fn new(
        dispatch: Arc<dyn Fn(Request) -> BoxFuture<'static, crate::ItemResult> + Send + Sync>,
        typed_dispatch: Arc<
            dyn Fn(
                    Operation,
                    InvocationScope,
                ) -> BoxFuture<'static, Result<OperationResult, KernelError>>
                + Send
                + Sync,
        >,
    ) -> Self {
        Self {
            dispatch,
            typed_dispatch,
        }
    }

    pub fn execute(&self, request: Request) -> BoxFuture<'static, crate::ItemResult> {
        (self.dispatch)(request)
    }

    pub fn execute_operation(
        &self,
        operation: Operation,
    ) -> BoxFuture<'static, Result<OperationResult, KernelError>> {
        (self.typed_dispatch)(
            operation,
            InvocationScope::new(InvocationContext::default()),
        )
    }

    pub fn execute_operation_with_context(
        &self,
        operation: Operation,
        context: InvocationContext,
    ) -> BoxFuture<'static, Result<OperationResult, KernelError>> {
        (self.typed_dispatch)(operation, InvocationScope::new(context))
    }

    pub fn execute_operation_with_scope(
        &self,
        operation: Operation,
        scope: InvocationScope,
    ) -> BoxFuture<'static, Result<OperationResult, KernelError>> {
        (self.typed_dispatch)(operation, scope)
    }
}
