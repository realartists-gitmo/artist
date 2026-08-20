//! Model-facing tool registry owned by the component layer.
//!
//! The kernel only transports resource operations and generic component
//! lifecycle. Tool names and TOON invocation belong here.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use anyhow::anyhow;
use async_trait::async_trait;
use artist_wasm_verbs::component::{ToolError as WasmToolError, WasmTool};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolError {
    InvalidArgument(String),
    NotFound(String),
    Unsupported(String),
    PermissionDenied(String),
    Conflict(String),
    Aborted(String),
    Internal(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidArgument(value)
            | Self::NotFound(value)
            | Self::Unsupported(value)
            | Self::PermissionDenied(value)
            | Self::Conflict(value)
            | Self::Aborted(value)
            | Self::Internal(value) => value,
        };
        f.write_str(message)
    }
}

impl std::error::Error for ToolError {}

#[async_trait]
pub trait ToolComponent: Send + Sync {
    fn name(&self) -> &str;
    async fn invoke(&self, request: &[u8]) -> Result<Vec<u8>, ToolError>;
}

/// The live model-facing tool set for one session/view.
#[derive(Clone, Default)]
pub struct ComponentToolRegistry {
    tools: Arc<RwLock<BTreeMap<String, Arc<dyn ToolComponent>>>>,
}

impl ComponentToolRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn register<T: ToolComponent + 'static>(&self, tool: T) -> Result<(), ToolError> {
        let mut tools = self.tools.write().unwrap();
        if tools.contains_key(tool.name()) {
            return Err(ToolError::Conflict(format!("tool already registered: {}", tool.name())));
        }
        tools.insert(tool.name().to_owned(), Arc::new(tool));
        Ok(())
    }

    pub fn unregister(&self, name: &str) -> bool {
        self.tools.write().unwrap().remove(name).is_some()
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.read().unwrap().keys().cloned().collect()
    }

    pub async fn invoke(&self, name: &str, request: &[u8]) -> Result<Vec<u8>, ToolError> {
        let tool = self.tools.read().unwrap().get(name).cloned()
            .ok_or_else(|| ToolError::NotFound(name.to_owned()))?;
        tool.invoke(request).await
    }
}

/// Adapts a live WASM tool generation into the session registry.
pub struct WasmToolComponent {
    tool: WasmTool,
}

impl WasmToolComponent {
    pub fn new(tool: WasmTool) -> Self { Self { tool } }
}

#[async_trait]
impl ToolComponent for WasmToolComponent {
    fn name(&self) -> &str { self.tool.name() }

    async fn invoke(&self, request: &[u8]) -> Result<Vec<u8>, ToolError> {
        self.tool.invoke(request).await.map_err(map_wasm_error)
    }
}

fn map_wasm_error(error: WasmToolError) -> ToolError {
    match error {
        WasmToolError::InvalidArgument => ToolError::InvalidArgument("tool rejected request".into()),
        WasmToolError::NotFound => ToolError::NotFound("tool target not found".into()),
        WasmToolError::Unsupported => ToolError::Unsupported("tool operation unsupported".into()),
        WasmToolError::PermissionDenied => ToolError::PermissionDenied("tool operation denied".into()),
        WasmToolError::Conflict => ToolError::Conflict("tool operation conflicted".into()),
        WasmToolError::Aborted => ToolError::Aborted("tool operation aborted".into()),
        WasmToolError::Internal => ToolError::Internal("tool component failed".into()),
        WasmToolError::Unavailable => ToolError::NotFound("tool component unavailable".into()),
    }
}

/// A component registry error can be converted into an anyhow error at
/// process boundaries without exposing the kernel's legacy verb algebra.
pub fn tool_error(error: ToolError) -> anyhow::Error { anyhow!(error.to_string()) }

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait]
    impl ToolComponent for Echo {
        fn name(&self) -> &str { "echo" }
        async fn invoke(&self, request: &[u8]) -> Result<Vec<u8>, ToolError> { Ok(request.to_vec()) }
    }

    #[tokio::test]
    async fn registry_is_live_and_session_local() {
        let registry = ComponentToolRegistry::new();
        registry.register(Echo).unwrap();
        assert_eq!(registry.names(), vec!["echo"]);
        assert_eq!(registry.invoke("echo", b"x").await.unwrap(), b"x");
        assert!(registry.unregister("echo"));
        assert!(matches!(registry.invoke("echo", b"").await, Err(ToolError::NotFound(_))));
    }
}
