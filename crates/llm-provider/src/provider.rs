use crate::{CHATGPT_CODEX_BASE_URL, Error, Result, Secret};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::fmt;
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

/// Which rig backend a provider drives — and therefore how its client is
/// constructed and which request shape it speaks. Persisted so a saved provider
/// dispatches to the right backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// OpenAI Responses API over a ChatGPT/Codex subscription (OAuth).
    ChatGpt,
    /// OpenAI-compatible chat-completions over an API key and a base URL —
    /// covers xAI/Grok, Groq, DeepSeek, OpenRouter, Together, Mistral,
    /// Perplexity, and the plain OpenAI API.
    OpenAi,
    /// Anthropic (Claude): `x-api-key` header, `thinking` reasoning shape.
    Anthropic,
    /// Google Gemini.
    Gemini,
}

impl ProviderKind {
    /// Whether this backend authenticates with a bearer/API key rather than the
    /// ChatGPT subscription OAuth flow.
    pub fn is_api_key(self) -> bool {
        !matches!(self, Self::ChatGpt)
    }
}

/// The two credential shapes persisted by the UI: a plain API key, or the
/// ChatGPT subscription tokens.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Auth {
    ApiKey {
        api_key: Secret,
    },
    ChatGpt {
        access_token: Secret,
        refresh_token: Secret,
        account_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<u64>,
    },
}

impl Auth {
    /// The API key, if this is an API-key credential.
    pub fn api_key(&self) -> Option<&str> {
        match self {
            Self::ApiKey { api_key } => Some(api_key.expose()),
            Self::ChatGpt { .. } => None,
        }
    }
    /// The ChatGPT access token, if this is a ChatGPT credential.
    pub fn access_token(&self) -> Option<&str> {
        match self {
            Self::ChatGpt { access_token, .. } => Some(access_token.expose()),
            Self::ApiKey { .. } => None,
        }
    }
    /// The ChatGPT account id, if this is a ChatGPT credential.
    pub fn account_id(&self) -> Option<&str> {
        match self {
            Self::ChatGpt { account_id, .. } => Some(account_id),
            Self::ApiKey { .. } => None,
        }
    }
    /// The ChatGPT account email, if known.
    pub fn email(&self) -> Option<&str> {
        match self {
            Self::ChatGpt { email, .. } => email.as_deref(),
            Self::ApiKey { .. } => None,
        }
    }
    /// The token expiry (ChatGPT only — API keys don't expire on our clock).
    pub fn expires_at(&self) -> Option<u64> {
        match self {
            Self::ChatGpt { expires_at, .. } => *expires_at,
            Self::ApiKey { .. } => None,
        }
    }
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiKey { .. } => f.debug_struct("ApiKey").finish_non_exhaustive(),
            Self::ChatGpt {
                account_id,
                email,
                expires_at,
                ..
            } => f
                .debug_struct("ChatGpt")
                .field("account_id", account_id)
                .field("email", email)
                .field("expires_at", expires_at)
                .finish_non_exhaustive(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedProvider {
    pub id: ProviderId,
    pub name: String,
    pub base_url: Url,
    /// The backend this provider drives. Defaults to `ChatGpt` so a
    /// pre-multi-provider `providers.toml` (no `kind`) loads unchanged.
    #[serde(default = "default_kind")]
    pub kind: ProviderKind,
    // Model and reasoning effort are no longer persisted here — they live in
    // `settings.toml` (global/project layered). These fields are runtime-only
    // carriers, populated from the resolved settings; `default` still reads a
    // value from a pre-migration `providers.toml`, and `skip_serializing`
    // ensures it is never written back, so the field drops out on the next save.
    #[serde(default, skip_serializing)]
    pub model: Option<String>,
    #[serde(default, skip_serializing)]
    pub reasoning_effort: Option<String>,
    pub auth: Auth,
}

fn default_kind() -> ProviderKind {
    ProviderKind::ChatGpt
}

impl SavedProvider {
    pub fn chatgpt(id: ProviderId, name: impl Into<String>, auth: Auth) -> Self {
        Self {
            id,
            name: name.into(),
            base_url: Url::parse(CHATGPT_CODEX_BASE_URL).expect("constant URL"),
            kind: ProviderKind::ChatGpt,
            model: None,
            reasoning_effort: None,
            auth,
        }
    }

    /// Construct an API-key provider (xAI, Anthropic, Gemini, an OpenAI-
    /// compatible endpoint, …). The base URL must be a valid HTTP(S) base.
    pub fn api_key(
        id: ProviderId,
        name: impl Into<String>,
        kind: ProviderKind,
        base_url: Url,
        api_key: Secret,
    ) -> Result<Self> {
        if api_key.is_empty() {
            return Err(Error::InvalidConfig("API key cannot be empty".into()));
        }
        validate_base_url(&base_url)?;
        Ok(Self {
            id,
            name: name.into(),
            base_url,
            kind,
            model: None,
            reasoning_effort: None,
            auth: Auth::ApiKey { api_key },
        })
    }

    pub fn request_auth(&self) -> Result<RequestAuth> {
        let mut headers = HeaderMap::new();
        let invalid =
            || Error::InvalidConfig("credential contains invalid header characters".into());
        match &self.auth {
            // Anthropic authenticates with `x-api-key` + a version header rather
            // than a bearer token.
            Auth::ApiKey { api_key } if self.kind == ProviderKind::Anthropic => {
                headers.insert(
                    "x-api-key",
                    HeaderValue::from_str(api_key.expose()).map_err(|_| invalid())?,
                );
                headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
            }
            Auth::ApiKey { api_key } => {
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {}", api_key.expose()))
                        .map_err(|_| invalid())?,
                );
            }
            Auth::ChatGpt {
                access_token,
                account_id,
                ..
            } => {
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {}", access_token.expose()))
                        .map_err(|_| invalid())?,
                );
                headers.insert(
                    "chatgpt-account-id",
                    HeaderValue::from_str(account_id).map_err(|_| invalid())?,
                );
            }
        }
        Ok(RequestAuth { headers })
    }
}

fn validate_base_url(url: &Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") || url.cannot_be_a_base() {
        return Err(Error::InvalidConfig(
            "base URL must be an HTTP(S) base URL".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct RequestAuth {
    pub headers: HeaderMap,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secrets_are_redacted_and_headers_are_set() {
        let provider = SavedProvider::chatgpt(
            ProviderId::new("chatgpt").unwrap(),
            "ChatGPT",
            Auth::ChatGpt {
                access_token: Secret::new("access"),
                refresh_token: Secret::new("refresh"),
                account_id: "acct".into(),
                email: Some("me@example.com".into()),
                expires_at: None,
            },
        );
        let json = serde_json::to_string(&provider).unwrap();
        assert!(json.contains("access"));
        assert!(json.contains("\"type\":\"chat_gpt\""));
        assert!(!format!("{provider:?}").contains("access"));
        let headers = provider.request_auth().unwrap().headers;
        assert_eq!(headers[AUTHORIZATION], "Bearer access");
        assert_eq!(headers["chatgpt-account-id"], "acct");
    }

    #[test]
    fn api_key_provider_serializes_tagged_and_sets_bearer() {
        let provider = SavedProvider::api_key(
            ProviderId::new("xai").unwrap(),
            "xAI Grok",
            ProviderKind::OpenAi,
            Url::parse("https://api.x.ai/v1/").unwrap(),
            Secret::new("sk-xai"),
        )
        .unwrap();
        let json = serde_json::to_string(&provider).unwrap();
        assert!(json.contains("\"type\":\"api_key\""));
        assert!(json.contains("\"kind\":\"open_ai\""));
        let headers = provider.request_auth().unwrap().headers;
        assert_eq!(headers[AUTHORIZATION], "Bearer sk-xai");
        assert!(!headers.contains_key("chatgpt-account-id"));
    }

    #[test]
    fn anthropic_uses_x_api_key_header() {
        let provider = SavedProvider::api_key(
            ProviderId::new("anthropic").unwrap(),
            "Claude",
            ProviderKind::Anthropic,
            Url::parse("https://api.anthropic.com/").unwrap(),
            Secret::new("sk-ant"),
        )
        .unwrap();
        let headers = provider.request_auth().unwrap().headers;
        assert_eq!(headers["x-api-key"], "sk-ant");
        assert!(!headers.contains_key(AUTHORIZATION));
    }

    #[test]
    fn empty_api_key_is_rejected() {
        assert!(
            SavedProvider::api_key(
                ProviderId::new("x").unwrap(),
                "X",
                ProviderKind::OpenAi,
                Url::parse("https://api.x.ai/v1/").unwrap(),
                Secret::new(""),
            )
            .is_err()
        );
    }
}
