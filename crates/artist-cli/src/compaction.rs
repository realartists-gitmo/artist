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

fn supports_remote_compaction(provider: &SavedProvider) -> bool {
    supports_remote_compaction_transport(provider.provider, provider.api)
}

fn supports_remote_compaction_transport(
    provider: llm_provider::ProviderKind,
    api: Option<llm_provider::OpenAiApi>,
) -> bool {
    provider == llm_provider::ProviderKind::Chatgpt
        || (provider == llm_provider::ProviderKind::Openai
            && api == Some(llm_provider::OpenAiApi::Responses))
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

/// Replace stale computer observations with stubs before a turn.
///
/// Runs *before* the compaction threshold check, so the reclaimed context
/// counts toward it — decaying a few screenshots often avoids a compaction
/// outright, which is much cheaper than summarizing.
///
/// **Skipped when the provider sidecar is authoritative.** On ChatGPT and
/// OpenAI Responses, remote compaction resets Rig memory to empty and the
/// canonical history lives provider-side; rewriting a local copy the provider is
/// about to supersede changes nothing the model sees and saves no tokens.
pub(crate) async fn decay(
    active: &ActiveSession,
    provider: &SavedProvider,
    settings: crate::settings::ComputerConfig,
) -> Result<Option<Vec<Message>>> {
    if !settings.enabled || settings.keep_recent_observations == 0 {
        return Ok(None);
    }
    if supports_remote_compaction(provider) {
        return Ok(None);
    }

    active.recorder.flush().await;
    let mut history = active
        .memory
        .load(&active.session.id)
        .await
        .context("load history for observation decay")?;

    let policy = artist_session::decay::DecayPolicy {
        keep_recent: settings.keep_recent_observations,
        ..artist_session::decay::DecayPolicy::default()
    };
    let Some(outcome) = artist_session::decay::decay_observations(&mut history, &policy) else {
        return Ok(None);
    };

    active
        .memory
        .revise(history.clone())
        .await
        .context("record decayed history")?;
    active.recorder.record(artist_session::ComputerElided {
        count: outcome.elided,
        bytes_saved: outcome.bytes_saved,
    });
    Ok(Some(history))
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

    // Custom instructions are a local summarizer feature; never silently drop
    // them on the provider-side path. Other providers likewise remain local.
    if custom_instructions.is_none() && supports_remote_compaction(provider) {
        let model = provider
            .model
            .as_deref()
            .context("no model selected; run `artist model` first")?;
        match artist_agent::compaction::remote(
            provider,
            history.clone(),
            &active.session.id,
            active.provider_context.clone(),
            model,
        )
        .await?
        {
            artist_agent::compaction::RemoteCompaction::Compacted { .. } => {
                let tokens_before = measured_tokens.unwrap_or(plan.tokens_before);
                let summarized_messages = history.len();
                // The opaque canonical item now lives solely in the durable
                // provider sidecar. Reset Rig memory so it cannot resend the
                // superseded portable transcript; display projection remains
                // append-only because SessionMemory::compact hides this reset.
                active
                    .memory
                    .compact(
                        Vec::new(),
                        ConversationCompacted {
                            summary: "Context compacted by OpenAI Responses".to_owned(),
                            tokens_before,
                            kept_messages: 0,
                            reason: reason.to_owned(),
                            read_files: plan.read_files.clone(),
                            modified_files: plan.modified_files.clone(),
                        },
                    )
                    .await
                    .context("persist provider-compacted conversation")?;
                return Ok(Some(CompactionResult {
                    history: Vec::new(),
                    summarized_messages,
                    tokens_before,
                }));
            }
            artist_agent::compaction::RemoteCompaction::Unsupported => {
                // No sidecar or memory mutation occurred; use the existing
                // local summary path below.
            }
        }
    }

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
    fn remote_compaction_excludes_openai_chat_completions() {
        use llm_provider::{OpenAiApi, ProviderKind};
        assert!(supports_remote_compaction_transport(
            ProviderKind::Chatgpt,
            None
        ));
        assert!(supports_remote_compaction_transport(
            ProviderKind::Openai,
            Some(OpenAiApi::Responses)
        ));
        assert!(!supports_remote_compaction_transport(
            ProviderKind::Openai,
            Some(OpenAiApi::ChatCompletions)
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
