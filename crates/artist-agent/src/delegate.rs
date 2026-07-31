use std::sync::Arc;

use crate::{
    PromptEvent, SessionHandles,
    capture::{CaptureHook, ToolMeta},
    delegate_jobs::DelegateJobs,
    resources::Resources,
    ttsr::{TtsrHook, TtsrShared, reminder_message},
};
use artist_session::{
    ConversationMessages, DelegateFinished, DelegateStarted, RunFinished, RunStarted,
};
use artist_tools::ToolBundle;
use futures::StreamExt;
use llm_provider::SavedProvider;
use rig_core::{
    agent::MultiTurnStreamItem,
    client::CompletionClient,
    completion::{Message, message::ToolResultContent},
    streaming::{StreamedAssistantContent, StreamedUserContent, StreamingChat},
    tool::{Tool, ToolDyn},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone)]
pub(crate) struct Delegate {
    provider: SavedProvider,
    tools: ToolBundle,
    /// The main-agent context to seed a `fork=true` delegate with. Shared via
    /// `Arc` so constructing a Delegate each run/retry is a cheap refcount bump;
    /// the history is only deep-cloned if the model actually forks.
    context: Arc<Vec<Message>>,
    jobs: DelegateJobs,
    resources: Resources,
    handles: SessionHandles,
    disabled_tools: Vec<String>,
    profiles: crate::profiles::Profiles,
    events: UnboundedSender<PromptEvent>,
}

pub(crate) struct DelegateRuntime {
    pub handles: SessionHandles,
    pub events: UnboundedSender<PromptEvent>,
}

struct DelegateRun {
    actor: String,
    background: bool,
}

impl DelegateRun {
    fn new(task_id: Option<String>) -> Self {
        let background = task_id.is_some();
        Self {
            actor: task_id.unwrap_or_else(|| artist_tools::short_id("a")),
            background,
        }
    }
}

impl Delegate {
    pub fn new(
        provider: SavedProvider,
        tools: ToolBundle,
        context: Arc<Vec<Message>>,
        resources: Resources,
        runtime: DelegateRuntime,
        disabled_tools: Vec<String>,
        profiles: crate::profiles::Profiles,
    ) -> Self {
        let jobs = DelegateJobs::for_project(tools.project_root());
        Self {
            provider,
            tools,
            context,
            jobs,
            resources,
            handles: runtime.handles,
            disabled_tools,
            profiles,
            events: runtime.events,
        }
    }

    fn emit(&self, event: PromptEvent) {
        // Background delegates can outlive the parent stream that owned the
        // receiver. Display delivery is therefore best-effort and must never
        // turn a successful background job into a tool failure.
        let _ = self.events.send(event);
    }

    fn emit_child(&self, id: &str, event: PromptEvent) {
        self.emit(PromptEvent::SubagentEvent {
            id: id.to_owned(),
            event: Box::new(event),
        });
    }

    fn finish_child(&self, id: &str, outcome: &str) {
        self.emit(PromptEvent::SubagentFinished {
            id: id.to_owned(),
            outcome: outcome.to_owned(),
        });
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DelegateArgs {
    mode: Option<String>,
    prompt: Option<String>,
    agent: Option<String>,
    fork: Option<bool>,
    background: Option<bool>,
    task_id: Option<String>,
    wait_ms: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DelegateError {
    #[error("subagent failed: {0}")]
    Failed(String),
    /// An availability failure on this candidate. The fallback loop advances
    /// to the next candidate rather than failing the run.
    #[error("subagent provider unavailable: {0}")]
    Unavailable(String),
}

impl Tool for Delegate {
    const NAME: &'static str = "subagent";
    type Error = DelegateError;
    type Args = DelegateArgs;
    type Output = String;

    fn description(&self) -> String {
        "Run a focused subagent. Set background=true to continue other work, then use status/read/wait/cancel with taskId. Set fork=true to include the main chat context."
            .into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{
            "mode":{"enum":["run","start","status","read","wait","cancel","list"],"default":"run"},
            "prompt":{"type":"string"},
            "agent":{"type":"string","enum":self.profiles.names(),"description":"Configured subagent role"},
            "fork":{"type":"boolean","default":false,"description":"Include the full main-agent chat context."},
            "background":{"type":"boolean","default":false,"description":"Start the subagent and return immediately."},
            "taskId":{"type":"string"},
            "waitMs":{"type":"integer","minimum":1,"maximum":30000}
        },"additionalProperties":false})
    }

    async fn call(&self, args: DelegateArgs) -> Result<String, DelegateError> {
        let mode = args
            .mode
            .as_deref()
            .unwrap_or(if args.background.unwrap_or(false) {
                "start"
            } else {
                "run"
            });
        match mode {
            "run" => {
                let prompt = required(args.prompt, "prompt")?;
                self.run_agent(
                    prompt,
                    args.agent.as_deref().unwrap_or("default"),
                    args.fork.unwrap_or(false),
                    None,
                )
                .await
            }
            "start" => {
                let prompt = required(args.prompt, "prompt")?;
                let delegate = self.clone();
                let task_prompt = prompt.clone();
                let role = args.agent.unwrap_or_else(|| "default".into());
                let task_role = role.clone();
                Ok(self
                    .jobs
                    .start(prompt, role, move |task_id| async move {
                        delegate
                            .run_agent(
                                task_prompt,
                                &task_role,
                                args.fork.unwrap_or(false),
                                Some(task_id),
                            )
                            .await
                            .map_err(|error| error.to_string())
                    })
                    .await)
            }
            "status" => self
                .jobs
                .status(&required(args.task_id, "taskId")?)
                .await
                .map_err(DelegateError::Failed),
            "read" => self
                .jobs
                .read(&required(args.task_id, "taskId")?)
                .await
                .map_err(DelegateError::Failed),
            "wait" => self
                .jobs
                .wait(&required(args.task_id, "taskId")?, args.wait_ms)
                .await
                .map_err(DelegateError::Failed),
            "cancel" => self
                .jobs
                .cancel(&required(args.task_id, "taskId")?)
                .await
                .map_err(DelegateError::Failed),
            "list" => Ok(self.jobs.list().await),
            other => Err(DelegateError::Failed(format!(
                "invalid delegate mode: {other}"
            ))),
        }
    }
}

impl Delegate {
    /// Drive the subagent on the streaming surface (delta hooks — and
    /// therefore stream rules — only exist there), with the same TTSR
    /// abort/inject/retry loop as the main agent. The shared `RulesHandle`
    /// makes once-per-session global across main + delegates; delegate
    /// events land in the session log under a child lineage.
    async fn run_agent(
        &self,
        prompt: String,
        profile_name: &str,
        fork: bool,
        task_id: Option<String>,
    ) -> Result<String, DelegateError> {
        let run = DelegateRun::new(task_id);
        let profile = self
            .profiles
            .get(profile_name)
            .map_err(DelegateError::Failed)?;
        let _permit = self
            .profiles
            .semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| DelegateError::Failed("subagent concurrency limit reached".into()))?;
        let breaker = crate::fallback::Breaker::global();
        let mut skipped = Vec::new();
        let mut last_error = None;

        // Ordered, not round-robin: candidate 0 is preferred while healthy.
        for candidate in &profile.candidates {
            let label = crate::fallback::candidate_label(candidate);
            let provider = match self.resolve_provider(candidate) {
                Ok(provider) => provider,
                Err(error) => {
                    skipped.push(crate::fallback::Skipped {
                        candidate: label,
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
            let Some(model) = candidate.model_for(&provider) else {
                skipped.push(crate::fallback::Skipped {
                    candidate: label,
                    reason: "no model configured on the candidate or its account".into(),
                });
                continue;
            };
            let key = crate::fallback::CandidateKey {
                provider: provider.id.as_str().to_owned(),
                model: model.clone(),
            };
            if breaker.is_tripped(&key) {
                skipped.push(crate::fallback::Skipped {
                    candidate: label,
                    reason: "cooling down after repeated failures".into(),
                });
                continue;
            }
            let thinking = candidate.thinking.unwrap_or_default();
            match self
                .dispatch(
                    &provider,
                    &model,
                    thinking,
                    prompt.clone(),
                    &profile,
                    fork,
                    &run,
                )
                .await
            {
                Ok(output) => {
                    breaker.record_success(&key);
                    return Ok(output);
                }
                Err(DelegateError::Unavailable(message)) => {
                    breaker.record_failure(&key);
                    self.emit(PromptEvent::ProviderFallback {
                        from: label,
                        reason: message.clone(),
                    });
                    last_error = Some(message);
                }
                // A permanent failure reproduces on every candidate, so trying
                // the rest would only replace the real error with the last one.
                Err(other) => return Err(other),
            }
        }
        Err(DelegateError::Failed(crate::fallback::exhausted(
            &profile.name,
            last_error,
            &skipped,
        )))
    }

    /// A profile names a provider *account*, not a provider kind. Resolving
    /// against the session's configured set is what lets a delegated profile
    /// run on a different account — and a different provider entirely — from
    /// the parent that spawned it. An unnamed provider inherits the parent's.
    fn resolve_provider(
        &self,
        candidate: &crate::profiles::Candidate,
    ) -> Result<SavedProvider, DelegateError> {
        candidate
            .resolve(&self.handles.providers, &self.provider)
            .cloned()
            .map_err(DelegateError::Failed)
    }

    /// Select the typed Rig client for the resolved account. Each arm
    /// monomorphizes `run_agent_with` separately, which is why this stays a
    /// match over provider kinds rather than a unified client type.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch(
        &self,
        provider: &SavedProvider,
        model: &str,
        thinking: crate::profiles::Thinking,
        prompt: String,
        profile: &crate::profiles::Profile,
        fork: bool,
        run: &DelegateRun,
    ) -> Result<String, DelegateError> {
        use llm_provider::{OpenAiApi, ProviderKind};
        macro_rules! run_with {
            ($build:ident) => {
                self.run_agent_with(
                    crate::rig_provider::$build(provider)
                        .map_err(|error| DelegateError::Failed(error.to_string()))?,
                    prompt,
                    profile,
                    provider,
                    thinking,
                    fork,
                    model,
                    run,
                )
                .await
            };
        }
        match provider.provider {
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
            ProviderKind::Minimax => match provider.api.unwrap_or_default() {
                OpenAiApi::Responses => run_with!(build_minimax),
                OpenAiApi::ChatCompletions => run_with!(build_minimax_anthropic),
            },
            ProviderKind::Moonshot => match provider.api.unwrap_or_default() {
                OpenAiApi::Responses => run_with!(build_moonshot),
                OpenAiApi::ChatCompletions => run_with!(build_moonshot_anthropic),
            },
            ProviderKind::Openai => match provider.api.unwrap_or_default() {
                OpenAiApi::Responses => run_with!(build_openai_responses),
                OpenAiApi::ChatCompletions => run_with!(build_openai_chat),
            },
            ProviderKind::Xiaomimimo => match provider.api.unwrap_or_default() {
                OpenAiApi::Responses => run_with!(build_xiaomimimo),
                OpenAiApi::ChatCompletions => run_with!(build_xiaomimimo_anthropic),
            },
            ProviderKind::Zai => match provider.api.unwrap_or_default() {
                OpenAiApi::Responses => run_with!(build_zai),
                OpenAiApi::ChatCompletions => run_with!(build_zai_anthropic),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_agent_with<C: CompletionClient>(
        &self,
        client: C,
        prompt: String,
        role: &crate::profiles::Profile,
        provider: &SavedProvider,
        thinking: crate::profiles::Thinking,
        fork: bool,
        model: &str,
        run: &DelegateRun,
    ) -> Result<String, DelegateError>
    where
        C::CompletionModel: 'static,
    {
        let actor = run.actor.clone();
        let child_tools = self
            .tools
            .for_actor(&actor)
            .map_err(|error| DelegateError::Failed(error.to_string()))?;
        self.emit(PromptEvent::SubagentStarted {
            id: actor.clone(),
            role: role.name.clone(),
            prompt: prompt.clone(),
        });
        let recorder = self.handles.recorder.child_lineage(&actor);
        recorder.record(DelegateStarted {
            prompt: prompt.clone(),
            read_only: !["bash", "edit", "write"]
                .into_iter()
                .any(|tool| role.permits(tool)),
            fork,
            background: run.background,
        });
        let registered_tools = || {
            let mut tools: Vec<Box<dyn ToolDyn>> = Vec::new();
            if role.permits("read") {
                tools.push(Box::new(child_tools.read.clone()));
            }
            if role.permits("find") {
                tools.push(Box::new(child_tools.find.clone()));
            }
            if role.permits("grep") {
                tools.push(Box::new(child_tools.grep.clone()));
            }
            if role.permits("skill") {
                tools.push(Box::new(self.resources.skill_tool()));
            }
            if role.permits("todo") {
                // A child owns its own list and may read its parent's, but not
                // write to it: concurrent siblings sharing one list would race
                // with no obvious merge.
                tools.push(Box::new(crate::todo::TodoTool::new(
                    self.handles.todos.clone(),
                    recorder.clone(),
                    actor.clone(),
                    Some(self.handles.conversation_id.clone()),
                )));
            }
            if role.permits("bash") {
                tools.push(Box::new(child_tools.bash.clone()));
            }
            if role.permits("edit") {
                tools.push(Box::new(child_tools.edit.clone()));
            }
            if role.permits("write") {
                tools.push(Box::new(child_tools.write.clone()));
            }
            crate::tool_prompt::retain_enabled(&mut tools, &self.disabled_tools);
            tools
        };
        let prompt_tools = registered_tools();
        let policy = format!(
            "You are the '{}' subagent profile.\n{}\n\n{}{}\nCurrent working directory: {}",
            role.name,
            role.instructions,
            crate::tool_prompt::render(&prompt_tools),
            self.resources.prompt_section(),
            self.tools.project_root().display()
        );

        let mut seed_history = if fork {
            (*self.context).clone()
        } else {
            Vec::new()
        };
        // A forked delegate receives the main conversation as input history,
        // but that prefix belongs to the parent lineage. Only messages accepted
        // after this boundary are persisted as the child's transcript.
        let parent_history_len = seed_history.len();
        let mut seed_prompt = Message::user(&prompt);

        // Per-run abort-retry budget: this delegate's own counter, isolated
        // from the main agent and any sibling delegates.
        let retry_budget = self.handles.rules.retry_budget();
        let mut retries_used = 0u32;
        let cache_key = crate::prompt_cache_key(self.tools.project_root(), model);
        let (output, conversation) = loop {
            let run_id = format!("r-{}", uuid::Uuid::new_v4().simple());
            let run_recorder = recorder.with_run(&run_id);
            let ttsr = TtsrShared::new(
                self.handles.rules.clone(),
                Arc::clone(&self.handles.rule_set),
                true,
                retries_used < retry_budget,
            );
            let mut builder = client.agent(model).preamble(&policy);
            // The profile's own thinking configuration wins over the resolved
            // account's default effort (main b9d9193). Translation is
            // per-provider: mode and level are separate fields on Anthropic and
            // a single `reasoning` block on the ChatGPT backend.
            if let Some(params) =
                crate::thinking::request_params(provider.provider, &cache_key, thinking)
            {
                builder = builder.additional_params(params);
            }
            let tool_meta = ToolMeta::default();
            let agent = builder
                .tools(registered_tools())
                .add_hook(CaptureHook::new(tool_meta.clone()))
                .add_hook(TtsrHook(Arc::clone(&ttsr)))
                .default_max_turns(usize::MAX)
                .build();
            run_recorder.record(RunStarted {
                provider: format!("{:?}", provider.provider).to_lowercase(),
                model: model.to_owned(),
                reasoning_effort: thinking
                    .level
                    .map(|level| level.as_str().to_owned())
                    .or_else(|| provider.reasoning_effort.clone()),
            });

            let mut stream = agent
                .stream_chat(seed_prompt.clone(), seed_history.clone())
                .await;
            // Text of the current model turn; the last turn's text is the
            // delegate's answer (matching the non-streaming `chat` output).
            let mut turn_text = String::new();
            let mut last_turn_had_text_delta = false;
            let mut final_messages = None;
            let mut retry = false;
            loop {
                let item = tokio::select! {
                    biased;
                    _ = self.handles.cancel.cancelled() => {
                        run_recorder.record(RunFinished::Cancelled);
                        recorder.record(DelegateFinished { outcome: "cancelled".into() });
                        self.finish_child(&actor, "cancelled");
                        return Err(DelegateError::Failed("cancelled".into()));
                    }
                    item = stream.next() => item,
                };
                let Some(item) = item else {
                    // Match any trailing text/reasoning buffered below the
                    // coalesce threshold before the run completes.
                    if ttsr.finalize_reasoning() || ttsr.finalize_text() {
                        let firing = ttsr.take_pending().expect("finalize stashed the firing");
                        drop(stream);
                        let (committed, _) = ttsr.committed();
                        seed_history = committed;
                        crate::record_firing_events(&run_recorder, &ttsr, &firing);
                        self.emit_child(
                            &actor,
                            PromptEvent::RuleFired {
                                rule: firing.rule.0.clone(),
                                matched: firing.matched.clone(),
                            },
                        );
                        run_recorder.record(RunFinished::Cancelled);
                        seed_prompt = reminder_message(&firing);
                        retries_used += 1;
                        retry = true;
                        break;
                    }
                    run_recorder.record(RunFinished::Completed);
                    break;
                };
                match item {
                    Ok(MultiTurnStreamItem::CompletionCall(call)) => {
                        last_turn_had_text_delta = !turn_text.is_empty();
                        turn_text.clear();
                        self.emit_child(
                            &actor,
                            PromptEvent::CompletionUsage {
                                total_tokens: call.usage.total_tokens,
                            },
                        );
                    }
                    Ok(MultiTurnStreamItem::FinalResponse(response)) => {
                        // The final response is authoritative. Some providers do
                        // not emit text deltas, which previously produced a
                        // successful subagent run with an empty output.
                        turn_text = response.output().to_owned();
                        if !last_turn_had_text_delta && !turn_text.is_empty() {
                            self.emit_child(&actor, PromptEvent::TextDelta(turn_text.clone()));
                        }
                        final_messages = response.messages().map(ToOwned::to_owned);
                    }
                    Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::Text(text),
                    )) => {
                        turn_text.push_str(&text.text);
                        self.emit_child(&actor, PromptEvent::TextDelta(text.text));
                    }
                    Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::ReasoningDelta {
                            // Match summary (`id: None`) and raw (`id: Some`)
                            // reasoning alike; raw deltas were dropped before,
                            // so reasoning-target rules missed them.
                            id: _,
                            reasoning,
                        },
                    )) => {
                        if ttsr.push_reasoning(&reasoning) {
                            let firing = ttsr.take_pending().expect("reasoning firing stashed");
                            drop(stream);
                            let (committed, _) = ttsr.committed();
                            seed_history = committed;
                            crate::record_firing_events(&run_recorder, &ttsr, &firing);
                            self.emit_child(
                                &actor,
                                PromptEvent::RuleFired {
                                    rule: firing.rule.0.clone(),
                                    matched: firing.matched.clone(),
                                },
                            );
                            run_recorder.record(RunFinished::Cancelled);
                            seed_prompt = reminder_message(&firing);
                            retries_used += 1;
                            retry = true;
                            break;
                        }
                        self.emit_child(&actor, PromptEvent::ReasoningSummaryDelta(reasoning));
                    }
                    Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::ToolCall {
                            tool_call,
                            internal_call_id,
                        },
                    )) => self.emit_child(
                        &actor,
                        PromptEvent::ToolCall {
                            id: internal_call_id,
                            name: tool_call.function.name,
                            arguments: tool_call.function.arguments,
                        },
                    ),
                    Ok(MultiTurnStreamItem::ToolExecutionStart {
                        tool_call,
                        internal_call_id,
                    }) => self.emit_child(
                        &actor,
                        PromptEvent::ToolExecutionStart {
                            id: internal_call_id,
                            name: tool_call.function.name,
                        },
                    ),
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
                        let meta = tool_meta.take(&internal_call_id);
                        self.emit_child(
                            &actor,
                            PromptEvent::ToolResult {
                                id: internal_call_id,
                                content,
                                outcome: meta.as_ref().map(|(outcome, _)| outcome.clone()),
                                duration_ms: meta.map(|(_, duration)| duration),
                                images,
                            },
                        );
                    }
                    Ok(_) => {}
                    Err(error) => {
                        if let Some(firing) = ttsr.take_pending()
                            && let rig_core::agent::StreamingError::Prompt(boxed) = &error
                            && let rig_core::completion::PromptError::PromptCancelled {
                                chat_history,
                                ..
                            } = boxed.as_ref()
                        {
                            seed_history = chat_history.clone();
                            crate::record_firing_events(&run_recorder, &ttsr, &firing);
                            self.emit_child(
                                &actor,
                                PromptEvent::RuleFired {
                                    rule: firing.rule.0.clone(),
                                    matched: firing.matched.clone(),
                                },
                            );
                            run_recorder.record(RunFinished::Cancelled);
                            seed_prompt = reminder_message(&firing);
                            retries_used += 1;
                            retry = true;
                            break;
                        }
                        run_recorder.record(RunFinished::Error {
                            error: error.to_string(),
                        });
                        recorder.record(DelegateFinished {
                            outcome: "error".into(),
                        });
                        self.finish_child(&actor, "error");
                        return Err(match crate::fallback::classify(&error) {
                            crate::fallback::Failure::Unavailable => {
                                DelegateError::Unavailable(error.to_string())
                            }
                            crate::fallback::Failure::Permanent => {
                                DelegateError::Failed(error.to_string())
                            }
                        });
                    }
                }
            }
            if retry {
                continue;
            }
            let conversation = final_messages.map(|messages| {
                child_conversation_messages(&seed_history, parent_history_len, messages)
            });
            break (turn_text, conversation);
        };
        if let Some(conversation) = conversation {
            recorder.record(conversation);
        }
        recorder.record(DelegateFinished {
            outcome: "completed".into(),
        });
        self.finish_child(&actor, "completed");
        Ok(
            json!({"role":role.name,"taskId":actor,"output":shorten(&output, 50 * 1024)})
                .to_string(),
        )
    }
}

pub(crate) fn child_conversation_messages(
    seed_history: &[Message],
    parent_history_len: usize,
    run_messages: Vec<Message>,
) -> ConversationMessages {
    let mut messages = seed_history
        .get(parent_history_len..)
        .unwrap_or_default()
        .to_vec();
    messages.extend(run_messages);
    ConversationMessages {
        messages,
        reset: true,
        display_from: 0,
    }
}

fn required<T>(value: Option<T>, name: &str) -> Result<T, DelegateError> {
    value.ok_or_else(|| DelegateError::Failed(format!("{name} is required")))
}
fn shorten(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_owned();
    }
    let mut end = max.saturating_sub(16);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated]", &value[..end])
}

#[cfg(test)]
mod identity_tests {
    use super::DelegateRun;

    #[test]
    fn background_run_reuses_reserved_task_id() {
        let run = DelegateRun::new(Some("a-reserved-task".into()));

        assert_eq!(run.actor, "a-reserved-task");
        assert!(run.background);
    }

    #[test]
    fn foreground_run_allocates_one_actor_id() {
        let run = DelegateRun::new(None);

        assert!(run.actor.starts_with("a-"));
        assert!(!run.background);
    }
}
