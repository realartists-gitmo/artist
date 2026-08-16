//! The Artist agent loop, built on Rig.

mod capture;
mod conversation;
mod lifecycle;
pub mod openai_responses;
mod prompt_config;
mod provider_retry;
mod resource_tool;
mod rig_provider;
mod steering;

pub use lifecycle::{LifecycleEmitter, LifecycleEvent};
pub use steering::SteeringHandle;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result, anyhow};
use artist_session::{Recorder, RunFinished, RunStarted, ToolOutcomeRecord};
use base64::Engine;
use futures::StreamExt;
use llm_provider::SavedProvider;
use rig_agent::client::AgentClientExt;
use rig_agent::{agent::MultiTurnStreamItem, streaming::StreamingPrompt};
use rig_core::{
    OneOrMany,
    client::CompletionClient,
    completion::message::{
        DocumentSourceKind, Image, ImageMediaType, Message, ToolResultContent, UserContent,
    },
    memory::{ConversationMemory, InMemoryConversationMemory},
    streaming::{StreamedAssistantContent, StreamedUserContent},
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use capture::{CaptureHook, ToolMeta};
use resource_tool::{BatchedRunEvent, named_tools, run_batched_agent};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptEvent {
    ReasoningSummaryDelta(String),
    TextDelta(String),
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    ToolExecutionStart {
        id: String,
        name: String,
    },
    ToolResult {
        id: String,
        content: String,
        /// Structured outcome from the capture hook, when recording is on.
        outcome: Option<ToolOutcomeRecord>,
        duration_ms: Option<u64>,
        /// Count of image content items in the result (rendered as a marker;
        /// image payloads ride the event log, not the display stream).
        images: usize,
    },
    CompletionUsage {
        total_tokens: u64,
    },
}

/// How a `stream_chat` run ended (errors surface via `Result`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Cancelled,
}

/// Everything a run needs beyond the prompt: shared handles owned by the
/// CLI session. `Default` gives inert handles (no recording, no
/// cancellation) — the configuration tests and simple embedders want.
#[derive(Clone)]
pub struct SessionHandles {
    /// The universal resource kernel from which named model tools are built
    /// for each agent attempt.
    pub kernel: artist_kernel::Kernel,
    pub steering: SteeringHandle,
    pub recorder: Recorder,
    pub memory: Arc<dyn ConversationMemory>,
    pub conversation_id: String,
    /// Provider-private opaque context; currently only consumed by the opt-in OpenAI adapter.
    pub provider_context: artist_session::ProviderContextHandle,
    /// Request OpenAI priority processing for this session.
    pub fast_mode: bool,
    /// Non-display lifecycle notifications remain live across turns.
    pub lifecycle: LifecycleEmitter,
    pub cancel: CancellationToken,
}

impl Default for SessionHandles {
    fn default() -> Self {
        Self {
            kernel: artist_kernel::Kernel::new(),
            steering: SteeringHandle::default(),
            recorder: Recorder::noop(),
            memory: Arc::new(InMemoryConversationMemory::new()),
            conversation_id: "default".to_owned(),
            provider_context: artist_session::ProviderContextHandle::noop(),
            fast_mode: false,
            lifecycle: LifecycleEmitter::default(),
            cancel: CancellationToken::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

impl ChatMessage {
    /// Convert the compact text message to Rig's provider message type.
    pub fn to_rig(&self) -> Message {
        match self.role {
            ChatRole::User => Message::user(&self.content),
            ChatRole::Assistant => Message::assistant(&self.content),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ImageAttachment {
    pub data: Vec<u8>,
    pub media_type: ImageMediaType,
}

impl ImageAttachment {
    pub fn png(data: Vec<u8>) -> Self {
        Self {
            data,
            media_type: ImageMediaType::PNG,
        }
    }
    pub fn jpeg(data: Vec<u8>) -> Self {
        Self {
            data,
            media_type: ImageMediaType::JPEG,
        }
    }
    pub fn gif(data: Vec<u8>) -> Self {
        Self {
            data,
            media_type: ImageMediaType::GIF,
        }
    }
    pub fn webp(data: Vec<u8>) -> Self {
        Self {
            data,
            media_type: ImageMediaType::WEBP,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChatInput {
    pub text: String,
    pub images: Vec<ImageAttachment>,
}

impl From<String> for ChatInput {
    fn from(text: String) -> Self {
        Self {
            text,
            images: Vec::new(),
        }
    }
}

/// Sends the small completion used by provider health checks through the same
/// typed Rig client dispatch as normal runs.
pub async fn provider_health_check(provider: &SavedProvider, model: &str) -> Result<String> {
    provider_health_check_with_device_flow(provider, model, false).await
}

/// Interactive health check; only this explicit CLI path may start device OAuth.
pub async fn provider_health_check_with_device_flow(
    provider: &SavedProvider,
    model: &str,
    allow_device_flow: bool,
) -> Result<String> {
    rig_provider::RigClient::build_with_device_flow(provider, allow_device_flow)?
        .prompt(
            model,
            "Reply with exactly OK and nothing else.",
            "Reply with exactly OK.",
            16,
        )
        .await
}

/// Executes one prompt and emits model output as it arrives. Rig loads and
/// persists the conversation through [`SessionHandles::memory`].
pub async fn stream_chat(
    provider: &SavedProvider,
    input: &ChatInput,
    handles: SessionHandles,
    on_event: impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome> {
    match rig_provider::RigClient::build(provider)? {
        rig_provider::RigClient::ArtistOpenAi(client) => {
            let client = client.with_provider_context(
                handles.conversation_id.clone(),
                handles.provider_context.clone(),
            );
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Copilot(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::OpenAiChat(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Anthropic(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Cohere(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Gemini(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::DeepSeek(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Groq(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::HuggingFace(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Hyperbolic(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Mira(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Mistral(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::OpenRouter(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Perplexity(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Together(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::XAi(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Azure(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Llamafile(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Ollama(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Minimax(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::MinimaxAnthropic(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::Moonshot(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::MoonshotAnthropic(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::XiaomiMiMo(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::XiaomiMiMoAnthropic(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::ZAi(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
        rig_provider::RigClient::ZAiAnthropic(client) => {
            stream_chat_with(client, provider, input, handles, on_event).await
        }
    }
}

async fn stream_chat_with<C: CompletionClient>(
    client: C,
    provider: &SavedProvider,
    input: &ChatInput,
    handles: SessionHandles,
    mut on_event: impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome>
where
    C::CompletionModel: 'static,
{
    let model = provider
        .model
        .as_deref()
        .context("no model selected; run `artist model` first")?;

    let seed_history = handles
        .memory
        .load(&handles.conversation_id)
        .await
        .context("load conversation memory")?;
    let durable_history_len = seed_history.len();
    let seed_prompt = user_message(input);
    let tool_meta = ToolMeta::default();

    // Keep cache affinity within a conversation rather than pinning every main
    // agent in the project to the same provider route.
    let mut overload_retry = provider_retry::OverloadRetry::new(
        std::path::Path::new("."),
        model,
        &format!("main:{}", handles.conversation_id),
    );
    'retry: loop {
        let run_id = format!("r-{}", uuid::Uuid::new_v4().simple());
        let run_recorder = handles.recorder.with_run(&run_id);

        let custom_params = request_params(
            provider.provider,
            provider.api,
            overload_retry.cache_key(),
            provider.reasoning_effort.as_deref(),
            handles.fast_mode,
        );
        let mut builder = client.agent(model);
        // These fields belong to the ChatGPT subscription transport. Keep them
        // off OpenAI Responses and Chat Completions requests, whose accepted
        // parameter shapes differ.
        if let Some(params) = custom_params.clone() {
            builder = builder.additional_params(params);
        }
        let (main_prompt, prompt_diagnostics) = prompt_config::main_prompt();
        let prompt_diagnostics = prompt_diagnostics
            .iter()
            .map(|d| format!("<diagnostic>{}</diagnostic>", d))
            .collect::<String>();
        let resource_catalog = handles
            .kernel
            .resource_catalog()
            .await
            .into_iter()
            .map(|entry| {
                let docs = entry
                    .docs
                    .into_iter()
                    .map(|doc| {
                        let verbs = if doc.verbs.is_empty() {
                            String::new()
                        } else {
                            format!(" [{}]", doc.verbs.join(" "))
                        };
                        let query = if doc.query.is_empty() {
                            String::new()
                        } else {
                            format!(" query: {}", doc.query.join(" "))
                        };
                        format!("\n  {}{}{}\n    {}", doc.uri, verbs, query, doc.summary)
                    })
                    .collect::<String>();
                format!("\n{}\n  {}{}", entry.name, entry.description, docs)
            })
            .collect::<String>();
        let resource_prompt = if resource_catalog.is_empty() {
            String::new()
        } else {
            format!("\n\nResources:{}", resource_catalog)
        };
        let system_prompt = format!(
            "{}\n\n{}{}{}{}\nCurrent working directory: {}",
            main_prompt,
            prompt_diagnostics,
            "Use the registered named tools for filesystem, repository, and live-resource operations. Tool definitions may be inspected and modified through the tools:// namespace when explicitly needed.",
            "",
            resource_prompt,
            std::path::Path::new(".").display()
        );
        let persistence = conversation::PersistenceStatus::default();
        let attempt_memory = conversation::AttemptMemory::new(
            Arc::clone(&handles.memory),
            handles.conversation_id.clone(),
            seed_history.clone(),
            durable_history_len,
            persistence.clone(),
        );
        let invocation_context = artist_kernel::InvocationContext {
            working_uri: None,
            environment: Vec::new(),
            cancellation_token: Some(format!("run:{run_id}")),
            deadline_ms: None,
            correlation_id: Some(run_id.clone()),
        };
        let tool_catalog = handles.kernel.tool_definitions().await;
        let dynamic_tools = named_tools(
            handles.kernel.clone(),
            invocation_context.clone(),
            handles.cancel.clone(),
            tool_catalog.clone(),
        )
        .await;
        let agent = builder
            .preamble(&system_prompt)
            .dynamic_tools(dynamic_tools.clone())
            .memory(attempt_memory)
            .conversation(handles.conversation_id.clone())
            .add_hook(steering::SteeringHook(handles.steering.clone()))
            .add_hook(CaptureHook::new(tool_meta.clone()))
            .default_max_turns(usize::MAX)
            .build();

        run_recorder.record(RunStarted {
            provider: format!("{:?}", provider.provider).to_lowercase(),
            model: model.to_owned(),
            reasoning_effort: provider.reasoning_effort.clone(),
        });

        // The sans-IO driver receives the entire CallTools set from Rig's
        // AgentRun and executes it through Artist's grouped batch boundary.
        // This is the only execution path for the configured agent.
        if batched_driver_enabled() {
            let attempt_observed = Arc::new(AtomicBool::new(false));
            let attempt_observed_for_events = Arc::clone(&attempt_observed);
            let batched = run_batched_agent(
                client.completion_model(model),
                seed_prompt.clone(),
                seed_history.clone(),
                Some(system_prompt),
                dynamic_tools,
                tool_catalog,
                handles.kernel.clone(),
                invocation_context,
                handles.cancel.clone(),
                custom_params,
                || handles.steering.take_for_batched(),
                |event| match event {
                    BatchedRunEvent::ToolCall {
                        id,
                        name,
                        arguments,
                    } => {
                        attempt_observed_for_events.store(true, Ordering::Relaxed);
                        on_event(PromptEvent::ToolCall {
                            id,
                            name,
                            arguments,
                        })
                        .map_err(|error| error.to_string())
                    }
                    BatchedRunEvent::ToolExecutionStart { id, name } => {
                        attempt_observed_for_events.store(true, Ordering::Relaxed);
                        handles
                            .lifecycle
                            .emit(LifecycleEvent::ToolStarted(format!("main:{id}")));
                        on_event(PromptEvent::ToolExecutionStart { id, name })
                            .map_err(|error| error.to_string())
                    }
                    BatchedRunEvent::ToolResult {
                        id,
                        content,
                        outcome,
                        duration_ms,
                    } => {
                        attempt_observed_for_events.store(true, Ordering::Relaxed);
                        tool_meta.record(id.clone(), outcome.clone(), duration_ms);
                        handles
                            .lifecycle
                            .emit(LifecycleEvent::ToolFinished(format!("main:{id}")));
                        on_event(PromptEvent::ToolResult {
                            id,
                            content,
                            outcome: Some(outcome),
                            duration_ms: Some(duration_ms),
                            images: 0,
                        })
                        .map_err(|error| error.to_string())
                    }
                    BatchedRunEvent::Text(text) => {
                        attempt_observed_for_events.store(true, Ordering::Relaxed);
                        on_event(PromptEvent::TextDelta(text)).map_err(|error| error.to_string())
                    }
                    BatchedRunEvent::Reasoning(reasoning) => {
                        attempt_observed_for_events.store(true, Ordering::Relaxed);
                        on_event(PromptEvent::ReasoningSummaryDelta(reasoning))
                            .map_err(|error| error.to_string())
                    }
                    BatchedRunEvent::CompletionUsage(total_tokens) => {
                        on_event(PromptEvent::CompletionUsage { total_tokens })
                            .map_err(|error| error.to_string())
                    }
                },
            )
            .await;
            let (_output, run_messages) = match batched {
                Ok(value) => value,
                Err(error)
                    if !handles.cancel.is_cancelled()
                        && !error.message.contains("agent run aborted")
                        && !attempt_observed.load(Ordering::Relaxed)
                        && provider_retry::is_overload(provider.provider, &error)
                        && let Some(delay) = overload_retry.schedule() =>
                {
                    tokio::time::sleep(delay).await;
                    continue 'retry;
                }
                Err(error)
                    if handles.cancel.is_cancelled() || error.message == "agent run aborted" =>
                {
                    let retained = if error.messages.is_empty() {
                        vec![seed_prompt.clone()]
                    } else {
                        error.messages
                    };
                    conversation::retain_cancelled_turn(
                        handles.memory.as_ref(),
                        &handles.conversation_id,
                        retained,
                        String::new(),
                    )
                    .await
                    .context("retain cancelled batched turn")?;
                    run_recorder.record(RunFinished::Cancelled);
                    return Ok(RunOutcome::Cancelled);
                }
                Err(error) => return Err(anyhow!(error.message)),
            };
            handles
                .memory
                .append(&handles.conversation_id, run_messages)
                .await
                .context("persist batched agent turn")?;
            run_recorder.record(RunFinished::Completed);
            return Ok(RunOutcome::Completed);
        }

        let mut stream = agent.stream_prompt(seed_prompt.clone()).await;
        let mut streamed_assistant_text = String::new();
        // Provider overloads are retried only while pristine.
        let mut attempt_observed = false;
        loop {
            let item = tokio::select! {
                biased;
                item = stream.next() => item,
                _ = handles.cancel.cancelled() => {
                    drop(stream);
                    // Rig commits completed tool round trips straight through
                    // AttemptMemory, so the durable session memory is the best
                    // committed-state available on cancel.
                    let committed = handles
                        .memory
                        .load(&handles.conversation_id)
                        .await
                        .unwrap_or_else(|_| seed_history.clone());
                    let mut cancelled_delta = if committed.len() > durable_history_len {
                        committed
                    } else {
                        let mut fallback = seed_history.clone();
                        fallback.push(seed_prompt.clone());
                        fallback
                    };
                    cancelled_delta = cancelled_delta
                        [durable_history_len.min(cancelled_delta.len())..]
                        .to_vec();
                    conversation::retain_cancelled_turn(
                        handles.memory.as_ref(),
                        &handles.conversation_id,
                        cancelled_delta,
                        streamed_assistant_text,
                    )
                    .await
                    .context("retain cancelled turn in conversation memory")?;
                    run_recorder.record(RunFinished::Cancelled);
                    return Ok(RunOutcome::Cancelled);
                }
            };
            let Some(item) = item else {
                let error = anyhow!("Rig stream ended without a final response");
                run_recorder.record(RunFinished::Error {
                    error: error.to_string(),
                });
                let mut interrupted_delta = seed_history.clone();
                interrupted_delta.push(seed_prompt.clone());
                interrupted_delta =
                    interrupted_delta[durable_history_len.min(interrupted_delta.len())..].to_vec();
                conversation::retain_provider_interrupted_turn(
                    handles.memory.as_ref(),
                    &handles.conversation_id,
                    interrupted_delta,
                    streamed_assistant_text,
                    &error.to_string(),
                )
                .await
                .context("retain prematurely-ended turn in conversation memory")?;
                return Err(error);
            };
            if item.is_ok() {
                attempt_observed = true;
            }
            match item {
                Ok(MultiTurnStreamItem::FinalResponse(_)) => {
                    if let Err(error) = persistence.result() {
                        run_recorder.record(RunFinished::Error {
                            error: error.clone(),
                        });
                        return Err(anyhow!(error)).context("persist conversation memory");
                    }
                    run_recorder.record(RunFinished::Completed);
                    return Ok(RunOutcome::Completed);
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(
                    text,
                ))) => {
                    streamed_assistant_text.push_str(&text.text);
                    on_event(PromptEvent::TextDelta(text.text))?;
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(
                    StreamedAssistantContent::ReasoningDelta {
                        // Handle both summary deltas (`id: None`) and raw
                        // reasoning deltas (`id: Some`) for the live UI.
                        id: _,
                        reasoning,
                    },
                )) => {
                    on_event(PromptEvent::ReasoningSummaryDelta(reasoning))?;
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(
                    StreamedAssistantContent::ToolCall {
                        tool_call,
                        internal_call_id,
                    },
                )) => on_event(PromptEvent::ToolCall {
                    id: internal_call_id,
                    name: tool_call.function.name,
                    arguments: tool_call.function.arguments,
                })?,
                Ok(MultiTurnStreamItem::ToolExecutionCommitted {
                    tool_call,
                    internal_call_id,
                }) => {
                    handles.lifecycle.emit(LifecycleEvent::ToolStarted(format!(
                        "main:{internal_call_id}"
                    )));
                    on_event(PromptEvent::ToolExecutionStart {
                        id: internal_call_id,
                        name: tool_call.function.name,
                    })?;
                }
                Ok(MultiTurnStreamItem::CompletionCall(call)) => {
                    on_event(PromptEvent::CompletionUsage {
                        total_tokens: call.usage.total_tokens,
                    })?;
                }
                Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                    tool_result,
                    internal_call_id,
                })) => {
                    let mut images = 0usize;
                    let content = tool_result
                        .content
                        .into_iter()
                        .filter_map(|item| match item {
                            ToolResultContent::Text(text) => Some(text.text),
                            ToolResultContent::Image(_) => {
                                images += 1;
                                None
                            }
                            ToolResultContent::Json { value } => Some(value.to_string()),
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let content = handles
                        .steering
                        .take_original_result(&internal_call_id)
                        .unwrap_or(content);
                    let meta = tool_meta.take(&internal_call_id);
                    handles.lifecycle.emit(LifecycleEvent::ToolFinished(format!(
                        "main:{internal_call_id}"
                    )));
                    on_event(PromptEvent::ToolResult {
                        id: internal_call_id,
                        content,
                        outcome: meta.as_ref().map(|(outcome, _)| outcome.clone()),
                        duration_ms: meta.map(|(_, duration)| duration),
                        images,
                    })?;
                }
                Ok(_) => {}
                Err(error) => {
                    let error_text = error.to_string();
                    run_recorder.record(RunFinished::Error {
                        error: error_text.clone(),
                    });
                    if !attempt_observed
                        && provider_retry::is_overload(provider.provider, &error)
                        && let Some(delay) = overload_retry.schedule()
                    {
                        drop(stream);
                        tokio::select! {
                            _ = tokio::time::sleep(delay) => continue 'retry,
                            _ = handles.cancel.cancelled() => {
                                let mut cancelled_delta = seed_history.clone();
                                cancelled_delta.push(seed_prompt.clone());
                                cancelled_delta = cancelled_delta
                                    [durable_history_len.min(cancelled_delta.len())..]
                                    .to_vec();
                                conversation::retain_cancelled_turn(
                                    handles.memory.as_ref(),
                                    &handles.conversation_id,
                                    cancelled_delta,
                                    String::new(),
                                )
                                .await
                                .context("retain cancelled turn during provider retry")?;
                                return Ok(RunOutcome::Cancelled);
                            }
                        }
                    }

                    // Preserve the prompt and any partial answer just like a
                    // user cancellation, but tell the next turn why it ended.
                    let mut interrupted_delta = seed_history.clone();
                    interrupted_delta.push(seed_prompt.clone());
                    interrupted_delta = interrupted_delta
                        [durable_history_len.min(interrupted_delta.len())..]
                        .to_vec();
                    conversation::retain_provider_interrupted_turn(
                        handles.memory.as_ref(),
                        &handles.conversation_id,
                        interrupted_delta,
                        streamed_assistant_text,
                        &error_text,
                    )
                    .await
                    .context("retain provider-interrupted turn in conversation memory")?;
                    return Err(error).context("stream Artist agent");
                }
            }
        }
    }
}

fn batched_driver_enabled() -> bool {
    true
}

/// Provider parameters shared by every request attempt in a turn.
pub(crate) fn request_params(
    provider: llm_provider::ProviderKind,
    api: Option<llm_provider::OpenAiApi>,
    cache_key: &str,
    reasoning_effort: Option<&str>,
    fast_mode: bool,
) -> Option<serde_json::Value> {
    if provider == llm_provider::ProviderKind::Openai
        && api.unwrap_or_default() == llm_provider::OpenAiApi::ChatCompletions
    {
        return fast_mode.then(|| json!({ "service_tier": "priority" }));
    }
    if provider != llm_provider::ProviderKind::Chatgpt
        && !(provider == llm_provider::ProviderKind::Openai
            && api.unwrap_or_default() == llm_provider::OpenAiApi::Responses)
    {
        return None;
    }
    let mut params = json!({
        "store": false,
        "include": ["reasoning.encrypted_content"],
        "prompt_cache_key": cache_key,
    });
    // Request a provider-generated trace for the live UI even when the model's
    // default effort is in use. Rig's memory policy is independent: streaming
    // this summary does not make the CLI responsible for model context.
    params["reasoning"] = match reasoning_effort.filter(|effort| {
        matches!(
            *effort,
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
        )
    }) {
        Some(effort) => json!({ "effort": effort, "summary": "auto", "context": "all_turns" }),
        None => json!({ "summary": "auto", "context": "all_turns" }),
    };
    if fast_mode
        && matches!(
            provider,
            llm_provider::ProviderKind::Openai | llm_provider::ProviderKind::Chatgpt
        )
    {
        params["service_tier"] = json!("priority");
    }
    Some(params)
}

pub(crate) fn user_message(input: &ChatInput) -> Message {
    let mut content = vec![UserContent::text(input.text.clone())];
    content.extend(input.images.iter().map(|attachment| {
        UserContent::Image(Image {
            data: DocumentSourceKind::Base64(
                base64::engine::general_purpose::STANDARD.encode(&attachment.data),
            ),
            media_type: Some(attachment.media_type.clone()),
            detail: None,
            additional_params: None,
        })
    }));
    Message::User {
        content: OneOrMany::many(content).expect("chat input always contains text"),
    }
}

#[cfg(test)]
mod tests {
    use super::request_params;
    use llm_provider::ProviderKind;

    #[test]
    fn reasoning_requests_a_live_summary_trace() {
        let params =
            request_params(ProviderKind::Chatgpt, None, "cache", Some("high"), false).unwrap();
        assert_eq!(params["reasoning"]["effort"], "high");
        assert_eq!(params["reasoning"]["summary"], "auto");

        let default_effort =
            request_params(ProviderKind::Chatgpt, None, "cache", None, false).unwrap();
        assert_eq!(default_effort["reasoning"]["summary"], "auto");
        assert!(default_effort["reasoning"].get("effort").is_none());
    }

    #[test]
    fn responses_policy_is_sent_only_to_responses_transports() {
        assert_eq!(
            request_params(
                ProviderKind::Openai,
                Some(llm_provider::OpenAiApi::Responses),
                "cache",
                Some("high"),
                false,
            )
            .unwrap()["prompt_cache_key"],
            "cache"
        );
        assert!(
            request_params(
                ProviderKind::Openai,
                Some(llm_provider::OpenAiApi::ChatCompletions),
                "cache",
                None,
                false,
            )
            .is_none()
        );
    }

    #[test]
    fn fast_mode_requests_openai_priority_tier() {
        let params = request_params(
            ProviderKind::Openai,
            Some(llm_provider::OpenAiApi::Responses),
            "cache",
            None,
            true,
        )
        .unwrap();
        assert_eq!(params["service_tier"], "priority");

        let normal = request_params(
            ProviderKind::Openai,
            Some(llm_provider::OpenAiApi::Responses),
            "cache",
            None,
            false,
        )
        .unwrap();
        assert!(normal.get("service_tier").is_none());

        let subscription =
            request_params(ProviderKind::Chatgpt, None, "cache", None, true).unwrap();
        assert_eq!(subscription["service_tier"], "priority");
    }
}
