use std::sync::Arc;

use crate::{
    SessionHandles,
    capture::{CaptureHook, ToolMeta},
    delegate_jobs::DelegateJobs,
    resources::Resources,
    ttsr::{TtsrHook, TtsrShared, reminder_message},
};
use artist_session::{DelegateFinished, DelegateStarted, RunFinished, RunStarted};
use artist_tools::{TOOL_POLICY, ToolBundle};
use futures::StreamExt;
use llm_provider::SavedProvider;
use rig_core::{
    agent::MultiTurnStreamItem,
    client::CompletionClient,
    completion::Message,
    providers::chatgpt,
    streaming::{StreamedAssistantContent, StreamingChat},
    tool::Tool,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone)]
pub(crate) struct Delegate {
    provider: SavedProvider,
    tools: ToolBundle,
    context: Vec<Message>,
    jobs: DelegateJobs,
    resources: Resources,
    handles: SessionHandles,
}

impl Delegate {
    pub fn new(
        provider: SavedProvider,
        tools: ToolBundle,
        context: Vec<Message>,
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
                    args.read_only.unwrap_or(true),
                    args.fork.unwrap_or(false),
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
    ) -> Result<String, DelegateError> {
        let model = self
            .provider
            .model
            .as_deref()
            .ok_or(DelegateError::MissingModel)?;
        let client = chatgpt::Client::builder()
            .api_key(chatgpt::ChatGPTAuth::AccessToken {
                access_token: self.provider.auth.access_token.expose().to_owned(),
                account_id: Some(self.provider.auth.account_id.clone()),
            })
            .base_url(self.provider.base_url.as_str())
            .originator("artist")
            .user_agent(concat!("artist/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| DelegateError::Failed(error.to_string()))?;
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
            background: false,
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

        let mut seed_history = if fork {
            self.context.clone()
        } else {
            Vec::new()
        };
        let mut seed_prompt = Message::user(&prompt);

        let output = loop {
            let run_id = format!("r-{}", uuid::Uuid::new_v4().simple());
            let run_recorder = recorder.with_run(&run_id);
            let ttsr = TtsrShared::new(
                self.handles.rules.clone(),
                Arc::clone(&self.handles.rule_set),
                true,
            );
            let mut builder = client.agent(model).preamble(&policy);
            if let Some(effort) = &self.provider.reasoning_effort {
                builder = builder
                    .additional_params(json!({"reasoning":{"effort":effort,"summary":"auto"}}));
            }
            let mut builder = builder
                .tool(child_tools.read.clone())
                .tool(child_tools.find.clone())
                .tool(child_tools.grep.clone())
                .tool(self.resources.instructions_tool())
                .tool(self.resources.skill_tool())
                .add_hook(CaptureHook::new(
                    run_recorder.clone(),
                    self.handles.attachments.clone(),
                    ToolMeta::default(),
                ))
                .add_hook(TtsrHook(Arc::clone(&ttsr)));
            if !read_only {
                builder = builder
                    .tool(child_tools.bash.clone())
                    .tool(child_tools.edit.clone())
                    .tool(child_tools.write.clone());
            }
            let agent = builder.default_max_turns(usize::MAX).build();
            run_recorder.record(RunStarted {
                provider: "chatgpt".to_owned(),
                model: model.to_owned(),
                reasoning_effort: self.provider.reasoning_effort.clone(),
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
                    Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::Text(text),
                    )) => turn_text.push_str(&text.text),
                    Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::ReasoningDelta {
                            id: None,
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
        Ok(shorten(&output, 50 * 1024))
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
