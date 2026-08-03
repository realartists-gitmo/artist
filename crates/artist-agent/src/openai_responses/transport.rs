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
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
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
    /// Digest → provider file id, so an image goes up once instead of on every
    /// turn for the rest of the session. `None` disables the substitution.
    handles: Option<artist_session::HandleLedger>,
    /// Set once the endpoint has refused an upload. A files endpoint is not
    /// universal — the Codex backend has no reason to expose one — and probing
    /// it on every screenshot would turn a missing feature into a per-image
    /// round trip.
    /// Refused capabilities, held for the session rather than the client — a
    /// client is rebuilt every user turn, so flags kept here would be forgotten
    /// between turns and every missing endpoint re-probed forever.
    capabilities: artist_session::ProviderCapabilities,
    /// Where the provider-side conversation currently stands, when it is
    /// holding one for us. A cache over the local event log, never the source
    /// of truth — see [`artist_session::chain`].
    chain: artist_session::ChainState,
    /// Which provider-side handles this session may use. Passed in rather than
    /// read from the environment; see [`crate::statefulness`].
    statefulness: crate::Statefulness,
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
            handles: None,
            capabilities: artist_session::ProviderCapabilities::for_session(),
            chain: artist_session::ChainState::new(),
            statefulness: crate::Statefulness::default(),
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
            handles: None,
            capabilities: artist_session::ProviderCapabilities::for_session(),
            chain: artist_session::ChainState::new(),
            statefulness: crate::Statefulness::default(),
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

    /// Send images once and reference them thereafter.
    pub fn with_handles(mut self, handles: artist_session::HandleLedger) -> Self {
        self.handles = Some(handles);
        self
    }

    /// Adopt the session's chain position and refused-capability record.
    ///
    /// Both must outlive this client: it is rebuilt for every user turn, and a
    /// chain that resets each turn never saves anything while a capability
    /// probe that resets each turn costs a request every turn.
    pub fn with_session_state(
        mut self,
        chain: artist_session::ChainState,
        capabilities: artist_session::ProviderCapabilities,
        statefulness: crate::Statefulness,
    ) -> Self {
        self.chain = chain;
        self.capabilities = capabilities;
        self.statefulness = statefulness;
        self
    }

    /// Whether this endpoint can hold a conversation for us.
    ///
    /// Two conditions, both required. Chaining needs `store: true`, which the
    /// ChatGPT/Codex backend does not honour — that path authenticates a
    /// subscription rather than an account with retained responses, and asking
    /// it to keep a conversation is a request it has no way to satisfy. Only
    /// the API-key route reaches an endpoint that retains anything.
    ///
    /// And it is opt-in even there. The saving is real — a measured 95.4% of
    /// bytes on a 48-turn session were resends — but the failure mode of
    /// getting it wrong is a model shown a conversation that did not happen,
    /// which is worse than any bandwidth. It stays behind a switch until it has
    /// been exercised against the live endpoint rather than only a scripted
    /// one.
    fn chaining_supported(&self) -> bool {
        matches!(self.credentials, Credentials::ApiKey(_)) && self.statefulness.chaining
    }

    /// Replace inline image bytes with ids the provider already holds.
    ///
    /// A stateless API resends the whole conversation every turn, so a
    /// screenshot inlined as base64 is re-uploaded on every subsequent request
    /// for the rest of the session. On a measured 48-turn session in this
    /// repository 95.4% of all bytes uploaded were resends, and that session
    /// contained no images at all.
    ///
    /// Every failure path here degrades to inlining, which is exactly today's
    /// behaviour — an upload that does not happen costs bandwidth, never
    /// correctness. The first refusal disables uploads for the session so a
    /// backend without a files endpoint pays one wasted request, not one per
    /// image.
    async fn externalize_images(&self, input: &mut [Value]) {
        let Some(ledger) = &self.handles else {
            return;
        };
        if !self.has_side_endpoints() {
            return;
        }
        let pending = artist_session::inline_images::inline_images(input);
        if pending.is_empty() {
            return;
        }
        let namespace = self.context_namespace();
        let mut resolved = std::collections::BTreeMap::new();
        for image in pending {
            if let Some(id) = ledger.get(&namespace, &image.digest) {
                resolved.insert(image.digest, id);
                continue;
            }
            if !self.capabilities.may_upload() {
                continue;
            }
            match self.upload(&image).await {
                Ok(id) => {
                    // Recorded before use: a handle we uploaded but failed to
                    // remember would be re-uploaded on every later turn.
                    let _ = ledger.put(&namespace, &image.digest, &id);
                    resolved.insert(image.digest, id);
                }
                Err(_) => {
                    self.capabilities.refuse_uploads();
                    break;
                }
            }
        }
        if !resolved.is_empty() {
            artist_session::inline_images::apply_handles(input, &resolved);
        }
    }

    /// Replace the system prompt with a reference to a stored one.
    ///
    /// The preamble is the largest single thing resent on every request and the
    /// one that must never change mid-session. Referencing it by id addresses
    /// both at once: it leaves the request, so it costs nothing to resend and
    /// has no way to drift. Where [`crate::prefix::PrefixFreezer`] enforces
    /// stability in our code, this enforces it in the protocol — the strictly
    /// stronger form, because it holds regardless of what our code does.
    ///
    /// Opt-in, and separately from chaining. Unlike an image upload, publishing
    /// a prompt leaves a durable artifact on the provider's side rather than
    /// one that rides a single conversation; that is the user's call to make,
    /// not a default to inherit. The content itself is no more exposed than it
    /// already was — it goes up in `instructions` on every request today.
    async fn externalize_prompt(&self, body: &mut Request) {
        let (Some(ledger), Some(instructions)) = (&self.handles, body.instructions.clone()) else {
            return;
        };
        if !self.stored_prompts_enabled() || instructions.is_empty() || !self.has_side_endpoints() {
            return;
        }
        let namespace = format!("{}/prompts", self.context_namespace());
        let digest = artist_session::content_digest(instructions.as_bytes());

        let stored = match ledger.get(&namespace, &digest) {
            Some(stored) => stored,
            None => {
                if !self.capabilities.may_store_prompts() {
                    return;
                }
                match self.publish_prompt(&instructions).await {
                    Ok(stored) => {
                        let _ = ledger.put(&namespace, &digest, &stored);
                        stored
                    }
                    Err(_) => {
                        self.capabilities.refuse_prompts();
                        return;
                    }
                }
            }
        };

        // `id@version`, so a stored prompt is always referenced at the exact
        // revision that was published for this content.
        let (id, version) = match stored.split_once('@') {
            Some((id, version)) => (id.to_owned(), Some(version.to_owned())),
            None => (stored, None),
        };
        body.prompt = Some(super::PromptRef {
            id,
            version,
            variables: None,
        });
        // Both would be redundant, and the saving is precisely in not sending
        // this.
        body.instructions = None;
    }

    /// Store a system prompt with the provider, returning `id@version`.
    async fn publish_prompt(&self, instructions: &str) -> Result<String, CompletionError> {
        let response = self
            .http
            .post(format!("{}/prompts", self.endpoint.trim_end_matches('/')))
            .headers(self.headers()?)
            .json(&json!({
                "prompt": {"instructions": instructions},
            }))
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        let text = response.text().await.map_err(transport)?;
        if !status.is_success() {
            return Err(CompletionError::ProviderError(format!(
                "prompt publish HTTP {status}: {}",
                sanitize(&text)
            )));
        }
        let wire: Value = serde_json::from_str(&text)?;
        let id = wire.get("id").and_then(Value::as_str).ok_or_else(|| {
            CompletionError::ProviderError("prompt publish returned no id".into())
        })?;
        Ok(match wire.get("version").and_then(Value::as_str) {
            Some(version) => format!("{id}@{version}"),
            None => id.to_owned(),
        })
    }

    /// Whether this endpoint serves anything besides `/responses`.
    ///
    /// The ChatGPT/Codex backend does not. Probed directly rather than assumed:
    /// `GET /responses` there answers `405 Method Not Allowed` with a JSON body
    /// — a real route, wrong verb — while `/files` and `/prompts` answer `403`
    /// with an HTML error page, which is the edge refusing a path the API does
    /// not publish.
    ///
    /// Without this the first turn of every session spends a request
    /// discovering that again. The capability record would then suppress the
    /// rest, so this saves one request per session rather than one per turn —
    /// small, but it is a request that can never succeed. Revisit if that
    /// backend ever grows the endpoints.
    fn has_side_endpoints(&self) -> bool {
        matches!(self.credentials, Credentials::ApiKey(_))
    }

    fn stored_prompts_enabled(&self) -> bool {
        self.statefulness.stored_prompt
    }

    /// Put one image on the provider's files endpoint, returning its id.
    async fn upload(&self, image: &artist_session::InlineImage) -> Result<String, CompletionError> {
        let extension = image
            .media_type
            .rsplit('/')
            .next()
            .filter(|ext| ext.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or("png");
        let part = reqwest::multipart::Part::bytes(image.bytes.clone())
            .file_name(format!("{}.{extension}", image.digest))
            .mime_str(&image.media_type)
            .map_err(boxed)?;
        let form = reqwest::multipart::Form::new()
            // `vision` is the purpose that permits a file to be referenced from
            // an image content block.
            .text("purpose", "vision")
            .part("file", part);

        let mut headers = self.headers()?;
        // Set by the multipart body, which carries its own boundary.
        headers.remove(CONTENT_TYPE);

        let response = self
            .http
            .post(format!("{}/files", self.endpoint.trim_end_matches('/')))
            .headers(headers)
            .multipart(form)
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        let text = response.text().await.map_err(transport)?;
        if !status.is_success() {
            return Err(CompletionError::ProviderError(format!(
                "file upload HTTP {status}: {}",
                sanitize(&text)
            )));
        }
        serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|wire| wire.get("id")?.as_str().map(str::to_owned))
            .ok_or_else(|| CompletionError::ProviderError("file upload returned no id".to_owned()))
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
        let common = common_prefix(&saved, &checkpoint, &body.input, &fresh_fingerprints);
        let chain_key = artist_session::chain::key(
            &self.client.conversation_id,
            &self.client.context_namespace(),
        );

        // Any history rewrite invalidates the chain, detected here rather than
        // hooked onto each cause.
        //
        // A shortfall against the checkpoint means this turn's history no
        // longer matches what the provider was told — which is precisely what a
        // stream-rule replay, a compaction and a handoff all produce. Deriving
        // it from the fingerprints covers every one of them, plus whatever
        // rewrites history next year, without any of them having to remember
        // this exists.
        if common < checkpoint.len() {
            self.client
                .chain
                .invalidate(&chain_key, artist_session::chain::Broke::HistoryRewritten);
        }

        match self
            .client
            .chain
            .plan(&chain_key, self.client.chaining_supported())
        {
            artist_session::ChainSend::Chained {
                previous_response_id,
            } => {
                // The provider is holding everything up to `common`; sending it
                // again would duplicate the conversation rather than continue
                // it.
                body.previous_response_id = Some(previous_response_id);
                body.store = Some(true);
                body.input = answer_orphaned_calls(body.input.split_off(common));
            }
            artist_session::ChainSend::Full => {
                body.input =
                    reconcile_inputs(saved, &checkpoint, body.input.clone(), &fresh_fingerprints);
            }
        }

        // After reconciliation, so it covers the restored sidecar suffix as
        // well as this turn's new items — the older images are precisely the
        // ones that have been resent the most times.
        //
        // Fingerprints are taken above, before substitution: they identify a
        // conversation item, and an item must not change identity because its
        // image moved from bytes to a handle.
        self.client.externalize_images(&mut body.input).await;
        self.client.externalize_prompt(&mut body).await;
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
            // A refused chain must not be retried into: drop it, and the
            // loop's next attempt finds it cold and restates the conversation.
            // Recovery costs one request, which is why no attempt is made to
            // distinguish an expired response from a withdrawn feature.
            if body.previous_response_id.is_some() && chain_refused(status.as_u16(), &text) {
                self.client.chain.invalidate(
                    &artist_session::chain::key(
                        &self.client.conversation_id,
                        &self.client.context_namespace(),
                    ),
                    artist_session::chain::Broke::ProviderRefused,
                );
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
            // A refused chain must not be retried into: drop it, and the
            // loop's next attempt finds it cold and restates the conversation.
            // Recovery costs one request, which is why no attempt is made to
            // distinguish an expired response from a withdrawn feature.
            if body.previous_response_id.is_some() && chain_refused(status.as_u16(), &text) {
                self.client.chain.invalidate(
                    &artist_session::chain::key(
                        &self.client.conversation_id,
                        &self.client.context_namespace(),
                    ),
                    artist_session::chain::Broke::ProviderRefused,
                );
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
        let chain = self.client.chain.clone();
        let chain_key = artist_session::chain::key(&conversation_id, &provider_namespace);
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
                                        let normalized_wire = normalized_terminal_wire(
                                            &final_response.wire,
                                            &accumulated_text,
                                            &accumulated_items,
                                        );
                                        let normalized_output = match parse_output(&normalized_wire) {
                                            Ok(output) => output,
                                            Err(error) => { yield Err(error); return; }
                                        };
                                        let mut saved = canonical_input.clone();
                                        saved.extend(normalized_output.iter().map(|item| {
                                            let mut wire = item.wire().clone();
                                            if wire.get("id").and_then(Value::as_str)
                                                == Some("msg_artist_streamed_message")
                                            {
                                                wire.as_object_mut().map(|object| object.remove("id"));
                                            }
                                            wire
                                        }));
                                        let mut full_checkpoint = input_checkpoint.clone();
                                        match represented_output_from_wire(&normalized_wire) {
                                            Ok(represented) => full_checkpoint.extend(represented.iter().map(wire_fingerprint)),
                                            Err(error) => { yield Err(error); return; }
                                        }
                                        context.commit_checkpoint(&conversation_id, &provider_namespace, saved, full_checkpoint).await;
                                        // The turn landed whole, so the next
                                        // one may chain onto it. Advancing only
                                        // here means an interrupted or failed
                                        // turn leaves the chain where it was
                                        // and the next request restates
                                        // everything — the safe direction.
                                        chain.advance(
                                            &chain_key,
                                            final_response.wire.get("id").and_then(Value::as_str).unwrap_or_default(),
                                        );
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

/// The wrapper OpenAI's serving layer injects beside the caller's tools.
///
/// When parallel tool calling is enabled — the default, and we never send
/// `parallel_tool_calls: false` — OpenAI adds a `multi_tool_use` namespace
/// alongside the `functions` namespace holding our tools, so the model can emit
/// a batch as a single call whose arguments name the real tools. Nothing
/// registers that wrapper on our side, so a call to it would be a call to an
/// unknown tool.
const PARALLEL_TOOL: &str = "multi_tool_use.parallel";

/// Expand an injected parallel wrapper into the calls it names.
///
/// Returns `None` for anything that is not the wrapper, so ordinary calls pass
/// through untouched. Derived ids are a pure function of the wrapper's own ids,
/// so the live stream and the persisted canonical items agree on them without
/// coordinating — which is what keeps every result paired with a call the
/// provider can see.
fn expanded_parallel_call(item: &Value) -> Option<Vec<Value>> {
    if item.get("type").and_then(Value::as_str) != Some("function_call")
        || item.get("name").and_then(Value::as_str) != Some(PARALLEL_TOOL)
    {
        return None;
    }
    let arguments: Value = item
        .get("arguments")
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str(raw).ok())?;
    let uses = arguments.get("tool_uses")?.as_array()?;
    let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .unwrap_or(id)
        .to_owned();
    Some(
        uses.iter()
            .enumerate()
            .map(|(index, use_)| {
                // Tools live in the `functions` namespace, so the wrapper
                // refers to them across namespaces; our registry uses the bare
                // name.
                let name = use_
                    .get("recipient_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let name = name.strip_prefix("functions.").unwrap_or(name);
                let parameters = use_.get("parameters").cloned().unwrap_or_else(|| json!({}));
                json!({
                    "type": "function_call",
                    "id": format!("{id}.{index}"),
                    "call_id": format!("{call_id}.{index}"),
                    "name": name,
                    "arguments": parameters.to_string(),
                    "status": "completed",
                })
            })
            .collect(),
    )
}

fn expand_parallel_calls(items: Vec<Value>) -> Vec<Value> {
    items
        .into_iter()
        .flat_map(|item| expanded_parallel_call(&item).unwrap_or_else(|| vec![item]))
        .collect()
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
    let output = normalized
        .get_mut("output")
        .and_then(Value::as_array_mut)
        .filter(|_| !has_terminal_choice);
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
            // Rig's terminal response conversion requires an id. This sentinel
            // uses the valid shape only for local conversion and is stripped
            // before the item is persisted/replayed to OpenAI.
            output.push(json!({
                "type": "message", "id": "msg_artist_streamed_message", "role": "assistant", "status": "completed",
                "content": [{"type": "output_text", "text": text, "annotations": [], "logprobs": []}]
            }));
        }
    }
    // The wrapper must not reach the sidecar: the calls we dispatch are the
    // expanded ones, so the persisted record has to name them too or their
    // results pair with nothing.
    if let Some(output) = normalized.get_mut("output").and_then(Value::as_array_mut) {
        *output = expand_parallel_calls(std::mem::take(output));
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

/// How much of `fresh` the provider has already been told.
///
/// Split out of [`reconcile_inputs`] because conversation chaining needs the
/// same number for the opposite purpose: reconciliation uses it to know what to
/// append, chaining uses it to know what to *omit*.
fn common_prefix(
    saved: &[Value],
    checkpoint: &[String],
    fresh: &[Value],
    fingerprints: &[String],
) -> usize {
    if checkpoint.is_empty() && !saved.is_empty() {
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
    }
}

/// A refusal that names the chain we tried to continue, rather than the request
/// itself.
///
/// Deliberately broad: an expired response, an unknown one and an endpoint that
/// never supported chaining all warrant the same action, and mistaking an
/// ordinary bad request for a chain problem costs only a needless full resend.
fn chain_refused(status: u16, body: &str) -> bool {
    matches!(status, 400 | 404) && body.contains("previous_response")
}

fn reconcile_inputs(
    saved: Vec<Value>,
    checkpoint: &[String],
    fresh: Vec<Value>,
    fingerprints: &[String],
) -> Vec<Value> {
    let common = common_prefix(&saved, checkpoint, &fresh, fingerprints);
    let mut merged = saved;
    // A diverged history is intentionally appended from its divergence point. In the
    // normal growing-history case this adds only genuinely new framework items.
    merged.extend(fresh.into_iter().skip(common));
    answer_orphaned_calls(merged)
}

/// Answer any `function_call` in the assembled input that has no
/// `function_call_output`.
///
/// The Responses API requires the pairing, and rejects the whole request with
/// `No tool output found for function call <id>` when it is missing. A turn
/// that ends between the two — the process dying, a cancellation, a panic
/// inside the tool — leaves the call unanswered in the canonical items, and
/// because those items are persisted, every later request on that conversation
/// is rejected the same way. The session is then permanently unusable.
///
/// Enforcing the pairing here makes it a property of what we send rather than
/// of how the previous turn happened to end, and repairs conversations already
/// carrying an unanswered call.
fn answer_orphaned_calls(input: Vec<Value>) -> Vec<Value> {
    let answered = input
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call_output"))
        .filter_map(|item| item.get("call_id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect::<std::collections::HashSet<_>>();

    let mut repaired = Vec::with_capacity(input.len());
    for item in input {
        let orphan = (item.get("type").and_then(Value::as_str) == Some("function_call"))
            .then(|| item.get("call_id").and_then(Value::as_str))
            .flatten()
            .filter(|call_id| !answered.contains(*call_id))
            .map(str::to_owned);
        repaired.push(item);
        if let Some(call_id) = orphan {
            repaired.push(json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": "The tool did not run to completion and produced no output.",
                "status": "completed"
            }));
        }
    }
    repaired
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
                        for call in
                            expanded_parallel_call(item).unwrap_or_else(|| vec![item.clone()])
                        {
                            let id = call
                                .get("id")
                                .and_then(Value::as_str)
                                .or_else(|| call.get("call_id").and_then(Value::as_str))
                                .unwrap_or("")
                                .to_owned();
                            let call_id = call
                                .get("call_id")
                                .and_then(Value::as_str)
                                .unwrap_or(&id)
                                .to_owned();
                            let args = serde_json::from_str(
                                call.get("arguments")
                                    .and_then(Value::as_str)
                                    .unwrap_or("{}"),
                            )?;
                            out.push(RawStreamingChoice::ToolCall(
                                RawStreamingToolCall::new(
                                    id,
                                    call.get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .into(),
                                    args,
                                )
                                .with_internal_call_id(call_id.clone())
                                .with_call_id(call_id),
                            ));
                        }
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

    /// The signal that drives chain invalidation.
    ///
    /// A history that still matches what the provider was told yields a prefix
    /// as long as the checkpoint. Anything shorter means the conversation was
    /// rewritten underneath it — a stream-rule replay, a compaction, a handoff —
    /// and a chain continued across that would show the model a conversation
    /// that never happened.
    #[test]
    fn an_unchanged_history_matches_the_whole_checkpoint() {
        let fresh = vec![
            json!({"role": "user", "content": "one"}),
            json!({"role": "user", "content": "two"}),
        ];
        let fingerprints: Vec<String> = fresh.iter().map(wire_fingerprint).collect();
        let checkpoint = fingerprints.clone();
        let saved = vec![json!({"role": "assistant", "content": "ok"})];

        let common = common_prefix(&saved, &checkpoint, &fresh, &fingerprints);
        assert_eq!(
            common,
            checkpoint.len(),
            "an intact history must not look rewritten"
        );
    }

    #[test]
    fn a_rewritten_history_falls_short_of_the_checkpoint() {
        let told = vec![
            json!({"role": "user", "content": "one"}),
            json!({"role": "user", "content": "two"}),
        ];
        let checkpoint: Vec<String> = told.iter().map(wire_fingerprint).collect();
        // The second turn was replaced, which is what a rule replay does.
        let fresh = vec![
            json!({"role": "user", "content": "one"}),
            json!({"role": "user", "content": "rewritten"}),
        ];
        let fingerprints: Vec<String> = fresh.iter().map(wire_fingerprint).collect();
        let saved = vec![json!({"role": "assistant", "content": "ok"})];

        let common = common_prefix(&saved, &checkpoint, &fresh, &fingerprints);
        assert!(
            common < checkpoint.len(),
            "a rewrite must be detected: {common}"
        );
    }

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

    /// A turn that dies between a tool call and its result leaves the call
    /// unanswered in the persisted canonical items. Every later request is then
    /// rejected with "No tool output found for function call", permanently.
    #[test]
    fn an_unanswered_tool_call_is_answered_before_sending() {
        let call = json!({"type":"function_call","id":"fc1","call_id":"call_ra8","name":"edit","arguments":"{}"});
        let merged = reconcile_inputs(vec![call.clone()], &[], Vec::new(), &[]);

        assert_eq!(merged.len(), 2, "the orphaned call must be answered");
        assert_eq!(merged[0], call);
        assert_eq!(merged[1]["type"], "function_call_output");
        assert_eq!(merged[1]["call_id"], "call_ra8");
    }

    #[test]
    fn an_already_answered_call_is_left_alone() {
        let call =
            json!({"type":"function_call","id":"fc1","call_id":"c","name":"edit","arguments":"{}"});
        let output = json!({"type":"function_call_output","call_id":"c","output":"ok"});
        let merged = reconcile_inputs(vec![call.clone(), output.clone()], &[], Vec::new(), &[]);
        assert_eq!(merged, vec![call, output]);
    }

    /// Parallel tool calls answered out of order still count as answered.
    #[test]
    fn each_call_is_matched_by_call_id_not_position() {
        let first =
            json!({"type":"function_call","id":"fc1","call_id":"a","name":"read","arguments":"{}"});
        let second =
            json!({"type":"function_call","id":"fc2","call_id":"b","name":"read","arguments":"{}"});
        let answer_b = json!({"type":"function_call_output","call_id":"b","output":"ok"});
        let merged = reconcile_inputs(vec![first, second, answer_b], &[], Vec::new(), &[]);

        let outputs: Vec<&str> = merged
            .iter()
            .filter(|item| item["type"] == "function_call_output")
            .map(|item| item["call_id"].as_str().unwrap())
            .collect();
        assert_eq!(
            outputs,
            ["a", "b"],
            "only the unanswered call gains an output"
        );
    }

    /// The pairing requirement stated over arbitrary item sequences: the
    /// provider rejects the entire request if any call is unanswered, so this
    /// has to hold for every history we can assemble, not just the cases
    /// someone thought to enumerate.
    mod properties {
        use super::*;
        use proptest::prelude::*;

        #[derive(Debug, Clone)]
        enum Item {
            Call(u8),
            Answer(u8),
            Message,
        }

        fn item(kind: &Item) -> Value {
            match kind {
                Item::Call(id) => json!({
                    "type": "function_call", "id": format!("fc{id}"),
                    "call_id": format!("c{id}"), "name": "read", "arguments": "{}"
                }),
                Item::Answer(id) => json!({
                    "type": "function_call_output",
                    "call_id": format!("c{id}"), "output": "ok"
                }),
                Item::Message => json!({
                    "type": "message", "role": "assistant", "content": []
                }),
            }
        }

        proptest! {
            #[test]
            fn every_call_is_answered_after_reconciling(
                items in prop::collection::vec(
                    prop_oneof![
                        (0u8..4).prop_map(Item::Call),
                        (0u8..4).prop_map(Item::Answer),
                        Just(Item::Message),
                    ],
                    0..12,
                )
            ) {
                let input: Vec<Value> = items.iter().map(item).collect();
                let calls_in = input
                    .iter()
                    .filter(|entry| entry["type"] == "function_call")
                    .count();

                let merged = reconcile_inputs(input, &[], Vec::new(), &[]);

                let answered: std::collections::HashSet<&str> = merged
                    .iter()
                    .filter(|entry| entry["type"] == "function_call_output")
                    .filter_map(|entry| entry["call_id"].as_str())
                    .collect();
                let calls_out = merged
                    .iter()
                    .filter(|entry| entry["type"] == "function_call")
                    .count();

                prop_assert_eq!(calls_in, calls_out, "reconciling must not drop a call");
                for entry in &merged {
                    if entry["type"] == "function_call" {
                        let call_id = entry["call_id"].as_str().unwrap();
                        prop_assert!(answered.contains(call_id), "call {} left unanswered", call_id);
                    }
                }
            }
        }
    }

    fn wrapper(call_id: &str) -> Value {
        json!({
            "type": "function_call",
            "id": "fc_1",
            "call_id": call_id,
            "name": "multi_tool_use.parallel",
            "arguments": json!({"tool_uses": [
                {"recipient_name": "functions.read", "parameters": {"path": "a.rs"}},
                {"recipient_name": "functions.grep", "parameters": {"query": "Foo"}}
            ]}).to_string()
        })
    }

    /// OpenAI injects `multi_tool_use.parallel` beside our tools, so the model
    /// can batch calls into one we have no tool registered for. Expanding it
    /// names the real tools instead.
    #[test]
    fn an_injected_parallel_wrapper_expands_into_the_calls_it_names() {
        let calls = expanded_parallel_call(&wrapper("call_a")).unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0]["name"], "read",
            "the functions. prefix is stripped"
        );
        assert_eq!(calls[1]["name"], "grep");
        assert_eq!(
            calls[0]["call_id"], "call_a.0",
            "ids derive from the wrapper"
        );
        assert_eq!(calls[1]["call_id"], "call_a.1");
        // `arguments` is a JSON string on the wire, not an object.
        assert_eq!(
            serde_json::from_str::<Value>(calls[0]["arguments"].as_str().unwrap()).unwrap(),
            json!({"path": "a.rs"})
        );
    }

    #[test]
    fn an_ordinary_call_is_left_alone() {
        let call =
            json!({"type":"function_call","id":"fc1","call_id":"c","name":"read","arguments":"{}"});
        assert!(expanded_parallel_call(&call).is_none());
        assert_eq!(expand_parallel_calls(vec![call.clone()]), vec![call]);
    }

    /// A wrapper we cannot parse stays intact rather than vanishing: an
    /// unknown-tool error is recoverable, a silently dropped call is not.
    #[test]
    fn a_malformed_wrapper_is_not_dropped() {
        let broken = json!({
            "type": "function_call", "id": "fc_1", "call_id": "c",
            "name": "multi_tool_use.parallel", "arguments": "{\"nope\":1}"
        });
        assert!(expanded_parallel_call(&broken).is_none());
    }

    /// The dispatched calls and the persisted canonical items must agree, or
    /// each result pairs with a call the provider never saw.
    #[test]
    fn the_stream_and_the_canonical_items_derive_the_same_ids() {
        let event = json!({"type": "response.output_item.done", "item": wrapper("call_a")});
        let streamed = parse_event(&event.to_string()).unwrap();
        let stream_ids: Vec<String> = streamed
            .iter()
            .filter_map(|choice| match choice {
                RawStreamingChoice::ToolCall(call) => call.call_id.clone(),
                _ => None,
            })
            .collect();

        let wire = normalized_terminal_wire(&json!({"output": [wrapper("call_a")]}), "", &[]);
        let canonical_ids: Vec<String> = wire["output"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["call_id"].as_str().unwrap().to_owned())
            .collect();

        assert_eq!(stream_ids, ["call_a.0", "call_a.1"]);
        assert_eq!(stream_ids, canonical_ids);
    }

    /// With both sides expanded, the results pair up and nothing is synthesized.
    #[test]
    fn expanded_calls_need_no_orphan_repair_once_answered() {
        let calls = expanded_parallel_call(&wrapper("call_a")).unwrap();
        let answers: Vec<Value> = calls
            .iter()
            .map(|call| {
                json!({
                    "type": "function_call_output",
                    "call_id": call["call_id"],
                    "output": "ok"
                })
            })
            .collect();
        let mut saved = calls.clone();
        saved.extend(answers);

        let merged = reconcile_inputs(saved.clone(), &[], Vec::new(), &[]);
        assert_eq!(merged, saved, "every expanded call is already answered");
    }
}
