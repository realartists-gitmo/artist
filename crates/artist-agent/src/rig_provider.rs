//! Centralized construction and typed dispatch for Rig completion clients.

use anyhow::{Context, Result, bail};
use llm_provider::{Credentials, OpenAiApi, ProviderKind, SavedProvider};
use rig_core::{
    client::CompletionClient,
    completion::Prompt,
    providers::{chatgpt, openai},
};

pub(crate) enum RigClient {
    ChatGpt(chatgpt::Client),
    OpenAiResponses(openai::Client),
    OpenAiChat(openai::CompletionsClient),
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
        })
    }
}
