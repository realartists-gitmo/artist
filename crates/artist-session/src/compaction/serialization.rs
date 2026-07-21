use rig_core::completion::Message;
use rig_core::completion::message::{AssistantContent, ToolResultContent, UserContent};

const TOOL_RESULT_MAX_CHARS: usize = 2_000;

/// Serialize history as labelled source instead of chat messages, preventing
/// the summarizer from trying to continue the embedded conversation.
pub fn serialize_conversation(messages: &[Message]) -> String {
    let mut parts = Vec::new();
    for message in messages {
        match message {
            Message::System { content } if !content.trim().is_empty() => {
                parts.push(format!("[System]: {}", content.trim()));
            }
            Message::User { content } => serialize_user(content, &mut parts),
            Message::Assistant { content, .. } => {
                let mut reasoning = Vec::new();
                let mut text = Vec::new();
                let mut calls = Vec::new();
                for item in content.iter() {
                    match item {
                        AssistantContent::Text(value) if !value.text.trim().is_empty() => {
                            text.push(value.text.trim().to_owned());
                        }
                        AssistantContent::Reasoning(value) => {
                            let display = value.display_text();
                            if !display.trim().is_empty() {
                                reasoning.push(display.trim().to_owned());
                            }
                        }
                        AssistantContent::ToolCall(call) => calls.push(format!(
                            "{}({})",
                            call.function.name, call.function.arguments
                        )),
                        _ => {}
                    }
                }
                if !reasoning.is_empty() {
                    parts.push(format!("[Assistant thinking]: {}", reasoning.join("\n")));
                }
                if !text.is_empty() {
                    parts.push(format!("[Assistant]: {}", text.join("\n")));
                }
                if !calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", calls.join("; ")));
                }
            }
            _ => {}
        }
    }
    parts.join("\n\n")
}

fn serialize_user(content: &rig_core::OneOrMany<UserContent>, parts: &mut Vec<String>) {
    let mut user = Vec::new();
    for item in content.iter() {
        match item {
            UserContent::Text(text) if !text.text.trim().is_empty() => {
                user.push(text.text.trim().to_owned());
            }
            UserContent::ToolResult(result) => {
                let output = result
                    .content
                    .iter()
                    .filter_map(|item| match item {
                        ToolResultContent::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !output.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate(&output, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
            UserContent::Image(_) => user.push("[image]".to_owned()),
            _ => {}
        }
    }
    if !user.is_empty() {
        parts.push(format!("[User]: {}", user.join("\n")));
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    let kept = text.chars().take(max_chars).collect::<String>();
    format!(
        "{kept}\n\n[... {} more characters truncated]",
        count - max_chars
    )
}

pub fn format_file_operations(read_files: &[String], modified_files: &[String]) -> String {
    let mut sections = Vec::new();
    if !read_files.is_empty() {
        sections.push(format!(
            "<read-files>\n{}\n</read-files>",
            read_files.join("\n")
        ));
    }
    if !modified_files.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified_files.join("\n")
        ));
    }
    if sections.is_empty() {
        String::new()
    } else {
        format!("\n\n{}", sections.join("\n\n"))
    }
}
