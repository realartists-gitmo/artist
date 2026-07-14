//! The tool-free Artist agent loop, built on Rig.

use anyhow::{Context, Result};
use futures::StreamExt;
use llm_provider::SavedProvider;
use rig_core::{
    agent::MultiTurnStreamItem,
    client::CompletionClient,
    providers::chatgpt,
    streaming::{StreamedAssistantContent, StreamingPrompt},
};
use serde_json::json;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptEvent {
    ReasoningSummaryDelta(String),
    TextDelta(String),
}

/// Executes one prompt and emits model output as soon as each delta arrives.
pub async fn stream_prompt(
    provider: &SavedProvider,
    input: &str,
    mut on_event: impl FnMut(PromptEvent) -> Result<()>,
) -> Result<()> {
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
        builder =
            builder.additional_params(json!({"reasoning": {"effort": effort, "summary": "auto"}}));
    }
    let mut stream = builder.build().stream_prompt(input).await;
    while let Some(item) = stream.next().await {
        match item.context("stream Artist agent")? {
            MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text)) => {
                on_event(PromptEvent::TextDelta(text.text))?;
            }
            MultiTurnStreamItem::StreamAssistantItem(
                StreamedAssistantContent::ReasoningDelta {
                    id: None,
                    reasoning,
                },
            ) => on_event(PromptEvent::ReasoningSummaryDelta(reasoning))?,
            _ => {}
        }
    }
    Ok(())
}
