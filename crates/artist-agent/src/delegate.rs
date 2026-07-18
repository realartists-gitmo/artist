use std::sync::Arc;

use crate::{
    SessionHandles,
    capture::{CaptureHook, ToolMeta},
    delegate_jobs::DelegateJobs,
    resources::Resources,
    ttsr::{TtsrHook, TtsrShared, reminder_message},
};
use artist_session::{DelegateFinished, DelegateStarted, Recorder, RunFinished, RunStarted};
use artist_tools::{TOOL_POLICY, ToolBundle};
use futures::StreamExt;
use llm_provider::{ProviderKind, SavedProvider};
use rig_core::{
    agent::MultiTurnStreamItem,
    client::CompletionClient,
    completion::Message,
    streaming::{StreamedAssistantContent, StreamingChat},
    tool::Tool,
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
}

impl Delegate {
    pub fn new(
        provider: SavedProvider,
        tools: ToolBundle,
        context: Arc<Vec<Message>>,
        resources: Resources,
        handles: SessionHandles,
    ) -> Self {
        let jobs = DelegateJobs::for_project(tools.project_root());
        Self {
            provider,
            tools,
            context,
            jobs,
            resources,
            handles,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DelegateArgs {
    mode: Option<String>,
    prompt: Option<String>,
    read_only: Option<bool>,
    fork: Option<bool>,
    background: Option<bool>,
    task_id: Option<String>,
    wait_ms: Option<u64>,
    model: Option<String>,
    #[serde(alias = "reasoningLevel", alias = "reasoningEffort")]
    reasoning: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DelegateError {
    #[error("delegate model is not configured")]
    MissingModel,
    #[error("delegate failed: {0}")]
    Failed(String),
}

impl Tool for Delegate {
    const NAME: &'static str = "delegate";
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
            "readOnly":{"type":"boolean","default":true},
            "fork":{"type":"boolean","default":false,"description":"Include the full main-agent chat context."},
            "background":{"type":"boolean","default":false,"description":"Start the delegate and return immediately."},
            "taskId":{"type":"string"},
            "waitMs":{"type":"integer","minimum":1,"maximum":30000},
            "model":{"type":"string","description":"Model slug for this subagent. Defaults to the main agent's model."},
            "reasoning":{"type":"string","description":"Reasoning effort for this subagent. Defaults to the main agent's reasoning effort."}
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
                    args.read_only.unwrap_or(true),
                    args.fork.unwrap_or(false),
                    args.model,
                    args.reasoning,
                    false,
                )
                .await
            }
            "start" => {
                let prompt = required(args.prompt, "prompt")?;
                let delegate = self.clone();
                let task_prompt = prompt.clone();
                Ok(self
                    .jobs
                    .start(prompt, async move {
                        delegate
                            .run_agent(
                                task_prompt,
                                args.read_only.unwrap_or(true),
                                args.fork.unwrap_or(false),
                                args.model,
                                args.reasoning,
                                true,
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
        read_only: bool,
        fork: bool,
        model: Option<String>,
        reasoning: Option<String>,
        background: bool,
    ) -> Result<String, DelegateError> {
        let model = model
            .as_deref()
            .or(self.provider.model.as_deref())
            .ok_or(DelegateError::MissingModel)?
            .to_owned();
        let actor = format!("delegate-{}", uuid::Uuid::new_v4().simple());
        let child_tools = self
            .tools
            .for_actor(&actor)
            .map_err(|error| DelegateError::Failed(error.to_string()))?;
        let recorder = self.handles.recorder.child_lineage(&actor);
        recorder.record(DelegateStarted {
            prompt: prompt.clone(),
            read_only,
            fork,
            background,
        });
        let mut policy = if read_only {
            format!(
                "{TOOL_POLICY}\nYou are a read-only subagent. Inspect with read, find, and grep only. Return concise findings and evidence."
            )
        } else {
            format!(
                "{TOOL_POLICY}\nComplete only the focused delegated task. Return concise findings and evidence. You cannot delegate further."
            )
        };
        policy.push_str(&self.resources.prompt_section());

        let seed_history = if fork {
            (*self.context).clone()
        } else {
            Vec::new()
        };
        let seed_prompt = Message::user(&prompt);
        // The subagent's own `reasoning` arg overrides the main agent's effort
        // (main b9d9193); fall back to the provider default.
        let effort = reasoning
            .as_deref()
            .or(self.provider.reasoning_effort.as_deref());
        let cache_key = crate::prompt_cache_key(self.tools.project_root(), &model);
        let ctx = DelegateRun {
            model: &model,
            policy: &policy,
            params: crate::params_for(self.provider.kind, effort, &cache_key),
            child_tools: &child_tools,
            read_only,
            resources: &self.resources,
            handles: &self.handles,
            recorder: &recorder,
            provider_label: crate::provider_label(self.provider.kind),
            reasoning_effort: effort.map(str::to_owned),
        };
        // Dispatch on the provider's backend (same set as the main agent); the
        // subagent inherits the session's provider.
        let build = |error: anyhow::Error| DelegateError::Failed(error.to_string());
        let output = match self.provider.kind {
            ProviderKind::ChatGpt => {
                drive_delegate(crate::build_chatgpt(&self.provider).map_err(build)?, ctx, seed_prompt, seed_history).await?
            }
            ProviderKind::OpenAi => {
                drive_delegate(crate::build_openai_responses(&self.provider).map_err(build)?, ctx, seed_prompt, seed_history).await?
            }
            ProviderKind::Anthropic => {
                drive_delegate(crate::build_anthropic(&self.provider).map_err(build)?, ctx, seed_prompt, seed_history).await?
            }
            ProviderKind::Gemini => {
                drive_delegate(crate::build_gemini(&self.provider).map_err(build)?, ctx, seed_prompt, seed_history).await?
            }
            ProviderKind::Groq => {
                drive_delegate(crate::build_groq(&self.provider).map_err(build)?, ctx, seed_prompt, seed_history).await?
            }
            ProviderKind::DeepSeek => {
                drive_delegate(crate::build_deepseek(&self.provider).map_err(build)?, ctx, seed_prompt, seed_history).await?
            }
            ProviderKind::Together => {
                drive_delegate(crate::build_together(&self.provider).map_err(build)?, ctx, seed_prompt, seed_history).await?
            }
            ProviderKind::OpenRouter => {
                drive_delegate(crate::build_openrouter(&self.provider).map_err(build)?, ctx, seed_prompt, seed_history).await?
            }
            ProviderKind::Mistral => {
                drive_delegate(crate::build_mistral(&self.provider).map_err(build)?, ctx, seed_prompt, seed_history).await?
            }
            ProviderKind::Perplexity => {
                drive_delegate(crate::build_perplexity(&self.provider).map_err(build)?, ctx, seed_prompt, seed_history).await?
            }
        };
        recorder.record(DelegateFinished {
            outcome: "completed".into(),
        });
        Ok(shorten(&output, 50 * 1024))
    }
}

/// Immutable context for one delegate run, shared across its retry loop.
struct DelegateRun<'a> {
    model: &'a str,
    policy: &'a str,
    params: Value,
    child_tools: &'a ToolBundle,
    read_only: bool,
    resources: &'a Resources,
    handles: &'a SessionHandles,
    recorder: &'a Recorder,
    provider_label: &'static str,
    reasoning_effort: Option<String>,
}

/// Drive a subagent to its answer, generic over the provider client `C` (so
/// every backend shares this loop). Returns the last model turn's text.
async fn drive_delegate<C>(
    client: C,
    ctx: DelegateRun<'_>,
    mut seed_prompt: Message,
    mut seed_history: Vec<Message>,
) -> Result<String, DelegateError>
where
    C: CompletionClient,
    C::CompletionModel: 'static,
{
    let handles = ctx.handles;
    let recorder = ctx.recorder;
    // Per-run abort-retry budget: this delegate's own counter, isolated from
    // the main agent and any sibling delegates.
    let retry_budget = handles.rules.retry_budget();
    let mut retries_used = 0u32;
    let output = loop {
        let run_id = format!("r-{}", uuid::Uuid::new_v4().simple());
        let run_recorder = recorder.with_run(&run_id);
        let ttsr = TtsrShared::new(
            handles.rules.clone(),
            Arc::clone(&handles.rule_set),
            true,
            retries_used < retry_budget,
        );
        let mut builder = client
            .agent(ctx.model)
            .preamble(ctx.policy)
            .additional_params(ctx.params.clone())
            .tool(ctx.child_tools.read.clone())
            .tool(ctx.child_tools.find.clone())
            .tool(ctx.child_tools.grep.clone())
            .tool(ctx.resources.skill_tool())
            .add_hook(CaptureHook::new(
                run_recorder.clone(),
                handles.attachments.clone(),
                ToolMeta::default(),
            ))
            .add_hook(TtsrHook(Arc::clone(&ttsr)));
        if !ctx.read_only {
            builder = builder
                .tool(ctx.child_tools.bash.clone())
                .tool(ctx.child_tools.edit.clone())
                .tool(ctx.child_tools.write.clone());
        }
        let agent = builder.default_max_turns(usize::MAX).build();
        run_recorder.record(RunStarted {
            provider: ctx.provider_label.to_owned(),
            model: ctx.model.to_owned(),
            reasoning_effort: ctx.reasoning_effort.clone(),
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
                _ = handles.cancel.cancelled() => {
                    run_recorder.record(RunFinished::Cancelled);
                    recorder.record(DelegateFinished { outcome: "cancelled".into() });
                    return Err(DelegateError::Failed("cancelled".into()));
                }
                item = stream.next() => item,
            };
            let Some(item) = item else {
                // Match any trailing text/reasoning below the coalesce threshold
                // before the run completes.
                if ttsr.finalize_reasoning() || ttsr.finalize_text() {
                    let firing = ttsr.take_pending().expect("finalize stashed the firing");
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
                run_recorder.record(RunFinished::Completed);
                break;
            };
            match item {
                Ok(MultiTurnStreamItem::CompletionCall(_)) => turn_text.clear(),
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(
                    text,
                ))) => turn_text.push_str(&text.text),
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
    Ok(output)
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
mod tests {
    use super::*;

    #[test]
    fn delegate_args_accept_model_and_reasoning_overrides() {
        let args: DelegateArgs = serde_json::from_value(json!({
            "model": "gpt-5.1-codex-mini",
            "reasoning": "high"
        }))
        .unwrap();

        assert_eq!(args.model.as_deref(), Some("gpt-5.1-codex-mini"));
        assert_eq!(args.reasoning.as_deref(), Some("high"));
    }

    #[test]
    fn delegate_args_accept_reasoning_level_alias() {
        let args: DelegateArgs =
            serde_json::from_value(json!({"reasoningLevel": "medium"})).unwrap();

        assert_eq!(args.reasoning.as_deref(), Some("medium"));
    }
}
