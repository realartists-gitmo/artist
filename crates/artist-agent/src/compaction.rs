use anyhow::{Context, Result};
use llm_provider::SavedProvider;
use rig_core::{client::CompletionClient, completion::Prompt, providers::chatgpt};

const COMPACTION_PROMPT: &str = r#"Create a dense continuation summary of the coding session below.

Preserve:
- the user's objectives, constraints, and corrections
- completed work and important implementation decisions
- exact file paths, symbols, commands, test results, and errors
- unresolved work and concrete next steps

Do not invent actions or results. Distinguish completed work from suggestions. Omit conversational filler and secrets. Write structured plain text for another coding agent that will continue the session.

SESSION HISTORY:
"#;

/// Summarize old session history without exposing tools or recording a normal run.
pub async fn summarize_history(provider: &SavedProvider, source: &str) -> Result<String> {
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
        .context("build ChatGPT client for context compaction")?;
    let agent = client.agent(model).preamble(COMPACTION_PROMPT).build();
    let summary = agent
        .prompt(source)
        .await
        .context("generate context compaction summary")?;
    if summary.trim().is_empty() {
        anyhow::bail!("context compaction returned an empty summary");
    }
    Ok(summary)
}
