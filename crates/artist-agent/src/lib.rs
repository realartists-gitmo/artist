//! The Artist agent loop, built on Rig.

mod capture;
pub mod compaction;
mod conversation;
mod delegate;
mod delegate_jobs;
#[cfg(test)]
mod delegate_tests;
mod fallback;
pub mod handoff;
pub mod mcp;
pub mod profiles;
mod prompt_config;
mod resources;
mod rig_provider;
mod ttsr;
#[cfg(test)]
mod ttsr_tests;

pub use resources::AvailableSkill;
mod steering;

mod thinking;
pub mod todo;
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
    /// A routing candidate failed and the run moved to the next one. Surfaced
    /// because a silent move to a weaker model is discovered hours later.
    ProviderFallback {
        from: String,
        reason: String,
    },
    /// The session changed profile. The transcript above stays on screen; the
    /// model's context does not.
    HandedOff {
        from: String,
        to: String,
    },
}

/// How a `stream_chat` run ended (errors surface via `Result`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Cancelled,
    /// The agent handed the session to another profile. Terminal for this run:
    /// the caller clears the conversation and starts the target profile with
    /// [`handoff::Handoff::seed`] as its opening task.
    HandedOff {
        from: String,
        handoff: handoff::Handoff,
    },
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
    /// Blob store for tool-result image payloads. `None` for inert handles,
    /// where nothing is recorded and so nothing needs storing.
    pub attachments: Option<artist_session::AttachmentStore>,
    /// The configured provider accounts, so a profile naming a `provider:` can
    /// be resolved to an account other than the one running the session.
    /// Empty for inert handles, where every profile inherits the parent.
    pub providers: llm_provider::ProviderSet,
    /// Harness-owned todo lists, keyed by owner. Lives outside the model
    /// context so it survives compaction and handoff intact.
    pub todos: todo::TodoStore,
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
            attachments: None,
            providers: llm_provider::ProviderSet::default(),
            todos: todo::TodoStore::default(),
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
#[derive(Clone, Copy)]
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
/// Runs a turn on the session's default profile.
pub async fn stream_chat(
    provider: &SavedProvider,
    input: &ChatInput,
    tool_context: ToolContext<'_>,
    handles: SessionHandles,
    on_event: impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome> {
    stream_chat_as(provider, "default", input, tool_context, handles, on_event).await
}

/// Runs a turn with the session root instantiated from a named profile.
///
/// The profile determines the system prompt, the tool surface, and the routing:
/// `provider` is only the account the session was launched with, and a profile
/// naming its own account resolves against [`SessionHandles::providers`]
/// instead. This is the same object and the same resolution a delegated
/// subagent uses — the session root is one of its three instantiation modes.
pub async fn stream_chat_as(
    provider: &SavedProvider,
    profile_name: &str,
    input: &ChatInput,
    tool_context: ToolContext<'_>,
    handles: SessionHandles,
    mut on_event: impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome> {
    let mut chain = vec![profile_name.to_owned()];
    let mut current = profile_name.to_owned();
    let mut seeded: Option<ChatInput> = None;
    loop {
        let turn = seeded.as_ref().unwrap_or(input);
        let outcome = run_profile(
            provider,
            &current,
            turn,
            tool_context,
            handles.clone(),
            &mut on_event,
        )
        .await?;
        let RunOutcome::HandedOff { from, handoff } = outcome else {
            return Ok(outcome);
        };
        chain.push(handoff.to.clone());
        // Functionally `/clear` followed by a fresh session on the target
        // profile: the conversation reset is the same event compaction emits,
        // and the payload becomes the incoming profile's opening task.
        handles.recorder.record(artist_session::HandoffPerformed {
            from: from.clone(),
            to: handoff.to.clone(),
            summary: handoff.summary.clone(),
            chain: chain.clone(),
            read_files: Vec::new(),
            modified_files: Vec::new(),
            jobs: Vec::new(),
            todos: handles.todos.get(&handles.conversation_id),
        });
        handles
            .memory
            .clear(&handles.conversation_id)
            .await
            .context("clear conversation for handoff")?;
        on_event(PromptEvent::HandedOff {
            from,
            to: handoff.to.clone(),
        })?;
        seeded = Some(ChatInput {
            text: handoff.seed(
                &current,
                &input.text,
                &chain,
                &todo::render(&handles.todos.get(&handles.conversation_id)),
            ),
            images: Vec::new(),
        });
        current = handoff.to.clone();
    }
}

/// One profile's turn, including its candidate fallback.
async fn run_profile(
    provider: &SavedProvider,
    profile_name: &str,
    input: &ChatInput,
    tool_context: ToolContext<'_>,
    handles: SessionHandles,
    on_event: &mut impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome> {
    let profiles = profiles::Profiles::discover(tool_context.native.project_root());
    let profile = profiles.get(profile_name).map_err(|error| anyhow!(error))?;
    let breaker = fallback::Breaker::global();
    let mut skipped = Vec::new();
    let mut last_error = None;

    // Ordered, not round-robin: candidate 0 is preferred while healthy.
    for candidate in &profile.candidates {
        let label = fallback::candidate_label(candidate);
        let resolved = match candidate.resolve(&handles.providers, provider) {
            Ok(resolved) => resolved.clone(),
            Err(error) => {
                skipped.push(fallback::Skipped {
                    candidate: label,
                    reason: error,
                });
                continue;
            }
        };
        let Some(model) = candidate.model_for(&resolved) else {
            skipped.push(fallback::Skipped {
                candidate: label,
                reason: "no model configured on the candidate or its account".into(),
            });
            continue;
        };
        let key = fallback::CandidateKey {
            provider: resolved.id.as_str().to_owned(),
            model: model.clone(),
        };
        if breaker.is_tripped(&key) {
            skipped.push(fallback::Skipped {
                candidate: label,
                reason: "cooling down after repeated failures".into(),
            });
            continue;
        }
        let run = RootRun {
            profiles: profiles.clone(),
            profile: profile.clone(),
            thinking: candidate.thinking.unwrap_or_default(),
            model,
        };
        match attempt(&resolved, &run, input, tool_context, handles.clone(), on_event).await {
            Ok(outcome) => {
                breaker.record_success(&key);
                return Ok(outcome);
            }
            Err(error) if error.downcast_ref::<fallback::Unavailable>().is_some() => {
                breaker.record_failure(&key);
                let reason = error.to_string();
                on_event(PromptEvent::ProviderFallback {
                    from: label,
                    reason: reason.clone(),
                })?;
                last_error = Some(reason);
            }
            // A permanent failure reproduces on every candidate, so trying the
            // rest would only replace the real error with the last one.
            Err(error) => return Err(error),
        }
    }
    Err(anyhow!(fallback::exhausted(
        &profile.name,
        last_error,
        &skipped
    )))
}

/// One attempt on one resolved candidate.
///
/// Each arm monomorphizes `stream_chat_with` separately, which is why this
/// stays a match over provider kinds rather than a unified client type.
async fn attempt(
    resolved: &SavedProvider,
    run: &RootRun,
    input: &ChatInput,
    tool_context: ToolContext<'_>,
    handles: SessionHandles,
    on_event: &mut impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome> {
    use llm_provider::OpenAiApi;

    macro_rules! run_with {
        ($build:ident) => {
            stream_chat_with(
                rig_provider::$build(resolved)?,
                resolved,
                run,
                input,
                tool_context,
                handles,
                on_event,
            )
            .await
        };
    }
    match resolved.provider {
        ProviderKind::Anthropic => run_with!(build_anthropic),
        ProviderKind::Azure => run_with!(build_azure),
        ProviderKind::Chatgpt => run_with!(build_chatgpt),
        ProviderKind::Cohere => run_with!(build_cohere),
        ProviderKind::Copilot => run_with!(build_copilot),
        ProviderKind::Deepseek => run_with!(build_deepseek),
        ProviderKind::Gemini => run_with!(build_gemini),
        ProviderKind::Groq => run_with!(build_groq),
        ProviderKind::Huggingface => run_with!(build_huggingface),
        ProviderKind::Hyperbolic => run_with!(build_hyperbolic),
        ProviderKind::Llamafile => run_with!(build_llamafile),
        ProviderKind::Mira => run_with!(build_mira),
        ProviderKind::Mistral => run_with!(build_mistral),
        ProviderKind::Ollama => run_with!(build_ollama),
        ProviderKind::Openrouter => run_with!(build_openrouter),
        ProviderKind::Perplexity => run_with!(build_perplexity),
        ProviderKind::Together => run_with!(build_together),
        ProviderKind::Xai => run_with!(build_xai),
        ProviderKind::Minimax => match resolved.api.unwrap_or_default() {
            OpenAiApi::Responses => run_with!(build_minimax),
            OpenAiApi::ChatCompletions => run_with!(build_minimax_anthropic),
        },
        ProviderKind::Moonshot => match resolved.api.unwrap_or_default() {
            OpenAiApi::Responses => run_with!(build_moonshot),
            OpenAiApi::ChatCompletions => run_with!(build_moonshot_anthropic),
        },
        ProviderKind::Openai => match resolved.api.unwrap_or_default() {
            OpenAiApi::Responses => run_with!(build_openai_responses),
            OpenAiApi::ChatCompletions => run_with!(build_openai_chat),
        },
        ProviderKind::Xiaomimimo => match resolved.api.unwrap_or_default() {
            OpenAiApi::Responses => run_with!(build_xiaomimimo),
            OpenAiApi::ChatCompletions => run_with!(build_xiaomimimo_anthropic),
        },
        ProviderKind::Zai => match resolved.api.unwrap_or_default() {
            OpenAiApi::Responses => run_with!(build_zai),
            OpenAiApi::ChatCompletions => run_with!(build_zai_anthropic),
        },
    }
}


/// The resolved profile context for a session-root run.
struct RootRun {
    profiles: profiles::Profiles,
    profile: profiles::Profile,
    thinking: profiles::Thinking,
    model: String,
}

#[allow(clippy::too_many_arguments)]
async fn stream_chat_with<C: CompletionClient>(
    client: C,
    provider: &SavedProvider,
    run: &RootRun,
    input: &ChatInput,
    tool_context: ToolContext<'_>,
    handles: SessionHandles,
    on_event: &mut impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome>
where
    C::CompletionModel: 'static,
{
    let tools = tool_context.native;
    let mcp = tool_context.mcp;
    let model = run.model.as_str();
    let profile = &run.profile;
    let profiles = &run.profiles;

    let resources = resources::Resources::discover(tools.project_root());
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
    // Delegate tools execute inside `stream.next()`. A separate channel lets
    // their events wake this outer driver while that future is still pending,
    // instead of buffering the entire child transcript until the tool returns.
    let (subagent_events_tx, mut subagent_events_rx) =
        tokio::sync::mpsc::unbounded_channel::<PromptEvent>();
    let visible_steering = handles.steering.clone();
    let tool_meta = ToolMeta::default();
    let mcp_tools = mcp.tools().await;
    // Where a fired handoff lands. Shared with the tool for the whole turn so a
    // TTSR retry rebuilds the tool against the same slot.
    let pending_handoff = handoff::HandoffShared::default();

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
        // Rebuilt per attempt from the *current* seed, so a `fork=true` delegate
        // spawned after a TTSR retry inherits the reminder-injected history
        // rather than the stale original turn.
        let fork_context = Arc::new({
            let mut context = seed_history.clone();
            context.push(seed_prompt.clone());
            context
        });
        // A display-callback failure ends the run like any other error, so it
        // must record a terminal event first. Propagating the error directly
        // would leave `run.started` with no `run.finished`, which replay reads
        // as a run still in flight.
        macro_rules! emit {
            ($event:expr) => {
                if let Err(error) = on_event($event) {
                    run_recorder.record(RunFinished::Error {
                        error: error.to_string(),
                    });
                    return Err(error);
                }
            };
        }

        let mut builder = client.agent(model);
        // These fields belong to the ChatGPT subscription transport. Keep them
        // off OpenAI Responses and Chat Completions requests, whose accepted
        // parameter shapes differ.
        if let Some(params) =
            thinking::request_params(provider.provider, &cache_key, run.thinking)
        {
            builder = builder.additional_params(params);
        }
        let mut registered: Vec<Box<dyn rig_core::tool::ToolDyn>> = Vec::new();
        if profile.permits("bash") {
            registered.push(Box::new(tools.bash.clone()));
        }
        if profile.permits("read") {
            registered.push(Box::new(tools.read.clone()));
        }
        if profile.permits("find") {
            registered.push(Box::new(tools.find.clone()));
        }
        if profile.permits("grep") {
            registered.push(Box::new(tools.grep.clone()));
        }
        if profile.permits("edit") {
            registered.push(Box::new(tools.edit.clone()));
        }
        if profile.permits("write") {
            registered.push(Box::new(tools.write.clone()));
        }
        if profile.permits("skill") {
            registered.push(Box::new(resources.skill_tool()));
        }
        if profile.permits("todo") {
            registered.push(Box::new(todo::TodoTool::new(
                handles.todos.clone(),
                handles.recorder.clone(),
                handles.conversation_id.clone(),
                None,
            )));
        }
        if profile.permits("handoff") && profiles.names().len() > 1 {
            registered.push(Box::new(handoff::HandoffTool::new(
                pending_handoff.clone(),
                profiles.clone(),
                profile.name.clone(),
            )));
        }
        if profile.permits("subagent") {
            registered.push(Box::new(delegate::Delegate::new(
                provider.clone(),
                tools.clone(),
                fork_context,
                resources.clone(),
                delegate::DelegateRuntime {
                    handles: handles.clone(),
                    events: subagent_events_tx.clone(),
                },
                tool_context.disabled.to_vec(),
                profiles.clone(),
            )));
        }
        registered.extend(
            mcp_tools
                .iter()
                .cloned()
                .map(|tool| Box::new(tool) as Box<dyn rig_core::tool::ToolDyn>),
        );
        if let Some(extensions) = tool_context.extensions {
            registered.extend(extensions.tools());
        }
        // MCP and extension tools are addressable by the same glob policy, so a
        // profile can trim a bloated server down to the handful it needs.
        registered.retain(|tool| profile.permits(&tool.name()));
        tool_prompt::retain_enabled(&mut registered, tool_context.disabled);
        let prompt_diagnostics = profiles
            .diagnostics()
            .iter()
            .map(|d| format!("<diagnostic>{}</diagnostic>", d))
            .collect::<String>();
        let system_prompt = format!(
            "{}\n\n{}{}{}<available_profiles>{}</available_profiles>\nCurrent working directory: {}",
            profile.instructions,
            prompt_diagnostics,
            tool_prompt::render(&registered),
            resources.prompt_section(),
            profiles.catalog(),
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
            reasoning_effort: run
                .thinking
                .level
                .map(|level| level.as_str().to_owned())
                .or_else(|| provider.reasoning_effort.clone()),
        });

        let mut stream = agent.stream_prompt(seed_prompt.clone()).await;
        let mut streamed_assistant_text = String::new();
        let mut streamed_turn = ttsr.turn();
        loop {
            let item = tokio::select! {
                biased;
                event = subagent_events_rx.recv() => {
                    if let Some(event) = event {
                        emit!(event);
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
                    // The turn is over: match any trailing text/reasoning that
                    // never reached the coalesce threshold. Short trailing
                    // content without a newline is otherwise never evaluated.
                    if ttsr.finalize_reasoning() || ttsr.finalize_text() {
                        let firing = ttsr.take_pending().expect("finalize stashed the firing");
                        drop(stream);
                        let (committed, _) = ttsr.committed();
                        seed_history = committed;
                        record_firing_events(&run_recorder, &ttsr, &firing);
                        emit!(PromptEvent::RuleFired {
                            rule: firing.rule.0.clone(),
                            matched: firing.matched.clone(),
                        });
                        run_recorder.record(RunFinished::Cancelled);
                        seed_prompt = reminder_message(&firing);
                        retries_used += 1;
                        continue 'retry;
                    }
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
                    emit!(PromptEvent::TextDelta(text.text));
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
                        emit!(PromptEvent::RuleFired {
                            rule: firing.rule.0.clone(),
                            matched: firing.matched.clone(),
                        });
                        run_recorder.record(RunFinished::Cancelled);
                        seed_prompt = reminder_message(&firing);
                        retries_used += 1;
                        continue 'retry;
                    }
                    emit!(PromptEvent::ReasoningSummaryDelta(reasoning));
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(
                    StreamedAssistantContent::ToolCall {
                        tool_call,
                        internal_call_id,
                    },
                )) => emit!(PromptEvent::ToolCall {
                    id: internal_call_id,
                    name: tool_call.function.name,
                    arguments: tool_call.function.arguments,
                }),
                Ok(MultiTurnStreamItem::ToolExecutionStart {
                    tool_call,
                    internal_call_id,
                }) => emit!(PromptEvent::ToolExecutionStart {
                    id: internal_call_id,
                    name: tool_call.function.name,
                }),
                Ok(MultiTurnStreamItem::CompletionCall(call)) => {
                    emit!(PromptEvent::CompletionUsage {
                        total_tokens: call.usage.total_tokens,
                    });
                }
                Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                    tool_result,
                    internal_call_id,
                })) => {
                    // The capture hook only sees result text; persist any images
                    // here (the display path) into the attachment store and log
                    // their references so history replay can reattach them.
                    let mut image_blocks = Vec::new();
                    let mut images = 0usize;
                    let content = tool_result
                        .content
                        .into_iter()
                        .filter_map(|item| match item {
                            ToolResultContent::Text(text) => Some(text.text),
                            ToolResultContent::Image(image) => {
                                images += 1;
                                if let Some(store) = &handles.attachments
                                    && let Some(block) =
                                        artist_session::store_tool_image(&image, store)
                                {
                                    image_blocks.push(block);
                                }
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !image_blocks.is_empty() {
                        run_recorder.record(artist_session::ToolResultImagesEvent {
                            internal_call_id: internal_call_id.clone(),
                            images: image_blocks,
                        });
                    }
                    let content = visible_steering
                        .take_original_result(&internal_call_id)
                        .unwrap_or(content);
                    let meta = tool_meta.take(&internal_call_id);
                    emit!(PromptEvent::ToolResult {
                        id: internal_call_id,
                        content,
                        outcome: meta.as_ref().map(|(outcome, _)| outcome.clone()),
                        duration_ms: meta.map(|(_, duration)| duration),
                        images,
                    });
                    if let Some(handoff) = pending_handoff.take() {
                        drop(stream);
                        run_recorder.record(RunFinished::Completed);
                        return Ok(RunOutcome::HandedOff {
                            from: profile.name.clone(),
                            handoff,
                        });
                    }
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
                        emit!(PromptEvent::RuleFired {
                            rule: firing.rule.0.clone(),
                            matched: firing.matched.clone(),
                        });
                        run_recorder.record(RunFinished::Cancelled);
                        seed_prompt = reminder_message(&firing);
                        retries_used += 1;
                        continue 'retry;
                    }
                    run_recorder.record(RunFinished::Error {
                        error: error.to_string(),
                    });
                    if crate::fallback::classify(&error) == crate::fallback::Failure::Unavailable {
                        return Err(anyhow!(fallback::Unavailable(error.to_string())));
                    }
                    return Err(error).context("stream Artist agent");
                }
            }
        }
    }
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
