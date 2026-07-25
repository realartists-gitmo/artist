use crate::{prompt, store::ProviderStore};
use anyhow::{Context, Result, bail};
use dialoguer::{Confirm, Input, Password};
use llm_provider::{
    Credentials, OpenAiApi, PROVIDERS, ProviderId, ProviderKind, SavedProvider, Secret, metadata,
};
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
    // ChatGPT subscription credentials are created by `artist login`, not here.
    let available = PROVIDERS
        .iter()
        .filter(|item| item.kind != ProviderKind::Chatgpt)
        .collect::<Vec<_>>();
    let choices = available
        .iter()
        .map(|item| item.display_name.to_owned())
        .collect::<Vec<_>>();
    let kind = available[prompt::select("Provider", &choices, 0)?].kind;
    let info = metadata(kind);
    let name: String = Input::new()
        .with_prompt("Display name")
        .default(info.display_name.into())
        .interact_text()?;
    let base_url: String = Input::new()
        .with_prompt("Base URL")
        .default(info.default_base_url.unwrap_or_default().into())
        .interact_text()?;
    let api = if matches!(
        kind,
        ProviderKind::Openai
            | ProviderKind::Minimax
            | ProviderKind::Moonshot
            | ProviderKind::Xiaomimimo
            | ProviderKind::Zai
    ) {
        let protocols = if kind == ProviderKind::Openai {
            vec![
                "Responses API".to_owned(),
                "Chat Completions API".to_owned(),
            ]
        } else {
            vec![
                "OpenAI-compatible".to_owned(),
                "Anthropic-compatible".to_owned(),
            ]
        };
        Some(match prompt::select("Protocol", &protocols, 0)? {
            0 => OpenAiApi::Responses,
            _ => OpenAiApi::ChatCompletions,
        })
    } else {
        None
    };
    let credentials = match kind {
        ProviderKind::Llamafile => Credentials::None,
        ProviderKind::Ollama => {
            let key = Password::new()
                .with_prompt("API key (optional)")
                .allow_empty_password(true)
                .interact()?;
            if key.trim().is_empty() {
                Credentials::None
            } else {
                Credentials::ApiKey {
                    api_key: Secret::new(key),
                }
            }
        }
        ProviderKind::Azure => {
            let methods = vec!["API key".to_owned(), "Bearer token".to_owned()];
            let secret = Password::new().with_prompt("Credential").interact()?;
            if secret.trim().is_empty() {
                bail!("credential cannot be empty");
            }
            if prompt::select("Authentication", &methods, 0)? == 0 {
                Credentials::ApiKey {
                    api_key: Secret::new(secret),
                }
            } else {
                Credentials::BearerToken {
                    token: Secret::new(secret),
                }
            }
        }
        _ => {
            let api_key = Password::new().with_prompt("API key").interact()?;
            if api_key.trim().is_empty() {
                bail!("API key cannot be empty");
            }
            Credentials::ApiKey {
                api_key: Secret::new(api_key),
            }
        }
    };
    let api_version = if kind == ProviderKind::Azure {
        let version: String = Input::new()
            .with_prompt("API version")
            .default("2024-10-21".into())
            .interact_text()?;
        Some(version)
    } else {
        None
    };
    let parsed_url = Url::parse(&base_url).context("invalid base URL")?;
    if !matches!(parsed_url.scheme(), "http" | "https") {
        bail!("base URL must use HTTP or HTTPS");
    }
    store.add(SavedProvider {
        id: ProviderId::new(id)?,
        name,
        provider: kind,
        base_url: parsed_url,
        api,
        api_version,
        model: None,
        reasoning_effort: None,
        credentials,
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
    if matches!(
        provider.provider,
        ProviderKind::Openai
            | ProviderKind::Minimax
            | ProviderKind::Moonshot
            | ProviderKind::Xiaomimimo
            | ProviderKind::Zai
    ) {
        let choices = if provider.provider == ProviderKind::Openai {
            vec![
                "Responses API".to_owned(),
                "Chat Completions API".to_owned(),
            ]
        } else {
            vec![
                "OpenAI-compatible".to_owned(),
                "Anthropic-compatible".to_owned(),
            ]
        };
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
