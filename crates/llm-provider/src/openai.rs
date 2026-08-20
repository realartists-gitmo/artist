//! OpenAI Responses API adapter.
//!
//! The adapter keeps OpenAI's HTTP envelope private and translates the result
//! into the provider-neutral IR. TOON custom tools are the preferred mode;
//! JSON function tools remain available for models or deployments that do not
//! support free-form custom tools.

use std::time::Duration;

use crate::codec::{StandardToolPayloadCodec, ToolPayloadCodec, ToolWireMode};
use crate::{
    ContentPart, Message, ModelCapabilities, ModelEvent, ModelEventStream, ModelProvider,
    ModelRequest, ModelResponse, ProviderError, Role, ToolCall, ToolDefinition, Usage,
};
use reqwest::Client;
use serde_json::{Map, Value, json};

const DEFAULT_ENDPOINT: &str = "https://api.openai.com/v1/responses";

#[derive(Clone, Debug)]
pub struct OpenAiRetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for OpenAiRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(8),
        }
    }
}

impl OpenAiRetryPolicy {
    fn delay_for(&self, attempt: u32) -> Duration {
        let multiplier = 2u32.saturating_pow(attempt.saturating_sub(1));
        self.base_delay
            .saturating_mul(multiplier)
            .min(self.max_delay)
    }
}

/// OpenAI Responses API provider. The API key is intentionally not exposed in
/// `Debug` output or serialized state.
#[derive(Clone)]
pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    endpoint: String,
    retry: OpenAiRetryPolicy,
    tool_mode: ToolWireMode,
}

impl OpenAiProvider {
    pub fn new(api_key: impl Into<String>) -> Result<Self, ProviderError> {
        Self::with_client(Client::new(), api_key)
    }

    pub fn from_env() -> Result<Self, ProviderError> {
        let key = std::env::var("OPENAI_API_KEY").map_err(|_| ProviderError::Authentication {
            message: "OPENAI_API_KEY is not set".into(),
        })?;
        Self::new(key)
    }

    pub fn with_client(client: Client, api_key: impl Into<String>) -> Result<Self, ProviderError> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(ProviderError::Authentication {
                message: "OpenAI API key cannot be empty".into(),
            });
        }
        Ok(Self {
            client,
            api_key,
            endpoint: DEFAULT_ENDPOINT.into(),
            retry: Default::default(),
            tool_mode: ToolWireMode::ToonCustom,
        })
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Result<Self, ProviderError> {
        let endpoint = endpoint.into();
        if !endpoint.starts_with("https://") && !endpoint.starts_with("http://") {
            return Err(ProviderError::Request {
                message: "OpenAI endpoint must be an HTTP(S) URL".into(),
            });
        }
        self.endpoint = endpoint;
        Ok(self)
    }

    pub fn with_retry_policy(mut self, retry: OpenAiRetryPolicy) -> Result<Self, ProviderError> {
        if retry.max_attempts == 0 {
            return Err(ProviderError::Request {
                message: "OpenAI retry max_attempts must be greater than zero".into(),
            });
        }
        self.retry = retry;
        Ok(self)
    }

    pub const fn with_tool_mode(mut self, mode: ToolWireMode) -> Self {
        self.tool_mode = mode;
        self
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn tool_mode(&self) -> ToolWireMode {
        self.tool_mode
    }

    fn request_body(&self, request: &ModelRequest) -> Result<Value, ProviderError> {
        let codec = StandardToolPayloadCodec::new(self.tool_mode);
        let tools = request
            .tools
            .iter()
            .map(|tool| self.encode_tool(tool, &codec))
            .collect::<Result<Vec<_>, _>>()?;
        let mut body = Map::new();
        body.insert("model".into(), Value::String(request.model.clone()));
        body.insert(
            "input".into(),
            Value::Array(self.encode_messages(&request.messages)?),
        );
        body.insert("stream".into(), Value::Bool(false));
        if !tools.is_empty() {
            body.insert("tools".into(), Value::Array(tools));
        }
        if let Some(temperature) = request.temperature {
            body.insert("temperature".into(), json!(temperature));
        }
        if let Some(max_output_tokens) = request.max_output_tokens {
            body.insert("max_output_tokens".into(), json!(max_output_tokens));
        }
        if !request.metadata.is_null() {
            body.insert("metadata".into(), request.metadata.clone());
        }
        Ok(Value::Object(body))
    }

    fn encode_tool(
        &self,
        tool: &ToolDefinition,
        codec: &StandardToolPayloadCodec,
    ) -> Result<Value, ProviderError> {
        let description = match (&tool.description, self.tool_mode) {
            (Some(description), ToolWireMode::ToonCustom) => format!(
                "{description}\n\nInput contract (TOON):\n{}",
                codec.encode_schema(tool).map_err(codec_error)?
            ),
            (None, ToolWireMode::ToonCustom) => format!(
                "Input contract (TOON):\n{}",
                codec.encode_schema(tool).map_err(codec_error)?
            ),
            (description, ToolWireMode::JsonFunction) => description.clone().unwrap_or_default(),
        };
        Ok(match self.tool_mode {
            ToolWireMode::ToonCustom => json!({
                "type": "custom",
                "name": tool.name,
                "description": description,
            }),
            ToolWireMode::JsonFunction => json!({
                "type": "function",
                "name": tool.name,
                "description": description,
                "parameters": tool.input_schema,
                "strict": true,
            }),
        })
    }

    fn encode_messages(&self, messages: &[Message]) -> Result<Vec<Value>, ProviderError> {
        let codec = StandardToolPayloadCodec::new(self.tool_mode);
        messages
            .iter()
            .map(|message| -> Result<Value, ProviderError> {
                let role = match message.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => "user",
                };
                let mut content = String::new();
                for part in &message.content {
                    if let ContentPart::Text { text } = part {
                        if !content.is_empty() {
                            content.push('\n');
                        }
                        content.push_str(text);
                    }
                }
                for result in &message.tool_results {
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str(&format!(
                        "tool_result {}:\n{}",
                        result.call_id,
                        codec.encode_output(result).map_err(codec_error)?
                    ));
                }
                Ok(json!({"role": role, "content": content}))
            })
            .collect()
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ProviderError> {
        let body = self.request_body(&request)?;
        let mut attempt = 1;
        loop {
            let response = match self
                .client
                .post(&self.endpoint)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
            {
                Ok(response) => response,
                Err(_error) if attempt < self.retry.max_attempts => {
                    tokio::time::sleep(self.retry.delay_for(attempt)).await;
                    attempt += 1;
                    continue;
                }
                Err(error) => {
                    return Err(ProviderError::Request {
                        message: error.to_string(),
                    });
                }
            };
            let status = response.status();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(|seconds| seconds.saturating_mul(1000));
            let bytes = response
                .bytes()
                .await
                .map_err(|error| ProviderError::Request {
                    message: error.to_string(),
                })?;
            if status.is_success() {
                return parse_response(&bytes, self.tool_mode);
            }
            let message = String::from_utf8_lossy(&bytes).into_owned();
            if (status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
                && attempt < self.retry.max_attempts
            {
                let delay = retry_after
                    .map(Duration::from_millis)
                    .unwrap_or_else(|| self.retry.delay_for(attempt));
                tokio::time::sleep(delay).await;
                attempt += 1;
                continue;
            }
            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(ProviderError::Authentication { message });
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(ProviderError::RateLimited {
                    message,
                    retry_after_ms: retry_after,
                });
            }
            return Err(ProviderError::Request {
                message: format!("HTTP {status}: {message}"),
            });
        }
    }
}

impl ModelProvider for OpenAiProvider {
    fn provider_name(&self) -> &str {
        "openai"
    }

    fn capabilities(&self, _model: &str) -> ModelCapabilities {
        ModelCapabilities {
            streaming: false,
            tool_calls: true,
            tool_wire_modes: vec![self.tool_mode],
            structured_output: true,
            vision: true,
            reasoning: true,
            cancellation: true,
        }
    }

    fn stream<'a>(&'a self, request: ModelRequest) -> ModelEventStream<'a> {
        Box::pin(async_stream::try_stream! {
            yield ModelEvent::Started;
            let response = self.complete(request).await?;
            yield ModelEvent::Completed { response };
        })
    }
}

fn codec_error(error: crate::ToolPayloadError) -> ProviderError {
    ProviderError::Request {
        message: error.to_string(),
    }
}

fn parse_response(bytes: &[u8], mode: ToolWireMode) -> Result<ModelResponse, ProviderError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| ProviderError::InvalidResponse {
            message: error.to_string(),
        })?;
    let mut content = Vec::new();
    let mut tool_calls = Vec::new();
    let codec = StandardToolPayloadCodec::new(mode);
    if let Some(output) = value.get("output").and_then(Value::as_array) {
        for item in output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    if let Some(parts) = item.get("content").and_then(Value::as_array) {
                        for part in parts {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                content.push(ContentPart::Text { text: text.into() });
                            }
                        }
                    }
                }
                Some("function_call") => {
                    let arguments = item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid_response("function call arguments missing"))?;
                    let arguments = serde_json::from_str(arguments)
                        .map_err(|error| invalid_response(error.to_string()))?;
                    tool_calls.push(ToolCall {
                        id: item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                        name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                        arguments,
                    });
                }
                Some("custom_tool_call") => {
                    let input = item
                        .get("input")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid_response("custom tool input missing"))?;
                    let arguments = codec
                        .decode_input(input)
                        .map_err(|error| invalid_response(error.to_string()))?;
                    tool_calls.push(ToolCall {
                        id: item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                        name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                        arguments,
                    });
                }
                _ => {}
            }
        }
    }
    let usage = value.get("usage").map(parse_usage).unwrap_or_default();
    Ok(ModelResponse {
        content,
        tool_calls,
        finish_reason: value
            .get("status")
            .and_then(Value::as_str)
            .map(String::from),
        usage,
    })
}

fn parse_usage(value: &Value) -> Usage {
    Usage {
        input_tokens: value.get("input_tokens").and_then(Value::as_u64),
        output_tokens: value.get("output_tokens").and_then(Value::as_u64),
        total_tokens: value.get("total_tokens").and_then(Value::as_u64),
    }
}

fn invalid_response(message: impl Into<String>) -> ProviderError {
    ProviderError::InvalidResponse {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Role;

    #[test]
    fn custom_tool_request_uses_toon_contract_text() {
        let provider = OpenAiProvider::new("test-key").unwrap();
        let request = ModelRequest {
            model: "gpt-test".into(),
            messages: vec![Message::text(Role::User, "read it")],
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: Some("Read a file".into()),
                input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
            }],
            temperature: None,
            max_output_tokens: Some(10),
            metadata: Value::Null,
        };
        let body = provider.request_body(&request).unwrap();
        assert_eq!(body["tools"][0]["type"], "custom");
        assert!(
            body["tools"][0]["description"]
                .as_str()
                .unwrap()
                .contains("type:")
        );
    }

    #[test]
    fn response_parser_normalizes_custom_toon_call() {
        let response = json!({
            "status": "completed",
            "output": [{"type":"custom_tool_call","id":"call-1","name":"read_file","input":"path: src/lib.rs"}],
            "usage": {"input_tokens": 2, "output_tokens": 3, "total_tokens": 5}
        });
        let parsed =
            parse_response(response.to_string().as_bytes(), ToolWireMode::ToonCustom).unwrap();
        assert_eq!(parsed.tool_calls[0].arguments["path"], "src/lib.rs");
        assert_eq!(parsed.usage.total_tokens, Some(5));
    }

    #[test]
    fn custom_tool_results_are_rendered_as_toon() {
        let provider = OpenAiProvider::new("test-key").unwrap();
        let request = ModelRequest {
            model: "gpt-test".into(),
            messages: vec![Message {
                role: Role::Tool,
                content: Vec::new(),
                name: None,
                tool_calls: Vec::new(),
                tool_results: vec![crate::ToolResult {
                    call_id: "call-1".into(),
                    content: vec![ContentPart::Text { text: "ok".into() }],
                    is_error: false,
                }],
            }],
            tools: Vec::new(),
            temperature: None,
            max_output_tokens: None,
            metadata: Value::Null,
        };
        let body = provider.request_body(&request).unwrap();
        let content = body["input"][0]["content"].as_str().unwrap();
        assert!(content.contains("tool_result call-1:"));
        assert!(content.contains("call_id:"));
    }

    #[test]
    fn retry_policy_is_bounded_and_exponential() {
        let policy = OpenAiRetryPolicy::default();
        assert_eq!(policy.delay_for(1), Duration::from_millis(250));
        assert_eq!(policy.delay_for(2), Duration::from_millis(500));
        assert_eq!(policy.delay_for(20), Duration::from_secs(8));
    }
}
