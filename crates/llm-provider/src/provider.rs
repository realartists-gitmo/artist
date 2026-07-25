use crate::{CHATGPT_CODEX_BASE_URL, Error, ProviderKind, Result, Secret};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf};
use url::Url;

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderId(String);

impl ProviderId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(Error::InvalidConfig("provider id cannot be empty".into()));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Credentials obtained from a ChatGPT subscription login.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Auth {
    pub access_token: Secret,
    pub refresh_token: Secret,
    pub account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}
impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Auth")
            .field("account_id", &self.account_id)
            .field("email", &self.email)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// Provider credentials are explicitly tagged on disk. The legacy variant in
/// the deserializer keeps pre-v4 ChatGPT records (whose `auth` table was
/// untagged) readable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Credentials {
    None,
    ApiKey {
        api_key: Secret,
    },
    BearerToken {
        token: Secret,
    },
    /// Device OAuth using Rig's cached GitHub/Copilot tokens in this directory.
    CopilotOauth {
        token_dir: PathBuf,
    },
    Chatgpt(Auth),
}

impl<'de> Deserialize<'de> for Credentials {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Tagged {
            None,
            ApiKey { api_key: Secret },
            BearerToken { token: Secret },
            CopilotOauth { token_dir: PathBuf },
            Chatgpt(Auth),
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Compatible {
            Tagged(Tagged),
            Legacy(Auth),
        }
        Ok(match Compatible::deserialize(deserializer)? {
            Compatible::Tagged(Tagged::None) => Self::None,
            Compatible::Tagged(Tagged::ApiKey { api_key }) => Self::ApiKey { api_key },
            Compatible::Tagged(Tagged::BearerToken { token }) => Self::BearerToken { token },
            Compatible::Tagged(Tagged::CopilotOauth { token_dir }) => {
                Self::CopilotOauth { token_dir }
            }
            Compatible::Tagged(Tagged::Chatgpt(auth)) | Compatible::Legacy(auth) => {
                Self::Chatgpt(auth)
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiApi {
    #[default]
    Responses,
    ChatCompletions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedProvider {
    pub id: ProviderId,
    pub name: String,
    #[serde(default = "chatgpt_kind")]
    pub provider: ProviderKind,
    pub base_url: Url,
    /// OpenAI-compatible transport. Ignored by providers with a fixed API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<OpenAiApi>,
    /// Azure OpenAI API version. Ignored by other providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
    /// Provider-specific model selection. Persisting this beside the provider
    /// prevents switching providers from carrying an incompatible model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(rename = "credentials", alias = "auth")]
    pub credentials: Credentials,
}

fn chatgpt_kind() -> ProviderKind {
    ProviderKind::Chatgpt
}

impl SavedProvider {
    pub fn chatgpt(id: ProviderId, name: impl Into<String>, auth: Auth) -> Self {
        Self {
            id,
            name: name.into(),
            provider: ProviderKind::Chatgpt,
            base_url: Url::parse(CHATGPT_CODEX_BASE_URL).expect("constant URL"),
            api: None,
            api_version: None,
            model: None,
            reasoning_effort: None,
            credentials: Credentials::Chatgpt(auth),
        }
    }

    pub fn chatgpt_auth(&self) -> Result<&Auth> {
        match &self.credentials {
            Credentials::Chatgpt(auth) => Ok(auth),
            _ => Err(Error::InvalidConfig("ChatGPT credentials required".into())),
        }
    }

    pub fn chatgpt_auth_mut(&mut self) -> Result<&mut Auth> {
        match &mut self.credentials {
            Credentials::Chatgpt(auth) => Ok(auth),
            _ => Err(Error::InvalidConfig("ChatGPT credentials required".into())),
        }
    }

    pub fn request_auth(&self) -> Result<RequestAuth> {
        let (token, account_id) = match &self.credentials {
            Credentials::Chatgpt(auth) => {
                (auth.access_token.expose(), Some(auth.account_id.as_str()))
            }
            Credentials::ApiKey { api_key } => (api_key.expose(), None),
            Credentials::BearerToken { token } => (token.expose(), None),
            Credentials::CopilotOauth { .. } | Credentials::None => {
                return Err(Error::InvalidConfig("direct credentials required".into()));
            }
        };
        let mut headers = HeaderMap::new();
        let bearer = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
            Error::InvalidConfig("credential contains invalid header characters".into())
        })?;
        headers.insert(AUTHORIZATION, bearer);
        if let Some(account_id) = account_id {
            headers.insert(
                "chatgpt-account-id",
                HeaderValue::from_str(account_id).map_err(|_| {
                    Error::InvalidConfig("account id contains invalid header characters".into())
                })?,
            );
        }
        Ok(RequestAuth { headers })
    }
}

#[derive(Clone, Debug)]
pub struct RequestAuth {
    pub headers: HeaderMap,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_model_round_trips() {
        let mut provider = SavedProvider::chatgpt(
            ProviderId::new("chatgpt").unwrap(),
            "ChatGPT",
            Auth {
                access_token: Secret::new("access"),
                refresh_token: Secret::new("refresh"),
                account_id: "acct".into(),
                email: None,
                expires_at: None,
            },
        );
        provider.model = Some("gpt-5".into());
        provider.reasoning_effort = Some("high".into());
        let decoded: SavedProvider =
            serde_json::from_str(&serde_json::to_string(&provider).unwrap()).unwrap();
        assert_eq!(decoded.model.as_deref(), Some("gpt-5"));
        assert_eq!(decoded.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn secrets_are_redacted_and_headers_are_set() {
        let provider = SavedProvider::chatgpt(
            ProviderId::new("chatgpt").unwrap(),
            "ChatGPT",
            Auth {
                access_token: Secret::new("access"),
                refresh_token: Secret::new("refresh"),
                account_id: "acct".into(),
                email: Some("me@example.com".into()),
                expires_at: None,
            },
        );
        let json = serde_json::to_string(&provider).unwrap();
        assert!(json.contains("access"));
        assert!(!format!("{provider:?}").contains("access"));
        let headers = provider.request_auth().unwrap().headers;
        assert_eq!(headers[AUTHORIZATION], "Bearer access");
        assert_eq!(headers["chatgpt-account-id"], "acct");
    }
}
