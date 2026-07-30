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
use serde_json::Value;

#[derive(Clone, Debug)]
pub enum Credentials {
    ApiKey(String),
    ChatGpt {
        access_token: String,
        account_id: String,
    },
}

#[derive(Clone, Debug)]
pub struct Client {
    http: reqwest::Client,
    endpoint: String,
    credentials: Credentials,
}

impl Client {
    pub fn api_key(endpoint: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            endpoint: endpoint.into(),
            credentials: Credentials::ApiKey(key.into()),
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
        }
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

#[derive(Clone, Debug)]
pub struct ArtistOpenAiModel {
    client: Client,
    model: String,
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
        if !status.is_success() {
            return Err(CompletionError::ProviderError(format!(
                "Responses HTTP {status}: {}",
                sanitize(&text)
            )));
        }
        let wire: Value = serde_json::from_str(&text)?;
        let output = parse_output(&wire)?;
        let upstream: rig_core::providers::openai::responses_api::CompletionResponse =
            serde_json::from_value(wire.clone())?;
        let normalized: completion::CompletionResponse<_> = upstream.try_into()?;
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
        let mut body = Request::try_from((self.model.clone(), request))
            .map_err(|e| CompletionError::ResponseError(e.to_string()))?;
        body.stream = Some(true);
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
        if !status.is_success() {
            let text = response.text().await.map_err(transport)?;
            return Err(CompletionError::ProviderError(format!(
                "Responses HTTP {status}: {}",
                sanitize(&text)
            )));
        }
        let mut bytes = response.bytes_stream();
        let stream: StreamingResult<StreamResponse> = Box::pin(async_stream::stream! {
            let mut buffer = String::new();
            while let Some(chunk) = bytes.next().await {
                match chunk {
                    Err(e) => { yield Err(transport(e)); break; }
                    Ok(chunk) => {
                        buffer.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(pos) = buffer.find("\n\n") {
                            let event = buffer[..pos].replace("\r", "");
                            buffer.drain(..pos + 2);
                            let data = event.lines().filter_map(|l| l.strip_prefix("data:").map(str::trim)).collect::<Vec<_>>().join("\n");
                            if data.is_empty() || data == "[DONE]" { continue; }
                            match parse_event(&data) {
                                Ok(events) => for event in events { yield Ok(event); },
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
fn parse_output(wire: &Value) -> Result<Vec<OutputItem>, CompletionError> {
    Ok(serde_json::from_value(
        wire.get("output").cloned().unwrap_or(Value::Array(vec![])),
    )?)
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
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            out.push(RawStreamingChoice::ReasoningDelta {
                id: v.get("item_id").and_then(Value::as_str).map(str::to_owned),
                reasoning: v.get("delta").and_then(Value::as_str).unwrap_or("").into(),
            })
        }
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
