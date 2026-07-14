use crate::{prompt, store::ProviderStore};
use anyhow::{Context, Result, bail};
use llm_provider::{ChatGptOAuth, ProviderId, SavedProvider, Secret};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

pub fn openai_api(store: &mut ProviderStore) -> Result<()> {
    let name = prompt::text("Provider name", Some("OpenAI"))?;
    let base_url = Url::parse(&prompt::text(
        "API base URL",
        Some("https://api.openai.com/v1/"),
    )?)
    .context("invalid API base URL")?;
    let model = prompt::text("Model", Some("gpt-5-mini"))?;
    let key = prompt::secret("API key")?;
    let id = unique_id(store, &name);
    let mut provider =
        SavedProvider::openai_compatible(ProviderId::new(id)?, name, base_url, Secret::new(key))?;
    provider.model = Some(nonempty(model, "model")?);
    println!("Saved provider {}.", provider.name);
    store.add(provider);
    Ok(())
}

pub async fn chatgpt(store: &mut ProviderStore) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:1455")
        .await
        .context("bind OAuth callback on port 1455")?;
    let redirect = Url::parse("http://localhost:1455/auth/callback")?;
    let oauth = ChatGptOAuth::default();
    let login = oauth.begin_login(redirect)?;
    println!(
        "Open this link to log in with ChatGPT:\n\n{}\n",
        login.authorize_url
    );
    let (code, state) = tokio::time::timeout(Duration::from_secs(300), receive_callback(listener))
        .await
        .context("login timed out after 5 minutes")??;
    let auth = oauth.finish_login(login.pending, &code, &state).await?;
    let mut provider = SavedProvider::chatgpt(
        ProviderId::new(unique_id(store, "chatgpt"))?,
        "ChatGPT",
        auth,
    )?;
    provider.model = Some("gpt-5.1-codex-mini".into());
    println!("Logged in and saved ChatGPT.");
    store.add(provider);
    Ok(())
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

fn nonempty(value: String, field: &str) -> Result<String> {
    if value.trim().is_empty() {
        bail!("{field} cannot be empty")
    }
    Ok(value)
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
