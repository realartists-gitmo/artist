use anyhow::{Context, Result, bail};
use llm_provider::SavedProvider;

pub async fn test(provider: &SavedProvider) -> Result<()> {
    let model = provider
        .model
        .as_deref()
        .context("no model selected; run `artist model` first")?;
    let response = artist_agent::provider_health_check(provider, model).await?;
    if !response.trim().eq_ignore_ascii_case("ok") {
        bail!("provider responded successfully but did not reply OK");
    }
    Ok(())
}
