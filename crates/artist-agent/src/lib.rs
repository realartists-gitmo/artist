//! The Artist agent loop, built on Rig.

mod delegate;
mod delegate_jobs;
pub mod mcp;
mod resources;

pub use resources::AvailableSkill;
mod steering;

pub use steering::SteeringHandle;

use anyhow::{Context, Result};
use artist_tools::ToolBundle;
use base64::Engine;
use futures::StreamExt;
use llm_provider::SavedProvider;
use rig_core::{
    OneOrMany,
    agent::MultiTurnStreamItem,
    client::CompletionClient,
    completion::message::{
        DocumentSourceKind, Image, ImageMediaType, Message, ToolResultContent, UserContent,
    },
    providers::chatgpt,
    streaming::{StreamedAssistantContent, StreamedUserContent, StreamingChat},
};
use serde_json::json;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptEvent {
    ReasoningSummaryDelta(String),
    TextDelta(String),
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    ToolExecutionStart {
        id: String,
        name: String,
    },
    ToolResult {
        id: String,
        content: String,
    },
    CompletionUsage {
        total_tokens: u64,
    },
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

#[derive(Clone, Debug)]
pub struct ImageAttachment {
    pub data: Vec<u8>,
    pub media_type: ImageMediaType,
}

impl ImageAttachment {
    pub fn png(data: Vec<u8>) -> Self {
        Self {
            data,
            media_type: ImageMediaType::PNG,
        }
    }
    pub fn jpeg(data: Vec<u8>) -> Self {
        Self {
            data,
            media_type: ImageMediaType::JPEG,
        }
    }
    pub fn gif(data: Vec<u8>) -> Self {
        Self {
            data,
            media_type: ImageMediaType::GIF,
        }
    }
    pub fn webp(data: Vec<u8>) -> Self {
        Self {
            data,
            media_type: ImageMediaType::WEBP,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChatInput {
    pub text: String,
    pub images: Vec<ImageAttachment>,
}

impl From<String> for ChatInput {
    fn from(text: String) -> Self {
        Self {
            text,
            images: Vec::new(),
        }
    }
}

/// Executes one prompt with prior chat context and emits model output as it arrives.
pub async fn stream_chat(
    provider: &SavedProvider,
    input: &ChatInput,
    history: &[ChatMessage],
    tools: &ToolBundle,
    mcp: &mcp::McpManager,
    steering: SteeringHandle,
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
    let resources = resources::Resources::discover(tools.project_root());
    let system_prompt = format!(
        "{}{}{}\nCurrent working directory: {}",
        include_str!("system_prompt.md"),
        resources.prompt_section(),
        resources.explicit_skill_section(&input.text),
        tools.project_root().display()
    );
    let messages = history
        .iter()
        .map(|message| match message.role {
            ChatRole::User => rig_core::completion::Message::user(&message.content),
            ChatRole::Assistant => rig_core::completion::Message::assistant(&message.content),
        })
        .collect::<Vec<_>>();
    let mut fork_context = messages.clone();
    let input_message = user_message(input);
    fork_context.push(input_message.clone());
    let visible_steering = steering.clone();
    let mcp_tools = mcp
        .tools()
        .await
        .into_iter()
        .map(|tool| Box::new(tool) as Box<dyn rig_core::tool::ToolDyn>)
        .collect();
    let agent = builder
        .preamble(&system_prompt)
        .tool(tools.bash.clone())
        .tool(tools.read.clone())
        .tool(tools.find.clone())
        .tool(tools.grep.clone())
        .tool(tools.edit.clone())
        .tool(tools.write.clone())
        .tool(resources.instructions_tool())
        .tool(resources.skill_tool())
        .add_hook(steering::SteeringHook(steering))
        .tool(delegate::Delegate::new(
            provider.clone(),
            tools.clone(),
            fork_context,
            resources,
        ))
        .tools(mcp_tools)
        .default_max_turns(usize::MAX)
        .build();
    let mut stream = agent.stream_chat(input_message, messages).await;
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
            MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ToolCall {
                tool_call,
                internal_call_id,
            }) => on_event(PromptEvent::ToolCall {
                id: internal_call_id,
                name: tool_call.function.name,
                arguments: tool_call.function.arguments,
            })?,
            MultiTurnStreamItem::ToolExecutionStart {
                tool_call,
                internal_call_id,
            } => on_event(PromptEvent::ToolExecutionStart {
                id: internal_call_id,
                name: tool_call.function.name,
            })?,
            MultiTurnStreamItem::CompletionCall(call) => {
                on_event(PromptEvent::CompletionUsage {
                    total_tokens: call.usage.total_tokens,
                })?;
            }
            MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                tool_result,
                internal_call_id,
            }) => {
                let content = tool_result
                    .content
                    .into_iter()
                    .filter_map(|item| match item {
                        ToolResultContent::Text(text) => Some(text.text),
                        ToolResultContent::Image(_) => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let content = visible_steering
                    .take_original_result(&internal_call_id)
                    .unwrap_or(content);
                on_event(PromptEvent::ToolResult {
                    id: internal_call_id,
                    content,
                })?;
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn user_message(input: &ChatInput) -> Message {
    let mut content = vec![UserContent::text(input.text.clone())];
    content.extend(input.images.iter().map(|attachment| {
        UserContent::Image(Image {
            data: DocumentSourceKind::Base64(
                base64::engine::general_purpose::STANDARD.encode(&attachment.data),
            ),
            media_type: Some(attachment.media_type.clone()),
            detail: None,
            additional_params: None,
        })
    }));
    Message::User {
        content: OneOrMany::many(content).expect("chat input always contains text"),
    }
}

pub fn available_skills(project: &std::path::Path) -> Vec<AvailableSkill> {
    resources::Resources::discover(project).available_skills()
}

/// Executes a prompt without prior context.
pub async fn stream_prompt(
    provider: &SavedProvider,
    input: &str,
    tools: &ToolBundle,
    mcp: &mcp::McpManager,
    on_event: impl FnMut(PromptEvent) -> Result<()>,
) -> Result<()> {
    let input = ChatInput::from(input.to_owned());
    stream_chat(
        provider,
        &input,
        &[],
        tools,
        mcp,
        SteeringHandle::default(),
        on_event,
    )
    .await
}
