//! Lossy display projection from Rig-native conversation messages.

use std::collections::HashMap;

use rig_core::completion::Message;
use rig_core::completion::message::{AssistantContent, ToolResultContent, UserContent};

use crate::replay::ReplayItem;

const TOOL_PREVIEW_CAP: usize = 160;

#[derive(Default)]
pub(crate) struct ConversationReplay {
    tool_names: HashMap<String, String>,
}

impl ConversationReplay {
    pub(crate) fn clear(&mut self) {
        self.tool_names.clear();
    }

    pub(crate) fn push(&mut self, message: &Message, items: &mut Vec<ReplayItem>) {
        match message {
            Message::System { .. } => {}
            Message::User { content } => {
                if let Some(text) = user_message_text(message)
                    && !text.starts_with("<system-reminder")
                {
                    items.push(ReplayItem::User(text));
                }
                for item in content.iter() {
                    if let UserContent::ToolResult(result) = item {
                        let preview = result
                            .content
                            .iter()
                            .find_map(|content| match content {
                                ToolResultContent::Text(text) => Some(
                                    text.text
                                        .lines()
                                        .next()
                                        .unwrap_or("")
                                        .chars()
                                        .take(TOOL_PREVIEW_CAP)
                                        .collect(),
                                ),
                                _ => None,
                            })
                            .unwrap_or_default();
                        items.push(ReplayItem::Tool {
                            name: self
                                .tool_names
                                .remove(&result.id)
                                .unwrap_or_else(|| "tool".to_owned()),
                            preview,
                        });
                    }
                }
            }
            Message::Assistant { content, .. } => {
                let mut text = String::new();
                let mut reasoning = String::new();
                for item in content.iter() {
                    match item {
                        AssistantContent::Text(value) => text.push_str(&value.text),
                        AssistantContent::Reasoning(value) => {
                            reasoning.push_str(&value.display_text());
                        }
                        AssistantContent::ToolCall(call) => {
                            self.tool_names
                                .insert(call.id.clone(), call.function.name.clone());
                        }
                        _ => {}
                    }
                }
                if !reasoning.is_empty() {
                    items.push(ReplayItem::Reasoning(reasoning));
                }
                if !text.is_empty() {
                    items.push(ReplayItem::Assistant(text));
                }
            }
        }
    }
}

pub(crate) fn user_message_text(message: &Message) -> Option<String> {
    let Message::User { content } = message else {
        return None;
    };
    let mut display = None;
    for item in content.iter() {
        if let UserContent::Text(text) = item {
            display = Some(text.text.clone());
        }
    }
    display
}

pub(crate) fn markdown_messages(messages: &[Message]) -> String {
    let mut replay = ConversationReplay::default();
    let mut items = Vec::new();
    for message in messages {
        replay.push(message, &mut items);
    }
    items
        .into_iter()
        .filter_map(|item| match item {
            ReplayItem::User(text) => Some(format!("\n## User\n\n{text}\n")),
            ReplayItem::Assistant(text) => Some(format!("\n## Assistant\n\n{text}\n")),
            ReplayItem::Reasoning(text) => Some(format!("\n> reasoning: {text}\n")),
            ReplayItem::Tool { name, .. } => Some(format!("\n> tool: {name}\n")),
            _ => None,
        })
        .collect()
}
