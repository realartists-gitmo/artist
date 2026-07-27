//! Centralized construction of Rig completion clients.

use anyhow::{Context, Result, bail};
use llm_provider::{Credentials, OpenAiApi, ProviderKind, SavedProvider};
use rig_core::{
    client::CompletionClient,
    completion::Prompt,
    providers::{
        anthropic, azure, chatgpt, cohere, copilot, deepseek, gemini, groq, huggingface,
        hyperbolic, llamafile, minimax, mira, mistral, moonshot, ollama, openai, openrouter,
        perplexity, together, xai, xiaomimimo, zai,
    },
};

fn secure_token_dir(path: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(path).context("create Copilot token directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .context("secure Copilot token directory")?;
        for name in ["access-token", "api-key.json"] {
            let file = path.join(name);
            if file.exists() {
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))
                    .with_context(|| format!("secure {}", file.display()))?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Individual client builders
// ---------------------------------------------------------------------------

pub(crate) fn build_chatgpt(provider: &SavedProvider) -> Result<chatgpt::Client> {
    let auth = provider.chatgpt_auth()?;
    chatgpt::Client::builder()
        .api_key(chatgpt::ChatGPTAuth::AccessToken {
            access_token: auth.access_token.expose().to_owned(),
            account_id: Some(auth.account_id.clone()),
        })
        .base_url(provider.base_url.as_str())
        .originator("artist")
        .user_agent(concat!("artist/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build ChatGPT client")
}

fn build_copilot_inner(
    provider: &SavedProvider,
    allow_device_flow: bool,
) -> Result<copilot::Client> {
    let builder = copilot::Client::builder();
    let builder = match &provider.credentials {
        Credentials::ApiKey { api_key } => builder.api_key(api_key.expose()),
        Credentials::BearerToken { token } => builder.api_key(
            copilot::CopilotAuth::GitHubAccessToken(token.expose().to_owned()),
        ),
        Credentials::CopilotOauth { token_dir } => {
            secure_token_dir(token_dir)?;
            builder
                .api_key(copilot::CopilotAuth::OAuth)
                .token_dir(token_dir)
        }
        _ => bail!("Copilot API key, GitHub token, or OAuth token directory required"),
    }
    .base_url(provider.base_url.as_str())
    .allow_device_flow(allow_device_flow);
    builder.build().context("build GitHub Copilot client")
}

pub(crate) fn build_copilot(provider: &SavedProvider) -> Result<copilot::Client> {
    build_copilot_inner(provider, false)
}

pub(crate) fn build_copilot_with_device_flow(provider: &SavedProvider) -> Result<copilot::Client> {
    build_copilot_inner(provider, true)
}

pub(crate) fn build_azure(provider: &SavedProvider) -> Result<azure::Client> {
    let auth = match &provider.credentials {
        Credentials::ApiKey { api_key } => azure::AzureOpenAIAuth::ApiKey(api_key.expose().into()),
        Credentials::BearerToken { token } => azure::AzureOpenAIAuth::Token(token.expose().into()),
        _ => bail!("Azure API key or bearer token required"),
    };
    azure::Client::builder()
        .api_key(auth)
        .azure_endpoint(provider.base_url.to_string())
        .api_version(provider.api_version.as_deref().unwrap_or("2024-10-21"))
        .build()
        .context("build Azure OpenAI client")
}

pub(crate) fn build_llamafile(provider: &SavedProvider) -> Result<llamafile::Client> {
    llamafile::Client::from_url(provider.base_url.as_str()).context("build Llamafile client")
}

pub(crate) fn build_ollama(provider: &SavedProvider) -> Result<ollama::Client> {
    let key = match &provider.credentials {
        Credentials::None => "",
        Credentials::ApiKey { api_key } => api_key.expose(),
        _ => bail!("Ollama requires no credentials or an API key"),
    };
    ollama::Client::builder()
        .api_key(key)
        .base_url(provider.base_url.as_str())
        .build()
        .context("build Ollama client")
}

pub(crate) fn build_openai_responses(provider: &SavedProvider) -> Result<openai::Client> {
    let Credentials::ApiKey { api_key } = &provider.credentials else {
        bail!("OpenAI API-key credentials required")
    };
    openai::Client::builder()
        .api_key(api_key.expose())
        .base_url(provider.base_url.as_str())
        .build()
        .context("build OpenAI Responses client")
}

pub(crate) fn build_openai_chat(provider: &SavedProvider) -> Result<openai::CompletionsClient> {
    build_openai_responses(provider).map(|client| client.completions_api())
}

// ---------------------------------------------------------------------------
// Simple API-key providers
// ---------------------------------------------------------------------------

macro_rules! build_api_key_client {
    ($name:ident, $module:ident, $display:literal) => {
        pub(crate) fn $name(provider: &SavedProvider) -> Result<$module::Client> {
            let Credentials::ApiKey { api_key } = &provider.credentials else {
                bail!("{} API-key credentials required", $display)
            };
            $module::Client::builder()
                .api_key(api_key.expose())
                .base_url(provider.base_url.as_str())
                .build()
                .with_context(|| format!("build {} client", $display))
        }
    };
}

build_api_key_client!(build_anthropic, anthropic, "Anthropic");
build_api_key_client!(build_cohere, cohere, "Cohere");
build_api_key_client!(build_gemini, gemini, "Google Gemini");
build_api_key_client!(build_deepseek, deepseek, "DeepSeek");
build_api_key_client!(build_groq, groq, "Groq");
build_api_key_client!(build_huggingface, huggingface, "Hugging Face");
build_api_key_client!(build_hyperbolic, hyperbolic, "Hyperbolic");
build_api_key_client!(build_mira, mira, "Mira");
build_api_key_client!(build_mistral, mistral, "Mistral");
build_api_key_client!(build_openrouter, openrouter, "OpenRouter");
build_api_key_client!(build_perplexity, perplexity, "Perplexity");
build_api_key_client!(build_together, together, "Together AI");
build_api_key_client!(build_xai, xai, "xAI");

// ---------------------------------------------------------------------------
// Dual-mode providers (OpenAI-compatible + Anthropic-compatible)
// ---------------------------------------------------------------------------

pub(crate) fn build_minimax(provider: &SavedProvider) -> Result<minimax::Client> {
    let Credentials::ApiKey { api_key } = &provider.credentials else {
        bail!("MiniMax API-key credentials required")
    };
    minimax::Client::builder()
        .api_key(api_key.expose())
        .base_url(provider.base_url.as_str())
        .build()
        .context("build MiniMax client")
}

pub(crate) fn build_minimax_anthropic(
    provider: &SavedProvider,
) -> Result<minimax::AnthropicClient> {
    let Credentials::ApiKey { api_key } = &provider.credentials else {
        bail!("MiniMax API-key credentials required")
    };
    minimax::AnthropicClient::builder()
        .api_key(api_key.expose())
        .base_url(provider.base_url.as_str())
        .build()
        .context("build MiniMax Anthropic-compatible client")
}

pub(crate) fn build_moonshot(provider: &SavedProvider) -> Result<moonshot::Client> {
    let Credentials::ApiKey { api_key } = &provider.credentials else {
        bail!("Moonshot API-key credentials required")
    };
    moonshot::Client::builder()
        .api_key(api_key.expose())
        .base_url(provider.base_url.as_str())
        .build()
        .context("build Moonshot client")
}

pub(crate) fn build_moonshot_anthropic(
    provider: &SavedProvider,
) -> Result<moonshot::AnthropicClient> {
    let Credentials::ApiKey { api_key } = &provider.credentials else {
        bail!("Moonshot API-key credentials required")
    };
    moonshot::AnthropicClient::builder()
        .api_key(api_key.expose())
        .base_url(provider.base_url.as_str())
        .build()
        .context("build Moonshot Anthropic-compatible client")
}

pub(crate) fn build_xiaomimimo(provider: &SavedProvider) -> Result<xiaomimimo::Client> {
    let Credentials::ApiKey { api_key } = &provider.credentials else {
        bail!("Xiaomi MiMo API-key credentials required")
    };
    xiaomimimo::Client::builder()
        .api_key(api_key.expose())
        .base_url(provider.base_url.as_str())
        .build()
        .context("build Xiaomi MiMo client")
}

pub(crate) fn build_xiaomimimo_anthropic(
    provider: &SavedProvider,
) -> Result<xiaomimimo::AnthropicClient> {
    let Credentials::ApiKey { api_key } = &provider.credentials else {
        bail!("Xiaomi MiMo API-key credentials required")
    };
    let base_url = provider
        .base_url
        .join("/anthropic/v1/")
        .context("build Xiaomi MiMo Anthropic base URL")?;
    xiaomimimo::AnthropicClient::builder()
        .api_key(api_key.expose())
        .base_url(base_url.as_str())
        .build()
        .context("build Xiaomi MiMo Anthropic-compatible client")
}

pub(crate) fn build_zai(provider: &SavedProvider) -> Result<zai::Client> {
    let Credentials::ApiKey { api_key } = &provider.credentials else {
        bail!("Z.ai API-key credentials required")
    };
    zai::Client::builder()
        .api_key(api_key.expose())
        .base_url(provider.base_url.as_str())
        .build()
        .context("build Z.ai client")
}

pub(crate) fn build_zai_anthropic(provider: &SavedProvider) -> Result<zai::AnthropicClient> {
    let Credentials::ApiKey { api_key } = &provider.credentials else {
        bail!("Z.ai API-key credentials required")
    };
    zai::AnthropicClient::builder()
        .api_key(api_key.expose())
        .base_url(provider.base_url.as_str())
        .build()
        .context("build Z.ai Anthropic-compatible client")
}

// ---------------------------------------------------------------------------
// Generic prompt helper
// ---------------------------------------------------------------------------

pub(crate) async fn prompt_with<C: CompletionClient>(
    client: C,
    model: &str,
    preamble: &str,
    prompt: &str,
    max_tokens: u64,
) -> Result<String>
where
    C::CompletionModel: 'static,
{
    Ok(client
        .agent(model)
        .preamble(preamble)
        .max_tokens(max_tokens)
        .build()
        .prompt(prompt)
        .await?)
}

// ---------------------------------------------------------------------------
// Dispatch: health-check prompt (no device flow)
// ---------------------------------------------------------------------------

pub(crate) async fn health_check_prompt(
    provider: &SavedProvider,
    model: &str,
    preamble: &str,
    prompt: &str,
    max_tokens: u64,
) -> Result<String> {
    match provider.provider {
        ProviderKind::Chatgpt => {
            prompt_with(
                build_chatgpt(provider)?,
                model,
                preamble,
                prompt,
                max_tokens,
            )
            .await
        }
        ProviderKind::Copilot => {
            prompt_with(
                build_copilot(provider)?,
                model,
                preamble,
                prompt,
                max_tokens,
            )
            .await
        }
        ProviderKind::Azure => {
            prompt_with(build_azure(provider)?, model, preamble, prompt, max_tokens).await
        }
        ProviderKind::Llamafile => {
            prompt_with(
                build_llamafile(provider)?,
                model,
                preamble,
                prompt,
                max_tokens,
            )
            .await
        }
        ProviderKind::Ollama => {
            prompt_with(build_ollama(provider)?, model, preamble, prompt, max_tokens).await
        }
        ProviderKind::Openai => match provider.api.unwrap_or_default() {
            OpenAiApi::Responses => {
                prompt_with(
                    build_openai_responses(provider)?,
                    model,
                    preamble,
                    prompt,
                    max_tokens,
                )
                .await
            }
            OpenAiApi::ChatCompletions => {
                prompt_with(
                    build_openai_chat(provider)?,
                    model,
                    preamble,
                    prompt,
                    max_tokens,
                )
                .await
            }
        },
        ProviderKind::Anthropic => {
            prompt_with(
                build_anthropic(provider)?,
                model,
                preamble,
                prompt,
                max_tokens,
            )
            .await
        }
        ProviderKind::Cohere => {
            prompt_with(build_cohere(provider)?, model, preamble, prompt, max_tokens).await
        }
        ProviderKind::Gemini => {
            prompt_with(build_gemini(provider)?, model, preamble, prompt, max_tokens).await
        }
        ProviderKind::Deepseek => {
            prompt_with(
                build_deepseek(provider)?,
                model,
                preamble,
                prompt,
                max_tokens,
            )
            .await
        }
        ProviderKind::Groq => {
            prompt_with(build_groq(provider)?, model, preamble, prompt, max_tokens).await
        }
        ProviderKind::Huggingface => {
            prompt_with(
                build_huggingface(provider)?,
                model,
                preamble,
                prompt,
                max_tokens,
            )
            .await
        }
        ProviderKind::Hyperbolic => {
            prompt_with(
                build_hyperbolic(provider)?,
                model,
                preamble,
                prompt,
                max_tokens,
            )
            .await
        }
        ProviderKind::Mira => {
            prompt_with(build_mira(provider)?, model, preamble, prompt, max_tokens).await
        }
        ProviderKind::Mistral => {
            prompt_with(
                build_mistral(provider)?,
                model,
                preamble,
                prompt,
                max_tokens,
            )
            .await
        }
        ProviderKind::Openrouter => {
            prompt_with(
                build_openrouter(provider)?,
                model,
                preamble,
                prompt,
                max_tokens,
            )
            .await
        }
        ProviderKind::Perplexity => {
            prompt_with(
                build_perplexity(provider)?,
                model,
                preamble,
                prompt,
                max_tokens,
            )
            .await
        }
        ProviderKind::Together => {
            prompt_with(
                build_together(provider)?,
                model,
                preamble,
                prompt,
                max_tokens,
            )
            .await
        }
        ProviderKind::Xai => {
            prompt_with(build_xai(provider)?, model, preamble, prompt, max_tokens).await
        }
        ProviderKind::Minimax => match provider.api.unwrap_or_default() {
            OpenAiApi::Responses => {
                prompt_with(
                    build_minimax(provider)?,
                    model,
                    preamble,
                    prompt,
                    max_tokens,
                )
                .await
            }
            OpenAiApi::ChatCompletions => {
                prompt_with(
                    build_minimax_anthropic(provider)?,
                    model,
                    preamble,
                    prompt,
                    max_tokens,
                )
                .await
            }
        },
        ProviderKind::Moonshot => match provider.api.unwrap_or_default() {
            OpenAiApi::Responses => {
                prompt_with(
                    build_moonshot(provider)?,
                    model,
                    preamble,
                    prompt,
                    max_tokens,
                )
                .await
            }
            OpenAiApi::ChatCompletions => {
                prompt_with(
                    build_moonshot_anthropic(provider)?,
                    model,
                    preamble,
                    prompt,
                    max_tokens,
                )
                .await
            }
        },
        ProviderKind::Xiaomimimo => match provider.api.unwrap_or_default() {
            OpenAiApi::Responses => {
                prompt_with(
                    build_xiaomimimo(provider)?,
                    model,
                    preamble,
                    prompt,
                    max_tokens,
                )
                .await
            }
            OpenAiApi::ChatCompletions => {
                prompt_with(
                    build_xiaomimimo_anthropic(provider)?,
                    model,
                    preamble,
                    prompt,
                    max_tokens,
                )
                .await
            }
        },
        ProviderKind::Zai => match provider.api.unwrap_or_default() {
            OpenAiApi::Responses => {
                prompt_with(build_zai(provider)?, model, preamble, prompt, max_tokens).await
            }
            OpenAiApi::ChatCompletions => {
                prompt_with(
                    build_zai_anthropic(provider)?,
                    model,
                    preamble,
                    prompt,
                    max_tokens,
                )
                .await
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_provider::{ProviderId, Secret, metadata};

    #[test]
    fn builds_all_supported_api_key_clients() {
        let kinds = [
            ProviderKind::Anthropic,
            ProviderKind::Cohere,
            ProviderKind::Gemini,
            ProviderKind::Deepseek,
            ProviderKind::Groq,
            ProviderKind::Huggingface,
            ProviderKind::Hyperbolic,
            ProviderKind::Mira,
            ProviderKind::Mistral,
            ProviderKind::Openrouter,
            ProviderKind::Perplexity,
            ProviderKind::Together,
            ProviderKind::Xai,
            ProviderKind::Minimax,
            ProviderKind::Moonshot,
            ProviderKind::Xiaomimimo,
            ProviderKind::Zai,
        ];

        for kind in kinds {
            let provider = SavedProvider {
                id: ProviderId::new(format!("test-{kind:?}")).unwrap(),
                name: metadata(kind).display_name.into(),
                provider: kind,
                base_url: metadata(kind).default_base_url.unwrap().parse().unwrap(),
                api: None,
                api_version: None,
                model: Some("test-model".into()),
                reasoning_effort: None,
                credentials: Credentials::ApiKey {
                    api_key: Secret::new("test-key"),
                },
            };
            let result: Result<()> = match kind {
                ProviderKind::Anthropic => build_anthropic(&provider).map(|_| ()),
                ProviderKind::Cohere => build_cohere(&provider).map(|_| ()),
                ProviderKind::Gemini => build_gemini(&provider).map(|_| ()),
                ProviderKind::Deepseek => build_deepseek(&provider).map(|_| ()),
                ProviderKind::Groq => build_groq(&provider).map(|_| ()),
                ProviderKind::Huggingface => build_huggingface(&provider).map(|_| ()),
                ProviderKind::Hyperbolic => build_hyperbolic(&provider).map(|_| ()),
                ProviderKind::Mira => build_mira(&provider).map(|_| ()),
                ProviderKind::Mistral => build_mistral(&provider).map(|_| ()),
                ProviderKind::Openrouter => build_openrouter(&provider).map(|_| ()),
                ProviderKind::Perplexity => build_perplexity(&provider).map(|_| ()),
                ProviderKind::Together => build_together(&provider).map(|_| ()),
                ProviderKind::Xai => build_xai(&provider).map(|_| ()),
                ProviderKind::Minimax => build_minimax(&provider).map(|_| ()),
                ProviderKind::Moonshot => build_moonshot(&provider).map(|_| ()),
                ProviderKind::Xiaomimimo => build_xiaomimimo(&provider).map(|_| ()),
                ProviderKind::Zai => build_zai(&provider).map(|_| ()),
                _ => unreachable!(),
            };
            assert!(result.is_ok(), "failed to build {kind:?}");
        }
    }

    fn test_provider(kind: ProviderKind, credentials: Credentials) -> SavedProvider {
        SavedProvider {
            id: ProviderId::new(format!("test-{kind:?}")).unwrap(),
            name: metadata(kind).display_name.into(),
            provider: kind,
            base_url: metadata(kind).default_base_url.unwrap().parse().unwrap(),
            api: None,
            api_version: None,
            model: Some("test-model".into()),
            reasoning_effort: None,
            credentials,
        }
    }

    #[test]
    fn builds_all_copilot_auth_modes_without_starting_device_flow() {
        let temp = tempfile::tempdir().unwrap();
        let credentials = [
            Credentials::ApiKey {
                api_key: Secret::new("copilot-key"),
            },
            Credentials::BearerToken {
                token: Secret::new("github-token"),
            },
            Credentials::CopilotOauth {
                token_dir: temp.path().join("tokens"),
            },
        ];
        for cred in &credentials {
            assert!(
                build_copilot(&test_provider(ProviderKind::Copilot, cred.clone())).is_ok(),
                "failed with {cred:?}"
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(temp.path().join("tokens"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn builds_azure_local_and_anthropic_compatible_clients() {
        let key = || Credentials::ApiKey {
            api_key: Secret::new("test-key"),
        };
        let mut azure = test_provider(
            ProviderKind::Azure,
            Credentials::BearerToken {
                token: Secret::new("token"),
            },
        );
        azure.api_version = Some("2024-10-21".into());
        assert!(build_azure(&azure).is_ok());
        assert!(
            build_llamafile(&test_provider(ProviderKind::Llamafile, Credentials::None)).is_ok()
        );
        assert!(build_ollama(&test_provider(ProviderKind::Ollama, Credentials::None)).is_ok());
        for kind in [
            ProviderKind::Minimax,
            ProviderKind::Moonshot,
            ProviderKind::Xiaomimimo,
            ProviderKind::Zai,
        ] {
            let mut provider = test_provider(kind, key());
            provider.api = Some(OpenAiApi::ChatCompletions);
            let result = match kind {
                ProviderKind::Minimax => build_minimax_anthropic(&provider).map(|_| ()),
                ProviderKind::Moonshot => build_moonshot_anthropic(&provider).map(|_| ()),
                ProviderKind::Xiaomimimo => build_xiaomimimo_anthropic(&provider).map(|_| ()),
                ProviderKind::Zai => build_zai_anthropic(&provider).map(|_| ()),
                _ => unreachable!(),
            };
            assert!(result.is_ok(), "failed Anthropic variant {kind:?}");
        }
    }
}
