//! OpenAI Responses API provider component.
//!
//! This adapter owns the OpenAI wire format, including SSE streaming,
//! Responses API tool items, image inputs, refusal/incomplete states, and
//! HTTP error mapping. The rest of Artist sees only the provider-neutral IR.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::codec::{StandardToolPayloadCodec, ToolPayloadCodec, ToolWireMode};
use crate::{
    ContentPart, Message, ModelCapabilities, ModelEvent, ModelEventStream, ModelProvider,
    ModelRequest, ModelResponse, ProviderError, Role, ToolCall, ToolDefinition, Usage,
};
use async_trait::async_trait;
use base64::Engine;
use futures_util::StreamExt;
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedResource {
    pub mime_type: Option<String>,
    pub bytes: Vec<u8>,
}

/// Optional component/resource socket used to resolve a resource reference at
/// provider-call time. A missing resolver preserves the URI as text rather
/// than silently dropping it.
#[async_trait]
pub trait ResourceResolver: Send + Sync {
    async fn resolve(&self, uri: &str) -> Result<ResolvedResource, ProviderError>;
}

/// OpenAI Responses API provider. Credentials are intentionally omitted from
/// Debug output and never enter request/event payloads.
#[derive(Clone)]
pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    endpoint: String,
    retry: OpenAiRetryPolicy,
    tool_mode: ToolWireMode,
    resource_resolver: Option<Arc<dyn ResourceResolver>>,
}

impl std::fmt::Debug for OpenAiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiProvider")
            .field("endpoint", &self.endpoint)
            .field("retry", &self.retry)
            .field("tool_mode", &self.tool_mode)
            .field("has_resource_resolver", &self.resource_resolver.is_some())
            .finish()
    }
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
            resource_resolver: None,
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

    pub fn with_resource_resolver(mut self, resolver: Arc<dyn ResourceResolver>) -> Self {
        self.resource_resolver = Some(resolver);
        self
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn tool_mode(&self) -> ToolWireMode {
        self.tool_mode
    }

    /// Synchronous fixture/diagnostic encoding. Resource references remain
    /// explicit here; the live stream path resolves them asynchronously.
    #[cfg(test)]
    fn request_body(&self, request: &ModelRequest) -> Result<Value, ProviderError> {
        self.request_body_from_messages(request, &request.messages, true)
    }

    fn request_body_from_messages(
        &self,
        request: &ModelRequest,
        messages: &[Message],
        include_metadata: bool,
    ) -> Result<Value, ProviderError> {
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
            Value::Array(self.encode_messages(messages, &codec)?),
        );
        body.insert("stream".into(), Value::Bool(true));
        if !tools.is_empty() {
            body.insert("tools".into(), Value::Array(tools));
        }
        if let Some(temperature) = request.temperature {
            body.insert("temperature".into(), json!(temperature));
        }
        if let Some(max_output_tokens) = request.max_output_tokens {
            body.insert("max_output_tokens".into(), json!(max_output_tokens));
        }
        if let Some(structured_output) = &request.structured_output {
            body.insert("text".into(), json!({"format": structured_output}));
        }
        if include_metadata && !request.metadata.is_null() {
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

    fn encode_messages(
        &self,
        messages: &[Message],
        codec: &StandardToolPayloadCodec,
    ) -> Result<Vec<Value>, ProviderError> {
        let mut encoded = Vec::new();
        for message in messages {
            if message.role == Role::Tool {
                for result in &message.tool_results {
                    let output = codec.encode_output(result).map_err(codec_error)?;
                    encoded.push(match self.tool_mode {
                        ToolWireMode::JsonFunction => json!({
                            "type": "function_call_output",
                            "call_id": required(&result.call_id, "tool result call_id")?,
                            "output": output,
                        }),
                        ToolWireMode::ToonCustom => json!({
                            "type": "custom_tool_call_output",
                            "call_id": required(&result.call_id, "tool result call_id")?,
                            "output": output,
                        }),
                    });
                }
                continue;
            }

            let role = match message.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => unreachable!(),
            };
            let input_kind = if message.role == Role::Assistant {
                "output_text"
            } else {
                "input_text"
            };
            let mut parts = Vec::new();
            let mut continuation_items = Vec::new();
            for part in &message.content {
                match part {
                    ContentPart::Text { text } => {
                        parts.push(json!({"type": input_kind, "text": text}));
                    }
                    ContentPart::Image {
                        mime_type,
                        data_base64,
                    } => {
                        parts.push(json!({
                            "type": "input_image",
                            "image_url": format!("data:{mime_type};base64,{data_base64}"),
                        }));
                    }
                    ContentPart::Resource { uri, mime_type } => {
                        // The live path replaces resolved resources before
                        // this encoder. Keeping the URI here is lossless for
                        // fixtures and makes unsupported resolution visible.
                        parts.push(json!({
                            "type": input_kind,
                            "text": format!("resource: {uri} ({})", mime_type.as_deref().unwrap_or("unknown MIME")),
                        }));
                    }
                    ContentPart::ProviderExtension { provider, value } => {
                        if provider == "openai"
                            && value.get("type").and_then(Value::as_str) == Some("reasoning")
                        {
                            continuation_items.push(value.clone());
                        } else if provider == "openai" && value.is_object() {
                            parts.push(value.clone());
                        } else {
                            parts.push(json!({
                                "type": input_kind,
                                "text": format!("provider extension {provider}: {value}"),
                            }));
                        }
                    }
                }
            }
            if !parts.is_empty() {
                let mut item = json!({"role": role, "content": parts});
                if let Some(name) = &message.name {
                    item["name"] = Value::String(name.clone());
                }
                encoded.push(item);
            }
            encoded.extend(continuation_items);
            for call in &message.tool_calls {
                let id = required(&call.id, "assistant tool call id")?;
                let name = required(&call.name, "assistant tool call name")?;
                encoded.push(match self.tool_mode {
                    ToolWireMode::JsonFunction => json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": name,
                        "arguments": serde_json::to_string(&call.arguments).map_err(|error| invalid_response(error.to_string()))?,
                    }),
                    ToolWireMode::ToonCustom => json!({
                        "type": "custom_tool_call",
                        "call_id": id,
                        "name": name,
                        "input": codec.encode_value(&call.arguments).map_err(codec_error)?,
                    }),
                });
            }
        }
        Ok(encoded)
    }

    async fn resolved_messages(&self, messages: &[Message]) -> Result<Vec<Message>, ProviderError> {
        let Some(resolver) = &self.resource_resolver else {
            if messages.iter().any(|message| {
                message
                    .content
                    .iter()
                    .any(|part| matches!(part, ContentPart::Resource { .. }))
            }) {
                return Err(ProviderError::Unsupported {
                    feature: "resource inputs require an installed resource resolver".into(),
                });
            }
            return Ok(messages.to_vec());
        };
        let mut resolved = Vec::with_capacity(messages.len());
        for message in messages {
            let mut message = message.clone();
            let mut parts = Vec::with_capacity(message.content.len());
            for part in message.content {
                match part {
                    ContentPart::Resource { uri, mime_type } => {
                        let resource = resolver.resolve(&uri).await?;
                        let mime = resource.mime_type.or(mime_type).ok_or_else(|| {
                            ProviderError::Unsupported {
                                feature: format!("resource {uri} has no MIME type"),
                            }
                        })?;
                        let encoded =
                            base64::engine::general_purpose::STANDARD.encode(resource.bytes);
                        if mime.starts_with("image/") {
                            parts.push(ContentPart::Image {
                                mime_type: mime,
                                data_base64: encoded,
                            });
                        } else {
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(encoded)
                                .map_err(|error| invalid_response(error.to_string()))?;
                            let text = String::from_utf8(bytes).map_err(|_| {
                                ProviderError::Unsupported {
                                    feature: format!("non-image binary resource {uri}"),
                                }
                            })?;
                            parts.push(ContentPart::Text { text });
                        }
                    }
                    other => parts.push(other),
                }
            }
            message.content = parts;
            resolved.push(message);
        }
        Ok(resolved)
    }

    async fn send_stream(&self, body: Value) -> Result<reqwest::Response, ProviderError> {
        let mut attempt = 1;
        loop {
            let response = self
                .client
                .post(&self.endpoint)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|error| ProviderError::Request {
                    message: error.to_string(),
                })?;
            let status = response.status();
            if status.is_success() {
                return Ok(response);
            }
            let retry_after = retry_after_ms(&response);
            let bytes = response
                .bytes()
                .await
                .map_err(|error| ProviderError::Request {
                    message: error.to_string(),
                })?;
            let message = error_message(&bytes);
            if (status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
                && attempt < self.retry.max_attempts
            {
                tokio::time::sleep(
                    retry_after
                        .map(Duration::from_millis)
                        .unwrap_or_else(|| self.retry.delay_for(attempt)),
                )
                .await;
                attempt += 1;
                continue;
            }
            return Err(match status {
                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                    ProviderError::Authentication { message }
                }
                reqwest::StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited {
                    message,
                    retry_after_ms: retry_after,
                },
                reqwest::StatusCode::BAD_REQUEST if is_context_limit(&bytes) => {
                    ProviderError::ContextLimit { message }
                }
                _ => ProviderError::Request {
                    message: format!("HTTP {status}: {message}"),
                },
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
            streaming: true,
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
            let messages = self.resolved_messages(&request.messages).await?;
            let body = self.request_body_from_messages(&request, &messages, true)?;
            let response = self.send_stream(body).await?;
            let mut bytes = response.bytes_stream();
            let mut buffer = String::new();
            let mut accumulator = StreamAccumulator::new(self.tool_mode);
            while let Some(chunk) = bytes.next().await {
                let chunk = chunk.map_err(|error| ProviderError::Request {
                    message: error.to_string(),
                })?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(index) = buffer.find('\n') {
                    let line = buffer.drain(..=index).collect::<String>();
                    let line = line.trim_end_matches(['\r', '\n']);
                    let Some(data) = line.strip_prefix("data:") else { continue };
                    let data = data.trim();
                    if data.is_empty() { continue; }
                    if data == "[DONE]" { continue; }
                    if let Some(event) = parse_sse_event(data, &mut accumulator, self.tool_mode)? {
                        yield event;
                    }
                }
            }
            if let Some(event) = buffer
                .trim()
                .strip_prefix("data:")
                .map(|data| parse_sse_event(data.trim(), &mut accumulator, self.tool_mode))
                .transpose()?
                .flatten()
            {
                yield event;
            }
            let response = accumulator.finish()?;
            if response.usage != Usage::default() {
                yield ModelEvent::Usage { usage: response.usage.clone() };
            }
            yield ModelEvent::Completed { response };
        })
    }
}

struct StreamAccumulator {
    mode: ToolWireMode,
    text: String,
    reasoning: String,
    refusal: String,
    tool_calls: BTreeMap<String, PartialToolCall>,
    usage: Usage,
    status: Option<String>,
    completed_response: Option<ModelResponse>,
    terminal: bool,
}

impl StreamAccumulator {
    fn new(mode: ToolWireMode) -> Self {
        Self {
            mode,
            text: String::new(),
            reasoning: String::new(),
            refusal: String::new(),
            tool_calls: BTreeMap::new(),
            usage: Usage::default(),
            status: None,
            completed_response: None,
            terminal: false,
        }
    }
}

#[derive(Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
    custom: bool,
}

impl StreamAccumulator {
    fn finish(self) -> Result<ModelResponse, ProviderError> {
        if !self.terminal {
            return Err(invalid_response(
                "OpenAI SSE stream ended without a terminal response event",
            ));
        }
        if let Some(mut response) = self.completed_response {
            if response.usage == Usage::default() {
                response.usage = self.usage.clone();
            }
            if response.refusal.is_none() && !self.refusal.is_empty() {
                response.refusal = Some(self.refusal.clone());
            }
            if response.continuation_items.is_empty() && !self.reasoning.is_empty() {
                response.continuation_items.push(json!({
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": self.reasoning}],
                }));
            }
            return Ok(response);
        }
        let mut content = Vec::new();
        if !self.text.is_empty() {
            content.push(ContentPart::Text { text: self.text });
        }
        if !self.reasoning.is_empty() {
            content.push(ContentPart::ProviderExtension {
                provider: "openai".into(),
                value: json!({"kind":"reasoning", "text": self.reasoning}),
            });
        }
        let mut tool_calls = Vec::new();
        let codec = StandardToolPayloadCodec::new(self.mode);
        for partial in self.tool_calls.into_values() {
            let id = required(&partial.id, "streamed tool call id")?;
            let name = required(&partial.name, "streamed tool call name")?;
            let arguments = if partial.custom {
                codec
                    .decode_input(&partial.arguments)
                    .map_err(|error| invalid_response(error.to_string()))?
            } else {
                serde_json::from_str(&partial.arguments)
                    .map_err(|error| invalid_response(error.to_string()))?
            };
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
        let refusal = (!self.refusal.is_empty()).then_some(self.refusal);
        Ok(ModelResponse {
            content,
            tool_calls,
            finish_reason: self.status,
            usage: self.usage,
            refusal,
            incomplete: false,
            continuation_items: (!self.reasoning.is_empty())
                .then(|| {
                    vec![json!({
                        "type": "reasoning",
                        "summary": [{"type": "summary_text", "text": self.reasoning}],
                    })]
                })
                .unwrap_or_default(),
        })
    }
}

fn parse_sse_event(
    data: &str,
    accumulator: &mut StreamAccumulator,
    mode: ToolWireMode,
) -> Result<Option<ModelEvent>, ProviderError> {
    let value: Value =
        serde_json::from_str(data).map_err(|error| invalid_response(error.to_string()))?;
    if value.get("error").is_some() || value.get("type") == Some(&Value::String("error".into())) {
        return Err(invalid_response(error_message(data.as_bytes())));
    }
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match event_type {
        "response.output_text.delta" => {
            let text = value
                .get("delta")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_response("text delta missing"))?;
            accumulator.text.push_str(text);
            Ok(Some(ModelEvent::TextDelta { text: text.into() }))
        }
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            let text = value
                .get("delta")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_response("reasoning delta missing"))?;
            accumulator.reasoning.push_str(text);
            Ok(Some(ModelEvent::ReasoningDelta { text: text.into() }))
        }
        "response.refusal.delta" => {
            let text = value
                .get("delta")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_response("refusal delta missing"))?;
            accumulator.refusal.push_str(text);
            Ok(Some(ModelEvent::RefusalDelta { text: text.into() }))
        }
        "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
            let custom = event_type.contains("custom");
            let key = value
                .get("item_id")
                .or_else(|| value.get("call_id"))
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_response("tool-call delta id missing"))?;
            let partial = accumulator.tool_calls.entry(key.into()).or_default();
            partial.custom = custom;
            if let Some(id) = value.get("call_id").and_then(Value::as_str) {
                partial.id = id.into();
            }
            if let Some(name) = value.get("name").and_then(Value::as_str) {
                partial.name = name.into();
            }
            let delta = value
                .get("delta")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_response("tool-call argument delta missing"))?;
            partial.arguments.push_str(delta);
            if partial.id.is_empty() || partial.name.is_empty() {
                return Ok(None);
            }
            Ok(Some(ModelEvent::ToolCallDelta {
                id: partial.id.clone(),
                name: Some(partial.name.clone()),
                arguments_delta: delta.into(),
            }))
        }
        "response.output_item.added" => {
            let Some(item) = value.get("item") else {
                return Ok(None);
            };
            let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
            if kind != "function_call" && kind != "custom_tool_call" {
                return Ok(None);
            }
            let key = item
                .get("id")
                .or_else(|| item.get("call_id"))
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_response("tool-call item id missing"))?;
            let partial = accumulator.tool_calls.entry(key.into()).or_default();
            partial.custom = kind == "custom_tool_call";
            partial.id = item
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into();
            partial.name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into();
            if let Some(arguments) = item.get("arguments").and_then(Value::as_str) {
                partial.arguments = arguments.into();
            }
            if let Some(input) = item.get("input").and_then(Value::as_str) {
                partial.arguments = input.into();
            }
            Ok(None)
        }
        "response.output_item.done" => {
            if let Some(response) = value.get("response") {
                accumulator.completed_response = Some(parse_response_value(response, mode)?);
            } else if let Some(item) = value.get("item") {
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
                if item_type == "function_call" || item_type == "custom_tool_call" {
                    let key = item
                        .get("id")
                        .or_else(|| item.get("call_id"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid_response("tool-call item id missing"))?;
                    let partial = accumulator.tool_calls.entry(key.into()).or_default();
                    partial.custom = item_type == "custom_tool_call";
                    partial.id = item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into();
                    partial.name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into();
                    partial.arguments = item
                        .get("arguments")
                        .or_else(|| item.get("input"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into();
                }
            }
            Ok(None)
        }
        "response.completed" | "response.done" => {
            let response = value.get("response").unwrap_or(&value);
            let parsed = parse_response_value(response, mode)?;
            accumulator.status = parsed.finish_reason.clone();
            accumulator.usage = parsed.usage.clone();
            accumulator.completed_response = Some(parsed);
            accumulator.terminal = true;
            Ok(None)
        }
        "response.usage" => {
            let usage = value.get("usage").map(parse_usage).unwrap_or_default();
            accumulator.usage = usage.clone();
            Ok(Some(ModelEvent::Usage { usage }))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
fn parse_response(bytes: &[u8], mode: ToolWireMode) -> Result<ModelResponse, ProviderError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| invalid_response(error.to_string()))?;
    parse_response_value(&value, mode)
}

fn parse_response_value(value: &Value, mode: ToolWireMode) -> Result<ModelResponse, ProviderError> {
    let mut content = Vec::new();
    let mut tool_calls = Vec::new();
    let mut continuation_items = Vec::new();
    let mut refusal = String::new();
    let codec = StandardToolPayloadCodec::new(mode);
    if let Some(output) = value.get("output").and_then(Value::as_array) {
        for item in output {
            match item.get("type").and_then(Value::as_str) {
                Some("reasoning") => continuation_items.push(item.clone()),
                Some("message") => {
                    if let Some(parts) = item.get("content").and_then(Value::as_array) {
                        for part in parts {
                            match part.get("type").and_then(Value::as_str) {
                                Some("refusal") => refusal.push_str(
                                    part.get("refusal")
                                        .or_else(|| part.get("text"))
                                        .and_then(Value::as_str)
                                        .unwrap_or_default(),
                                ),
                                _ => {
                                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                                        content.push(ContentPart::Text { text: text.into() });
                                    }
                                }
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
                        id: required(
                            item.get("call_id")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                            "function call id",
                        )?,
                        name: required(
                            item.get("name").and_then(Value::as_str).unwrap_or_default(),
                            "function call name",
                        )?,
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
                        id: required(
                            item.get("call_id")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                            "custom tool call id",
                        )?,
                        name: required(
                            item.get("name").and_then(Value::as_str).unwrap_or_default(),
                            "custom tool call name",
                        )?,
                        arguments,
                    });
                }
                _ => {}
            }
        }
    }
    let usage = value.get("usage").map(parse_usage).unwrap_or_default();
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .map(String::from);
    let incomplete =
        status.as_deref() == Some("incomplete") || value.get("incomplete_details").is_some();
    Ok(ModelResponse {
        content,
        tool_calls,
        finish_reason: status,
        usage,
        refusal: (!refusal.is_empty()).then_some(refusal),
        incomplete,
        continuation_items,
    })
}

fn parse_usage(value: &Value) -> Usage {
    Usage {
        input_tokens: value.get("input_tokens").and_then(Value::as_u64),
        output_tokens: value.get("output_tokens").and_then(Value::as_u64),
        total_tokens: value.get("total_tokens").and_then(Value::as_u64),
    }
}

fn required(value: &str, field: &str) -> Result<String, ProviderError> {
    if value.trim().is_empty() {
        return Err(invalid_response(format!("{field} is empty")));
    }
    Ok(value.to_owned())
}

fn retry_after_ms(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(1000))
}

fn error_message(bytes: &[u8]) -> String {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| value.get("error").cloned().or(Some(value)))
        .and_then(|value| {
            value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned())
}

fn is_context_limit(bytes: &[u8]) -> bool {
    let value = serde_json::from_slice::<Value>(bytes).ok();
    let code = value
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    code.contains("context")
        || code.contains("token")
        || error_message(bytes)
            .to_ascii_lowercase()
            .contains("context length")
}

fn codec_error(error: crate::ToolPayloadError) -> ProviderError {
    ProviderError::Request {
        message: error.to_string(),
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
    use futures_util::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn request(messages: Vec<Message>) -> ModelRequest {
        ModelRequest {
            model: "gpt-test".into(),
            messages,
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: Some("Read a file".into()),
                input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
            }],
            temperature: None,
            max_output_tokens: Some(10),
            metadata: Value::Null,
            structured_output: None,
        }
    }

    #[test]
    fn custom_tool_request_uses_streaming_toon_contract_text() {
        let provider = OpenAiProvider::new("test-key").unwrap();
        let body = provider
            .request_body(&request(vec![Message::text(Role::User, "read it")]))
            .unwrap();
        assert_eq!(body["stream"], true);
        assert_eq!(body["tools"][0]["type"], "custom");
        assert!(
            body["tools"][0]["description"]
                .as_str()
                .unwrap()
                .contains("type:")
        );
    }

    #[test]
    fn responses_tool_items_preserve_call_ids_and_images() {
        let provider = OpenAiProvider::new("test-key")
            .unwrap()
            .with_tool_mode(ToolWireMode::JsonFunction);
        let body = provider
            .request_body(&request(vec![
                Message {
                    role: Role::Assistant,
                    content: vec![],
                    name: None,
                    tool_calls: vec![ToolCall {
                        id: "call-1".into(),
                        name: "read_file".into(),
                        arguments: json!({"path":"x"}),
                    }],
                    tool_results: vec![],
                },
                Message {
                    role: Role::Tool,
                    content: vec![],
                    name: None,
                    tool_calls: vec![],
                    tool_results: vec![crate::ToolResult {
                        call_id: "call-1".into(),
                        content: vec![ContentPart::Text { text: "ok".into() }],
                        is_error: false,
                    }],
                },
                Message {
                    role: Role::User,
                    content: vec![ContentPart::Image {
                        mime_type: "image/png".into(),
                        data_base64: "AA==".into(),
                    }],
                    name: None,
                    tool_calls: vec![],
                    tool_results: vec![],
                },
            ]))
            .unwrap();
        assert_eq!(body["input"][1]["type"], "function_call_output");
        assert_eq!(body["input"][1]["call_id"], "call-1");
        assert_eq!(body["input"][2]["content"][0]["type"], "input_image");
    }

    #[test]
    fn malformed_tool_calls_are_rejected() {
        let response = json!({
            "status": "completed",
            "output": [{"type":"function_call","id":"","name":"read_file","arguments":"{}"}]
        });
        assert!(matches!(
            parse_response(response.to_string().as_bytes(), ToolWireMode::JsonFunction),
            Err(ProviderError::InvalidResponse { .. })
        ));
    }

    #[test]
    fn parser_exposes_refusal_and_incomplete_states() {
        let response = json!({
            "status": "incomplete",
            "output": [{"type":"message","content":[{"type":"refusal","refusal":"no"}]}],
            "incomplete_details": {"reason":"max_output_tokens"}
        });
        let parsed =
            parse_response(response.to_string().as_bytes(), ToolWireMode::JsonFunction).unwrap();
        assert_eq!(parsed.refusal.as_deref(), Some("no"));
        assert!(parsed.incomplete);
    }

    #[test]
    fn reasoning_output_items_survive_stateless_continuation() {
        let reasoning = json!({
            "type": "reasoning",
            "id": "rs_123",
            "summary": [{"type": "summary_text", "text": "check the file"}]
        });
        let parsed = parse_response(
            json!({
                "status": "completed",
                "output": [
                    reasoning,
                    {"type":"message","content":[{"type":"output_text","text":"done"}]}
                ]
            })
            .to_string()
            .as_bytes(),
            ToolWireMode::JsonFunction,
        )
        .unwrap();
        assert_eq!(parsed.continuation_items, vec![reasoning.clone()]);

        let provider = OpenAiProvider::new("test-key")
            .unwrap()
            .with_tool_mode(ToolWireMode::JsonFunction);
        let body = provider
            .request_body(&request(vec![Message {
                role: Role::Assistant,
                content: vec![ContentPart::ProviderExtension {
                    provider: "openai".into(),
                    value: reasoning,
                }],
                name: None,
                tool_calls: Vec::new(),
                tool_results: Vec::new(),
            }]))
            .unwrap();
        assert_eq!(body["input"][0]["type"], "reasoning");
        assert_eq!(body["input"][0]["id"], "rs_123");
    }

    #[tokio::test]
    async fn streams_an_actual_responses_api_sse_exchange() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                assert!(read > 0, "client closed before sending its request");
                request.extend_from_slice(&buffer[..read]);
                let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
                    continue;
                };
                let content_length = String::from_utf8_lossy(&request[..end])
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        (name.eq_ignore_ascii_case("content-length"))
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if request.len() >= end + 4 + content_length {
                    break;
                }
            }
            let body = concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":",
                "{\"status\":\"completed\",\"output\":[{\"type\":\"message\",",
                "\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}],",
                "\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
                "data: [DONE]\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            request
        });

        let provider = OpenAiProvider::with_client(Client::new(), "test-key")
            .unwrap()
            .with_endpoint(format!("http://{address}/v1/responses"))
            .unwrap()
            .with_retry_policy(OpenAiRetryPolicy {
                max_attempts: 1,
                base_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
            })
            .unwrap();
        let events: Vec<_> = provider
            .stream(request(vec![Message::text(Role::User, "hello")]))
            .collect()
            .await;
        let request_bytes = server.await.unwrap();
        let request_text = String::from_utf8_lossy(&request_bytes);
        assert!(request_text.contains("\"stream\":true"));
        assert!(matches!(events.first(), Some(Ok(ModelEvent::Started))));
        assert!(
            events.iter().any(|event| {
                matches!(event, Ok(ModelEvent::TextDelta { text }) if text == "hi")
            })
        );
        assert!(events.iter().any(|event| {
            matches!(
                event,
                Ok(ModelEvent::Usage {
                    usage: Usage {
                        input_tokens: Some(1),
                        output_tokens: Some(2),
                        total_tokens: Some(3)
                    }
                })
            )
        }));
        let completed = events.iter().find_map(|event| match event {
            Ok(ModelEvent::Completed { response }) => Some(response),
            _ => None,
        });
        assert_eq!(
            completed.unwrap().content,
            vec![ContentPart::Text { text: "hi".into() }]
        );
    }
}
