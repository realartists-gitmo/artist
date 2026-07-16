mod args;
mod chat_ui;
mod command_ui;
mod login;
mod models;
mod prompt;
mod sessions;
mod slash_commands;
mod status_bar;
mod store;
mod test_provider;
mod tool_ui;

use anyhow::{Context, Result, bail};
use args::{Cli, Command, LoginKind, ProviderAction};
use artist_tools::{ToolBundle, Workspace};
use clap::Parser;
use llm_provider::ChatGptOAuth;
use sessions::{Role, SessionStore, Turn};
use std::{
    io::IsTerminal,
    time::{SystemTime, UNIX_EPOCH},
};
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
    if let Some(prompt) = cli.print_prompt {
        if cli.command.is_some() {
            bail!("-p cannot be combined with a subcommand");
        }
        return execute_prompt(&mut store, &path, &prompt, cli.resume.as_deref()).await;
    }
    match cli.command {
        Some(Command::Provider(args)) if cli.prompt.is_none() && cli.resume.is_none() => {
            match (args.login, args.action) {
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
            }
        }
        Some(Command::Model) if cli.prompt.is_none() && cli.resume.is_none() => {
            let selected = default_index(&store)?;
            if refresh_if_needed(&mut store.providers[selected]).await? {
                store.save(&path)?;
            }
            models::select(&mut store.providers[selected]).await?;
            store.save(&path)?;
        }
        Some(_) => bail!("prompts and --resume cannot be combined with a subcommand"),
        None => {
            let selected = default_index(&store)?;
            if refresh_if_needed(&mut store.providers[selected]).await? {
                store.save(&path)?;
            }
            let config_root = path.parent().context("providers path has no parent")?;
            let sessions = SessionStore::new(config_root);
            let project = std::env::current_dir().context("find current project directory")?;
            let resumed = load_resumed(&sessions, &project, cli.resume.as_deref())?;
            let tools = tool_bundle(config_root, &project)?;
            chat_ui::run(
                &mut store,
                selected,
                &path,
                chat_ui::ChatResources {
                    sessions: &sessions,
                    project: &project,
                    tools: &tools,
                },
                resumed,
                cli.prompt,
            )
            .await?;
        }
    }
    Ok(())
}

fn load_resumed(
    sessions: &SessionStore,
    project: &std::path::Path,
    resume: Option<&str>,
) -> Result<Option<(sessions::Session, Vec<Turn>)>> {
    let Some(requested) = resume else {
        return Ok(None);
    };
    let mut available = sessions.list_project(project)?;
    available.sort_by_key(|session| std::cmp::Reverse(session.created_at_ms));
    let id = if requested.is_empty() {
        if available.is_empty() {
            bail!("no sessions found for {}", project.display());
        }
        let items = available
            .iter()
            .map(|session| {
                format!(
                    "{}  {}",
                    session.id,
                    session.label.as_deref().unwrap_or("Untitled")
                )
            })
            .collect::<Vec<_>>();
        available[prompt::select("Session to resume", &items, 0)?]
            .id
            .clone()
    } else {
        if !available.iter().any(|session| session.id == requested) {
            bail!("session {requested} was not found in this project");
        }
        requested.to_owned()
    };
    Ok(Some(sessions.load(&id)?))
}

async fn execute_prompt(
    store: &mut ProviderStore,
    path: &std::path::Path,
    input: &str,
    resume: Option<&str>,
) -> Result<()> {
    let selected = default_index(store)?;
    if refresh_if_needed(&mut store.providers[selected]).await? {
        store.save(path)?;
    }
    let config_root = path.parent().context("providers path has no parent")?;
    let sessions = SessionStore::new(config_root);
    let project = std::env::current_dir().context("find current project directory")?;
    let tools = tool_bundle(config_root, &project)?;
    let (session, turns) = match resume {
        Some("") => {
            let mut available = sessions.list_project(&project)?;
            available.sort_by_key(|session| std::cmp::Reverse(session.created_at_ms));
            if available.is_empty() {
                bail!("no sessions found for {}", project.display());
            }
            let items = available
                .iter()
                .map(|session| {
                    format!(
                        "{}  {}",
                        session.id,
                        session.label.as_deref().unwrap_or("Untitled")
                    )
                })
                .collect::<Vec<_>>();
            let selected = prompt::select("Session to resume", &items, 0)?;
            sessions.load(&available[selected].id)?
        }
        Some(id) => {
            if !sessions
                .list_project(&project)?
                .iter()
                .any(|session| session.id == id)
            {
                bail!("session {id} was not found in this project");
            }
            sessions.load(id)?
        }
        None => (sessions.create(&project, Some(input))?, Vec::new()),
    };
    let history = turns
        .iter()
        .map(|turn| artist_agent::ChatMessage {
            role: match turn.role {
                Role::User => artist_agent::ChatRole::User,
                Role::Assistant => artist_agent::ChatRole::Assistant,
            },
            content: turn.content.clone(),
        })
        .collect::<Vec<_>>();
    sessions.append(
        &session.id,
        &Turn {
            role: Role::User,
            content: input.to_owned(),
        },
    )?;
    let styled = std::io::stdout().is_terminal();
    let mut reasoning = false;
    let mut response = String::new();
    artist_agent::stream_chat(
        &store.providers[selected],
        input,
        &history,
        &tools,
        |event| {
            use artist_agent::PromptEvent;
            use std::io::Write;
            let mut output = std::io::stdout().lock();
            match event {
                PromptEvent::ReasoningSummaryDelta(delta) => {
                    reasoning = true;
                    if styled {
                        write!(output, "\x1b[90;3m{delta}\x1b[0m")?;
                    } else {
                        write!(output, "{delta}")?;
                    }
                }
                PromptEvent::TextDelta(delta) => {
                    response.push_str(&delta);
                    if std::mem::take(&mut reasoning) {
                        writeln!(output)?;
                    }
                    write!(output, "{delta}")?;
                }
                PromptEvent::ToolCall { name, .. } => eprintln!("Calling {name}..."),
                PromptEvent::ToolExecutionStart { .. } => {}
                PromptEvent::ToolResult { .. } => eprintln!("Tool completed."),
                PromptEvent::CompletionUsage { .. } => {}
            }
            output.flush()?;
            Ok(())
        },
    )
    .await?;
    println!();
    sessions.append(
        &session.id,
        &Turn {
            role: Role::Assistant,
            content: response,
        },
    )?;
    Ok(())
}

fn tool_bundle(config_root: &std::path::Path, project: &std::path::Path) -> Result<ToolBundle> {
    use std::hash::{Hash, Hasher};
    let canonical = std::fs::canonicalize(project)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical.hash(&mut hasher);
    let state = config_root
        .join("tools")
        .join(format!("{:x}", hasher.finish()));
    Ok(ToolBundle::new(Workspace::open(canonical, state)?))
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
