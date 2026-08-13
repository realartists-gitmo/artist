use crate::{
    InvocationContext, KernelError, Operation, OperationResult, Request, ResourceAddress, Verb,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

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
    fn execute_typed<'a>(
        &'a self,
        operation: Operation,
        host: KernelHandle,
        context: InvocationContext,
    ) -> BoxFuture<'a, Result<OperationResult, KernelError>>;
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
                InvocationContext,
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
                    InvocationContext,
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
        (self.typed_dispatch)(operation, InvocationContext::default())
    }

    pub fn execute_operation_with_context(
        &self,
        operation: Operation,
        context: InvocationContext,
    ) -> BoxFuture<'static, Result<OperationResult, KernelError>> {
        (self.typed_dispatch)(operation, context)
    }
}
