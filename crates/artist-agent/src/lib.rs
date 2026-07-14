//! The tool-free Artist agent loop, built on Rig.

use anyhow::{Context, Result};
use llm_provider::SavedProvider;
use rig_core::{client::CompletionClient, completion::Prompt, providers::chatgpt};
use serde_json::json;

/// Executes one prompt with the provider's selected model and reasoning effort.
pub async fn prompt(provider: &SavedProvider, input: &str) -> Result<String> {
    let model = provider
        .model
        .as_deref()
        .context("no model selected; run `artist model` first")?;
    let client = chatgpt::Client::builder()
        .api_key(chatgpt::ChatGPTAuth::AccessToken {
            access_token: provider.auth.access_token.expose().to_owned(),
            account_id: Some(provider.auth.account_id.clone()),
        })
        .base_url(provider.base_url.as_str())
        .originator("artist")
        .user_agent(concat!("artist/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build ChatGPT client")?;

    let mut builder = client.agent(model);
    if let Some(effort) = &provider.reasoning_effort {
        builder = builder.additional_params(json!({"reasoning": {"effort": effort}}));
    }
    builder
        .build()
        .prompt(input)
        .await
        .context("run Artist agent")
}
