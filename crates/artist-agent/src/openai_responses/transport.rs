//! Artist-owned HTTP/SSE transport for OpenAI Responses.
//!
//! Event normalization is adapted from Rig 0.41's OpenAI Responses streaming
//! module (MIT), reduced to the event types Artist consumes.
use super::{OutputItem, Request};
use futures::StreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use rig_core::{
    client::CompletionClient,
    completion::{self, CompletionError, CompletionModel, GetTokenUsage, Usage},
    message::ReasoningContent,
    streaming::{
        RawStreamingChoice, RawStreamingToolCall, StreamingCompletionResponse, StreamingResult,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug)]
pub enum Credentials {
    ApiKey(String),
    ChatGpt {
        access_token: String,
        account_id: String,
    },
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    endpoint: String,
    credentials: Credentials,
    provider_context: artist_session::ProviderContextHandle,
    conversation_id: String,
    effective_context_window: Option<u64>,
    unsupported_context_management: Arc<Mutex<HashSet<String>>>,
}

impl Client {
    pub fn api_key(endpoint: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            endpoint: endpoint.into(),
            credentials: Credentials::ApiKey(key.into()),
            provider_context: artist_session::ProviderContextHandle::noop(),
            conversation_id: "default".into(),
            effective_context_window: None,
            unsupported_context_management: Arc::new(Mutex::new(HashSet::new())),
        }
    }
    pub fn chatgpt(
        endpoint: impl Into<String>,
        token: impl Into<String>,
        account_id: impl Into<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            endpoint: endpoint.into(),
            credentials: Credentials::ChatGpt {
                access_token: token.into(),
                account_id: account_id.into(),
            },
            provider_context: artist_session::ProviderContextHandle::noop(),
            conversation_id: "default".into(),
            effective_context_window: None,
            unsupported_context_management: Arc::new(Mutex::new(HashSet::new())),
        }
    }
    pub fn with_provider_context(
        mut self,
        conversation_id: impl Into<String>,
        provider_context: artist_session::ProviderContextHandle,
    ) -> Self {
        self.conversation_id = conversation_id.into();
        self.provider_context = provider_context;
        self
    }
    pub fn with_effective_context_window(mut self, window: Option<u64>) -> Self {
        self.effective_context_window = window;
        self
    }

    /// Compact the full canonical sidecar plus current context. The sidecar is
    /// replaced only after a successful, parseable response.
    pub async fn compact(
        &self,
        model: &str,
        current: Vec<Value>,
    ) -> Result<Vec<Value>, CompletionError> {
        let (saved, checkpoint) = self
            .provider_context
            .snapshot(&self.conversation_id, &self.context_namespace())
            .await;
        let fingerprints = current.iter().map(wire_fingerprint).collect::<Vec<_>>();
        let input = reconcile_inputs(saved, &checkpoint, current, &fingerprints);
        let response = self
            .http
            .post(format!(
                "{}/responses/compact",
                self.endpoint.trim_end_matches('/')
            ))
            .headers(self.headers()?)
            .json(&json!({"model": model, "input": input, "store": false}))
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        let text = response.text().await.map_err(transport)?;
        if !status.is_success() {
            return Err(CompletionError::ProviderError(format!(
                "Responses compact HTTP {status}: {}",
                sanitize(&text)
            )));
        }
        let wire: Value = serde_json::from_str(&text)?;
        let items = parse_output(&wire)?
            .into_iter()
            .map(|item| item.wire().clone())
            .collect::<Vec<_>>();
        self.provider_context
            .commit_checkpoint(
                &self.conversation_id,
                &self.context_namespace(),
                items.clone(),
                fingerprints,
            )
            .await;
        Ok(items)
    }

    fn context_namespace(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        digest.update(self.endpoint.as_bytes());
        digest.update([0]);
        match &self.credentials {
            Credentials::ApiKey(key) => {
                digest.update(b"api-key\0");
                digest.update(key.as_bytes());
            }
            Credentials::ChatGpt { account_id, .. } => {
                digest.update(b"chatgpt\0");
                digest.update(account_id.as_bytes());
            }
        }
        let digest = digest.finalize();
        let hex = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        format!("openai.responses:{hex}")
    }

    fn headers(&self) -> Result<HeaderMap, CompletionError> {
        let mut h = HeaderMap::new();
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let token = match &self.credentials {
            Credentials::ApiKey(k) => k,
            Credentials::ChatGpt { access_token, .. } => access_token,
        };
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(boxed)?,
        );
        if let Credentials::ChatGpt { account_id, .. } = &self.credentials {
            h.insert(
                "chatgpt-account-id",
                HeaderValue::from_str(account_id).map_err(boxed)?,
            );
            h.insert("originator", HeaderValue::from_static("artist"));
        }
        Ok(h)
    }
    fn url(&self) -> String {
        format!("{}/responses", self.endpoint.trim_end_matches('/'))
    }
}
fn boxed(e: impl std::error::Error + Send + Sync + 'static) -> CompletionError {
    CompletionError::RequestError(Box::new(e))
}
fn transport(e: reqwest::Error) -> CompletionError {
    CompletionError::ResponseError(format!(
        "Responses transport error: {}",
        if e.is_timeout() {
            "request timed out"
        } else if e.is_connect() {
            "connection failed"
        } else {
            "request failed"
        }
    ))
}

impl CompletionClient for Client {
    type CompletionModel = ArtistOpenAiModel;
}

#[derive(Clone)]
pub struct ArtistOpenAiModel {
    client: Client,
    model: String,
    provider_context: artist_session::ProviderContextHandle,
    conversation_id: String,
}

impl ArtistOpenAiModel {
    /// Opt-in wiring for the Artist-owned adapter. Production dispatch does not use this yet.
    pub fn with_provider_context(
        mut self,
        conversation_id: impl Into<String>,
        provider_context: artist_session::ProviderContextHandle,
    ) -> Self {
        self.conversation_id = conversation_id.into();
        self.provider_context = provider_context;
        self
    }

    async fn prepare(&self, mut body: Request) -> (Request, Vec<String>) {
        let (saved, checkpoint) = self
            .provider_context
            .snapshot(&self.conversation_id, &self.client.context_namespace())
            .await;
        let fresh_fingerprints = body.input.iter().map(wire_fingerprint).collect::<Vec<_>>();
        body.input = reconcile_inputs(saved, &checkpoint, body.input.clone(), &fresh_fingerprints);
        (body, fresh_fingerprints)
    }

    async fn save(
        &self,
        mut canonical: Vec<Value>,
        output: &[OutputItem],
        mut checkpoint: Vec<String>,
        represented_output: &[Value],
    ) {
        canonical.extend(output.iter().map(|item| item.wire().clone()));
        checkpoint.extend(represented_output.iter().map(wire_fingerprint));
        self.provider_context
            .commit_checkpoint(
                &self.conversation_id,
                &self.client.context_namespace(),
                canonical,
                checkpoint,
            )
            .await;
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub wire: Value,
    pub output: Vec<OutputItem>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StreamResponse {
    pub usage: Usage,
    pub output: Vec<OutputItem>,
    pub wire: Value,
}
impl GetTokenUsage for StreamResponse {
    fn token_usage(&self) -> Usage {
        self.usage.clone()
    }
}

impl CompletionModel for ArtistOpenAiModel {
    type Response = Response;
    type StreamingResponse = StreamResponse;
    type Client = Client;
    fn make(client: &Client, model: impl Into<String>) -> Self {
        Self {
            client: client.clone(),
            model: model.into(),
            provider_context: client.provider_context.clone(),
            conversation_id: client.conversation_id.clone(),
        }
    }
    fn composes_native_output_with_tools(&self) -> bool {
        true
    }

    async fn completion(
        &self,
        request: completion::CompletionRequest,
    ) -> Result<completion::CompletionResponse<Response>, CompletionError> {
        let body = Request::try_from((self.model.clone(), request))
            .map_err(|e| CompletionError::ResponseError(e.to_string()))?;
        let (mut body, checkpoint) = self.prepare(body).await;
        if self
            .client
            .unsupported_context_management
            .lock()
            .unwrap()
            .contains(&self.model)
        {
            body.context_management.clear();
        }
        let mut retried = false;
        let response = loop {
            let response = self
                .client
                .http
                .post(self.client.url())
                .headers(self.client.headers()?)
                .json(&body)
                .send()
                .await
                .map_err(transport)?;
            let status = response.status();
            let text = response.text().await.map_err(transport)?;
            if status.is_success() {
                break text;
            }
            if !retried
                && !body.context_management.is_empty()
                && context_management_unsupported(status.as_u16(), &text)
            {
                retried = true;
                body.context_management.clear();
                self.client
                    .unsupported_context_management
                    .lock()
                    .unwrap()
                    .insert(self.model.clone());
                continue;
            }
            return Err(CompletionError::ProviderError(format!(
                "Responses HTTP {status}: {}",
                sanitize(&text)
            )));
        };
        let wire: Value = serde_json::from_str(&response)?;
        let output = parse_output(&wire)?;
        validate_success(&wire)?;
        let upstream: rig_core::providers::openai::responses_api::CompletionResponse =
            serde_json::from_value(wire.clone())?;
        let normalized: completion::CompletionResponse<_> = upstream.try_into()?;
        let represented = represented_assistant_input(normalized.choice.clone())?;
        self.save(body.input.clone(), &output, checkpoint, &represented)
            .await;
        Ok(completion::CompletionResponse {
            choice: normalized.choice,
            usage: normalized.usage,
            message_id: normalized.message_id,
            raw_response: Response { wire, output },
        })
    }

    async fn stream(
        &self,
        request: completion::CompletionRequest,
    ) -> Result<StreamingCompletionResponse<StreamResponse>, CompletionError> {
        let body = Request::try_from((self.model.clone(), request))
            .map_err(|e| CompletionError::ResponseError(e.to_string()))?;
        let (mut body, checkpoint) = self.prepare(body).await;
        body.stream = Some(true);
        if self
            .client
            .unsupported_context_management
            .lock()
            .unwrap()
            .contains(&self.model)
        {
            body.context_management.clear();
        }
        let mut retried = false;
        let response = loop {
            let response = self
                .client
                .http
                .post(self.client.url())
                .headers(self.client.headers()?)
                .json(&body)
                .send()
                .await
                .map_err(transport)?;
            let status = response.status();
            if status.is_success() {
                break response;
            }
            let text = response.text().await.map_err(transport)?;
            if !retried
                && !body.context_management.is_empty()
                && context_management_unsupported(status.as_u16(), &text)
            {
                retried = true;
                body.context_management.clear();
                self.client
                    .unsupported_context_management
                    .lock()
                    .unwrap()
                    .insert(self.model.clone());
                continue;
            }
            return Err(CompletionError::ProviderError(format!(
                "Responses HTTP {status}: {}",
                sanitize(&text)
            )));
        };
        let mut bytes = response.bytes_stream();
        let context = self.provider_context.clone();
        let conversation_id = self.conversation_id.clone();
        let canonical_input = body.input.clone();
        let provider_namespace = self.client.context_namespace();
        let input_checkpoint = checkpoint;
        let stream: StreamingResult<StreamResponse> = Box::pin(async_stream::stream! {
            let mut buffer = String::new();
            let mut accumulated_text = String::new();
            let mut accumulated_items: Vec<Value> = Vec::new();
            while let Some(chunk) = bytes.next().await {
                match chunk {
                    Err(e) => { yield Err(transport(e)); break; }
                    Ok(chunk) => {
                        buffer.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(event) = take_sse_event(&mut buffer) {
                            let data = event.lines().filter_map(|l| l.strip_prefix("data:").map(str::trim)).collect::<Vec<_>>().join("\n");
                            if data.is_empty() || data == "[DONE]" { continue; }
                            let parsed: Value = match serde_json::from_str(&data) {
                                Ok(value) => value,
                                Err(_) => { yield Err(CompletionError::ResponseError("malformed Responses SSE event".into())); return; }
                            };
                            let kind = parsed.get("type").and_then(Value::as_str).unwrap_or("");
                            let had_text = !accumulated_text.is_empty();
                            if kind == "response.output_text.delta" {
                                accumulated_text.push_str(parsed.get("delta").and_then(Value::as_str).unwrap_or(""));
                            } else if kind == "response.output_text.done" && accumulated_text.is_empty() {
                                accumulated_text.push_str(parsed.get("text").and_then(Value::as_str).unwrap_or(""));
                            } else if matches!(kind, "response.output_item.added" | "response.output_item.done") {
                                if let Some(item) = parsed.get("item") {
                                    let id = item.get("id").and_then(Value::as_str);
                                    if !accumulated_items.iter().any(|old| id.is_some() && old.get("id").and_then(Value::as_str) == id) {
                                        accumulated_items.push(item.clone());
                                    } else if kind.ends_with(".done") && let Some(position) = accumulated_items.iter().position(|old| id.is_some() && old.get("id").and_then(Value::as_str) == id) {
                                        accumulated_items[position] = item.clone();
                                    }
                                }
                            }
                            match parse_event(&data) {
                                Ok(mut events) => {
                                    if kind == "response.output_text.done" && !had_text && !accumulated_text.is_empty() {
                                        events.push(RawStreamingChoice::Message(accumulated_text.clone()));
                                    }
                                    for event in events {
                                    if let RawStreamingChoice::FinalResponse(final_response) = &event {
                                        let mut saved = canonical_input.clone();
                                        saved.extend(final_response.output.iter().map(|item| item.wire().clone()));
                                        let mut full_checkpoint = input_checkpoint.clone();
                                        let normalized_wire = normalized_terminal_wire(&final_response.wire, &accumulated_text, &accumulated_items);
                                        match represented_output_from_wire(&normalized_wire) {
                                            Ok(represented) => full_checkpoint.extend(represented.iter().map(wire_fingerprint)),
                                            Err(error) => { yield Err(error); return; }
                                        }
                                        context.commit_checkpoint(&conversation_id, &provider_namespace, saved, full_checkpoint).await;
                                    }
                                    yield Ok(event);
                                    }
                                },
                                Err(e) => { yield Err(e); return; }
                            }
                        }
                    }
                }
            }
        });
        Ok(StreamingCompletionResponse::stream(stream))
    }
}

fn represented_assistant_input(
    choice: rig_core::OneOrMany<rig_core::message::AssistantContent>,
) -> Result<Vec<Value>, CompletionError> {
    let request = completion::CompletionRequest {
        model: None,
        preamble: None,
        chat_history: rig_core::OneOrMany::one(completion::Message::Assistant {
            id: None,
            content: choice,
        }),
        documents: vec![],
        tools: vec![],
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        additional_params: None,
        output_schema: None,
        record_telemetry_content: false,
    };
    Request::try_from(("checkpoint".to_owned(), request))
        .map(|request| request.input)
        .map_err(|error| CompletionError::ResponseError(error.to_string()))
}

fn normalized_terminal_wire(wire: &Value, text: &str, streamed_items: &[Value]) -> Value {
    let mut normalized = wire.clone();
    let has_terminal_choice = wire
        .get("output")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("message" | "function_call")
                )
            })
        });
    if has_terminal_choice {
        return normalized;
    }

    let output = normalized.get_mut("output").and_then(Value::as_array_mut);
    if let Some(output) = output {
        // Prefer completed tool items. Otherwise synthesize the assistant message
        // represented by text events; opaque reasoning stays only in raw output.
        let tools = streamed_items
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
            .cloned()
            .collect::<Vec<_>>();
        if !tools.is_empty() {
            output.extend(tools);
        } else if !text.is_empty() {
            output.push(json!({
                "type": "message", "id": "artist_streamed_message", "role": "assistant", "status": "completed",
                "content": [{"type": "output_text", "text": text, "annotations": [], "logprobs": []}]
            }));
        }
    }
    normalized
}

fn represented_output_from_wire(wire: &Value) -> Result<Vec<Value>, CompletionError> {
    let upstream: rig_core::providers::openai::responses_api::CompletionResponse =
        serde_json::from_value(wire.clone())?;
    let normalized: completion::CompletionResponse<_> = upstream.try_into()?;
    represented_assistant_input(normalized.choice)
}

fn take_sse_event(buffer: &mut String) -> Option<String> {
    let lf = buffer.find("\n\n").map(|pos| (pos, 2));
    let crlf = buffer.find("\r\n\r\n").map(|pos| (pos, 4));
    let (pos, delimiter_len) = match (lf, crlf) {
        (Some(a), Some(b)) => {
            if a.0 <= b.0 {
                a
            } else {
                b
            }
        }
        (Some(found), None) | (None, Some(found)) => found,
        (None, None) => return None,
    };
    let event = buffer[..pos].replace('\r', "");
    buffer.drain(..pos + delimiter_len);
    Some(event)
}

fn wire_fingerprint(value: &Value) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys = map.keys().collect::<Vec<_>>();
                keys.sort();
                Value::Object(
                    keys.into_iter()
                        .map(|key| (key.clone(), canonical(&map[key])))
                        .collect(),
                )
            }
            Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string(&canonical(value)).expect("JSON values serialize")
}

fn reconcile_inputs(
    saved: Vec<Value>,
    checkpoint: &[String],
    fresh: Vec<Value>,
    fingerprints: &[String],
) -> Vec<Value> {
    let common = if checkpoint.is_empty() && !saved.is_empty() {
        // Schema-v1 snapshots did not record a cursor. Rig history contains prior
        // assistant turns, so migrate conservatively at the last such boundary;
        // everything after it is the new user/tool-result suffix.
        fresh
            .iter()
            .rposition(|item| item.get("role").and_then(Value::as_str) == Some("assistant"))
            .map_or(0, |index| index + 1)
    } else {
        checkpoint
            .iter()
            .zip(fingerprints)
            .take_while(|(a, b)| a == b)
            .count()
    };
    let mut merged = saved;
    // A diverged history is intentionally appended from its divergence point. In the
    // normal growing-history case this adds only genuinely new framework items.
    merged.extend(fresh.into_iter().skip(common));
    merged
}

fn context_management_unsupported(status: u16, body: &str) -> bool {
    matches!(status, 400 | 404 | 422)
        && body.to_ascii_lowercase().contains("context_management")
        && ["unsupported", "unknown", "unrecognized", "not supported"]
            .iter()
            .any(|needle| body.to_ascii_lowercase().contains(needle))
}

fn sanitize(body: &str) -> String {
    let msg = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        v.pointer("/error/message")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    msg.unwrap_or_else(|| "provider request failed".into())
        .chars()
        .take(300)
        .collect()
}
fn validate_success(wire: &Value) -> Result<(), CompletionError> {
    if wire
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|s| s != "completed")
    {
        return Err(CompletionError::ProviderError(
            "Responses request did not complete".into(),
        ));
    }
    if !wire.get("output").is_some_and(Value::is_array) {
        return Err(CompletionError::ResponseError(
            "Responses response missing output array".into(),
        ));
    }
    Ok(())
}
fn parse_output(wire: &Value) -> Result<Vec<OutputItem>, CompletionError> {
    validate_success(wire)?;
    Ok(serde_json::from_value(wire["output"].clone())?)
}
fn usage(v: &Value) -> Usage {
    let u = v.get("usage").unwrap_or(&Value::Null);
    Usage {
        input_tokens: u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
        output_tokens: u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
        total_tokens: u.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
        cached_input_tokens: u
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        reasoning_tokens: u
            .pointer("/output_tokens_details/reasoning_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        ..Usage::default()
    }
}

fn parse_event(data: &str) -> Result<Vec<RawStreamingChoice<StreamResponse>>, CompletionError> {
    let v: Value = serde_json::from_str(data)
        .map_err(|_| CompletionError::ResponseError("malformed Responses SSE event".into()))?;
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    let mut out = vec![];
    match kind {
        "response.output_text.delta" => out.push(RawStreamingChoice::Message(
            v.get("delta").and_then(Value::as_str).unwrap_or("").into(),
        )),
        "response.reasoning_summary_text.delta" => out.push(RawStreamingChoice::ReasoningDelta {
            id: v.get("item_id").and_then(Value::as_str).map(str::to_owned),
            reasoning: v.get("delta").and_then(Value::as_str).unwrap_or("").into(),
        }),
        "response.reasoning_text.delta" => {}
        "response.output_item.done" => {
            if let Some(item) = v.get("item") {
                match item.get("type").and_then(Value::as_str) {
                    Some("function_call") => {
                        let id = item
                            .get("id")
                            .and_then(Value::as_str)
                            .or_else(|| item.get("call_id").and_then(Value::as_str))
                            .unwrap_or("")
                            .to_owned();
                        let call_id = item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or(&id)
                            .to_owned();
                        let args = serde_json::from_str(
                            item.get("arguments")
                                .and_then(Value::as_str)
                                .unwrap_or("{}"),
                        )?;
                        out.push(RawStreamingChoice::ToolCall(
                            RawStreamingToolCall::new(
                                id,
                                item.get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .into(),
                                args,
                            )
                            .with_internal_call_id(call_id.clone())
                            .with_call_id(call_id),
                        ));
                    }
                    Some("reasoning") => {
                        let id = item.get("id").and_then(Value::as_str).map(str::to_owned);
                        if let Some(enc) = item.get("encrypted_content").and_then(Value::as_str) {
                            out.push(RawStreamingChoice::Reasoning {
                                id,
                                content: ReasoningContent::Encrypted(enc.into()),
                            });
                        }
                    }
                    Some("message") => {
                        if let Some(id) = item.get("id").and_then(Value::as_str) {
                            out.push(RawStreamingChoice::MessageId(id.into()));
                        }
                    }
                    _ => out.push(RawStreamingChoice::Unknown(item.clone())),
                }
            }
        }
        "response.completed" => {
            let r = v.get("response").cloned().ok_or_else(|| {
                CompletionError::ResponseError("terminal event missing response".into())
            })?;
            out.push(RawStreamingChoice::FinalResponse(StreamResponse {
                usage: usage(&r),
                output: parse_output(&r)?,
                wire: r,
            }));
        }
        "response.failed" | "response.incomplete" | "error" => {
            return Err(CompletionError::ProviderError(
                v.pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Responses stream failed")
                    .chars()
                    .take(300)
                    .collect(),
            ));
        }
        _ => {}
    }
    Ok(out)
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use rig_core::{OneOrMany, completion::CompletionRequest, streaming::RawStreamingChoice};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn checkpoint_replay_adds_only_new_history_across_turns_tools_and_compaction() {
        let user1 =
            json!({"type":"message","role":"user","content":[{"type":"input_text","text":"one"}]});
        let user2 =
            json!({"type":"message","role":"user","content":[{"type":"input_text","text":"two"}]});
        let call = json!({"type":"function_call","id":"fc1","call_id":"c","name":"lookup","arguments":"{}"});
        let result = json!({"type":"function_call_output","call_id":"c","output":"ok"});
        // Provider output carries output-only status/phase/output_text fields.
        let output = json!({"type":"message","id":"m1","role":"assistant","status":"completed","phase":"final","content":[{"type":"output_text","text":"answer","annotations":[]}]});
        // Convert the validated provider response through Rig's response and
        // assistant-message representations, then back through its request converter.
        let response_wire = json!({
            "id":"resp_1","object":"response","created_at":1,"status":"completed",
            "error":null,"incomplete_details":null,"instructions":null,
            "max_output_tokens":null,"model":"gpt-test",
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2},
            "output":[output.clone()],"tools":[]
        });
        let represented_items = represented_output_from_wire(&response_wire).unwrap();
        assert_eq!(represented_items.len(), 1);
        let represented = represented_items[0].clone();
        assert_ne!(wire_fingerprint(&output), wire_fingerprint(&represented));
        let canonical = vec![user1.clone(), output.clone()];
        let checkpoint1 = vec![wire_fingerprint(&user1), wire_fingerprint(&represented)];

        // Rig supplies the complete prior assistant/function history again.
        let fresh2 = vec![user1.clone(), represented.clone(), user2.clone()];
        let fingerprints2 = fresh2.iter().map(wire_fingerprint).collect::<Vec<_>>();
        assert_eq!(
            reconcile_inputs(canonical.clone(), &checkpoint1, fresh2, &fingerprints2),
            vec![user1.clone(), output.clone(), user2.clone()]
        );

        let fresh_tool = vec![
            user1.clone(),
            represented.clone(),
            user2.clone(),
            call.clone(),
            result.clone(),
        ];
        let fingerprints_tool = fresh_tool.iter().map(wire_fingerprint).collect::<Vec<_>>();
        let merged = reconcile_inputs(canonical, &checkpoint1, fresh_tool, &fingerprints_tool);
        assert_eq!(merged, vec![user1, output, user2, call, result]);

        let compacted = vec![json!({"type":"compaction","encrypted_content":"opaque"})];
        let restarted = reconcile_inputs(
            compacted.clone(),
            &fingerprints_tool,
            vec![
                json!({"type":"message","role":"user","content":"one"}),
                json!({"type":"message","role":"user","content":"two"}),
                json!({"type":"function_call","id":"fc1","call_id":"c"}),
                json!({"type":"function_call_output","call_id":"c","output":"ok"}),
            ],
            &fingerprints_tool,
        );
        assert_eq!(restarted, compacted);

        let legacy_output =
            json!({"type":"message","id":"provider-output","role":"assistant","content":[]});
        let legacy_fresh = vec![
            json!({"role":"user","content":"old"}),
            json!({"role":"assistant","content":"answer"}),
            json!({"role":"user","content":"new"}),
        ];
        let legacy_fingerprints = legacy_fresh
            .iter()
            .map(wire_fingerprint)
            .collect::<Vec<_>>();
        assert_eq!(
            reconcile_inputs(
                vec![legacy_output.clone()],
                &[],
                legacy_fresh,
                &legacy_fingerprints
            ),
            vec![legacy_output, json!({"role":"user","content":"new"})]
        );
    }

    #[test]
    fn sse_framing_handles_lf_crlf_and_split_crlf_delimiters() {
        let mut buffer = "data: one\n\ndata: two\r\n\r".to_owned();
        assert_eq!(take_sse_event(&mut buffer).as_deref(), Some("data: one"));
        assert!(take_sse_event(&mut buffer).is_none());
        buffer.push_str("\n");
        assert_eq!(take_sse_event(&mut buffer).as_deref(), Some("data: two"));
        assert!(buffer.is_empty());
    }

    #[test]
    fn context_namespace_isolated_by_endpoint_key_and_account_without_exposure() {
        let a = Client::api_key("https://one", "key-a").context_namespace();
        let b = Client::api_key("https://one", "key-b").context_namespace();
        let c = Client::api_key("https://two", "key-a").context_namespace();
        let account = Client::chatgpt("https://one", "token", "acct").context_namespace();
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, account);
        assert!(!a.contains("key-a"));
        assert!(!account.contains("acct"));
    }

    fn request() -> CompletionRequest {
        CompletionRequest {
            model: None,
            preamble: None,
            chat_history: OneOrMany::one(completion::Message::user("hello")),
            documents: vec![],
            tools: vec![],
            temperature: None,
            max_tokens: None,
            tool_choice: None,
            additional_params: None,
            output_schema: None,
            record_telemetry_content: false,
        }
    }
    async fn server(
        body: &'static str,
        content_type: &'static str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = vec![0; 16384];
            let n = socket.read(&mut data).await.unwrap();
            let req = String::from_utf8_lossy(&data[..n]).into_owned();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
            req
        });
        (format!("http://{addr}"), task)
    }

    #[tokio::test]
    async fn api_key_headers_body_and_nonstream_output_are_owned() {
        let response = r#"{"id":"resp_1","object":"response","created_at":1,"status":"completed","error":null,"incomplete_details":null,"instructions":null,"max_output_tokens":null,"model":"gpt-test","usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3},"output":[{"type":"message","id":"msg_1","role":"assistant","status":"completed","phase":"final","content":[{"type":"output_text","text":"hi","annotations":[]} ]}],"tools":[]}"#;
        let (url, captured) = server(response, "application/json").await;
        let result = Client::api_key(url, "secret")
            .completion_model("gpt-test")
            .completion(request())
            .await
            .unwrap();
        let sent = captured.await.unwrap().to_lowercase();
        assert!(sent.contains("authorization: bearer secret"));
        assert!(sent.contains("post /responses"));
        assert!(sent.contains("\"input\""));
        assert_eq!(result.raw_response.output[0].phase(), Some("final"));
    }

    #[test]
    fn unsupported_detection_is_explicit_and_conservative() {
        assert!(context_management_unsupported(
            400,
            r#"{"error":{"message":"context_management is unsupported"}}"#
        ));
        assert!(!context_management_unsupported(
            500,
            "context_management unsupported"
        ));
        assert!(!context_management_unsupported(
            400,
            "unrelated bad request"
        ));
    }

    #[tokio::test]
    async fn standalone_compact_sends_canonical_context_and_replaces_sidecar() {
        let response = r#"{"output":[{"type":"compaction","encrypted_content":"new"},{"type":"message","role":"assistant","content":[]}]}"#;
        let (url, captured) = server(response, "application/json").await;
        let context = artist_session::ProviderContextHandle::noop();
        let client =
            Client::api_key(url, "secret").with_provider_context("conversation", context.clone());
        context
            .commit(
                "conversation",
                &client.context_namespace(),
                vec![serde_json::json!({"type":"reasoning","encrypted_content":"old"})],
            )
            .await;
        let output = client
            .compact(
                "gpt-test",
                vec![serde_json::json!({"role":"user","content":"now"})],
            )
            .await
            .unwrap();
        assert_eq!(output[0]["encrypted_content"], "new");
        assert_eq!(
            context
                .items("conversation", &client.context_namespace())
                .await,
            output
        );
        let sent = captured.await.unwrap();
        assert!(sent.starts_with("POST /responses/compact"));
        assert!(sent.contains("\"store\":false"));
        assert!(sent.contains("\"encrypted_content\":\"old\""));
        assert!(sent.contains("\"content\":\"now\""));
    }

    #[test]
    fn streamed_text_synthesizes_normalized_terminal_without_mutating_opaque_output() {
        let raw = json!({
            "id":"resp_1", "object":"response", "created_at":1, "status":"completed",
            "error":null, "incomplete_details":null, "instructions":null,
            "max_output_tokens":null, "model":"codex", "tools":[],
            "usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5},
            "output":[
                {"type":"compaction","id":"cmp_1","encrypted_content":"opaque"},
                {"type":"reasoning","id":"rs_1","encrypted_content":"private"}
            ]
        });
        let normalized = normalized_terminal_wire(&raw, "Hi! How can I help?", &[]);
        let represented = represented_output_from_wire(&normalized).unwrap();
        assert!(
            !represented.is_empty(),
            "Rig must receive a terminal assistant choice"
        );
        assert_eq!(raw["output"].as_array().unwrap().len(), 2);
        assert_eq!(raw["output"][0]["encrypted_content"], "opaque");
        let serialized = serde_json::to_string(&represented).unwrap();
        assert!(serialized.contains("Hi! How can I help?"));
    }

    #[tokio::test]
    async fn chatgpt_headers_and_sse_text_reasoning_tool_compaction() {
        let terminal = r#"{"id":"resp_1","object":"response","created_at":1,"status":"completed","error":null,"incomplete_details":null,"instructions":null,"max_output_tokens":null,"model":"codex","usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5},"output":[{"type":"compaction","id":"cmp_1","encrypted_content":"opaque"}],"tools":[]}"#;
        let body = format!(
            r#"data: {{"type":"response.output_text.delta","delta":"hi"}}\n\ndata: {{"type":"response.reasoning_summary_text.delta","item_id":"rs_1","delta":"why"}}\n\ndata: {{"type":"response.output_item.done","item":{{"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup","arguments":"{{}}"}}}}\n\ndata: {{"type":"response.completed","response":{terminal}}}\n\ndata: [DONE]\n\n"#
        );
        let leaked: &'static str = Box::leak(body.into_boxed_str());
        let (url, captured) = server(leaked, "text/event-stream").await;
        let mut response = Client::chatgpt(url, "token", "acct")
            .completion_model("codex")
            .stream(request())
            .await
            .unwrap();
        while let Some(event) = response.next().await {
            event.unwrap();
        }
        let completed = parse_event(&format!(
            r#"{{"type":"response.completed","response":{terminal}}}"#
        ))
        .unwrap();
        assert!(
            matches!(&completed[0], RawStreamingChoice::FinalResponse(r) if matches!(r.output[0], OutputItem::Compaction(_)) && r.usage.total_tokens == 5)
        );
        assert!(
            matches!(&parse_event(r#"{"type":"response.output_text.delta","delta":"hi"}"#).unwrap()[0], RawStreamingChoice::Message(s) if s == "hi")
        );
        assert!(
            matches!(&parse_event(r#"{"type":"response.reasoning_summary_text.delta","item_id":"rs_1","delta":"why"}"#).unwrap()[0], RawStreamingChoice::ReasoningDelta { reasoning, .. } if reasoning == "why")
        );
        assert!(
            parse_event(r#"{"type":"response.reasoning_text.delta","delta":"private"}"#)
                .unwrap()
                .is_empty()
        );
        let tool = parse_event(r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup","arguments":"{}"}}"#).unwrap();
        assert!(
            matches!(&tool[0], RawStreamingChoice::ToolCall(t) if t.id == "fc_1" && t.call_id.as_deref() == Some("call_1"))
        );
        let sent = captured.await.unwrap().to_lowercase();
        assert!(sent.contains("chatgpt-account-id: acct"));
        assert!(sent.contains("originator: artist"));
        assert!(sent.contains("\"stream\":true"));
    }
}
