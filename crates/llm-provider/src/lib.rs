//! Provider-neutral model contracts.
//!
//! Concrete providers belong in adapters behind [`ModelProvider`]. The agent
//! talks only in these types, so provider-specific authentication, request
//! formats, streaming quirks, and error vocabularies do not spread through the
//! rest of Artist.

use std::collections::BTreeMap;
use std::fmt;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use futures_core::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub mod catalog;
pub mod codec;
pub mod openai;

pub use catalog::ProviderCatalog;
pub use codec::{StandardToolPayloadCodec, ToolPayloadCodec, ToolPayloadError, ToolWireMode};
pub use openai::{OpenAiProvider, OpenAiRetryPolicy};
pub use openai::{ResolvedResource, ResourceResolver};

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
    /// Provider-neutral structured-output request. Provider components may
    /// translate this into their native response-format contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
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
    #[serde(default)]
    pub refusal: Option<String>,
    #[serde(default)]
    pub incomplete: bool,
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
    RefusalDelta {
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

/// Stable identity for a configured provider instance. It is intentionally
/// separate from a display name or provider type so two accounts/endpoints
/// can coexist and a session can replay the exact selected configuration.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderConfigId(String);

impl ProviderConfigId {
    pub fn new(value: impl Into<String>) -> Result<Self, ProviderConfigError> {
        let value = value.into();
        if value.trim().is_empty()
            || value.contains('/')
            || value.contains('\\')
            || value.contains('\0')
        {
            return Err(ProviderConfigError::InvalidId(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderConfigId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderAuth {
    None,
    ApiKey {
        secret: String,
    },
    ChatGptOAuth {
        access_token: String,
        refresh_token: Option<String>,
        expires_at_ms: Option<u64>,
    },
}

impl ProviderAuth {
    pub fn redacted_kind(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ApiKey { .. } => "api_key",
            Self::ChatGptOAuth { .. } => "chatgpt_oauth",
        }
    }
}

impl fmt::Debug for ProviderAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderAuth")
            .field("kind", &self.redacted_kind())
            .finish()
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
pub struct ProviderConfig {
    pub id: ProviderConfigId,
    pub provider_type: String,
    pub endpoint: Option<String>,
    pub default_model: Option<String>,
    #[serde(default)]
    pub options: Value,
    pub auth: ProviderAuth,
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("id", &self.id)
            .field("provider_type", &self.provider_type)
            .field("endpoint", &self.endpoint)
            .field("default_model", &self.default_model)
            .field("options", &redacted_value(&self.options))
            .field("auth", &self.auth)
            .finish()
    }
}

fn redacted_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let normalized = key.to_ascii_lowercase().replace('-', "_");
                    let redacted = [
                        "api_key",
                        "apikey",
                        "authorization",
                        "access_token",
                        "client_secret",
                        "credential",
                        "password",
                        "refresh_token",
                        "secret",
                        "token",
                    ]
                    .contains(&normalized.as_str());
                    (
                        key.clone(),
                        if redacted {
                            Value::String("[REDACTED]".into())
                        } else {
                            redacted_value(value)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(redacted_value).collect()),
        _ => value.clone(),
    }
}

impl ProviderConfig {
    pub fn validate(&self) -> Result<(), ProviderConfigError> {
        if self.provider_type.trim().is_empty() {
            return Err(ProviderConfigError::Invalid(
                "provider type cannot be empty".into(),
            ));
        }
        if let Some(endpoint) = &self.endpoint {
            let parsed = url::Url::parse(endpoint)
                .map_err(|error| ProviderConfigError::Invalid(error.to_string()))?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(ProviderConfigError::Invalid(
                    "provider endpoint must use HTTP(S)".into(),
                ));
            }
        }
        match &self.auth {
            ProviderAuth::ApiKey { secret } if secret.trim().is_empty() => Err(
                ProviderConfigError::Invalid("API key cannot be empty".into()),
            ),
            ProviderAuth::ChatGptOAuth { access_token, .. } if access_token.trim().is_empty() => {
                Err(ProviderConfigError::Invalid(
                    "OAuth access token cannot be empty".into(),
                ))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProviderConfigError {
    #[error("invalid provider configuration id {0:?}")]
    InvalidId(String),
    #[error("provider configuration {0} already exists")]
    Duplicate(ProviderConfigId),
    #[error("provider configuration {0} is not registered")]
    Unknown(ProviderConfigId),
    #[error("provider type {0:?} is not installed")]
    UnknownType(String),
    #[error("provider configuration is invalid: {0}")]
    Invalid(String),
    #[error("provider configuration authentication failed: {0}")]
    Authentication(String),
}

/// Component-side factory boundary. Authentication, endpoint interpretation,
/// refresh, and provider-specific options stay behind this socket.
pub trait ProviderComponent: Send + Sync {
    fn provider_type(&self) -> &str;
    fn configure(
        &self,
        config: &ProviderConfig,
    ) -> Result<Arc<dyn ModelProvider>, ProviderConfigError>;

    fn configure_with_resource_resolver(
        &self,
        config: &ProviderConfig,
        _resolver: Option<Arc<dyn ResourceResolver>>,
    ) -> Result<Arc<dyn ModelProvider>, ProviderConfigError> {
        self.configure(config)
    }

    fn authenticator(&self, _config: &ProviderConfig) -> Option<Arc<dyn ProviderAuthenticator>> {
        None
    }
}

/// Authentication behavior belongs to the provider component that owns the
/// scheme. The daemon stores only the selected configuration identity and
/// never receives a provider-specific refresh implementation.
#[async_trait::async_trait]
pub trait ProviderAuthenticator: Send + Sync {
    async fn refresh(&self, auth: &ProviderAuth) -> Result<ProviderAuth, ProviderError>;
}

/// Provider types restored as component identities. The catalogue contains
/// identities and truthful baseline capabilities; network implementations are
/// installed by components, with OpenAI supplied by this crate.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProviderKind {
    Anthropic,
    AzureOpenAi,
    ChatGpt,
    Cohere,
    GitHubCopilot,
    DeepSeek,
    Gemini,
    Groq,
    HuggingFace,
    Hyperbolic,
    Llamafile,
    MiniMax,
    Mira,
    Mistral,
    Moonshot,
    Ollama,
    OpenAi,
    OpenRouter,
    Perplexity,
    Together,
    Xai,
    XiaomiMiMo,
    Zai,
}

impl ProviderKind {
    pub const ALL: [Self; 23] = [
        Self::Anthropic,
        Self::AzureOpenAi,
        Self::ChatGpt,
        Self::Cohere,
        Self::GitHubCopilot,
        Self::DeepSeek,
        Self::Gemini,
        Self::Groq,
        Self::HuggingFace,
        Self::Hyperbolic,
        Self::Llamafile,
        Self::MiniMax,
        Self::Mira,
        Self::Mistral,
        Self::Moonshot,
        Self::Ollama,
        Self::OpenAi,
        Self::OpenRouter,
        Self::Perplexity,
        Self::Together,
        Self::Xai,
        Self::XiaomiMiMo,
        Self::Zai,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::AzureOpenAi => "azure_openai",
            Self::ChatGpt => "chatgpt",
            Self::Cohere => "cohere",
            Self::GitHubCopilot => "github_copilot",
            Self::DeepSeek => "deepseek",
            Self::Gemini => "gemini",
            Self::Groq => "groq",
            Self::HuggingFace => "huggingface",
            Self::Hyperbolic => "hyperbolic",
            Self::Llamafile => "llamafile",
            Self::MiniMax => "minimax",
            Self::Mira => "mira",
            Self::Mistral => "mistral",
            Self::Moonshot => "moonshot",
            Self::Ollama => "ollama",
            Self::OpenAi => "openai",
            Self::OpenRouter => "openrouter",
            Self::Perplexity => "perplexity",
            Self::Together => "together",
            Self::Xai => "xai",
            Self::XiaomiMiMo => "xiaomi_mimo",
            Self::Zai => "zai",
        }
    }
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
    #[error("provider context limit exceeded: {message}")]
    ContextLimit { message: String },
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
    configurations: RwLock<BTreeMap<ProviderConfigId, ProviderConfig>>,
    configured_providers: RwLock<BTreeMap<ProviderConfigId, Arc<dyn ModelProvider>>>,
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

    pub fn register_configuration(
        &self,
        config: ProviderConfig,
        provider: Arc<dyn ModelProvider>,
    ) -> Result<(), ProviderConfigError> {
        config.validate()?;
        let mut configurations = self.configurations.write().unwrap();
        if configurations.contains_key(&config.id) {
            return Err(ProviderConfigError::Duplicate(config.id));
        }
        let id = config.id.clone();
        configurations.insert(id.clone(), config);
        drop(configurations);
        // Keep the stable configuration identity separate from the
        // provider-type index: multiple configured accounts may share one
        // display/provider name.
        self.configured_providers
            .write()
            .unwrap()
            .insert(id, Arc::clone(&provider));
        self.providers
            .write()
            .unwrap()
            .entry(provider.provider_name().to_owned())
            .or_insert(provider);
        Ok(())
    }

    pub fn configuration(
        &self,
        id: &ProviderConfigId,
    ) -> Result<ProviderConfig, ProviderConfigError> {
        self.configurations
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| ProviderConfigError::Unknown(id.clone()))
    }

    pub fn configuration_ids(&self) -> Vec<ProviderConfigId> {
        self.configurations
            .read()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    pub fn get_configured(
        &self,
        id: &ProviderConfigId,
    ) -> Result<Arc<dyn ModelProvider>, ProviderConfigError> {
        self.configured_providers
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| ProviderConfigError::Unknown(id.clone()))
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
            structured_output: None,
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
