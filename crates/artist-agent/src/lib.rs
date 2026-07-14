//! The tool-free Artist agent loop, built on Rig.

use anyhow::{Context, Result};
use futures::StreamExt;
use llm_provider::SavedProvider;
use rig_core::{
    agent::MultiTurnStreamItem,
    client::CompletionClient,
    providers::chatgpt,
    streaming::{StreamedAssistantContent, StreamingChat},
};
use serde_json::json;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptEvent {
    ReasoningSummaryDelta(String),
    TextDelta(String),
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

/// Executes one prompt with prior chat context and emits model output as it arrives.
pub async fn stream_chat(
    provider: &SavedProvider,
    input: &str,
    history: &[ChatMessage],
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
    let agent = builder.build();
    let messages = history.iter().map(|message| match message.role {
        ChatRole::User => rig_core::completion::Message::user(&message.content),
        ChatRole::Assistant => rig_core::completion::Message::assistant(&message.content),
    });
    let mut stream = agent.stream_chat(input, messages).await;
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

/// Executes a prompt without prior context.
pub async fn stream_prompt(
    provider: &SavedProvider,
    input: &str,
    on_event: impl FnMut(PromptEvent) -> Result<()>,
) -> Result<()> {
    stream_chat(provider, input, &[], on_event).await
}
