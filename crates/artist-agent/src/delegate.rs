use artist_tools::{TOOL_POLICY, ToolBundle};
use llm_provider::SavedProvider;
use rig_core::tool::Tool;
use rig_core::{client::CompletionClient, completion::Prompt, providers::chatgpt};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone)]
pub(crate) struct Delegate {
    pub provider: SavedProvider,
    pub tools: ToolBundle,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DelegateArgs {
    prompt: String,
    read_only: Option<bool>,
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
        "Run one focused subagent. The subagent never receives delegate; readOnly defaults to true."
            .into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"prompt":{"type":"string"},"readOnly":{"type":"boolean","default":true}},"required":["prompt"],"additionalProperties":false})
    }
    async fn call(&self, args: DelegateArgs) -> Result<String, DelegateError> {
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
        let read_only = args.read_only.unwrap_or(true);
        let child_tools = self
            .tools
            .for_actor(&format!("delegate-{}", uuid::Uuid::new_v4().simple()))
            .map_err(|error| DelegateError::Failed(error.to_string()))?;
        let policy = if read_only {
            format!(
                "{TOOL_POLICY}\nYou are a read-only subagent. You may inspect with read, find, and grep. Do not modify files or run shell commands. Return concise findings, evidence, uncertainty, and errors."
            )
        } else {
            format!(
                "{TOOL_POLICY}\nYou are a focused subagent. Complete only the delegated task and return concise findings, evidence, uncertainty, and errors. You cannot delegate further."
            )
        };
        let mut builder = client.agent(model).preamble(&policy);
        if let Some(effort) = &self.provider.reasoning_effort {
            builder = builder
                .additional_params(json!({"reasoning": {"effort": effort, "summary": "auto"}}));
        }
        let mut builder = builder
            .tool(child_tools.read.clone())
            .tool(child_tools.find.clone())
            .tool(child_tools.grep.clone());
        if !read_only {
            builder = builder
                .tool(child_tools.bash.clone())
                .tool(child_tools.edit.clone())
                .tool(child_tools.write.clone());
        }
        let output = builder
            .default_max_turns(12)
            .build()
            .prompt(args.prompt)
            .await
            .map_err(|error| DelegateError::Failed(error.to_string()))?;
        let summary = if output.len() > 50 * 1024 {
            let mut end = 50 * 1024 - 64;
            while end > 0 && !output.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}\n[truncated]", &output[..end])
        } else {
            output
        };
        Ok(json!({"status":"completed","summary":summary,"findings":[],"uncertainty":[],"errors":[]}).to_string())
    }
}
