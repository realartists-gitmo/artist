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
            ..
        } = auth
        else {
            return Err(Error::InvalidConfig(
                "only ChatGPT credentials can be refreshed".into(),
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
            auth: auth_from_response(response, Some((refresh_token, account_id)))?,
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
