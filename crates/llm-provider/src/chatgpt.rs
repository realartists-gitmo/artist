use crate::{Auth, Error, Result, Secret};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Current public-client identifier used by the open-source Codex CLI.
/// OpenAI may rotate or restrict this; applications should allow overriding it.
pub const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const DEFAULT_ISSUER: &str = "https://auth.openai.com";
pub const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex/";

#[derive(Clone, Debug)]
pub struct ChatGptOAuth {
    client: Client,
    issuer: Url,
    client_id: String,
    originator: String,
}

impl Default for ChatGptOAuth {
    fn default() -> Self {
        Self::new(
            Client::new(),
            Url::parse(DEFAULT_ISSUER).expect("constant URL"),
            CODEX_CLIENT_ID,
        )
        .expect("valid defaults")
    }
}

impl ChatGptOAuth {
    pub fn new(client: Client, issuer: Url, client_id: impl Into<String>) -> Result<Self> {
        if issuer.scheme() != "https"
            && issuer.host_str() != Some("localhost")
            && issuer.host_str() != Some("127.0.0.1")
        {
            return Err(Error::InvalidConfig("OAuth issuer must use HTTPS".into()));
        }
        let client_id = client_id.into();
        if client_id.trim().is_empty() {
            return Err(Error::InvalidConfig(
                "OAuth client id cannot be empty".into(),
            ));
        }
        Ok(Self {
            client,
            issuer,
            client_id,
            originator: "artist".into(),
        })
    }

    pub fn with_originator(mut self, originator: impl Into<String>) -> Self {
        self.originator = originator.into();
        self
    }

    /// Starts Authorization Code + PKCE. The caller owns the loopback callback server.
    pub fn begin_login(&self, redirect_uri: Url) -> Result<LoginRequest> {
        if redirect_uri.scheme() != "http"
            || !matches!(
                redirect_uri.host_str(),
                Some("localhost" | "127.0.0.1" | "::1")
            )
        {
            return Err(Error::InvalidConfig(
                "redirect URI must be an HTTP loopback URL".into(),
            ));
        }
        let mut verifier_bytes = [0u8; 32];
        let mut state_bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut verifier_bytes);
        rand::rng().fill_bytes(&mut state_bytes);
        let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
        let state = URL_SAFE_NO_PAD.encode(state_bytes);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut authorize_url = self.issuer.join("oauth/authorize")?;
        authorize_url
            .query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", redirect_uri.as_str())
            .append_pair("scope", "openid profile email offline_access")
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true")
            .append_pair("state", &state)
            .append_pair("originator", &self.originator);
        Ok(LoginRequest {
            authorize_url,
            pending: PendingLogin {
                redirect_uri,
                state,
                verifier,
            },
        })
    }

    pub async fn finish_login(
        &self,
        pending: PendingLogin,
        code: &str,
        returned_state: &str,
    ) -> Result<Auth> {
        if !constant_time_eq(pending.state.as_bytes(), returned_state.as_bytes()) {
            return Err(Error::StateMismatch);
        }
        let response = self
            .client
            .post(self.issuer.join("oauth/token")?)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", pending.redirect_uri.as_str()),
                ("client_id", self.client_id.as_str()),
                ("code_verifier", pending.verifier.as_str()),
            ])
            .send()
            .await?
            .error_for_status()?
            .json::<TokenResponse>()
            .await?;
        auth_from_response(response, None)
    }

    pub async fn refresh(&self, auth: &Auth) -> Result<RefreshOutcome> {
        let Auth::ChatGpt {
            refresh_token,
            account_id,
            email,
            ..
        } = auth
        else {
            return Err(Error::InvalidConfig(
                "token refresh applies only to ChatGPT credentials".into(),
            ));
        };
        let response = self
            .client
            .post(self.issuer.join("oauth/token")?)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.expose()),
            ])
            .send()
            .await?
            .error_for_status()?
            .json::<TokenResponse>()
            .await?;
        Ok(RefreshOutcome {
            auth: auth_from_response(response, Some((refresh_token, account_id, email)))?,
        })
    }
}

#[derive(Debug)]
pub struct LoginRequest {
    pub authorize_url: Url,
    pub pending: PendingLogin,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PendingLogin {
    #[zeroize(skip)]
    redirect_uri: Url,
    state: String,
    verifier: String,
}
impl std::fmt::Debug for PendingLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PendingLogin([REDACTED])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshOutcome {
    pub auth: Auth,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
}

fn auth_from_response(
    response: TokenResponse,
    previous: Option<(&Secret, &String, &Option<String>)>,
) -> Result<Auth> {
    let access_token = response
        .access_token
        .ok_or(Error::MissingToken("access_token"))?;
    let refresh_token = response
        .refresh_token
        .or_else(|| previous.map(|(token, _, _)| token.expose().to_owned()))
        .ok_or(Error::MissingToken("refresh_token"))?;
    let mut identity = match response.id_token {
        Some(token) => identity_from_jwt(&token)?,
        None => previous
            .map(|(_, account_id, email)| Identity {
                account_id: account_id.clone(),
                email: email.clone(),
            })
            .ok_or(Error::MissingToken("id_token"))?,
    };
    if let Some((_, expected_account, expected_email)) = previous {
        if expected_account != &identity.account_id {
            return Err(Error::InvalidIdentityToken(
                "refreshed token belongs to a different ChatGPT account".into(),
            ));
        }
        if let (Some(expected), Some(actual)) = (expected_email, &identity.email)
            && expected != actual
        {
            return Err(Error::InvalidIdentityToken(
                "refreshed token belongs to a different email".into(),
            ));
        }
        if identity.email.is_none() {
            identity.email.clone_from(expected_email);
        }
    }
    let expires_at = response.expires_in.map(|seconds| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_add(seconds)
    });
    Ok(Auth::ChatGpt {
        access_token: Secret::new(access_token),
        refresh_token: Secret::new(refresh_token),
        account_id: identity.account_id,
        email: identity.email,
        expires_at,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct Identity {
    account_id: String,
    email: Option<String>,
}

fn identity_from_jwt(jwt: &str) -> Result<Identity> {
    let payload = jwt
        .split('.')
        .nth(1)
        .ok_or_else(|| Error::InvalidIdentityToken("expected a JWT".into()))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|e| Error::InvalidIdentityToken(e.to_string()))?;
    let claims: Value =
        serde_json::from_slice(&bytes).map_err(|e| Error::InvalidIdentityToken(e.to_string()))?;
    let account_id = claims
        .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
        .or_else(|| claims.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Error::InvalidIdentityToken("missing ChatGPT account id claim".into()))?;
    let email = claims
        .get("email")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Ok(Identity { account_id, email })
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn login_uses_pkce_and_state() {
        let oauth = ChatGptOAuth::default();
        let login = oauth
            .begin_login(Url::parse("http://127.0.0.1:1455/auth/callback").unwrap())
            .unwrap();
        let query: std::collections::HashMap<_, _> =
            login.authorize_url.query_pairs().into_owned().collect();
        assert_eq!(
            query.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(query.get("state"), Some(&login.pending.state));
        assert_ne!(query.get("code_challenge"), Some(&login.pending.verifier));
    }
    #[test]
    fn extracts_nested_account_and_email_claims() {
        let claims = serde_json::json!({
            "email": "me@example.com",
            "https://api.openai.com/auth": {"chatgpt_account_id":"acct-1"}
        });
        let jwt = format!(
            "e30.{}.sig",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
        );
        assert_eq!(
            identity_from_jwt(&jwt).unwrap(),
            Identity {
                account_id: "acct-1".into(),
                email: Some("me@example.com".into()),
            }
        );
    }

    #[test]
    fn refresh_without_id_token_preserves_identity() {
        let old_refresh = Secret::new("refresh");
        let account = "acct-1".to_string();
        let email = Some("me@example.com".to_string());
        let auth = auth_from_response(
            TokenResponse {
                access_token: Some("new-access".into()),
                refresh_token: None,
                id_token: None,
                expires_in: Some(60),
            },
            Some((&old_refresh, &account, &email)),
        )
        .unwrap();
        let Auth::ChatGpt {
            account_id,
            email: actual_email,
            ..
        } = auth
        else {
            panic!("expected ChatGpt auth");
        };
        assert_eq!(account_id, account);
        assert_eq!(actual_email, email);
    }
}
