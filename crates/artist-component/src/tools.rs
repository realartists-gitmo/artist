//! Model-facing tool registry owned by the component layer.
//!
//! The kernel only transports resource operations and generic component
//! lifecycle. Tool names and TOON invocation belong here.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use anyhow::anyhow;
use artist_wasm_verbs::component::{ToolError as WasmToolError, WasmTool};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The intentionally small provider-neutral envelope used at the model
/// tool boundary. Provider adapters translate this value into their native
/// tool-output item; the component layer never needs to know that syntax.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ToolResultEnvelope {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ToolFailure>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ToolFailure {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl ToolResultEnvelope {
    pub fn success(output: Value) -> Self {
        Self {
            ok: true,
            output: Some(output),
            error: None,
        }
    }

    pub fn failure(error: &ToolError) -> Self {
        Self {
            ok: false,
            output: None,
            error: Some(ToolFailure {
                code: error.code(),
                message: error.to_string(),
                details: error.details(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolError {
    InvalidArgument(String),
    NotFound(String),
    Unsupported(String),
    PermissionDenied(String),
    Conflict(String),
    Aborted(String),
    Unavailable(String),
    Internal(String),
    Detailed {
        code: String,
        message: String,
        details: Value,
    },
}

impl ToolError {
    pub fn code(&self) -> String {
        match self {
            Self::InvalidArgument(_) => "invalid_argument".into(),
            Self::NotFound(_) => "not_found".into(),
            Self::Unsupported(_) => "unsupported".into(),
            Self::PermissionDenied(_) => "permission_denied".into(),
            Self::Conflict(_) => "conflict".into(),
            Self::Aborted(_) => "aborted".into(),
            Self::Unavailable(_) => "unavailable".into(),
            Self::Internal(_) => "internal".into(),
            Self::Detailed { code, .. } => code.clone(),
        }
    }

    pub fn details(&self) -> Option<Value> {
        match self {
            Self::Detailed { details, .. } => Some(details.clone()),
            _ => None,
        }
    }
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
            | Self::Unavailable(value)
            | Self::Internal(value) => value,
            Self::Detailed { message, .. } => message,
        };
        f.write_str(message)
    }
}

impl std::error::Error for ToolError {}

#[async_trait]
pub trait ToolComponent: Send + Sync {
    fn name(&self) -> &str;
    /// The component owns the model-facing contract. Hosts may filter or
    /// temporarily disable this definition, but never infer a schema from a
    /// tool name.
    fn definition(&self) -> llm_provider::ToolDefinition;
    async fn invoke(&self, request: &[u8]) -> Result<Vec<u8>, ToolError>;
}

/// The live model-facing tool set for one session/view.
#[derive(Clone, Default)]
pub struct ComponentToolRegistry {
    tools: Arc<RwLock<BTreeMap<String, RegisteredTool>>>,
}

struct RegisteredTool {
    component: Arc<dyn ToolComponent>,
    enabled: bool,
}

impl ComponentToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T: ToolComponent + 'static>(&self, tool: T) -> Result<(), ToolError> {
        let mut tools = self.tools.write().unwrap();
        if tools.contains_key(tool.name()) {
            return Err(ToolError::Conflict(format!(
                "tool already registered: {}",
                tool.name()
            )));
        }
        tools.insert(
            tool.name().to_owned(),
            RegisteredTool {
                component: Arc::new(tool),
                enabled: true,
            },
        );
        Ok(())
    }

    pub fn unregister(&self, name: &str) -> bool {
        self.tools.write().unwrap().remove(name).is_some()
    }

    /// Replace one complete extension generation atomically. Validation is
    /// performed while the registry is still unchanged, so a bad candidate
    /// cannot remove the last valid model-facing tool surface.
    pub fn replace_generation(
        &self,
        remove: &std::collections::BTreeSet<String>,
        additions: Vec<(String, Arc<dyn ToolComponent>)>,
    ) -> Result<(), ToolError> {
        let mut tools = self.tools.write().unwrap();
        let mut names = std::collections::BTreeSet::new();
        for (name, _) in &additions {
            if !names.insert(name.clone()) {
                return Err(ToolError::Conflict(format!(
                    "tool generation declares {name:?} more than once"
                )));
            }
            if tools.contains_key(name) && !remove.contains(name) {
                return Err(ToolError::Conflict(format!(
                    "tool already registered outside this generation: {name}"
                )));
            }
        }
        for name in remove {
            tools.remove(name);
        }
        for (name, component) in additions {
            tools.insert(
                name,
                RegisteredTool {
                    component,
                    enabled: true,
                },
            );
        }
        Ok(())
    }

    /// Temporarily remove a tool from the executable surface while retaining
    /// its component generation for a later availability transition.
    pub fn disable(&self, name: &str) -> bool {
        let mut tools = self.tools.write().unwrap();
        let Some(tool) = tools.get_mut(name) else {
            return false;
        };
        let changed = tool.enabled;
        tool.enabled = false;
        changed
    }

    pub fn enable(&self, name: &str) -> bool {
        let mut tools = self.tools.write().unwrap();
        let Some(tool) = tools.get_mut(name) else {
            return false;
        };
        let changed = !tool.enabled;
        tool.enabled = true;
        changed
    }

    pub fn names(&self) -> Vec<String> {
        self.tools
            .read()
            .unwrap()
            .iter()
            .filter_map(|(name, tool)| tool.enabled.then_some(name.clone()))
            .collect()
    }

    pub fn all_names(&self) -> Vec<String> {
        self.tools.read().unwrap().keys().cloned().collect()
    }

    pub fn definitions(&self) -> Vec<llm_provider::ToolDefinition> {
        self.tools
            .read()
            .unwrap()
            .values()
            .filter(|tool| tool.enabled)
            .map(|tool| tool.component.definition())
            .collect()
    }

    pub async fn invoke(&self, name: &str, request: &[u8]) -> Result<Vec<u8>, ToolError> {
        let (component, enabled) = {
            let tools = self.tools.read().unwrap();
            let tool = tools
                .get(name)
                .ok_or_else(|| ToolError::NotFound(name.to_owned()))?;
            (Arc::clone(&tool.component), tool.enabled)
        };
        if !enabled {
            return Err(ToolError::Unavailable(name.to_owned()));
        }
        component.invoke(request).await
    }

    pub async fn invoke_enveloped(&self, name: &str, request: &[u8]) -> Vec<u8> {
        let envelope = match self.invoke(name, request).await {
            Ok(bytes) => {
                let output = serde_json::from_slice::<Value>(&bytes)
                    .or_else(|_| {
                        std::str::from_utf8(&bytes)
                            .map_err(|error| anyhow!(error.to_string()))
                            .and_then(|text| {
                                toon_format::decode_default(text)
                                    .map_err(|error| anyhow!(error.to_string()))
                            })
                    })
                    .unwrap_or_else(|_| String::from_utf8_lossy(&bytes).into_owned().into());
                ToolResultEnvelope::success(output)
            }
            Err(error) => ToolResultEnvelope::failure(&error),
        };
        serde_json::to_vec(&envelope).expect("tool result envelope is serializable")
    }
}

/// Adapts a live WASM tool generation into the session registry.
pub struct WasmToolComponent {
    tool: WasmTool,
    definition: llm_provider::ToolDefinition,
}

impl WasmToolComponent {
    pub fn new(tool: WasmTool, definition: llm_provider::ToolDefinition) -> Self {
        Self { tool, definition }
    }
}

#[async_trait]
impl ToolComponent for WasmToolComponent {
    fn name(&self) -> &str {
        self.tool.name()
    }

    fn definition(&self) -> llm_provider::ToolDefinition {
        self.definition.clone()
    }

    async fn invoke(&self, request: &[u8]) -> Result<Vec<u8>, ToolError> {
        self.tool.invoke(request).await.map_err(map_wasm_error)
    }
}

fn map_wasm_error(error: WasmToolError) -> ToolError {
    match error {
        WasmToolError::InvalidArgument => {
            ToolError::InvalidArgument("tool rejected request".into())
        }
        WasmToolError::NotFound => ToolError::NotFound("tool target not found".into()),
        WasmToolError::Unsupported => ToolError::Unsupported("tool operation unsupported".into()),
        WasmToolError::PermissionDenied => {
            ToolError::PermissionDenied("tool operation denied".into())
        }
        WasmToolError::Conflict => ToolError::Conflict("tool operation conflicted".into()),
        WasmToolError::Aborted => ToolError::Aborted("tool operation aborted".into()),
        WasmToolError::Internal => ToolError::Internal("tool component failed".into()),
        WasmToolError::Unavailable => ToolError::Unavailable("tool component unavailable".into()),
        WasmToolError::Detailed {
            code,
            message,
            details,
        } => ToolError::Detailed {
            code,
            message,
            details: details
                .and_then(|details| serde_json::from_str(&details).ok())
                .unwrap_or(Value::Null),
        },
    }
}

/// A component registry error can be converted into an anyhow error at
/// process boundaries without exposing the kernel's legacy verb algebra.
pub fn tool_error(error: ToolError) -> anyhow::Error {
    anyhow!(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait]
    impl ToolComponent for Echo {
        fn name(&self) -> &str {
            "echo"
        }
        fn definition(&self) -> llm_provider::ToolDefinition {
            llm_provider::ToolDefinition {
                name: "echo".into(),
                description: None,
                input_schema: serde_json::json!({"type":"object"}),
            }
        }
        async fn invoke(&self, request: &[u8]) -> Result<Vec<u8>, ToolError> {
            Ok(request.to_vec())
        }
    }

    struct ToonResult;

    #[async_trait]
    impl ToolComponent for ToonResult {
        fn name(&self) -> &str {
            "toon-result"
        }

        fn definition(&self) -> llm_provider::ToolDefinition {
            llm_provider::ToolDefinition {
                name: "toon-result".into(),
                description: None,
                input_schema: serde_json::json!({"type":"object"}),
            }
        }

        async fn invoke(&self, _request: &[u8]) -> Result<Vec<u8>, ToolError> {
            Ok(toon_format::encode_default(&serde_json::json!({
                "structured": true,
                "items": [1, 2]
            }))
            .unwrap()
            .into_bytes())
        }
    }

    #[tokio::test]
    async fn registry_is_live_and_session_local() {
        let registry = ComponentToolRegistry::new();
        registry.register(Echo).unwrap();
        assert!(matches!(
            registry.register(Echo),
            Err(ToolError::Conflict(_))
        ));
        assert_eq!(registry.names(), vec!["echo"]);
        assert_eq!(registry.invoke("echo", b"x").await.unwrap(), b"x");
        assert!(registry.unregister("echo"));
        assert!(matches!(
            registry.invoke("echo", b"").await,
            Err(ToolError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn successful_toon_results_remain_structured() {
        let registry = ComponentToolRegistry::new();
        registry.register(ToonResult).unwrap();
        let envelope: ToolResultEnvelope =
            serde_json::from_slice(&registry.invoke_enveloped("toon-result", b"{}").await).unwrap();
        assert_eq!(
            envelope.output,
            Some(serde_json::json!({
                "structured": true,
                "items": [1, 2]
            }))
        );
        assert!(envelope.ok);
    }
}
