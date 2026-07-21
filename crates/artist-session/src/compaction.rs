//! Pure, Rig-message-native context compaction preparation.
//!
//! Artist keeps its append-only event log intact. Compaction replaces only the
//! active [`rig_core::memory::ConversationMemory`] snapshot with a structured
//! summary plus a recent suffix.

mod files;
mod serialization;
mod tokens;

use rig_core::completion::Message;
use rig_core::completion::message::UserContent;

pub use serialization::{format_file_operations, serialize_conversation};
pub use tokens::{estimate_message_tokens, estimate_messages_tokens};

use files::FileOperations;

const SUMMARY_OPEN: &str = "<conversation-summary>";
const SUMMARY_CLOSE: &str = "</conversation-summary>";

/// Prepared compaction input and the recent suffix that remains verbatim.
#[derive(Clone, Debug)]
pub struct CompactionPlan {
    pub previous_summary: Option<String>,
    pub messages_to_summarize: Vec<Message>,
    pub turn_prefix_messages: Vec<Message>,
    pub kept_messages: Vec<Message>,
    pub is_split_turn: bool,
    pub tokens_before: u64,
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

impl CompactionPlan {
    pub fn serialized_history(&self) -> String {
        serialize_conversation(&self.messages_to_summarize)
    }

    pub fn serialized_turn_prefix(&self) -> String {
        serialize_conversation(&self.turn_prefix_messages)
    }

    /// Build the model-facing snapshot: summary first, recent messages after.
    pub fn snapshot(&self, summary: &str) -> Vec<Message> {
        let mut messages = Vec::with_capacity(self.kept_messages.len() + 1);
        messages.push(summary_message(summary));
        messages.extend(self.kept_messages.clone());
        messages
    }
}

/// Find a turn-aware cut point that retains approximately `keep_recent_tokens`.
/// Tool-result messages are never selected as cut points.
pub fn prepare_compaction(messages: &[Message], keep_recent_tokens: u64) -> Option<CompactionPlan> {
    let previous_summary = messages.first().and_then(compaction_summary);
    let boundary = usize::from(previous_summary.is_some());
    if boundary >= messages.len() {
        return None;
    }

    let valid = (boundary..messages.len())
        .filter(|&index| is_cut_point(&messages[index]))
        .collect::<Vec<_>>();
    let first_valid = *valid.first()?;
    let mut cut = first_valid;
    let mut accumulated = 0u64;
    let mut reached_budget = false;
    for index in (boundary..messages.len()).rev() {
        accumulated = accumulated.saturating_add(estimate_message_tokens(&messages[index]));
        if accumulated >= keep_recent_tokens {
            if let Some(index) = valid.iter().copied().find(|candidate| *candidate >= index) {
                cut = index;
            }
            reached_budget = true;
            break;
        }
    }
    if !reached_budget || cut == boundary {
        return None;
    }

    let turn_start = if is_turn_start(&messages[cut]) {
        None
    } else {
        (boundary..cut)
            .rev()
            .find(|&index| is_turn_start(&messages[index]))
    };
    let is_split_turn = turn_start.is_some();
    let history_end = turn_start.unwrap_or(cut);
    let messages_to_summarize = messages[boundary..history_end].to_vec();
    let turn_prefix_messages = turn_start
        .map(|start| messages[start..cut].to_vec())
        .unwrap_or_default();
    if messages_to_summarize.is_empty() && turn_prefix_messages.is_empty() {
        return None;
    }

    let mut files = FileOperations::default();
    if let Some(summary) = previous_summary.as_deref() {
        files.extend_summary(summary);
    }
    for message in messages_to_summarize.iter().chain(&turn_prefix_messages) {
        files.observe(message);
    }
    let (read_files, modified_files) = files.finish();

    Some(CompactionPlan {
        previous_summary,
        messages_to_summarize,
        turn_prefix_messages,
        kept_messages: messages[cut..].to_vec(),
        is_split_turn,
        tokens_before: estimate_messages_tokens(messages),
        read_files,
        modified_files,
    })
}

pub fn summary_message(summary: &str) -> Message {
    Message::user(format!(
        "{SUMMARY_OPEN}\n{}\n{SUMMARY_CLOSE}",
        summary.trim()
    ))
}

fn compaction_summary(message: &Message) -> Option<String> {
    let Message::User { content } = message else {
        return None;
    };
    let text = content.iter().find_map(|item| match item {
        UserContent::Text(text) => Some(text.text.trim()),
        _ => None,
    })?;
    text.strip_prefix(SUMMARY_OPEN)?
        .strip_suffix(SUMMARY_CLOSE)
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .map(str::to_owned)
}

fn is_cut_point(message: &Message) -> bool {
    matches!(message, Message::Assistant { .. } | Message::System { .. }) || is_turn_start(message)
}

fn is_turn_start(message: &Message) -> bool {
    let Message::User { content } = message else {
        return matches!(message, Message::System { .. });
    };
    !content
        .iter()
        .any(|item| matches!(item, UserContent::ToolResult(_)))
}

#[cfg(test)]
mod tests;
