use crate::store::ProviderStore;
use anyhow::{Context, Result, bail};
use llm_provider::{ChatGptOAuth, ProviderId, ProviderKind, SavedProvider, Secret};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

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

/// A backend the `provider add` flow knows how to pre-fill. The base URLs are
/// hosts (the rig client appends its own path); the last entry lets the user
/// point at any endpoint.
pub struct KnownProvider {
    pub label: &'static str,
    pub base_url: &'static str,
    pub kind: ProviderKind,
    pub env_key: &'static str,
}

pub const KNOWN_PROVIDERS: &[KnownProvider] = &[
    KnownProvider {
        label: "xAI (Grok)",
        base_url: "https://api.x.ai",
        kind: ProviderKind::OpenAi,
        env_key: "XAI_API_KEY",
    },
    KnownProvider {
        label: "OpenAI (API key)",
        base_url: "https://api.openai.com",
        kind: ProviderKind::OpenAi,
        env_key: "OPENAI_API_KEY",
    },
    KnownProvider {
        label: "Anthropic (Claude)",
        base_url: "https://api.anthropic.com",
        kind: ProviderKind::Anthropic,
        env_key: "ANTHROPIC_API_KEY",
    },
    KnownProvider {
        label: "Google Gemini",
        base_url: "https://generativelanguage.googleapis.com",
        kind: ProviderKind::Gemini,
        env_key: "GEMINI_API_KEY",
    },
    KnownProvider {
        label: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        kind: ProviderKind::Groq,
        env_key: "GROQ_API_KEY",
    },
    KnownProvider {
        label: "DeepSeek",
        base_url: "https://api.deepseek.com",
        kind: ProviderKind::DeepSeek,
        env_key: "DEEPSEEK_API_KEY",
    },
    KnownProvider {
        label: "Together AI",
        base_url: "https://api.together.xyz/v1",
        kind: ProviderKind::Together,
        env_key: "TOGETHER_API_KEY",
    },
    KnownProvider {
        label: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        kind: ProviderKind::OpenRouter,
        env_key: "OPENROUTER_API_KEY",
    },
    KnownProvider {
        label: "Mistral",
        base_url: "https://api.mistral.ai/v1",
        kind: ProviderKind::Mistral,
        env_key: "MISTRAL_API_KEY",
    },
    KnownProvider {
        label: "Perplexity",
        base_url: "https://api.perplexity.ai",
        kind: ProviderKind::Perplexity,
        env_key: "PERPLEXITY_API_KEY",
    },
    KnownProvider {
        label: "Custom (OpenAI Responses-compatible endpoint)",
        base_url: "",
        kind: ProviderKind::OpenAi,
        env_key: "",
    },
];

/// Interactively add an API-key provider: pick a backend, confirm name and base
/// URL, and supply the key (from the provider's env var if set, else prompted).
pub fn add_provider(store: &mut ProviderStore) -> Result<()> {
    let labels: Vec<String> = KNOWN_PROVIDERS
        .iter()
        .map(|provider| provider.label.to_owned())
        .collect();
    let choice = &KNOWN_PROVIDERS[crate::prompt::select("Provider", &labels, 0)?];
    let name = crate::prompt::text("Name", Some(choice.label))?;
    let base_default = (!choice.base_url.is_empty()).then_some(choice.base_url);
    let base_url = url::Url::parse(&crate::prompt::text("API base URL", base_default)?)
        .context("invalid base URL")?;
    let key = match (!choice.env_key.is_empty())
        .then(|| std::env::var(choice.env_key).ok())
        .flatten()
    {
        Some(key) => {
            println!("Using {} from the environment.", choice.env_key);
            key
        }
        None => crate::prompt::secret("API key")?,
    };
    let id = unique_id(store, &name);
    let provider = SavedProvider::api_key(
        ProviderId::new(id)?,
        name,
        choice.kind,
        base_url,
        Secret::new(key),
    )?;
    println!("Saved {} — set a model with `artist model`.", provider.name);
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
