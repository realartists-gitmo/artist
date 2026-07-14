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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedProvider {
    pub id: ProviderId,
    pub name: String,
    pub base_url: Url,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    pub auth: Auth,
}

impl SavedProvider {
    pub fn chatgpt(id: ProviderId, name: impl Into<String>, auth: Auth) -> Self {
        Self {
            id,
            name: name.into(),
            base_url: Url::parse(CHATGPT_CODEX_BASE_URL).expect("constant URL"),
            model: None,
            reasoning_effort: None,
            auth,
        }
    }

    pub fn request_auth(&self) -> Result<RequestAuth> {
        let mut headers = HeaderMap::new();
        let bearer = HeaderValue::from_str(&format!("Bearer {}", self.auth.access_token.expose()))
            .map_err(|_| {
                Error::InvalidConfig("credential contains invalid header characters".into())
            })?;
        headers.insert(AUTHORIZATION, bearer);
        headers.insert(
            "chatgpt-account-id",
            HeaderValue::from_str(&self.auth.account_id).map_err(|_| {
                Error::InvalidConfig("account id contains invalid header characters".into())
            })?,
        );
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
