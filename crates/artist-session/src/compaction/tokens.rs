use rig_core::completion::Message;
use rig_core::completion::message::{
    AssistantContent, ReasoningContent, ToolResultContent, UserContent,
};

const ESTIMATED_IMAGE_CHARS: usize = 4_800;

/// Conservative character-based estimate used when provider usage is absent.
pub fn estimate_messages_tokens(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_message_tokens).sum()
}

pub fn estimate_message_tokens(message: &Message) -> u64 {
    let chars = match message {
        Message::System { content } => content.len(),
        Message::User { content } => content.iter().map(estimate_user_content).sum(),
        Message::Assistant { content, .. } => content
            .iter()
            .map(|item| match item {
                AssistantContent::Text(text) => text.text.len(),
                AssistantContent::ToolCall(call) => {
                    call.function.name.len() + call.function.arguments.to_string().len()
                }
                AssistantContent::Reasoning(reasoning) => reasoning
                    .content
                    .iter()
                    .map(|part| match part {
                        ReasoningContent::Text { text, .. } | ReasoningContent::Summary(text) => {
                            text.len()
                        }
                        ReasoningContent::Encrypted(data) => data.len(),
                        ReasoningContent::Redacted { data } => data.len(),
                        _ => 0,
                    })
                    .sum(),
                _ => ESTIMATED_IMAGE_CHARS,
            })
            .sum(),
    };
    chars.div_ceil(4) as u64
}

fn estimate_user_content(item: &UserContent) -> usize {
    match item {
        UserContent::Text(text) => text.text.len(),
        UserContent::ToolResult(result) => result
            .content
            .iter()
            .map(|item| match item {
                ToolResultContent::Text(text) => text.text.len(),
                ToolResultContent::Image(_) => ESTIMATED_IMAGE_CHARS,
            })
            .sum(),
        UserContent::Image(_) => ESTIMATED_IMAGE_CHARS,
        _ => ESTIMATED_IMAGE_CHARS,
    }
}
