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
use rig_agent::client::AgentClientExt;
use rig_agent::{agent::MultiTurnStreamItem, prelude::PromptError, streaming::StreamingChat};
use rig_core::{
    client::CompletionClient,
    completion::{Message, message::ToolResultContent},
    streaming::{StreamedAssistantContent, StreamedUserContent},
    tool::{PortableDynamicTool, PortableTool},
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
    subagents: crate::subagents::Subagents,
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
        subagents: crate::subagents::Subagents,
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
            subagents,
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
    #[error("subagent model is not configured")]
    MissingModel,
    #[error("subagent failed: {0}")]
    Failed(String),
}

impl PortableTool for Delegate {
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
            "agent":{"type":"string","enum":self.subagents.names(),"description":"Configured subagent role"},
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
        role_name: &str,
        fork: bool,
        task_id: Option<String>,
    ) -> Result<String, DelegateError> {
        let run = DelegateRun::new(task_id);
        let role = self
            .subagents
            .role(role_name)
            .map_err(DelegateError::Failed)?;
        let _permit = self
            .subagents
            .semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| DelegateError::Failed("subagent concurrency limit reached".into()))?;
        let model = role
            .model
            .as_deref()
            .or(self.provider.model.as_deref())
            .ok_or(DelegateError::MissingModel)?;
        let client = crate::rig_provider::RigClient::build(&self.provider)
            .map_err(|error| DelegateError::Failed(error.to_string()))?;
        match client {
            crate::rig_provider::RigClient::ArtistOpenAi(client) => {
                // Delegates have isolated provider-private lineage even when
                // their portable conversation is seeded from the parent.
                let lineage = format!("{}:delegate:{}", self.handles.conversation_id, run.actor);
                let client =
                    client.with_provider_context(lineage, self.handles.provider_context.clone());
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Copilot(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::OpenAiChat(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Anthropic(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Cohere(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Gemini(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::DeepSeek(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Groq(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::HuggingFace(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Hyperbolic(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Mira(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Mistral(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::OpenRouter(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Perplexity(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Together(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::XAi(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Azure(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Llamafile(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Ollama(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Minimax(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::MinimaxAnthropic(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::Moonshot(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::MoonshotAnthropic(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::XiaomiMiMo(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::XiaomiMiMoAnthropic(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::ZAi(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
            crate::rig_provider::RigClient::ZAiAnthropic(client) => {
                self.run_agent_with(client, prompt, &role, fork, model, &run)
                    .await
            }
        }
    }

    async fn run_agent_with<C: CompletionClient>(
        &self,
        client: C,
        prompt: String,
        role: &crate::subagents::Role,
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
            let mut tools: Vec<PortableDynamicTool> = Vec::new();
            if role.permits("read") {
                tools.push(crate::tool_prompt::dynamic(child_tools.read.clone()));
            }
            if role.permits("find") {
                tools.push(crate::tool_prompt::dynamic(child_tools.find.clone()));
            }
            if role.permits("grep") {
                tools.push(crate::tool_prompt::dynamic(child_tools.grep.clone()));
            }
            if role.permits("skill") {
                tools.push(crate::tool_prompt::dynamic(self.resources.skill_tool()));
            }
            if role.permits("bash") {
                tools.push(crate::tool_prompt::dynamic(child_tools.bash.clone()));
            }
            if role.permits("edit") {
                tools.push(crate::tool_prompt::dynamic(child_tools.edit.clone()));
            }
            if role.permits("write") {
                tools.push(crate::tool_prompt::dynamic(child_tools.write.clone()));
            }
            crate::tool_prompt::retain_enabled(&mut tools, &self.disabled_tools);
            tools
        };
        let prompt_tools = registered_tools();
        let role_instructions = role.instructions.as_deref().unwrap_or("Complete only the delegated task and return concise findings with evidence. You cannot delegate further.");
        let policy = format!(
            "You are the '{}' subagent role.\n{}\n\n{}{}\nCurrent working directory: {}",
            role.name,
            role_instructions,
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
        let mut overload_retry = crate::provider_retry::OverloadRetry::new(
            self.tools.project_root(),
            model,
            &format!("delegate:{actor}"),
        );
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
            // The subagent's own `reasoning` arg overrides the main agent's
            // effort; use the same Responses-only policy and absolute token
            // threshold as the main agent.
            let reasoning_effort = role
                .reasoning_effort
                .as_deref()
                .or(self.provider.reasoning_effort.as_deref());
            if let Some(params) = crate::request_params(
                self.provider.provider,
                self.provider.api,
                overload_retry.cache_key(),
                reasoning_effort,
                delegate_context_window(
                    model,
                    self.provider.model.as_deref(),
                    self.handles.effective_context_window,
                ),
            ) {
                builder = builder.additional_params(params);
            }
            let tool_meta = ToolMeta::default();
            let agent = builder
                .dynamic_tools(
                    registered_tools()
                        .into_iter()
                        .map(rig_agent::tool::DynamicTool::from)
                        .collect(),
                )
                .add_hook(CaptureHook::new(tool_meta.clone()))
                .add_hook(TtsrHook(Arc::clone(&ttsr)))
                .default_max_turns(usize::MAX)
                .build();
            run_recorder.record(RunStarted {
                provider: format!("{:?}", self.provider.provider).to_lowercase(),
                model: model.to_owned(),
                reasoning_effort: role
                    .reasoning_effort
                    .clone()
                    .or_else(|| self.provider.reasoning_effort.clone()),
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
            let mut attempt_observed = false;
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
                    run_recorder.record(RunFinished::Completed);
                    break;
                };
                if item.is_ok() {
                    attempt_observed = true;
                }
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
                    Ok(MultiTurnStreamItem::ToolExecutionCommitted {
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
                                ToolResultContent::Json { value } => Some(value.to_string()),
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
                            && let rig_agent::agent::StreamingError::Prompt(boxed) = &error
                            && let PromptError::PromptCancelled { chat_history, .. } =
                                boxed.as_ref()
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
                        if !attempt_observed
                            && crate::provider_retry::is_overload(self.provider.provider, &error)
                            && let Some(delay) = overload_retry.schedule()
                        {
                            drop(stream);
                            tokio::select! {
                                _ = tokio::time::sleep(delay) => {
                                    retry = true;
                                    break;
                                }
                                _ = self.handles.cancel.cancelled() => {
                                    recorder.record(DelegateFinished { outcome: "cancelled".into() });
                                    self.finish_child(&actor, "cancelled");
                                    return Err(DelegateError::Failed("cancelled".into()));
                                }
                            }
                        }
                        recorder.record(DelegateFinished {
                            outcome: "error".into(),
                        });
                        self.finish_child(&actor, "error");
                        return Err(DelegateError::Failed(error.to_string()));
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

fn delegate_context_window(
    delegate_model: &str,
    main_model: Option<&str>,
    main_window: Option<u64>,
) -> Option<u64> {
    (main_model == Some(delegate_model))
        .then_some(main_window)
        .flatten()
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
    use super::{DelegateRun, delegate_context_window};

    #[test]
    fn model_override_does_not_inherit_main_context_window() {
        assert_eq!(
            delegate_context_window("main", Some("main"), Some(100_000)),
            Some(100_000)
        );
        assert_eq!(
            delegate_context_window("small", Some("main"), Some(100_000)),
            None
        );
        assert_eq!(delegate_context_window("small", None, Some(100_000)), None);
    }

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
