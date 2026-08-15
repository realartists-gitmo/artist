use crate::{InvocationContext, InvocationScope, KernelError};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin, sync::Arc};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimDecision {
    Pass,
    Handle,
    Reserve,
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
        args: crate::DynamicValue,
        host: KernelHandle,
    ) -> BoxFuture<'a, Result<crate::DynamicValue, KernelError>>;

    fn execute_tool_with_context<'a>(
        &'a self,
        name: &'a str,
        args: crate::DynamicValue,
        host: KernelHandle,
        _context: InvocationContext,
    ) -> BoxFuture<'a, Result<crate::DynamicValue, KernelError>> {
        self.execute_tool(name, args, host)
    }

    fn execute_tool_with_scope<'a>(
        &'a self,
        name: &'a str,
        args: crate::DynamicValue,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> BoxFuture<'a, Result<crate::DynamicValue, KernelError>> {
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

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: crate::DynamicValue,
}

/// The shared kernel surface available to handlers for nested calls.
#[derive(Clone)]
pub struct KernelHandle {
    dynamic_dispatch: Arc<
        dyn Fn(
                crate::DynamicVerbCall,
                InvocationScope,
            )
                -> BoxFuture<'static, Result<Vec<crate::DynamicResourceResult>, KernelError>>
            + Send
            + Sync,
    >,
    direct_dynamic_dispatch: Arc<
        dyn Fn(
                crate::VerbId,
                crate::ResourceUri,
                crate::DynamicValue,
                InvocationScope,
            ) -> BoxFuture<'static, Result<crate::DynamicVerbResult, KernelError>>
            + Send
            + Sync,
    >,
}

impl KernelHandle {
    /// Construct a host for a provider invocation that cannot perform nested
    /// kernel calls. Dynamic resource providers use this when the generic
    /// provider ABI does not carry a caller host; nested composition must use
    /// the kernel-routed component host instead.
    pub fn detached() -> Self {
        let dynamic_error = || {
            Box::pin(async {
                Err(KernelError::Handler {
                    message: "detached provider host does not support nested calls".to_owned(),
                })
            })
        };
        let direct_dynamic_error = || {
            Box::pin(async {
                Err(KernelError::Handler {
                    message: "detached provider host does not support direct dynamic calls"
                        .to_owned(),
                })
            })
        };
        Self::with_dispatch(
            Arc::new(move |_, _| dynamic_error()),
            Arc::new(move |_, _, _, _| direct_dynamic_error()),
        )
    }

    pub(crate) fn with_dispatch(
        dynamic_dispatch: Arc<
            dyn Fn(
                    crate::DynamicVerbCall,
                    InvocationScope,
                )
                    -> BoxFuture<'static, Result<Vec<crate::DynamicResourceResult>, KernelError>>
                + Send
                + Sync,
        >,
        direct_dynamic_dispatch: Arc<
            dyn Fn(
                    crate::VerbId,
                    crate::ResourceUri,
                    crate::DynamicValue,
                    InvocationScope,
                )
                    -> BoxFuture<'static, Result<crate::DynamicVerbResult, KernelError>>
                + Send
                + Sync,
        >,
    ) -> Self {
        Self {
            dynamic_dispatch,
            direct_dynamic_dispatch,
        }
    }

    pub fn execute_dynamic_resources(
        &self,
        call: crate::DynamicVerbCall,
    ) -> BoxFuture<'static, Result<Vec<crate::DynamicResourceResult>, KernelError>> {
        (self.dynamic_dispatch)(call, InvocationScope::new(InvocationContext::default()))
    }

    pub fn execute_dynamic_resources_with_scope(
        &self,
        call: crate::DynamicVerbCall,
        scope: InvocationScope,
    ) -> BoxFuture<'static, Result<Vec<crate::DynamicResourceResult>, KernelError>> {
        (self.dynamic_dispatch)(call, scope)
    }

    /// Invoke one provider directly through the open resource ABI. This is a
    /// transitional nested-component seam: it intentionally does not rebuild
    /// a closed kernel operation router.
    pub fn invoke_dynamic_resource_with_scope(
        &self,
        verb: crate::VerbId,
        uri: crate::ResourceUri,
        input: crate::DynamicValue,
        scope: InvocationScope,
    ) -> BoxFuture<'static, Result<crate::DynamicVerbResult, KernelError>> {
        (self.direct_dynamic_dispatch)(verb, uri, input, scope)
    }
}
