use crate::{Error, Result, Secret};
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

/// The two credential shapes persisted by the UI.
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
        expires_at: Option<u64>,
    },
}
impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiKey { .. } => f.debug_struct("ApiKey").finish_non_exhaustive(),
            Self::ChatGpt {
                account_id,
                expires_at,
                ..
            } => f
                .debug_struct("ChatGpt")
                .field("account_id", account_id)
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
    pub auth: Auth,
}

impl SavedProvider {
    pub fn openai_compatible(
        id: ProviderId,
        name: impl Into<String>,
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
            auth: Auth::ApiKey { api_key },
        })
    }

    pub fn request_auth(&self) -> Result<RequestAuth> {
        let (token, account_id) = match &self.auth {
            Auth::ApiKey { api_key } => (api_key.expose(), None),
            Auth::ChatGpt {
                access_token,
                account_id,
                ..
            } => (access_token.expose(), Some(account_id.as_str())),
        };
        let mut headers = HeaderMap::new();
        let bearer = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
            Error::InvalidConfig("credential contains invalid header characters".into())
        })?;
        headers.insert(AUTHORIZATION, bearer);
        if let Some(id) = account_id {
            headers.insert(
                "chatgpt-account-id",
                HeaderValue::from_str(id).map_err(|_| {
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

fn validate_base_url(url: &Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") || url.cannot_be_a_base() {
        return Err(Error::InvalidConfig(
            "base URL must be an HTTP(S) base URL".into(),
        ));
    }
    Ok(())
}
