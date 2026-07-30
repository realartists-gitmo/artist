use crate::store::ProviderStore;
use anyhow::{Context, Result, bail};
use llm_provider::{
    ChatGptOAuth, Credentials, OpenAiApi, ProviderId, ProviderKind, SavedProvider, Secret,
};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

/// Authentication categories and providers are deliberately separate so new
/// providers can be registered without leaking provider names into `/login`'s
/// first picker.
#[derive(Clone, Copy)]
enum AuthKind {
    Subscription,
    ApiKey,
}

impl AuthKind {
    const ALL: [Self; 2] = [Self::Subscription, Self::ApiKey];
    const fn label(self) -> &'static str {
        match self {
            Self::Subscription => "Subscription",
            Self::ApiKey => "API key",
        }
    }
}

#[derive(Clone, Copy)]
enum LoginProvider {
    OpenAiCodex,
    OpenAi,
}

impl LoginProvider {
    const fn label(self) -> &'static str {
        match self {
            Self::OpenAiCodex => "OpenAI Codex",
            Self::OpenAi => "OpenAI",
        }
    }
}

fn providers(kind: AuthKind) -> &'static [LoginProvider] {
    match kind {
        AuthKind::Subscription => &[LoginProvider::OpenAiCodex],
        AuthKind::ApiKey => &[LoginProvider::OpenAi],
    }
}

/// Interactive provider login. API keys are read without echo and validated
/// before they are ever written to the provider store.
pub async fn login(store: &mut ProviderStore) -> Result<()> {
    let categories = AuthKind::ALL.map(|kind| kind.label().to_owned());
    let kind = AuthKind::ALL[crate::prompt::select("Authentication method", &categories, 0)?];
    let available = providers(kind);
    let choices = available
        .iter()
        .map(|provider| provider.label().to_owned())
        .collect::<Vec<_>>();
    let provider = available[crate::prompt::select("Provider", &choices, 0)?];
    match provider {
        LoginProvider::OpenAiCodex => chatgpt(store).await,
        LoginProvider::OpenAi => api_key(store).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn login_registry_has_generic_categories_and_nested_provider_labels() {
        assert_eq!(
            AuthKind::ALL.map(AuthKind::label),
            ["Subscription", "API key"]
        );
        assert_eq!(
            providers(AuthKind::Subscription)
                .iter()
                .map(|p| p.label())
                .collect::<Vec<_>>(),
            ["OpenAI Codex"]
        );
        assert_eq!(
            providers(AuthKind::ApiKey)
                .iter()
                .map(|p| p.label())
                .collect::<Vec<_>>(),
            ["OpenAI"]
        );
    }
}

async fn api_key(store: &mut ProviderStore) -> Result<()> {
    let key = dialoguer::Password::new()
        .with_prompt("OpenAI API key")
        .interact()?;
    if key.trim().is_empty() {
        bail!("API key cannot be empty");
    }
    let provider = SavedProvider {
        id: ProviderId::new(unique_id(store, "openai"))?,
        name: "OpenAI".into(),
        provider: ProviderKind::Openai,
        base_url: Url::parse("https://api.openai.com/v1/")?,
        api: Some(OpenAiApi::Responses),
        api_version: None,
        model: None,
        reasoning_effort: None,
        credentials: Credentials::ApiKey {
            api_key: Secret::new(key),
        },
    };
    crate::models::catalog(&provider)
        .await
        .context("OpenAI API key validation failed")?;
    store.add(provider);
    println!("Validated and saved OpenAI API key.");
    Ok(())
}

pub async fn chatgpt(store: &mut ProviderStore) -> Result<()> {
    // The callback port must stay 1455 to match the registered redirect URI, so
    // a busy port can't be worked around — point the user at the likely cause.
    let listener = TcpListener::bind("127.0.0.1:1455").await.context(
        "OAuth callback port 1455 is in use — is another login (e.g. Codex) already running? Close it and retry",
    )?;
    let redirect = Url::parse("http://localhost:1455/auth/callback")?;
    let oauth = ChatGptOAuth::default();
    let login = oauth.begin_login(redirect)?;
    println!(
        "Opening your browser to log in with ChatGPT. If it doesn't open, visit:\n\n{}\n",
        login.authorize_url
    );
    open_in_browser(login.authorize_url.as_str());
    let (code, state) = tokio::time::timeout(Duration::from_secs(300), receive_callback(listener))
        .await
        .context("login timed out after 5 minutes")??;
    let auth = oauth.finish_login(login.pending, &code, &state).await?;
    let provider = SavedProvider::chatgpt(
        ProviderId::new(unique_id(store, "chatgpt"))?,
        "ChatGPT",
        auth,
    );
    println!("Logged in and saved ChatGPT.");
    store.add(provider);
    Ok(())
}

/// Best-effort open of `url` in the user's browser; the printed link is the
/// fallback if the platform opener isn't available.
fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

async fn receive_callback(listener: TcpListener) -> Result<(String, String)> {
    let (mut stream, _) = listener.accept().await?;
    let mut bytes = vec![0; 8192];
    let count = stream.read(&mut bytes).await?;
    let request = String::from_utf8_lossy(&bytes[..count]);
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .context("invalid OAuth callback")?;
    let url = Url::parse(&format!("http://localhost{target}"))?;
    if url.path() != "/auth/callback" {
        bail!("unexpected OAuth callback path");
    }
    let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    let result = match (params.get("code"), params.get("state"), params.get("error")) {
        (Some(code), Some(state), _) => Ok((code.clone(), state.clone())),
        (_, _, Some(error)) => Err(anyhow::anyhow!("OpenAI login failed: {error}")),
        _ => Err(anyhow::anyhow!("OAuth callback omitted code or state")),
    };
    let (status, body) = if result.is_ok() {
        ("200 OK", "Login complete. You can close this tab.")
    } else {
        ("400 Bad Request", "Login failed. Return to the terminal.")
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    result
}

fn unique_id(store: &ProviderStore, name: &str) -> String {
    let base: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .into();
    let base = if base.is_empty() { "provider" } else { &base };
    let mut id = base.to_owned();
    let mut suffix = 2;
    while store.providers.iter().any(|p| p.id.as_str() == id) {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    id
}
