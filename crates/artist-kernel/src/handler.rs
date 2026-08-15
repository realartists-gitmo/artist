use crate::{InvocationContext, InvocationScope, KernelError};
use futures::future::join_all;
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

    /// Model-facing execution consumes the package-owned `stdobs` projection;
    /// programmatic callers continue to receive the authoritative value from
    /// `execute_tool_with_scope`.
    fn execute_tool_for_model<'a>(
        &'a self,
        name: &'a str,
        args: crate::DynamicValue,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> BoxFuture<'a, Result<crate::DynamicValue, KernelError>> {
        self.execute_tool_with_scope(name, args, host, scope)
    }

    fn execute_tool_for_model_result<'a>(
        &'a self,
        name: &'a str,
        args: crate::DynamicValue,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> BoxFuture<'a, Result<ToolModelResult, KernelError>> {
        Box::pin(async move {
            let value = self
                .execute_tool_with_scope(name, args, host, scope)
                .await?;
            Ok(ToolModelResult {
                stdout: Ok(value.clone()),
                stdobs: format!("{value:?}"),
                verb: crate::VerbId::new(format!("artist:tool/{name}@1.0.0"))
                    .map_err(|message| KernelError::InvalidRequest { message })?,
                generation: 0,
            })
        })
    }

    fn execute_tools_for_model_results<'a>(
        &'a self,
        name: &'a str,
        args: Vec<crate::DynamicValue>,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> BoxFuture<'a, Vec<Result<ToolModelResult, KernelError>>> {
        Box::pin(async move {
            join_all(args.into_iter().map(|arg| {
                self.execute_tool_for_model_result(name, arg, host.clone(), scope.child())
            }))
            .await
        })
    }

    fn execute_tools_for_model<'a>(
        &'a self,
        name: &'a str,
        args: Vec<crate::DynamicValue>,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> BoxFuture<'a, Vec<Result<crate::DynamicValue, KernelError>>> {
        Box::pin(async move {
            join_all(
                args.into_iter()
                    .map(|arg| self.execute_tool_for_model(name, arg, host.clone(), scope.child())),
            )
            .await
        })
    }

    fn execute_tool_batch_with_scope<'a>(
        &'a self,
        name: &'a str,
        args: Vec<crate::DynamicValue>,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> BoxFuture<'a, Vec<Result<crate::DynamicVerbResult, KernelError>>> {
        Box::pin(async move {
            let verb = match crate::VerbId::new(format!("artist:tool/{name}@1.0.0")) {
                Ok(verb) => verb,
                Err(error) => {
                    let error = KernelError::InvalidRequest {
                        message: error.to_string(),
                    };
                    return args.into_iter().map(|_| Err(error.clone())).collect();
                }
            };
            let mut results = Vec::with_capacity(args.len());
            for arg in args {
                results.push(
                    self.execute_tool_with_scope(name, arg, host.clone(), scope.child())
                        .await
                        .map(|output| crate::DynamicVerbResult {
                            verb: verb.clone(),
                            function: name.to_owned(),
                            output,
                        }),
                );
            }
            results
        })
    }
}

#[derive(Clone, Debug)]
pub struct ToolModelResult {
    pub stdout: Result<crate::DynamicValue, KernelError>,
    pub stdobs: String,
    pub verb: crate::VerbId,
    pub generation: u64,
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
    pub input_type: Option<crate::DynamicType>,
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
    direct_dynamic_batch_dispatch: Arc<
        dyn Fn(
                crate::VerbId,
                Vec<crate::ResourceRequest>,
                InvocationScope,
            )
                -> BoxFuture<'static, Vec<Result<crate::DynamicVerbResult, KernelError>>>
            + Send
            + Sync,
    >,
    universal_dispatch: Option<
        Arc<
            dyn Fn(
                    String,
                    crate::DynamicValue,
                    InvocationScope,
                )
                    -> BoxFuture<'static, Result<Vec<crate::DynamicResourceResult>, KernelError>>
                + Send
                + Sync,
        >,
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
        let direct_dynamic_batch_error = || {
            Box::pin(async {
                vec![Err(KernelError::Handler {
                    message: "detached provider host does not support direct dynamic batches"
                        .to_owned(),
                })]
            })
        };
        Self::with_dispatch(
            Arc::new(move |_, _| dynamic_error()),
            Arc::new(move |_, _, _, _| direct_dynamic_error()),
            Arc::new(move |_, _, _| direct_dynamic_batch_error()),
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
        direct_dynamic_batch_dispatch: Arc<
            dyn Fn(
                    crate::VerbId,
                    Vec<crate::ResourceRequest>,
                    InvocationScope,
                )
                    -> BoxFuture<'static, Vec<Result<crate::DynamicVerbResult, KernelError>>>
                + Send
                + Sync,
        >,
    ) -> Self {
        Self {
            dynamic_dispatch,
            direct_dynamic_dispatch,
            direct_dynamic_batch_dispatch,
            universal_dispatch: None,
        }
    }

    pub(crate) fn with_universal_dispatch(
        mut self,
        dispatch: Arc<
            dyn Fn(
                    String,
                    crate::DynamicValue,
                    InvocationScope,
                )
                    -> BoxFuture<'static, Result<Vec<crate::DynamicResourceResult>, KernelError>>
                + Send
                + Sync,
        >,
    ) -> Self {
        self.universal_dispatch = Some(dispatch);
        self
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

    pub fn execute_universal_with_scope(
        &self,
        function: String,
        input: crate::DynamicValue,
        scope: InvocationScope,
    ) -> BoxFuture<'static, Result<Vec<crate::DynamicResourceResult>, KernelError>> {
        match &self.universal_dispatch {
            Some(dispatch) => dispatch(function, input, scope),
            None => Box::pin(async {
                Err(KernelError::Handler {
                    message: "kernel handle does not support universal dispatch".to_owned(),
                })
            }),
        }
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

    pub fn invoke_dynamic_resource_batch_with_scope(
        &self,
        verb: crate::VerbId,
        requests: Vec<crate::ResourceRequest>,
        scope: InvocationScope,
    ) -> BoxFuture<'static, Vec<Result<crate::DynamicVerbResult, KernelError>>> {
        (self.direct_dynamic_batch_dispatch)(verb, requests, scope)
    }
}
