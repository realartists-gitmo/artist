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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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
            model: None,
            auth: Auth::ApiKey { api_key },
        })
    }

    pub fn chatgpt(id: ProviderId, name: impl Into<String>, auth: Auth) -> Result<Self> {
        if !matches!(auth, Auth::ChatGpt { .. }) {
            return Err(Error::InvalidConfig(
                "ChatGPT provider requires ChatGPT credentials".into(),
            ));
        }
        Ok(Self {
            id,
            name: name.into(),
            base_url: Url::parse(CHATGPT_CODEX_BASE_URL).expect("constant URL"),
            model: None,
            auth,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn api_provider() -> SavedProvider {
        SavedProvider::openai_compatible(
            ProviderId::new("local").unwrap(),
            "Local",
            Url::parse("https://llm.example/v1/").unwrap(),
            Secret::new("top-secret"),
        )
        .unwrap()
    }

    #[test]
    fn api_key_round_trips_but_debug_is_redacted() {
        let provider = api_provider();
        let json = serde_json::to_string(&provider).unwrap();
        assert!(json.contains("top-secret"));
        assert!(!format!("{provider:?}").contains("top-secret"));
        assert_eq!(
            serde_json::from_str::<SavedProvider>(&json).unwrap(),
            provider
        );
    }

    #[test]
    fn old_records_without_optional_fields_still_load() {
        let json = r#"{"id":"old","name":"Old","base_url":"https://example.com/v1/","auth":{"type":"api_key","api_key":"key"}}"#;
        let provider: SavedProvider = serde_json::from_str(json).unwrap();
        assert_eq!(provider.model, None);

        let old_chatgpt = r#"{"id":"chat","name":"ChatGPT","base_url":"https://chatgpt.com/backend-api/codex/","auth":{"type":"chat_gpt","access_token":"a","refresh_token":"r","account_id":"acct","expires_at":1}}"#;
        let provider: SavedProvider = serde_json::from_str(old_chatgpt).unwrap();
        assert!(matches!(provider.auth, Auth::ChatGpt { email: None, .. }));
    }

    #[test]
    fn request_headers_differ_by_auth_kind() {
        let api = api_provider().request_auth().unwrap();
        assert_eq!(api.headers[AUTHORIZATION], "Bearer top-secret");
        assert!(!api.headers.contains_key("chatgpt-account-id"));

        let chatgpt = SavedProvider::chatgpt(
            ProviderId::new("chatgpt").unwrap(),
            "ChatGPT",
            Auth::ChatGpt {
                access_token: Secret::new("access"),
                refresh_token: Secret::new("refresh"),
                account_id: "acct-1".into(),
                email: Some("me@example.com".into()),
                expires_at: None,
            },
        )
        .unwrap();
        let headers = chatgpt.request_auth().unwrap().headers;
        assert_eq!(headers[AUTHORIZATION], "Bearer access");
        assert_eq!(headers["chatgpt-account-id"], "acct-1");
        assert_eq!(chatgpt.base_url.as_str(), CHATGPT_CODEX_BASE_URL);
    }
}
