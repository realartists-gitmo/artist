use crate::{prompt, store::ProviderStore};
use anyhow::{Context, Result, bail};
use dialoguer::{Confirm, Input, Password};
use llm_provider::{Credentials, OpenAiApi, ProviderId, ProviderKind, SavedProvider, Secret};
use url::Url;

pub fn add(store: &mut ProviderStore) -> Result<()> {
    let id: String = Input::new().with_prompt("Provider ID").interact_text()?;
    if store
        .providers
        .iter()
        .any(|provider| provider.id.as_str() == id)
    {
        bail!("provider already exists: {id}");
    }
    let name: String = Input::new()
        .with_prompt("Display name")
        .default("OpenAI".into())
        .interact_text()?;
    let base_url: String = Input::new()
        .with_prompt("Base URL")
        .default("https://api.openai.com/v1/".into())
        .interact_text()?;
    let choices = vec![
        "Responses API".to_owned(),
        "Chat Completions API".to_owned(),
    ];
    let api = match prompt::select("API", &choices, 0)? {
        0 => OpenAiApi::Responses,
        _ => OpenAiApi::ChatCompletions,
    };
    let api_key = Password::new().with_prompt("API key").interact()?;
    if api_key.is_empty() {
        bail!("API key cannot be empty");
    }
    store.add(SavedProvider {
        id: ProviderId::new(id)?,
        name,
        provider: ProviderKind::Openai,
        base_url: Url::parse(&base_url).context("invalid base URL")?,
        api: Some(api),
        model: None,
        reasoning_effort: None,
        credentials: Credentials::ApiKey {
            api_key: Secret::new(api_key),
        },
    });
    Ok(())
}

pub fn edit(store: &mut ProviderStore, id: Option<&str>) -> Result<()> {
    let index = select_index(store, id)?;
    let provider = &mut store.providers[index];
    provider.name = Input::new()
        .with_prompt("Display name")
        .default(provider.name.clone())
        .interact_text()?;
    let base_url: String = Input::new()
        .with_prompt("Base URL")
        .default(provider.base_url.to_string())
        .interact_text()?;
    provider.base_url = Url::parse(&base_url).context("invalid base URL")?;
    if provider.provider == ProviderKind::Openai {
        let choices = vec![
            "Responses API".to_owned(),
            "Chat Completions API".to_owned(),
        ];
        provider.api = Some(
            match prompt::select(
                "API",
                &choices,
                usize::from(provider.api == Some(OpenAiApi::ChatCompletions)),
            )? {
                0 => OpenAiApi::Responses,
                _ => OpenAiApi::ChatCompletions,
            },
        );
        if Confirm::new()
            .with_prompt("Replace API key?")
            .default(false)
            .interact()?
        {
            let api_key = Password::new().with_prompt("New API key").interact()?;
            if api_key.is_empty() {
                bail!("API key cannot be empty");
            }
            provider.credentials = Credentials::ApiKey {
                api_key: Secret::new(api_key),
            };
        }
    }
    Ok(())
}

pub fn remove(store: &mut ProviderStore, id: Option<&str>) -> Result<()> {
    let index = select_index(store, id)?;
    let provider = &store.providers[index];
    if !Confirm::new()
        .with_prompt(format!(
            "Remove {} ({})?",
            provider.name,
            provider.id.as_str()
        ))
        .default(false)
        .interact()?
    {
        return Ok(());
    }
    let removed = store.providers.remove(index);
    if store.default_provider.as_ref() == Some(&removed.id) {
        store.default_provider = store.providers.first().map(|provider| provider.id.clone());
    }
    Ok(())
}

fn select_index(store: &ProviderStore, id: Option<&str>) -> Result<usize> {
    if let Some(id) = id {
        return store
            .providers
            .iter()
            .position(|provider| provider.id.as_str() == id)
            .with_context(|| format!("provider not found: {id}"));
    }
    if store.providers.is_empty() {
        bail!("no providers configured");
    }
    let items = store
        .providers
        .iter()
        .map(|provider| format!("{} ({})", provider.name, provider.id.as_str()))
        .collect::<Vec<_>>();
    prompt::select("Provider", &items, 0)
}
