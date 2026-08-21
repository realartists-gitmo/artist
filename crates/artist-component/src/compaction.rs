//! Minimal component socket for provider-neutral context compaction.
//!
//! There is intentionally no host implementation or retention heuristic here.
//! A selected component receives the current context and returns a new
//! provider-neutral context; absence is a valid configuration.

use std::collections::BTreeMap;
use std::sync::Arc;

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
    components: Arc<BTreeMap<String, Arc<dyn CompactionComponent>>>,
}

impl CompactionSocket {
    pub fn new(components: impl IntoIterator<Item = Arc<dyn CompactionComponent>>) -> Self {
        let mut map = BTreeMap::new();
        for component in components {
            map.insert(component.resource_id().to_owned(), component);
        }
        Self {
            components: Arc::new(map),
        }
    }

    pub fn selected(
        &self,
        resource: Option<&str>,
    ) -> Result<Option<Arc<dyn CompactionComponent>>, CompactionError> {
        let Some(resource) = resource else {
            return Ok(None);
        };
        self.components
            .get(resource)
            .cloned()
            .ok_or_else(|| CompactionError::Unavailable(resource.into()))
            .map(Some)
    }

    pub fn resource_ids(&self) -> Vec<String> {
        self.components.keys().cloned().collect()
    }
}
