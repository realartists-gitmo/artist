use crate::prompt;
use anyhow::{Context, Result, bail};
use llm_provider::SavedProvider;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

const CODEX_PROTOCOL_VERSION: &str = "0.144.1";

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    models: Vec<SelectableModel>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct SelectableModel {
    pub slug: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    visibility: String,
    #[serde(default)]
    pub default_reasoning_level: Option<String>,
    #[serde(default)]
    pub supported_reasoning_levels: Vec<ReasoningLevel>,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default = "default_context_percent")]
    pub effective_context_window_percent: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct ReasoningLevel {
    pub effort: String,
    pub description: String,
}

fn default_context_percent() -> u64 {
    100
}

impl SelectableModel {
    /// Context available to the client after the service's reserved percentage.
    #[allow(dead_code)] // Consumed by the pending status-bar UI integration.
    pub(crate) fn effective_context_window(&self) -> Option<u64> {
        self.context_window
            .map(|window| window.saturating_mul(self.effective_context_window_percent) / 100)
    }
}

/// Fetches the account's visible, selectable models in service priority order.
pub(crate) async fn catalog(provider: &SavedProvider) -> Result<Vec<SelectableModel>> {
    let mut models = fetch(provider).await?;
    models.retain(|model| model.visibility == "list");
    models.sort_by_key(|model| model.priority);
    if models.is_empty() {
        bail!("ChatGPT returned no selectable models for this account");
    }
    Ok(models)
}

/// Applies an exact model slug and optional exact reasoning effort from `models`.
pub(crate) fn apply_selection(
    provider: &mut SavedProvider,
    models: &[SelectableModel],
    slug: &str,
    reasoning: Option<&str>,
) -> Result<()> {
    let model = models
        .iter()
        .find(|model| model.slug == slug)
        .with_context(|| format!("model `{slug}` is not selectable"))?;
    let reasoning = reasoning.or(model.default_reasoning_level.as_deref());
    if let Some(effort) = reasoning {
        if !model
            .supported_reasoning_levels
            .iter()
            .any(|level| level.effort == effort)
        {
            bail!("reasoning effort `{effort}` is not supported by model `{slug}`");
        }
    }
    provider.model = Some(model.slug.clone());
    provider.reasoning_effort = reasoning.map(str::to_owned);
    Ok(())
}

/// Preserves the interactive `artist model` selection flow.
pub async fn select(provider: &mut SavedProvider) -> Result<()> {
    let models = catalog(provider).await?;
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
        let preferred = provider
            .reasoning_effort
            .as_ref()
            .or(model.default_reasoning_level.as_ref());
        let default = preferred
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
                .as_str(),
        )
    };
    apply_selection(provider, &models, &model.slug, reasoning)?;
    println!(
        "Model set to {} (reasoning: {}).",
        model.display_name,
        provider.reasoning_effort.as_deref().unwrap_or("default")
    );
    Ok(())
}

async fn fetch(provider: &SavedProvider) -> Result<Vec<SelectableModel>> {
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

    fn model() -> SelectableModel {
        SelectableModel {
            slug: "gpt-5".into(),
            display_name: "GPT-5".into(),
            description: None,
            priority: 1,
            visibility: "list".into(),
            default_reasoning_level: Some("medium".into()),
            context_window: Some(200_000),
            effective_context_window_percent: 95,
            supported_reasoning_levels: vec![
                ReasoningLevel {
                    effort: "medium".into(),
                    description: "Balanced".into(),
                },
                ReasoningLevel {
                    effort: "high".into(),
                    description: "Deep".into(),
                },
            ],
        }
    }

    fn provider() -> SavedProvider {
        serde_json::from_value(serde_json::json!({"id":"x","name":"x","base_url":"https://example.com/","auth":{"access_token":"token","refresh_token":"refresh","account_id":"account"}})).unwrap()
    }

    #[test]
    fn parses_forward_compatible_reasoning_efforts() {
        let response: ModelsResponse = serde_json::from_str(r#"{"models":[{"slug":"future","display_name":"Future","visibility":"list","default_reasoning_level":"ultra","supported_reasoning_levels":[{"effort":"ultra","description":"Deep"}]}]}"#).unwrap();
        assert_eq!(
            response.models[0].supported_reasoning_levels[0].effort,
            "ultra"
        );
    }

    #[test]
    fn parses_and_computes_context_metadata() {
        let response: ModelsResponse = serde_json::from_str(r#"{"models":[{"slug":"gpt","display_name":"GPT","context_window":1000,"effective_context_window_percent":80}]}"#).unwrap();
        assert_eq!(response.models[0].effective_context_window(), Some(800));
        let response: ModelsResponse = serde_json::from_str(
            r#"{"models":[{"slug":"gpt","display_name":"GPT","context_window":1000}]}"#,
        )
        .unwrap();
        assert_eq!(response.models[0].effective_context_window(), Some(1000));
    }

    #[test]
    fn applies_exact_selection_and_model_default() {
        let mut provider = provider();
        apply_selection(&mut provider, &[model()], "gpt-5", None).unwrap();
        assert_eq!(provider.model.as_deref(), Some("gpt-5"));
        assert_eq!(provider.reasoning_effort.as_deref(), Some("medium"));
        apply_selection(&mut provider, &[model()], "gpt-5", Some("high")).unwrap();
        assert_eq!(provider.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn rejects_inexact_or_unsupported_values_without_mutation() {
        let mut provider = provider();
        assert!(apply_selection(&mut provider, &[model()], "GPT-5", None).is_err());
        assert!(apply_selection(&mut provider, &[model()], "gpt-5", Some("HIGH")).is_err());
        assert_eq!(provider.model, None);
    }
}
