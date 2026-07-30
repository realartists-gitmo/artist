//! Centralized construction and typed dispatch for Rig completion clients.

use anyhow::{Context, Result, bail};
use llm_provider::{Credentials, OpenAiApi, ProviderKind, SavedProvider};
use rig_agent::client::AgentClientExt;
use rig_agent::prelude::Prompt;
use rig_core::providers::{
    anthropic, azure, cohere, copilot, deepseek, gemini, groq, huggingface, hyperbolic, llamafile,
    minimax, mira, mistral, moonshot, ollama, openai, openrouter, perplexity, together, xai,
    xiaomimimo, zai,
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

pub(crate) enum RigClient {
    ArtistOpenAi(crate::openai_responses::ArtistOpenAiClient),
    Copilot(copilot::Client),
    OpenAiChat(openai::CompletionsClient),
    Anthropic(anthropic::Client),
    Cohere(cohere::Client),
    Gemini(gemini::Client),
    DeepSeek(deepseek::Client),
    Groq(groq::Client),
    HuggingFace(huggingface::Client),
    Hyperbolic(hyperbolic::Client),
    Mira(mira::Client),
    Mistral(mistral::Client),
    OpenRouter(openrouter::Client),
    Perplexity(perplexity::Client),
    Together(together::Client),
    XAi(xai::Client),
    Azure(azure::Client),
    Llamafile(llamafile::Client),
    Ollama(ollama::Client),
    Minimax(minimax::Client),
    MinimaxAnthropic(minimax::AnthropicClient),
    Moonshot(moonshot::Client),
    MoonshotAnthropic(moonshot::AnthropicClient),
    XiaomiMiMo(xiaomimimo::Client),
    XiaomiMiMoAnthropic(xiaomimimo::AnthropicClient),
    ZAi(zai::Client),
    ZAiAnthropic(zai::AnthropicClient),
}

impl RigClient {
    pub(crate) fn build(provider: &SavedProvider) -> Result<Self> {
        Self::build_with_device_flow(provider, false)
    }

    pub(crate) fn build_with_device_flow(
        provider: &SavedProvider,
        allow_device_flow: bool,
    ) -> Result<Self> {
        match provider.provider {
            ProviderKind::Chatgpt => {
                let auth = provider.chatgpt_auth()?;
                Ok(Self::ArtistOpenAi(
                    crate::openai_responses::ArtistOpenAiClient::chatgpt(
                        provider.base_url.as_str(),
                        auth.access_token.expose(),
                        auth.account_id.clone(),
                    ),
                ))
            }
            ProviderKind::Copilot => {
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
                Ok(Self::Copilot(
                    builder.build().context("build GitHub Copilot client")?,
                ))
            }
            ProviderKind::Azure => {
                let auth = match &provider.credentials {
                    Credentials::ApiKey { api_key } => {
                        azure::AzureOpenAIAuth::ApiKey(api_key.expose().into())
                    }
                    Credentials::BearerToken { token } => {
                        azure::AzureOpenAIAuth::Token(token.expose().into())
                    }
                    _ => bail!("Azure API key or bearer token required"),
                };
                Ok(Self::Azure(
                    azure::Client::builder()
                        .api_key(auth)
                        .azure_endpoint(provider.base_url.to_string())
                        .api_version(provider.api_version.as_deref().unwrap_or("2024-10-21"))
                        .build()
                        .context("build Azure OpenAI client")?,
                ))
            }
            ProviderKind::Llamafile => Ok(Self::Llamafile(
                llamafile::Client::from_url(provider.base_url.as_str())
                    .context("build Llamafile client")?,
            )),
            ProviderKind::Ollama => {
                let key = match &provider.credentials {
                    Credentials::None => "",
                    Credentials::ApiKey { api_key } => api_key.expose(),
                    _ => bail!("Ollama requires no credentials or an API key"),
                };
                Ok(Self::Ollama(
                    ollama::Client::builder()
                        .api_key(key)
                        .base_url(provider.base_url.as_str())
                        .build()
                        .context("build Ollama client")?,
                ))
            }
            ProviderKind::Openai => {
                let Credentials::ApiKey { api_key } = &provider.credentials else {
                    bail!("OpenAI API-key credentials required")
                };
                if provider.api.unwrap_or_default() == OpenAiApi::Responses {
                    return Ok(Self::ArtistOpenAi(
                        crate::openai_responses::ArtistOpenAiClient::api_key(
                            provider.base_url.as_str(),
                            api_key.expose(),
                        ),
                    ));
                }
                let client = openai::Client::builder()
                    .api_key(api_key.expose())
                    .base_url(provider.base_url.as_str())
                    .build()
                    .context("build OpenAI client")?;
                Ok(Self::OpenAiChat(client.completions_api()))
            }
            kind @ (ProviderKind::Minimax
            | ProviderKind::Moonshot
            | ProviderKind::Xiaomimimo
            | ProviderKind::Zai) => {
                let Credentials::ApiKey { api_key } = &provider.credentials else {
                    bail!(
                        "{} API-key credentials required",
                        llm_provider::metadata(kind).display_name
                    )
                };
                let anthropic = provider.api == Some(OpenAiApi::ChatCompletions);
                let anthropic_base_url = (kind == ProviderKind::Xiaomimimo)
                    .then(|| provider.base_url.join("/anthropic/v1/"))
                    .transpose()?;
                macro_rules! dual {
                    ($module:ident, $normal:ident, $anthropic:ident) => {{
                        if anthropic {
                            Self::$anthropic(
                                $module::AnthropicClient::builder()
                                    .api_key(api_key.expose())
                                    .base_url(
                                        anthropic_base_url
                                            .as_ref()
                                            .unwrap_or(&provider.base_url)
                                            .as_str(),
                                    )
                                    .build()
                                    .context("build Anthropic-compatible client")?,
                            )
                        } else {
                            Self::$normal(
                                $module::Client::builder()
                                    .api_key(api_key.expose())
                                    .base_url(provider.base_url.as_str())
                                    .build()
                                    .context("build OpenAI-compatible client")?,
                            )
                        }
                    }};
                }
                Ok(match kind {
                    ProviderKind::Minimax => dual!(minimax, Minimax, MinimaxAnthropic),
                    ProviderKind::Moonshot => dual!(moonshot, Moonshot, MoonshotAnthropic),
                    ProviderKind::Xiaomimimo => dual!(xiaomimimo, XiaomiMiMo, XiaomiMiMoAnthropic),
                    ProviderKind::Zai => dual!(zai, ZAi, ZAiAnthropic),
                    _ => unreachable!(),
                })
            }
            kind @ (ProviderKind::Anthropic
            | ProviderKind::Cohere
            | ProviderKind::Gemini
            | ProviderKind::Deepseek
            | ProviderKind::Groq
            | ProviderKind::Huggingface
            | ProviderKind::Hyperbolic
            | ProviderKind::Mira
            | ProviderKind::Mistral
            | ProviderKind::Openrouter
            | ProviderKind::Perplexity
            | ProviderKind::Together
            | ProviderKind::Xai) => {
                let Credentials::ApiKey { api_key } = &provider.credentials else {
                    bail!(
                        "{} API-key credentials required",
                        llm_provider::metadata(kind).display_name
                    )
                };
                macro_rules! build {
                    ($module:ident, $variant:ident) => {
                        Self::$variant(
                            $module::Client::builder()
                                .api_key(api_key.expose())
                                .base_url(provider.base_url.as_str())
                                .build()
                                .with_context(|| {
                                    format!(
                                        "build {} client",
                                        llm_provider::metadata(kind).display_name
                                    )
                                })?,
                        )
                    };
                }
                Ok(match kind {
                    ProviderKind::Anthropic => build!(anthropic, Anthropic),
                    ProviderKind::Cohere => build!(cohere, Cohere),
                    ProviderKind::Gemini => build!(gemini, Gemini),
                    ProviderKind::Deepseek => build!(deepseek, DeepSeek),
                    ProviderKind::Groq => build!(groq, Groq),
                    ProviderKind::Huggingface => build!(huggingface, HuggingFace),
                    ProviderKind::Hyperbolic => build!(hyperbolic, Hyperbolic),
                    ProviderKind::Mira => build!(mira, Mira),
                    ProviderKind::Mistral => build!(mistral, Mistral),
                    ProviderKind::Openrouter => build!(openrouter, OpenRouter),
                    ProviderKind::Perplexity => build!(perplexity, Perplexity),
                    ProviderKind::Together => build!(together, Together),
                    ProviderKind::Xai => build!(xai, XAi),
                    _ => unreachable!(),
                })
            }
        }
    }

    pub(crate) async fn prompt(
        self,
        model: &str,
        preamble: &str,
        prompt: &str,
        max_tokens: u64,
    ) -> Result<String> {
        macro_rules! run {
            ($client:expr) => {{
                $client
                    .agent(model)
                    .preamble(preamble)
                    .max_tokens(max_tokens)
                    .build()
                    .prompt(prompt)
                    .await?
            }};
        }
        Ok(match self {
            Self::ArtistOpenAi(client) => run!(client),
            Self::Copilot(client) => run!(client),
            Self::OpenAiChat(client) => run!(client),
            Self::Anthropic(client) => run!(client),
            Self::Cohere(client) => run!(client),
            Self::Gemini(client) => run!(client),
            Self::DeepSeek(client) => run!(client),
            Self::Groq(client) => run!(client),
            Self::HuggingFace(client) => run!(client),
            Self::Hyperbolic(client) => run!(client),
            Self::Mira(client) => run!(client),
            Self::Mistral(client) => run!(client),
            Self::OpenRouter(client) => run!(client),
            Self::Perplexity(client) => run!(client),
            Self::Together(client) => run!(client),
            Self::XAi(client) => run!(client),
            Self::Azure(client) => run!(client),
            Self::Llamafile(client) => run!(client),
            Self::Ollama(client) => run!(client),
            Self::Minimax(client) => run!(client),
            Self::MinimaxAnthropic(client) => run!(client),
            Self::Moonshot(client) => run!(client),
            Self::MoonshotAnthropic(client) => run!(client),
            Self::XiaomiMiMo(client) => run!(client),
            Self::XiaomiMiMoAnthropic(client) => run!(client),
            Self::ZAi(client) => run!(client),
            Self::ZAiAnthropic(client) => run!(client),
        })
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
            assert!(
                RigClient::build(&provider).is_ok(),
                "failed to build {kind:?}"
            );
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
        for credentials in credentials {
            assert!(matches!(
                RigClient::build(&test_provider(ProviderKind::Copilot, credentials)),
                Ok(RigClient::Copilot(_))
            ));
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
        assert!(matches!(RigClient::build(&azure), Ok(RigClient::Azure(_))));
        assert!(matches!(
            RigClient::build(&test_provider(ProviderKind::Llamafile, Credentials::None)),
            Ok(RigClient::Llamafile(_))
        ));
        assert!(matches!(
            RigClient::build(&test_provider(ProviderKind::Ollama, Credentials::None)),
            Ok(RigClient::Ollama(_))
        ));
        for kind in [
            ProviderKind::Minimax,
            ProviderKind::Moonshot,
            ProviderKind::Xiaomimimo,
            ProviderKind::Zai,
        ] {
            let mut provider = test_provider(kind, key());
            provider.api = Some(OpenAiApi::ChatCompletions);
            assert!(
                RigClient::build(&provider).is_ok(),
                "failed Anthropic variant {kind:?}"
            );
        }
    }
}
