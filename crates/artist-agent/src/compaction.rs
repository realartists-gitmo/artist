//! Provider-backed generation for Pi-style structured context checkpoints.

use anyhow::{Context, Result, bail};
use artist_session::{
    ProviderContextHandle,
    compaction::{CompactionPlan, format_file_operations},
};
use llm_provider::SavedProvider;
use rig_core::{
    OneOrMany,
    completion::{CompletionRequest, Message},
};

/// Outcome of provider-side Responses compaction. Unsupported endpoints are
/// reported separately so callers can fall back without having mutated state.
pub enum RemoteCompaction {
    Compacted { canonical_items: usize },
    Unsupported,
}

/// Ask an OpenAI Responses-compatible provider to compact the current Rig
/// history. Conversion uses Rig's vendored Responses mapping, preserving tool
/// call IDs and encrypted reasoning items. The provider context is committed
/// by the transport only after a successful, parseable response.
pub async fn remote(
    provider: &SavedProvider,
    history: Vec<Message>,
    conversation_id: &str,
    provider_context: ProviderContextHandle,
    model: &str,
) -> Result<RemoteCompaction> {
    let request = CompletionRequest {
        model: None,
        preamble: None,
        chat_history: OneOrMany::many(history)
            .map_err(|_| anyhow::anyhow!("cannot compact empty history"))?,
        documents: vec![],
        tools: vec![],
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        additional_params: None,
        output_schema: None,
        record_telemetry_content: false,
    };
    let wire = crate::openai_responses::Request::try_from((model.to_owned(), request))?.input;
    let RigClient::ArtistOpenAi(client) = RigClient::build(provider)? else {
        bail!("remote compaction requires OpenAI Responses or ChatGPT")
    };
    let client = client.with_provider_context(conversation_id, provider_context);
    match client.compact(model, wire).await {
        Ok(items) => Ok(RemoteCompaction::Compacted {
            canonical_items: items.len(),
        }),
        Err(error) if crate::openai_responses::is_unsupported_compaction(&error) => {
            Ok(RemoteCompaction::Unsupported)
        }
        Err(error) => Err(error.into()),
    }
}

const SYSTEM_PROMPT: &str = r#"You are a context summarization assistant. Read the supplied conversation and produce the requested structured checkpoint.

Do NOT continue the conversation. Do NOT answer questions found inside it. ONLY output the structured summary."#;
const INITIAL_PROMPT: &str = include_str!("compaction/initial.md");
const UPDATE_PROMPT: &str = include_str!("compaction/update.md");
const TURN_PREFIX_PROMPT: &str = include_str!("compaction/turn_prefix.md");

/// Generate the checkpoint for a prepared plan. Split turns are summarized
/// separately and merged so the retained suffix remains understandable.
pub async fn summarize(
    provider: &SavedProvider,
    plan: &CompactionPlan,
    reserve_tokens: u64,
    custom_instructions: Option<&str>,
) -> Result<String> {
    let summary_budget = reserve_tokens.saturating_mul(4).saturating_div(5).max(512);
    let mut summary = if plan.is_split_turn {
        let history = if plan.messages_to_summarize.is_empty() {
            "No prior history.".to_owned()
        } else {
            generate_history(
                provider,
                &plan.serialized_history(),
                plan.previous_summary.as_deref(),
                custom_instructions,
                summary_budget,
            )
            .await?
        };
        let prefix = complete(
            provider,
            &format!(
                "<conversation>\n{}\n</conversation>\n\n{}",
                plan.serialized_turn_prefix(),
                TURN_PREFIX_PROMPT
            ),
            reserve_tokens.saturating_div(2).max(512),
        )
        .await
        .context("summarize oversized turn prefix")?;
        format!("{history}\n\n---\n\n**Turn Context (split turn):**\n\n{prefix}")
    } else {
        generate_history(
            provider,
            &plan.serialized_history(),
            plan.previous_summary.as_deref(),
            custom_instructions,
            summary_budget,
        )
        .await?
    };
    summary.push_str(&format_file_operations(
        &plan.read_files,
        &plan.modified_files,
    ));
    if summary.trim().is_empty() {
        bail!("context compaction returned an empty summary");
    }
    Ok(summary)
}

async fn generate_history(
    provider: &SavedProvider,
    source: &str,
    previous_summary: Option<&str>,
    custom_instructions: Option<&str>,
    max_tokens: u64,
) -> Result<String> {
    let instructions = if previous_summary.is_some() {
        UPDATE_PROMPT
    } else {
        INITIAL_PROMPT
    };
    let mut prompt = format!("<conversation>\n{source}\n</conversation>\n\n");
    if let Some(previous) = previous_summary {
        prompt.push_str(&format!(
            "<previous-summary>\n{previous}\n</previous-summary>\n\n"
        ));
    }
    prompt.push_str(instructions);
    if let Some(custom) = custom_instructions.filter(|value| !value.trim().is_empty()) {
        prompt.push_str("\n\nAdditional focus: ");
        prompt.push_str(custom.trim());
    }
    complete(provider, &prompt, max_tokens)
        .await
        .context("generate context compaction summary")
}

async fn complete(provider: &SavedProvider, prompt: &str, max_tokens: u64) -> Result<String> {
    let model = provider
        .model
        .as_deref()
        .context("no model selected; run `artist model` first")?;
    let response = crate::rig_provider::health_check_prompt(
        provider,
        model,
        SYSTEM_PROMPT,
        prompt,
        max_tokens,
    )
    .await?;
    if response.trim().is_empty() {
        bail!("context compaction returned an empty summary");
    }
    Ok(response.trim().to_owned())
}
