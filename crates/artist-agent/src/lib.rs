//! The Artist agent loop, built on Rig.

mod capture;
pub mod compaction;
mod conversation;
mod delegate;
mod delegate_jobs;
#[cfg(test)]
mod delegate_tests;
pub mod mcp;
mod prompt_config;
mod resources;
mod rig_provider;
mod ttsr;
#[cfg(test)]
mod ttsr_tests;

pub use resources::AvailableSkill;
mod steering;
mod subagents;
mod tool_prompt;

pub use steering::SteeringHandle;

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use artist_rules::matcher::RuleSet;
use artist_rules::state::RulesHandle;
use artist_rules::types::Firing;
use artist_session::{
    Recorder, RuleFired, RuleInjection, RunFinished, RunStarted, ToolOutcomeRecord,
};
use artist_tools::ToolBundle;
use base64::Engine;
use futures::StreamExt;
use llm_provider::{ProviderKind, SavedProvider};
use rig_core::{
    OneOrMany,
    agent::MultiTurnStreamItem,
    client::CompletionClient,
    completion::message::{
        DocumentSourceKind, Image, ImageMediaType, Message, ToolResultContent, UserContent,
    },
    memory::{ConversationMemory, InMemoryConversationMemory},
    streaming::{StreamedAssistantContent, StreamedUserContent, StreamingPrompt},
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use capture::{CaptureHook, ToolMeta};
use ttsr::{TtsrHook, TtsrShared, reminder_message};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptEvent {
    ReasoningSummaryDelta(String),
    TextDelta(String),
    /// A delegated agent began running. Its subsequent stream events are
    /// wrapped in [`PromptEvent::SubagentEvent`] with the same id.
    SubagentStarted {
        id: String,
        role: String,
        prompt: String,
    },
    /// One event from a delegated agent's own streaming tool loop.
    SubagentEvent {
        id: String,
        event: Box<PromptEvent>,
    },
    /// A delegated agent stopped. `outcome` mirrors the persisted
    /// `delegate.finished` lifecycle value.
    SubagentFinished {
        id: String,
        outcome: String,
    },
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
    /// A stream rule matched: the run aborted, the reminder was injected,
    /// and the run is retrying from the same point. The UI should clear any
    /// partial streaming output and show the rule card.
    RuleFired {
        rule: String,
        matched: String,
    },
}

/// How a `stream_chat` run ended (errors surface via `Result`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Cancelled,
}

/// Everything a run needs beyond the prompt: shared handles owned by the
/// CLI session. `Default` gives inert handles (no recording, no rules, no
/// cancellation) — the configuration tests and simple embedders want.
#[derive(Clone)]
pub struct SessionHandles {
    pub steering: SteeringHandle,
    pub rules: RulesHandle,
    pub rule_set: Arc<RuleSet>,
    pub recorder: Recorder,
    pub memory: Arc<dyn ConversationMemory>,
    pub conversation_id: String,
    pub cancel: CancellationToken,
}

impl Default for SessionHandles {
    fn default() -> Self {
        Self {
            steering: SteeringHandle::default(),
            rules: RulesHandle::default(),
            rule_set: Arc::new(RuleSet::compile(Vec::new())),
            recorder: Recorder::noop(),
            memory: Arc::new(InMemoryConversationMemory::new()),
            conversation_id: "default".to_owned(),
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
    /// Text-only rig message (legacy sessions and simple callers).
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

/// The tool surfaces available to a run: native tools, MCP proxies,
/// extension-provided tools, and the user's disabled-tool list.
pub struct ToolContext<'a> {
    pub native: &'a ToolBundle,
    pub mcp: &'a mcp::McpManager,
    pub extensions: Option<&'a artist_extensions::Manager>,
    pub disabled: &'a [String],
}

/// Sends the small completion used by provider health checks through the same
/// typed Rig client dispatch as normal and delegated runs.
pub async fn provider_health_check(provider: &SavedProvider, model: &str) -> Result<String> {
    rig_provider::health_check_prompt(
        provider,
        model,
        "Reply with exactly OK and nothing else.",
        "Reply with exactly OK.",
        16,
    )
    .await
}

/// Interactive health check; only this explicit CLI path may start device OAuth.
pub async fn provider_health_check_with_device_flow(
    provider: &SavedProvider,
    model: &str,
    allow_device_flow: bool,
) -> Result<String> {
    match provider.provider {
        ProviderKind::Copilot if allow_device_flow => {
            rig_provider::prompt_with(
                rig_provider::build_copilot_with_device_flow(provider)?,
                model,
                "Reply with exactly OK and nothing else.",
                "Reply with exactly OK.",
                16,
            )
            .await
        }
        _ => provider_health_check(provider, model).await,
    }
}

/// Executes one prompt and emits model output as it arrives. Rig loads and
/// persists the conversation through [`SessionHandles::memory`].
pub async fn stream_chat(
    provider: &SavedProvider,
    input: &ChatInput,
    tool_context: ToolContext<'_>,
    handles: SessionHandles,
    on_event: impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome> {
    use rig_provider::{
        build_anthropic, build_azure, build_chatgpt, build_cohere, build_copilot, build_deepseek,
        build_gemini, build_groq, build_huggingface, build_hyperbolic, build_llamafile,
        build_minimax, build_minimax_anthropic, build_mira, build_mistral, build_moonshot,
        build_moonshot_anthropic, build_ollama, build_openai_chat, build_openai_responses,
        build_openrouter, build_perplexity, build_together, build_xai, build_xiaomimimo,
        build_xiaomimimo_anthropic, build_zai, build_zai_anthropic,
    };
    use llm_provider::OpenAiApi;

    match provider.provider {
        ProviderKind::Chatgpt => {
            stream_chat_with(build_chatgpt(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Copilot => {
            stream_chat_with(build_copilot(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Azure => {
            stream_chat_with(build_azure(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Llamafile => {
            stream_chat_with(build_llamafile(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Ollama => {
            stream_chat_with(build_ollama(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Openai => match provider.api.unwrap_or_default() {
            OpenAiApi::Responses => {
                stream_chat_with(build_openai_responses(provider)?, provider, input, tool_context, handles, on_event).await
            }
            OpenAiApi::ChatCompletions => {
                stream_chat_with(build_openai_chat(provider)?, provider, input, tool_context, handles, on_event).await
            }
        },
        ProviderKind::Anthropic => {
            stream_chat_with(build_anthropic(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Cohere => {
            stream_chat_with(build_cohere(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Gemini => {
            stream_chat_with(build_gemini(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Deepseek => {
            stream_chat_with(build_deepseek(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Groq => {
            stream_chat_with(build_groq(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Huggingface => {
            stream_chat_with(build_huggingface(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Hyperbolic => {
            stream_chat_with(build_hyperbolic(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Mira => {
            stream_chat_with(build_mira(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Mistral => {
            stream_chat_with(build_mistral(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Openrouter => {
            stream_chat_with(build_openrouter(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Perplexity => {
            stream_chat_with(build_perplexity(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Together => {
            stream_chat_with(build_together(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Xai => {
            stream_chat_with(build_xai(provider)?, provider, input, tool_context, handles, on_event).await
        }
        ProviderKind::Minimax => match provider.api.unwrap_or_default() {
            OpenAiApi::Responses => {
                stream_chat_with(build_minimax(provider)?, provider, input, tool_context, handles, on_event).await
            }
            OpenAiApi::ChatCompletions => {
                stream_chat_with(build_minimax_anthropic(provider)?, provider, input, tool_context, handles, on_event).await
            }
        },
        ProviderKind::Moonshot => match provider.api.unwrap_or_default() {
            OpenAiApi::Responses => {
                stream_chat_with(build_moonshot(provider)?, provider, input, tool_context, handles, on_event).await
            }
            OpenAiApi::ChatCompletions => {
                stream_chat_with(build_moonshot_anthropic(provider)?, provider, input, tool_context, handles, on_event).await
            }
        },
        ProviderKind::Xiaomimimo => match provider.api.unwrap_or_default() {
            OpenAiApi::Responses => {
                stream_chat_with(build_xiaomimimo(provider)?, provider, input, tool_context, handles, on_event).await
            }
            OpenAiApi::ChatCompletions => {
                stream_chat_with(build_xiaomimimo_anthropic(provider)?, provider, input, tool_context, handles, on_event).await
            }
        },
        ProviderKind::Zai => match provider.api.unwrap_or_default() {
            OpenAiApi::Responses => {
                stream_chat_with(build_zai(provider)?, provider, input, tool_context, handles, on_event).await
            }
            OpenAiApi::ChatCompletions => {
                stream_chat_with(build_zai_anthropic(provider)?, provider, input, tool_context, handles, on_event).await
            }
        },
    }
}

async fn stream_chat_with<C: CompletionClient>(
    client: C,
    provider: &SavedProvider,
    input: &ChatInput,
    tool_context: ToolContext<'_>,
    handles: SessionHandles,
    mut on_event: impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome>
where
    C::CompletionModel: 'static,
{
    let tools = tool_context.native;
    let mcp = tool_context.mcp;
    let model = provider
        .model
        .as_deref()
        .context("no model selected; run `artist model` first")?;

    let resources = resources::Resources::discover(tools.project_root());
    let subagents = subagents::Subagents::discover(tools.project_root());
    handles.rules.note_user_turn();

    let mut seed_history = handles
        .memory
        .load(&handles.conversation_id)
        .await
        .context("load conversation memory")?;
    let durable_history_len = seed_history.len();
    let mut seed_prompt = user_message(input);
    // Skill instructions depend on what the user just typed, so ride them on
    // the user turn instead of folding them into the (otherwise stable)
    // preamble — that keeps the preamble a stable prompt-cache prefix so the
    // history behind it can be reused turn to turn.
    let skill_section = resources.explicit_skill_section(&input.text);
    if !skill_section.is_empty()
        && let Message::User { content } = &mut seed_prompt
    {
        content.insert(0, UserContent::text(skill_section));
    }
    let fork_context = Arc::new({
        let mut context = seed_history.clone();
        context.push(seed_prompt.clone());
        context
    });
    // Delegate tools execute inside `stream.next()`. A separate channel lets
    // their events wake this outer driver while that future is still pending,
    // instead of buffering the entire child transcript until the tool returns.
    let (subagent_events_tx, mut subagent_events_rx) =
        tokio::sync::mpsc::unbounded_channel::<PromptEvent>();
    let visible_steering = handles.steering.clone();
    let tool_meta = ToolMeta::default();
    let mcp_tools = mcp.tools().await;

    // Per-run abort-retry budget: spans this turn's retries but is isolated
    // from concurrent delegate runs (each has its own counter).
    let retry_budget = handles.rules.retry_budget();
    let mut retries_used = 0u32;
    // Stable per-project+model prompt-cache key so a session's turns route to
    // the same server-side prefix cache — better hit rate, fewer billed tokens.
    let cache_key = prompt_cache_key(tools.project_root(), model);
    'retry: loop {
        let run_id = format!("r-{}", uuid::Uuid::new_v4().simple());
        let run_recorder = handles.recorder.with_run(&run_id);
        let ttsr = TtsrShared::new(
            handles.rules.clone(),
            Arc::clone(&handles.rule_set),
            false,
            retries_used < retry_budget,
        );

        let mut builder = client.agent(model);
        // These fields belong to the ChatGPT subscription transport. Keep them
        // off OpenAI Responses and Chat Completions requests, whose accepted
        // parameter shapes differ.
        if let Some(params) = request_params(
            provider.provider,
            &cache_key,
            provider.reasoning_effort.as_deref(),
        ) {
            builder = builder.additional_params(params);
        }
        let mut registered: Vec<Box<dyn rig_core::tool::ToolDyn>> = vec![
            Box::new(tools.bash.clone()),
            Box::new(tools.read.clone()),
            Box::new(tools.find.clone()),
            Box::new(tools.grep.clone()),
            Box::new(tools.edit.clone()),
            Box::new(tools.write.clone()),
            Box::new(resources.skill_tool()),
            Box::new(delegate::Delegate::new(
                provider.clone(),
                tools.clone(),
                Arc::clone(&fork_context),
                resources.clone(),
                delegate::DelegateRuntime {
                    handles: handles.clone(),
                    events: subagent_events_tx.clone(),
                },
                tool_context.disabled.to_vec(),
                subagents.clone(),
            )),
        ];
        registered.extend(
            mcp_tools
                .iter()
                .cloned()
                .map(|tool| Box::new(tool) as Box<dyn rig_core::tool::ToolDyn>),
        );
        if let Some(extensions) = tool_context.extensions {
            registered.extend(extensions.tools());
        }
        tool_prompt::retain_enabled(&mut registered, tool_context.disabled);
        let (main_prompt, prompt_diagnostics) = prompt_config::main_prompt();
        let prompt_diagnostics = prompt_diagnostics
            .iter()
            .map(|d| format!("<diagnostic>{}</diagnostic>", d))
            .collect::<String>();
        let system_prompt = format!(
            "{}\n\n{}{}{}\nCurrent working directory: {}",
            main_prompt,
            prompt_diagnostics,
            tool_prompt::render(&registered),
            format!(
                "{}<available_subagents>{}</available_subagents>",
                resources.prompt_section(),
                subagents.catalog()
            ),
            tools.project_root().display()
        );
        let persistence = conversation::PersistenceStatus::default();
        let attempt_memory = conversation::AttemptMemory::new(
            Arc::clone(&handles.memory),
            handles.conversation_id.clone(),
            seed_history.clone(),
            durable_history_len,
            persistence.clone(),
        );
        let agent = builder
            .preamble(&system_prompt)
            .memory(attempt_memory)
            .conversation(handles.conversation_id.clone())
            .tools(registered)
            .add_hook(steering::SteeringHook(handles.steering.clone()))
            .add_hook(CaptureHook::new(tool_meta.clone()))
            .add_hook(TtsrHook(Arc::clone(&ttsr)))
            .default_max_turns(usize::MAX)
            .build();

        run_recorder.record(RunStarted {
            provider: format!("{:?}", provider.provider).to_lowercase(),
            model: model.to_owned(),
            reasoning_effort: provider.reasoning_effort.clone(),
        });

        let mut stream = agent.stream_prompt(seed_prompt.clone()).await;
        let mut streamed_assistant_text = String::new();
        let mut streamed_turn = ttsr.turn();
        loop {
            let item = tokio::select! {
                biased;
                event = subagent_events_rx.recv() => {
                    if let Some(event) = event {
                        on_event(event)?;
                    }
                    continue;
                }
                item = stream.next() => item,
                _ = handles.cancel.cancelled() => {
                    drop(stream);
                    let (committed, _) = ttsr.committed();
                    let mut cancelled_delta = if committed.is_empty() {
                        let mut fallback = seed_history.clone();
                        fallback.push(seed_prompt.clone());
                        fallback
                    } else {
                        committed
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
                return Err(error);
            };
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
                    let turn = ttsr.turn();
                    if turn != streamed_turn {
                        streamed_assistant_text.clear();
                        streamed_turn = turn;
                    }
                    streamed_assistant_text.push_str(&text.text);
                    on_event(PromptEvent::TextDelta(text.text))?;
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(
                    StreamedAssistantContent::ReasoningDelta {
                        // Handle both summary deltas (`id: None`) and raw
                        // reasoning deltas (`id: Some`); the latter used to fall
                        // through and be dropped, so reasoning-target rules
                        // silently failed to match whenever the backend streamed
                        // raw reasoning instead of a summary.
                        id: _,
                        reasoning,
                    },
                )) => {
                    // rig has no hook event for reasoning deltas, so
                    // reasoning rules match here on the driver side.
                    if ttsr.push_reasoning(&reasoning) {
                        let firing = ttsr
                            .take_pending()
                            .expect("push_reasoning stashed the firing");
                        drop(stream);
                        let (committed, _) = ttsr.committed();
                        seed_history = committed;
                        // `committed` includes the current seed prompt; the
                        // reminder becomes the new prompt.
                        record_firing_events(&run_recorder, &ttsr, &firing);
                        on_event(PromptEvent::RuleFired {
                            rule: firing.rule.0.clone(),
                            matched: firing.matched.clone(),
                        })?;
                        run_recorder.record(RunFinished::Cancelled);
                        seed_prompt = reminder_message(&firing);
                        retries_used += 1;
                        continue 'retry;
                    }
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
                Ok(MultiTurnStreamItem::ToolExecutionStart {
                    tool_call,
                    internal_call_id,
                }) => on_event(PromptEvent::ToolExecutionStart {
                    id: internal_call_id,
                    name: tool_call.function.name,
                })?,
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
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let content = visible_steering
                        .take_original_result(&internal_call_id)
                        .unwrap_or(content);
                    let meta = tool_meta.take(&internal_call_id);
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
                    // A TTSR abort surfaces as PromptCancelled with the
                    // committed history (rig excludes the partial turn).
                    if let Some(firing) = ttsr.take_pending()
                        && let rig_core::agent::StreamingError::Prompt(boxed) = &error
                        && let rig_core::completion::PromptError::PromptCancelled {
                            chat_history,
                            ..
                        } = boxed.as_ref()
                    {
                        seed_history = chat_history.clone();
                        record_firing_events(&run_recorder, &ttsr, &firing);
                        on_event(PromptEvent::RuleFired {
                            rule: firing.rule.0.clone(),
                            matched: firing.matched.clone(),
                        })?;
                        run_recorder.record(RunFinished::Cancelled);
                        seed_prompt = reminder_message(&firing);
                        retries_used += 1;
                        continue 'retry;
                    }
                    run_recorder.record(RunFinished::Error {
                        error: error.to_string(),
                    });
                    return Err(error).context("stream Artist agent");
                }
            }
        }
    }
}

/// Provider parameters shared by every request attempt in a turn.
fn request_params(
    provider: llm_provider::ProviderKind,
    cache_key: &str,
    reasoning_effort: Option<&str>,
) -> Option<serde_json::Value> {
    if provider != llm_provider::ProviderKind::Chatgpt {
        return None;
    }
    let mut params = json!({ "prompt_cache_key": cache_key });
    // Request a provider-generated trace for the live UI even when the model's
    // default effort is in use. Rig's memory policy is independent: streaming
    // this summary does not make the CLI responsible for model context.
    params["reasoning"] = match reasoning_effort {
        Some(effort) => json!({ "effort": effort, "summary": "auto" }),
        None => json!({ "summary": "auto" }),
    };
    Some(params)
}

/// A stable `prompt_cache_key` derived from the project root and model, so a
/// project's turns route to the same server-side prefix cache. Deterministic
/// across process runs (`DefaultHasher` uses fixed keys).
pub(crate) fn prompt_cache_key(project_root: &std::path::Path, model: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    project_root.hash(&mut hasher);
    model.hash(&mut hasher);
    format!("artist-{:016x}", hasher.finish())
}

/// Log rule bookkeeping; Rig conversation memory persists the reminder prompt
/// when the retried run succeeds.
pub(crate) fn record_firing_events(recorder: &Recorder, ttsr: &TtsrShared, firing: &Firing) {
    recorder.record(RuleFired {
        rule: firing.rule.0.clone(),
        target: firing.target.as_str().to_owned(),
        matched: firing.matched.clone(),
        turn: ttsr.turn(),
        per_turn: firing.fire == artist_rules::types::FirePolicy::PerTurn,
    });
    recorder.record(RuleInjection {
        rule: firing.rule.0.clone(),
        reminder: firing.reminder.clone(),
        session_persistent: firing.persistence == artist_rules::types::Persistence::Session,
    });
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

pub fn available_skills(project: &std::path::Path) -> Vec<AvailableSkill> {
    resources::Resources::discover(project).available_skills()
}

/// Executes a prompt without prior context.
pub async fn stream_prompt(
    provider: &SavedProvider,
    input: &str,
    tools: &ToolBundle,
    mcp: &mcp::McpManager,
    handles: SessionHandles,
    on_event: impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome> {
    let input = ChatInput::from(input.to_owned());
    stream_chat(
        provider,
        &input,
        ToolContext {
            native: tools,
            mcp,
            extensions: None,
            disabled: &[],
        },
        handles,
        on_event,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::request_params;
    use llm_provider::ProviderKind;

    #[test]
    fn reasoning_requests_a_live_summary_trace() {
        let params = request_params(ProviderKind::Chatgpt, "cache", Some("high")).unwrap();
        assert_eq!(params["reasoning"]["effort"], "high");
        assert_eq!(params["reasoning"]["summary"], "auto");

        let default_effort = request_params(ProviderKind::Chatgpt, "cache", None).unwrap();
        assert_eq!(default_effort["reasoning"]["summary"], "auto");
        assert!(default_effort["reasoning"].get("effort").is_none());
    }

    #[test]
    fn chatgpt_only_params_are_not_sent_to_openai() {
        assert!(request_params(ProviderKind::Openai, "cache", Some("high")).is_none());
    }
}
