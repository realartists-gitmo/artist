use crate::{delegate_jobs::DelegateJobs, resources::Resources};
use artist_tools::{TOOL_POLICY, ToolBundle};
use llm_provider::SavedProvider;
use rig_core::{
    client::CompletionClient,
    completion::{Chat, Message, Prompt},
    providers::chatgpt,
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
}

impl Delegate {
    pub fn new(
        provider: SavedProvider,
        tools: ToolBundle,
        context: Vec<Message>,
        resources: Resources,
    ) -> Self {
        let jobs = DelegateJobs::for_project(tools.project_root());
        Self {
            provider,
            tools,
            context,
            jobs,
            resources,
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
        let child_tools = self
            .tools
            .for_actor(&format!("delegate-{}", uuid::Uuid::new_v4().simple()))
            .map_err(|error| DelegateError::Failed(error.to_string()))?;
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
        let mut builder = client.agent(model).preamble(&policy);
        if let Some(effort) = &self.provider.reasoning_effort {
            builder =
                builder.additional_params(json!({"reasoning":{"effort":effort,"summary":"auto"}}));
        }
        let mut builder = builder
            .tool(child_tools.read.clone())
            .tool(child_tools.find.clone())
            .tool(child_tools.grep.clone())
            .tool(self.resources.instructions_tool())
            .tool(self.resources.skill_tool());
        if !read_only {
            builder = builder
                .tool(child_tools.bash.clone())
                .tool(child_tools.edit.clone())
                .tool(child_tools.write.clone());
        }
        let agent = builder.default_max_turns(usize::MAX).build();
        let output = if fork {
            let mut context = self.context.clone();
            agent.chat(prompt, &mut context).await
        } else {
            agent.prompt(prompt).await
        }
        .map_err(|error| DelegateError::Failed(error.to_string()))?;
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
