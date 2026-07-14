mod args;
mod login;
mod models;
mod prompt;
mod store;
mod test_provider;

use anyhow::{Context, Result, bail};
use args::{Cli, Command, LoginKind, ProviderAction};
use clap::Parser;
use llm_provider::ChatGptOAuth;
use std::time::{SystemTime, UNIX_EPOCH};
use store::{ProviderStore, config_path};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let path = config_path()?;
    let mut store = ProviderStore::load(&path)?;
    match cli.command {
        Command::Provider(args) => match (args.login, args.action) {
            (Some(LoginKind::Chatgpt), None) => {
                login::chatgpt(&mut store).await?;
                store.save(&path)?;
            }
            (None, Some(ProviderAction::List)) => list(&store),
            (None, Some(ProviderAction::Set)) => {
                set_default(&mut store)?;
                store.save(&path)?;
            }
            (None, Some(ProviderAction::Test)) => {
                test_selected(&mut store, &path).await?;
                store.save(&path)?;
            }
            _ => bail!("choose --login chatgpt or list, set, or test"),
        },
        Command::Model => {
            let selected = default_index(&store)?;
            if refresh_if_needed(&mut store.providers[selected]).await? {
                store.save(&path)?;
            }
            models::select(&mut store.providers[selected]).await?;
            store.save(&path)?;
        }
    }
    Ok(())
}

fn list(store: &ProviderStore) {
    if store.providers.is_empty() {
        println!("No providers saved.");
        return;
    }
    for provider in &store.providers {
        let marker = if store.default_provider.as_ref() == Some(&provider.id) {
            "*"
        } else {
            " "
        };
        println!(
            "{marker} {}  {}",
            provider.name,
            provider
                .auth
                .email
                .as_deref()
                .unwrap_or(&provider.auth.account_id)
        );
    }
}

fn choose(store: &ProviderStore, label: &str) -> Result<usize> {
    if store.providers.is_empty() {
        bail!("no providers saved; log in first");
    }
    let items: Vec<_> = store.providers.iter().map(|p| p.name.clone()).collect();
    let default = store
        .default_provider
        .as_ref()
        .and_then(|id| store.providers.iter().position(|p| &p.id == id))
        .unwrap_or(0);
    prompt::select(label, &items, default)
}
fn set_default(store: &mut ProviderStore) -> Result<()> {
    let selected = choose(store, "Default provider")?;
    let provider = &store.providers[selected];
    store.default_provider = Some(provider.id.clone());
    println!("Default provider set to {}.", provider.name);
    Ok(())
}
async fn test_selected(store: &mut ProviderStore, path: &std::path::Path) -> Result<()> {
    let selected = choose(store, "Provider to test")?;
    if refresh_if_needed(&mut store.providers[selected]).await? {
        store.save(path)?;
    }
    let provider = &store.providers[selected];
    print!("Testing {}... ", provider.name);
    std::io::Write::flush(&mut std::io::stdout())?;
    test_provider::test(provider).await?;
    println!("OK");
    Ok(())
}
fn default_index(store: &ProviderStore) -> Result<usize> {
    let id = store
        .default_provider
        .as_ref()
        .context("no default provider; run `artist provider set`")?;
    store
        .providers
        .iter()
        .position(|provider| &provider.id == id)
        .context("default provider is missing")
}

async fn refresh_if_needed(provider: &mut llm_provider::SavedProvider) -> Result<bool> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if provider
        .auth
        .expires_at
        .is_some_and(|expiry| expiry <= now.saturating_add(60))
    {
        provider.auth = ChatGptOAuth::default()
            .refresh(&provider.auth)
            .await
            .context("refresh ChatGPT login")?
            .auth;
        return Ok(true);
    }
    Ok(false)
}
