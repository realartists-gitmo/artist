use crate::{KernelError, Request, ResourceAddress, Verb};
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
}

impl KernelHandle {
    pub(crate) fn new(
        dispatch: Arc<dyn Fn(Request) -> BoxFuture<'static, crate::ItemResult> + Send + Sync>,
    ) -> Self {
        Self { dispatch }
    }

    pub fn execute(&self, request: Request) -> BoxFuture<'static, crate::ItemResult> {
        (self.dispatch)(request)
    }
}
