//! Minimal component socket for provider-neutral context compaction.
//!
//! There is intentionally no host implementation or retention heuristic here.
//! A selected component receives the current context and returns a new
//! provider-neutral context; absence is a valid configuration.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use artist_wasm_verbs::component::WasmTool;
use async_trait::async_trait;
use llm_provider::Message;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CompactionRequest {
    pub context: Vec<Message>,
    pub model: String,
    pub context_limit: Option<u64>,
    pub metadata: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CompactionResponse {
    pub context: Vec<Message>,
    pub metadata: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("compaction component is unavailable: {0}")]
    Unavailable(String),
    #[error("compaction component failed: {0}")]
    Failed(String),
    #[error("context exceeds the provider/component limit")]
    ContextLimit,
}

#[async_trait]
pub trait CompactionComponent: Send + Sync {
    fn resource_id(&self) -> &str;
    async fn compact(
        &self,
        request: CompactionRequest,
    ) -> Result<CompactionResponse, CompactionError>;
}

/// URL-addressed optional socket. The host may select a component by its
/// `compaction://...` or `url://compaction/...` identity without making the
/// component registry a kernel feature branch.
#[derive(Clone, Default)]
pub struct CompactionSocket {
    components: Arc<RwLock<BTreeMap<String, Arc<dyn CompactionComponent>>>>,
}

impl CompactionSocket {
    pub fn new(components: impl IntoIterator<Item = Arc<dyn CompactionComponent>>) -> Self {
        let mut map = BTreeMap::new();
        for component in components {
            map.insert(component.resource_id().to_owned(), component);
        }
        Self {
            components: Arc::new(RwLock::new(map)),
        }
    }

    pub fn register(&self, component: Arc<dyn CompactionComponent>) {
        self.components
            .write()
            .unwrap()
            .insert(component.resource_id().to_owned(), component);
    }

    pub fn replace(&self, components: impl IntoIterator<Item = Arc<dyn CompactionComponent>>) {
        let mut values = BTreeMap::new();
        for component in components {
            values.insert(component.resource_id().to_owned(), component);
        }
        *self.components.write().unwrap() = values;
    }

    pub fn selected(
        &self,
        resource: Option<&str>,
    ) -> Result<Option<Arc<dyn CompactionComponent>>, CompactionError> {
        let components = self.components.read().unwrap();
        let Some(resource) = resource else {
            return match components.values().next().cloned() {
                None => Ok(None),
                Some(component) if components.len() == 1 => Ok(Some(component)),
                Some(_) => Err(CompactionError::Unavailable(
                    "multiple compaction components are active; select a resource URI".into(),
                )),
            };
        };
        components
            .get(resource)
            .cloned()
            .ok_or_else(|| CompactionError::Unavailable(resource.into()))
            .map(Some)
    }

    pub fn resource_ids(&self) -> Vec<String> {
        self.components.read().unwrap().keys().cloned().collect()
    }
}

/// Adapter for a generic component tool exported through the URL graph. The
/// guest owns the compaction algorithm; this adapter only carries the typed
/// socket contract across the existing opaque tool ABI.
pub struct WasmCompactionComponent {
    resource_id: String,
    tool: WasmTool,
}

impl WasmCompactionComponent {
    pub fn new(resource_id: impl Into<String>, tool: WasmTool) -> Self {
        Self {
            resource_id: resource_id.into(),
            tool,
        }
    }
}

#[async_trait]
impl CompactionComponent for WasmCompactionComponent {
    fn resource_id(&self) -> &str {
        &self.resource_id
    }

    async fn compact(
        &self,
        request: CompactionRequest,
    ) -> Result<CompactionResponse, CompactionError> {
        let bytes =
            toon_format::encode_default(&serde_json::to_value(request).map_err(|error| {
                CompactionError::Failed(format!("encode compaction request: {error}"))
            })?)
            .map_err(|error| {
                CompactionError::Failed(format!("encode compaction request: {error}"))
            })?;
        let result = self
            .tool
            .invoke(bytes.as_bytes())
            .await
            .map_err(|error| CompactionError::Failed(format!("{error:?}")))?;
        let text = std::str::from_utf8(&result).map_err(|error| {
            CompactionError::Failed(format!("compaction response is not UTF-8: {error}"))
        })?;
        let value: Value = toon_format::decode_default(text).map_err(|error| {
            CompactionError::Failed(format!("decode compaction response: {error}"))
        })?;
        serde_json::from_value(value).map_err(|error| {
            CompactionError::Failed(format!("invalid compaction response: {error}"))
        })
    }
}
