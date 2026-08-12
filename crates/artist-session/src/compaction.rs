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
const EXACT_OPEN: &str = "<artist-exact-context codec=\"presentation-v1\">";
const EXACT_CLOSE: &str = "</artist-exact-context>";

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

    /// Build the non-lossy replacement snapshot.  The checkpoint is a
    /// reversible structural encoding of every compacted Rig message; it is
    /// not an LLM-produced account of those messages.  Repeated compactions
    /// first expand the prior checkpoint, so they do not accumulate nested
    /// approximations or discard continuation/stale-recovery details.
    pub fn exact_snapshot(&self, messages: &[Message]) -> Vec<Message> {
        let mut snapshot = Vec::with_capacity(self.kept_messages.len() + 1);
        snapshot.push(exact_context_message(messages));
        snapshot.extend(self.kept_messages.clone());
        snapshot
    }

    /// The complete exact prefix that the checkpoint replaces.  A prior exact
    /// checkpoint is expanded before re-encoding; a legacy summary cannot be
    /// expanded and is retained verbatim as historical input rather than being
    /// silently treated as ground truth.
    pub fn exact_prefix(&self, original: &[Message]) -> Vec<Message> {
        let boundary = usize::from(original.first().is_some_and(is_compaction_checkpoint));
        let mut messages = if boundary == 1 {
            exact_context_messages(&original[0]).unwrap_or_else(|| vec![original[0].clone()])
        } else {
            Vec::new()
        };
        messages.extend_from_slice(&original[boundary..self.kept_start(original)]);
        messages
    }

    fn kept_start(&self, original: &[Message]) -> usize {
        original.len().saturating_sub(self.kept_messages.len())
    }
}

/// Find a turn-aware cut point that retains approximately `keep_recent_tokens`.
/// Tool-result messages are never selected as cut points.
pub fn prepare_compaction(messages: &[Message], keep_recent_tokens: u64) -> Option<CompactionPlan> {
    let previous_summary = messages.first().and_then(compaction_summary);
    let boundary = usize::from(messages.first().is_some_and(is_compaction_checkpoint));
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

/// Make a lossless, model-readable structural checkpoint.  JSON is used for
/// the Rig-native shape (roles, tool IDs, arguments, result blocks and all
/// continuation selectors); the presentation grammar then applies only
/// reversible syntax compression.  This function intentionally returns a
/// `Message` rather than a string so the only model-visible representation is
/// the exact checkpoint, never an expanded transcript substituted after the
/// fact.
pub fn exact_context_message(messages: &[Message]) -> Message {
    let canonical = serde_json::to_string(messages)
        .expect("Rig messages used by the durable session event schema serialize");
    let presentation = crate::presentation::Presentation::from_canonical(canonical);
    presentation
        .verify()
        .expect("freshly encoded exact compaction presentation verifies");
    Message::user(format!(
        "{EXACT_OPEN}\n{}\n{EXACT_CLOSE}",
        presentation.encoded
    ))
}

/// Recover the original compacted messages from an exact checkpoint.  Invalid
/// checkpoints are deliberately not guessed at: callers retain the literal
/// message instead of claiming a fabricated recovery.
pub fn exact_context_messages(message: &Message) -> Option<Vec<Message>> {
    let Message::User { content } = message else {
        return None;
    };
    let text = content.iter().find_map(|item| match item {
        UserContent::Text(text) => Some(text.text.trim()),
        _ => None,
    })?;
    let encoded = text
        .strip_prefix(EXACT_OPEN)?
        .strip_suffix(EXACT_CLOSE)?
        .trim();
    let canonical = crate::presentation::decode(encoded).ok()?;
    serde_json::from_str(&canonical).ok()
}

fn is_compaction_checkpoint(message: &Message) -> bool {
    compaction_summary(message).is_some() || exact_context_messages(message).is_some()
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
