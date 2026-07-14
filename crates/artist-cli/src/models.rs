use crate::prompt;
use anyhow::{Context, Result, bail};
use llm_provider::SavedProvider;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

// The service filters its catalog by Codex protocol version, not Artist's package version.
// Keep this aligned with the Codex CLI release whose API contract we implement.
const CODEX_PROTOCOL_VERSION: &str = "0.144.1";

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    models: Vec<Model>,
}

#[derive(Debug, Deserialize)]
struct Model {
    slug: String,
    display_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    visibility: String,
    #[serde(default)]
    default_reasoning_level: Option<String>,
    #[serde(default)]
    supported_reasoning_levels: Vec<ReasoningLevel>,
}

#[derive(Debug, Deserialize)]
struct ReasoningLevel {
    effort: String,
    description: String,
}

pub async fn select(provider: &mut SavedProvider) -> Result<()> {
    let mut models = fetch(provider).await?;
    models.retain(|model| model.visibility == "list");
    models.sort_by_key(|model| model.priority);
    if models.is_empty() {
        bail!("ChatGPT returned no selectable models for this account");
    }

    let labels: Vec<_> = models
        .iter()
        .map(|model| match &model.description {
            Some(description) if !description.is_empty() => {
                format!("{} — {}", model.display_name, description)
            }
            _ => model.display_name.clone(),
        })
        .collect();
    let current = provider
        .model
        .as_ref()
        .and_then(|slug| models.iter().position(|model| &model.slug == slug))
        .unwrap_or(0);
    let model = &models[prompt::select("Model", &labels, current)?];

    let reasoning = if model.supported_reasoning_levels.is_empty() {
        None
    } else {
        let labels: Vec<_> = model
            .supported_reasoning_levels
            .iter()
            .map(|level| format!("{} — {}", level.effort, level.description))
            .collect();
        let default_effort = provider
            .reasoning_effort
            .as_ref()
            .or(model.default_reasoning_level.as_ref());
        let default = default_effort
            .and_then(|effort| {
                model
                    .supported_reasoning_levels
                    .iter()
                    .position(|level| &level.effort == effort)
            })
            .unwrap_or(0);
        Some(
            model.supported_reasoning_levels[prompt::select("Reasoning effort", &labels, default)?]
                .effort
                .clone(),
        )
    };
    provider.model = Some(model.slug.clone());
    provider.reasoning_effort = reasoning;
    println!(
        "Model set to {} (reasoning: {}).",
        model.display_name,
        provider.reasoning_effort.as_deref().unwrap_or("default")
    );
    Ok(())
}

async fn fetch(provider: &SavedProvider) -> Result<Vec<Model>> {
    let mut endpoint = provider.base_url.join("models")?;
    endpoint
        .query_pairs_mut()
        .append_pair("client_version", CODEX_PROTOCOL_VERSION);
    let response = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?
        .get(endpoint)
        .headers(provider.request_auth()?.headers)
        .header("originator", "artist")
        .header(
            reqwest::header::USER_AGENT,
            concat!("artist/", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await?
        .error_for_status()
        .context("ChatGPT model discovery failed")?;
    Ok(response
        .json::<ModelsResponse>()
        .await
        .context("invalid ChatGPT models response")?
        .models)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_forward_compatible_reasoning_efforts() {
        let response: ModelsResponse = serde_json::from_str(r#"{"models":[{"slug":"future","display_name":"Future","visibility":"list","default_reasoning_level":"ultra","supported_reasoning_levels":[{"effort":"ultra","description":"Deep"}]}]}"#).unwrap();
        assert_eq!(
            response.models[0].supported_reasoning_levels[0].effort,
            "ultra"
        );
    }
}
