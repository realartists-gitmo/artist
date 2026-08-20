//! Provider-neutral model contracts.
//!
//! Concrete providers belong in adapters behind [`ModelProvider`]. The agent
//! talks only in these types, so provider-specific authentication, request
//! formats, streaming quirks, and error vocabularies do not spread through the
//! rest of Artist.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use futures_core::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub mod codec;
pub mod openai;

pub use codec::{StandardToolPayloadCodec, ToolPayloadCodec, ToolPayloadError, ToolWireMode};
pub use openai::{OpenAiProvider, OpenAiRetryPolicy};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    Image {
        mime_type: String,
        data_base64: String,
    },
    Resource {
        uri: String,
        mime_type: Option<String>,
    },
    ProviderExtension {
        provider: String,
        value: Value,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentPart>,
    pub name: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub tool_results: Vec<ToolResult>,
}

impl Message {
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![ContentPart::Text { text: text.into() }],
            name: None,
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    /// Provider-neutral structured schema. Adapters translate it to their
    /// native schema representation.
    pub input_schema: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ToolResult {
    pub call_id: String,
    pub content: Vec<ContentPart>,
    pub is_error: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct ModelCapabilities {
    pub streaming: bool,
    pub tool_calls: bool,
    /// Tool payload modes supported by this provider/model adapter.
    pub tool_wire_modes: Vec<ToolWireMode>,
    pub structured_output: bool,
    pub vision: bool,
    pub reasoning: bool,
    pub cancellation: bool,
}

impl ModelCapabilities {
    pub fn supports_tool_mode(&self, mode: ToolWireMode) -> bool {
        self.tool_wire_modes.contains(&mode)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
    pub metadata: Value,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ModelResponse {
    pub content: Vec<ContentPart>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub usage: Usage,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelEvent {
    Started,
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ToolCallDelta {
        id: String,
        name: Option<String>,
        arguments_delta: String,
    },
    Usage {
        usage: Usage,
    },
    Completed {
        response: ModelResponse,
    },
}

#[derive(Debug, Error, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum ProviderError {
    #[error("provider authentication failed: {message}")]
    Authentication { message: String },
    #[error("provider rate limit: {message}")]
    RateLimited {
        message: String,
        retry_after_ms: Option<u64>,
    },
    #[error("provider request failed: {message}")]
    Request { message: String },
    #[error("provider returned an invalid response: {message}")]
    InvalidResponse { message: String },
    #[error("provider operation was cancelled")]
    Cancelled,
    #[error("provider does not support {feature}")]
    Unsupported { feature: String },
}

pub type ModelEventStream<'a> =
    Pin<Box<dyn Stream<Item = Result<ModelEvent, ProviderError>> + Send + 'a>>;

/// The only model boundary required by the agent core.
pub trait ModelProvider: Send + Sync {
    fn provider_name(&self) -> &str;
    fn capabilities(&self, model: &str) -> ModelCapabilities;

    /// Register the session's formal/static tool set when the provider has a
    /// persistent registration API (for example a Rig-backed adapter). APIs
    /// whose tool declarations are request-scoped may leave the default no-op;
    /// the same set is still carried by [`ModelRequest::tools`].
    fn register_formal_tools(
        &self,
        _model: &str,
        _tools: &[ToolDefinition],
    ) -> Result<(), ProviderError> {
        Ok(())
    }

    fn stream<'a>(&'a self, request: ModelRequest) -> ModelEventStream<'a>;
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProviderRegistryError {
    #[error("provider {0:?} is already registered")]
    Duplicate(String),
    #[error("provider {0:?} is not registered")]
    Unknown(String),
}

/// Runtime provider catalog. The agent sees only the selected trait object;
/// provider-specific request/response translation stays behind each entry.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: RwLock<BTreeMap<String, Arc<dyn ModelProvider>>>,
}

impl ProviderRegistry {
    pub fn register(&self, provider: Arc<dyn ModelProvider>) -> Result<(), ProviderRegistryError> {
        let name = provider.provider_name().to_string();
        let mut providers = self.providers.write().unwrap();
        if providers.contains_key(&name) {
            return Err(ProviderRegistryError::Duplicate(name));
        }
        providers.insert(name, provider);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn ModelProvider>, ProviderRegistryError> {
        self.providers
            .read()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| ProviderRegistryError::Unknown(name.into()))
    }

    pub fn names(&self) -> Vec<String> {
        self.providers.read().unwrap().keys().cloned().collect()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ObservationInput {
    pub uri: Option<String>,
    pub mime_type: Option<String>,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StandardObservation {
    pub text: String,
    pub source_uri: Option<String>,
    pub truncated: bool,
}

/// Model-facing observation formatting is separate from authoritative resource
/// bytes. Tool/resource implementations can provide deterministic renderers
/// without changing the kernel or provider contracts.
pub trait ObservationRenderer: Send + Sync {
    fn render(&self, input: ObservationInput) -> StandardObservation;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_and_tool_calls_round_trip() {
        let request = ModelRequest {
            model: "example-model".into(),
            messages: vec![Message::text(Role::User, "hello")],
            tools: vec![ToolDefinition {
                name: "read".into(),
                description: Some("Read a resource".into()),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            temperature: None,
            max_output_tokens: Some(128),
            metadata: serde_json::json!({"request_id": "r1"}),
        };
        let encoded = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<ModelRequest>(&encoded).unwrap(),
            request
        );
    }

    #[test]
    fn provider_extensions_preserve_unknown_provider_data() {
        let part = ContentPart::ProviderExtension {
            provider: "example".into(),
            value: serde_json::json!({"latency_ms": 4}),
        };
        let encoded = serde_json::to_string(&part).unwrap();
        assert_eq!(serde_json::from_str::<ContentPart>(&encoded).unwrap(), part);
    }

    struct NamedProvider(&'static str);

    impl ModelProvider for NamedProvider {
        fn provider_name(&self) -> &str {
            self.0
        }

        fn capabilities(&self, _model: &str) -> ModelCapabilities {
            Default::default()
        }

        fn stream<'a>(&'a self, _request: ModelRequest) -> ModelEventStream<'a> {
            Box::pin(futures_util::stream::empty::<
                Result<ModelEvent, ProviderError>,
            >())
        }
    }

    #[test]
    fn registry_supports_multiple_named_providers_without_leaking_adapters() {
        let registry = ProviderRegistry::default();
        registry.register(Arc::new(NamedProvider("one"))).unwrap();
        registry.register(Arc::new(NamedProvider("two"))).unwrap();
        assert_eq!(registry.names(), vec!["one", "two"]);
        assert_eq!(registry.get("two").unwrap().provider_name(), "two");
        assert!(matches!(
            registry.register(Arc::new(NamedProvider("one"))),
            Err(ProviderRegistryError::Duplicate(_))
        ));
    }
}
