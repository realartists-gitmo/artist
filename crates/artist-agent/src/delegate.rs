use std::sync::Arc;

use crate::{
    SessionHandles,
    capture::{CaptureHook, ToolMeta},
    delegate_jobs::DelegateJobs,
    resources::Resources,
    ttsr::{TtsrHook, TtsrShared, reminder_message},
};
use artist_session::{DelegateFinished, DelegateStarted, RunFinished, RunStarted};
use artist_tools::ToolBundle;
use futures::StreamExt;
use llm_provider::SavedProvider;
use rig_core::{
    agent::MultiTurnStreamItem,
    client::CompletionClient,
    completion::Message,
    streaming::{StreamedAssistantContent, StreamingChat},
    tool::{Tool, ToolDyn},
};
use serde::Deserialize;
use serde_json::{Value, json};

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
}

impl Delegate {
    pub fn new(
        provider: SavedProvider,
        tools: ToolBundle,
        context: Arc<Vec<Message>>,
        resources: Resources,
        handles: SessionHandles,
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
            handles,
            disabled_tools,
            subagents,
        }
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
                    .start(prompt, role, async move {
                        delegate
                            .run_agent(task_prompt, &task_role, args.fork.unwrap_or(false))
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
    ) -> Result<String, DelegateError> {
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
            crate::rig_provider::RigClient::ChatGpt(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Copilot(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::OpenAiResponses(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::OpenAiChat(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Anthropic(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Cohere(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Gemini(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::DeepSeek(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Groq(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::HuggingFace(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Hyperbolic(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Mira(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Mistral(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::OpenRouter(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Perplexity(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Together(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::XAi(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Azure(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Llamafile(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Ollama(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Minimax(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::MinimaxAnthropic(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::Moonshot(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::MoonshotAnthropic(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::XiaomiMiMo(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::XiaomiMiMoAnthropic(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::ZAi(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
                    .await
            }
            crate::rig_provider::RigClient::ZAiAnthropic(client) => {
                self.run_agent_with(client, prompt, &role, fork, model)
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
    ) -> Result<String, DelegateError>
    where
        C::CompletionModel: 'static,
    {
        let actor = artist_tools::short_id("a");
        let child_tools = self
            .tools
            .for_actor(&actor)
            .map_err(|error| DelegateError::Failed(error.to_string()))?;
        let recorder = self.handles.recorder.child_lineage(&actor);
        recorder.record(DelegateStarted {
            prompt: prompt.clone(),
            read_only: !["bash", "edit", "write"]
                .into_iter()
                .any(|tool| role.permits(tool)),
            fork,
            background: false,
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
        let mut seed_prompt = Message::user(&prompt);

        // Per-run abort-retry budget: this delegate's own counter, isolated
        // from the main agent and any sibling delegates.
        let retry_budget = self.handles.rules.retry_budget();
        let mut retries_used = 0u32;
        let cache_key = crate::prompt_cache_key(self.tools.project_root(), model);
        let output = loop {
            let run_id = format!("r-{}", uuid::Uuid::new_v4().simple());
            let run_recorder = recorder.with_run(&run_id);
            let ttsr = TtsrShared::new(
                self.handles.rules.clone(),
                Arc::clone(&self.handles.rule_set),
                true,
                retries_used < retry_budget,
            );
            let mut builder = client.agent(model).preamble(&policy);
            if self.provider.provider == llm_provider::ProviderKind::Chatgpt {
                let mut params = json!({ "prompt_cache_key": cache_key.clone() });
                // The subagent's own `reasoning` arg overrides the main agent's
                // effort (main b9d9193); fall back to the provider default.
                if let Some(effort) = role
                    .reasoning_effort
                    .as_deref()
                    .or(self.provider.reasoning_effort.as_deref())
                {
                    // Summaries off (TOK-5) — subagent reasoning is never surfaced,
                    // so a summary was pure token waste here.
                    params["reasoning"] = json!({ "effort": effort });
                }
                builder = builder.additional_params(params);
            }
            let agent = builder
                .tools(registered_tools())
                .add_hook(CaptureHook::new(ToolMeta::default()))
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
            let mut retry = false;
            loop {
                let item = tokio::select! {
                    biased;
                    _ = self.handles.cancel.cancelled() => {
                        run_recorder.record(RunFinished::Cancelled);
                        recorder.record(DelegateFinished { outcome: "cancelled".into() });
                        return Err(DelegateError::Failed("cancelled".into()));
                    }
                    item = stream.next() => item,
                };
                let Some(item) = item else {
                    run_recorder.record(RunFinished::Completed);
                    break;
                };
                match item {
                    Ok(MultiTurnStreamItem::CompletionCall(_)) => turn_text.clear(),
                    Ok(MultiTurnStreamItem::FinalResponse(response)) => {
                        // The final response is authoritative. Some providers do
                        // not emit text deltas, which previously produced a
                        // successful subagent run with an empty output.
                        turn_text = response.output().to_owned();
                    }
                    Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::Text(text),
                    )) => turn_text.push_str(&text.text),
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
                            run_recorder.record(RunFinished::Cancelled);
                            seed_prompt = reminder_message(&firing);
                            retries_used += 1;
                            retry = true;
                            break;
                        }
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
                        return Err(DelegateError::Failed(error.to_string()));
                    }
                }
            }
            if retry {
                continue;
            }
            break turn_text;
        };
        recorder.record(DelegateFinished {
            outcome: "completed".into(),
        });
        Ok(
            json!({"role":role.name,"taskId":actor,"output":shorten(&output, 50 * 1024)})
                .to_string(),
        )
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
