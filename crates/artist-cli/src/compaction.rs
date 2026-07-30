use crate::{sessions::ActiveSession, settings::CompactionConfig};
use anyhow::{Context, Result};
use artist_session::ConversationCompacted;
use llm_provider::SavedProvider;
use rig_core::completion::Message;
use rig_core::memory::ConversationMemory;

pub(crate) struct CompactionResult {
    pub history: Vec<Message>,
    pub summarized_messages: usize,
    pub tokens_before: u64,
}

pub(crate) fn should_compact(
    context_tokens: u64,
    context_window: u64,
    settings: CompactionConfig,
) -> bool {
    settings.enabled && context_tokens > context_window.saturating_sub(settings.reserve_tokens)
}

pub(crate) fn projected_context_tokens(
    history: &[Message],
    last_usage: Option<u64>,
    prompt_text: &str,
    image_count: usize,
) -> u64 {
    let current = last_usage
        .filter(|tokens| *tokens > 0)
        .unwrap_or_else(|| artist_session::compaction::estimate_messages_tokens(history));
    current
        .saturating_add(prompt_text.len().div_ceil(4) as u64)
        .saturating_add((image_count as u64).saturating_mul(1_200))
}

/// Generate and atomically append a compaction checkpoint plus its reset
/// snapshot. Until summary generation succeeds, the active memory is untouched.
pub(crate) async fn compact(
    active: &ActiveSession,
    provider: &SavedProvider,
    settings: CompactionConfig,
    custom_instructions: Option<&str>,
    reason: &str,
    measured_tokens: Option<u64>,
) -> Result<Option<CompactionResult>> {
    active.recorder.flush().await;
    let history = active
        .memory
        .load(&active.session.id)
        .await
        .context("load conversation for compaction")?;
    let Some(plan) =
        artist_session::compaction::prepare_compaction(&history, settings.keep_recent_tokens)
    else {
        return Ok(None);
    };
    let summary = artist_agent::compaction::summarize(
        provider,
        &plan,
        settings.reserve_tokens,
        custom_instructions,
    )
    .await?;
    let summarized_messages = plan.messages_to_summarize.len() + plan.turn_prefix_messages.len();
    let tokens_before = measured_tokens.unwrap_or(plan.tokens_before);
    let kept_messages = plan.kept_messages.len();
    let read_files = plan.read_files.clone();
    let modified_files = plan.modified_files.clone();
    let snapshot = plan.snapshot(&summary);
    active
        .memory
        .compact(
            snapshot.clone(),
            ConversationCompacted {
                summary,
                tokens_before,
                kept_messages,
                reason: reason.to_owned(),
                read_files,
                modified_files,
            },
        )
        .await
        .context("persist compacted conversation")?;
    Ok(Some(CompactionResult {
        history: snapshot,
        summarized_messages,
        tokens_before,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_reserves_output_space_and_honors_disable() {
        let settings = CompactionConfig {
            enabled: true,
            reserve_tokens: 20,
            keep_recent_tokens: 10,
        };
        assert!(!should_compact(80, 100, settings));
        assert!(should_compact(81, 100, settings));
        assert!(!should_compact(
            99,
            100,
            CompactionConfig {
                enabled: false,
                ..settings
            }
        ));
    }

    #[test]
    fn projection_prefers_provider_usage_and_includes_new_prompt() {
        let history = vec![Message::user(&"x".repeat(400))];
        assert_eq!(
            projected_context_tokens(&history, Some(50), "12345678", 1),
            1_252
        );
        assert_eq!(projected_context_tokens(&history, None, "", 0), 100);
    }
}
