//! Centralized construction and typed dispatch for Rig completion clients.

use anyhow::{Context, Result, bail};
use llm_provider::{Credentials, OpenAiApi, ProviderKind, SavedProvider};
use rig_core::{
    client::CompletionClient,
    completion::Prompt,
    providers::{
        anthropic, chatgpt, cohere, deepseek, gemini, groq, huggingface, hyperbolic, mira, mistral,
        openai, openrouter, perplexity, together, xai,
    },
};

pub(crate) enum RigClient {
    ChatGpt(chatgpt::Client),
    OpenAiResponses(openai::Client),
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
}

impl RigClient {
    pub(crate) fn build(provider: &SavedProvider) -> Result<Self> {
        match provider.provider {
            ProviderKind::Chatgpt => {
                let auth = provider.chatgpt_auth()?;
                Ok(Self::ChatGpt(
                    chatgpt::Client::builder()
                        .api_key(chatgpt::ChatGPTAuth::AccessToken {
                            access_token: auth.access_token.expose().to_owned(),
                            account_id: Some(auth.account_id.clone()),
                        })
                        .base_url(provider.base_url.as_str())
                        .originator("artist")
                        .user_agent(concat!("artist/", env!("CARGO_PKG_VERSION")))
                        .build()
                        .context("build ChatGPT client")?,
                ))
            }
            ProviderKind::Openai => {
                let Credentials::ApiKey { api_key } = &provider.credentials else {
                    bail!("OpenAI API-key credentials required")
                };
                let client = openai::Client::builder()
                    .api_key(api_key.expose())
                    .base_url(provider.base_url.as_str())
                    .build()
                    .context("build OpenAI client")?;
                Ok(match provider.api.unwrap_or_default() {
                    OpenAiApi::Responses => Self::OpenAiResponses(client),
                    OpenAiApi::ChatCompletions => Self::OpenAiChat(client.completions_api()),
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
            other => bail!(
                "{} runtime is not implemented yet",
                llm_provider::metadata(other).display_name
            ),
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
            Self::ChatGpt(client) => run!(client),
            Self::OpenAiResponses(client) => run!(client),
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
        ];

        for kind in kinds {
            let provider = SavedProvider {
                id: ProviderId::new(format!("test-{kind:?}")).unwrap(),
                name: metadata(kind).display_name.into(),
                provider: kind,
                base_url: metadata(kind).default_base_url.unwrap().parse().unwrap(),
                api: None,
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
}
