mod args;
mod chat_ui;
mod clipboard;
mod command_ui;
mod custom_commands;
mod input_atoms;
mod input_images;
mod interaction;
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
use args::{Cli, Command, LoginKind, ProviderAction, RulesCommand, SessionsCommand};
use artist_tools::{ToolBundle, Workspace};
use clap::Parser;
use llm_provider::ChatGptOAuth;
use sessions::{ActiveSession, SessionStore};
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
    let mut cli = Cli::parse();
    enter_positional_project(&mut cli)?;
    let path = config_path()?;
    let mut store = ProviderStore::load(&path)?;
    let config_root = path.parent().context("providers path has no parent")?;
    if let Some(prompt) = cli.print_prompt {
        if cli.command.is_some() {
            bail!("-p cannot be combined with a subcommand");
        }
        let mcp = artist_agent::mcp::McpManager::load(config_root).await?;
        return execute_prompt(&mut store, &path, &prompt, cli.resume.as_deref(), &mcp).await;
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
        Some(Command::Rules(args)) if cli.prompt.is_none() && cli.resume.is_none() => {
            match args.action {
                RulesCommand::New { name } => scaffold_rule(&name)?,
            }
        }
        Some(Command::Sessions(args)) if cli.prompt.is_none() && cli.resume.is_none() => {
            let sessions = SessionStore::new(config_root);
            match args.action {
                SessionsCommand::List => sessions_list(&sessions)?,
                SessionsCommand::Render { id } => sessions_render(&sessions, &id)?,
                SessionsCommand::Gc {
                    keep,
                    older_than_days,
                    dry_run,
                } => sessions_gc(&sessions, keep, older_than_days, dry_run)?,
            }
        }
        Some(_) => bail!("prompts and --resume cannot be combined with a subcommand"),
        None => {
            let selected = default_index(&store)?;
            let mcp = artist_agent::mcp::McpManager::load(config_root).await?;
            if refresh_if_needed(&mut store.providers[selected]).await? {
                store.save(&path)?;
            }
            let sessions = SessionStore::new(config_root);
            let project = std::env::current_dir().context("find current project directory")?;
            let resumed = load_resumed(&sessions, &project, cli.resume.as_deref())?;
            let tools = tool_bundle(config_root, &project)?;
            let rules_engine = artist_rules::RulesEngine::discover(&project);
            let rules_handle = artist_rules::state::RulesHandle::default();
            chat_ui::run(
                &mut store,
                selected,
                &path,
                chat_ui::ChatResources {
                    sessions: &sessions,
                    project: &project,
                    tools: &tools,
                    mcp: &mcp,
                    rules_engine: &rules_engine,
                    rules_handle: &rules_handle,
                },
                resumed,
                cli.prompt,
            )
            .await?;
        }
    }
    Ok(())
}

fn enter_positional_project(cli: &mut Cli) -> Result<()> {
    let Some(candidate) = cli.prompt.as_deref() else {
        return Ok(());
    };
    let path = std::path::Path::new(candidate);
    if path.is_dir() {
        std::env::set_current_dir(path)
            .with_context(|| format!("enter project directory {}", path.display()))?;
        cli.prompt = None;
    }
    Ok(())
}

fn load_resumed(
    sessions: &SessionStore,
    project: &std::path::Path,
    resume: Option<&str>,
) -> Result<Option<(ActiveSession, Vec<artist_session::Envelope>)>> {
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
                    "{}  {}{}",
                    session.id,
                    session.label.as_deref().unwrap_or("Untitled"),
                    session
                        .parent
                        .as_deref()
                        .map(|parent| format!("  (fork of {parent})"))
                        .unwrap_or_default()
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
    Ok(Some(sessions.open(&id)?))
}

async fn execute_prompt(
    store: &mut ProviderStore,
    path: &std::path::Path,
    input: &str,
    resume: Option<&str>,
    mcp: &artist_agent::mcp::McpManager,
) -> Result<()> {
    let selected = default_index(store)?;
    if refresh_if_needed(&mut store.providers[selected]).await? {
        store.save(path)?;
    }
    let config_root = path.parent().context("providers path has no parent")?;
    let sessions = SessionStore::new(config_root);
    let project = std::env::current_dir().context("find current project directory")?;
    let tools = tool_bundle(config_root, &project)?;
    let (active, events) = match load_resumed(&sessions, &project, resume)? {
        Some(resumed) => resumed,
        None => (sessions.create(&project, Some(input))?, Vec::new()),
    };
    let history = artist_session::build_history(
        &events,
        &active.attachments,
        &artist_session::HistoryOptions::default(),
    )?;
    active.recorder.record(artist_session::TurnUser {
        content: vec![artist_session::ContentBlock::Text {
            text: input.to_owned(),
        }],
        display: None,
        source: "prompt".to_owned(),
    });
    let rules_engine = artist_rules::RulesEngine::discover(&project);
    let handles = artist_agent::SessionHandles {
        rules: artist_rules::state::RulesHandle::default(),
        rule_set: rules_engine.snapshot(),
        recorder: active.recorder.clone(),
        attachments: active.attachments.clone(),
        ..Default::default()
    };
    let styled = std::io::stdout().is_terminal();
    let mut reasoning = false;
    let mut response = String::new();
    let agent_input = artist_agent::ChatInput::from(input.to_owned());
    let _outcome = artist_agent::stream_chat(
        &store.providers[selected],
        &agent_input,
        history,
        &tools,
        mcp,
        handles,
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
                PromptEvent::RuleFired { rule, matched } => {
                    let excerpt: String = matched.chars().take(60).collect();
                    eprintln!("rule {rule} fired on \"{excerpt}\" — rewound, retrying");
                }
            }
            output.flush()?;
            Ok(())
        },
    )
    .await?;
    println!();
    let _ = response;
    active.close().await?;
    Ok(())
}

fn sessions_list(sessions: &SessionStore) -> Result<()> {
    let project = std::env::current_dir()?;
    let mut entries = sessions.list_project(&project)?;
    if entries.is_empty() {
        println!("No sessions for {}", project.display());
        return Ok(());
    }
    entries.sort_by_key(|session| std::cmp::Reverse(session.created_at_ms));
    for session in entries {
        let size = dir_size(session.dir());
        println!(
            "{}  {:>8}  {}{}",
            session.id,
            format_size(size),
            session.label.as_deref().unwrap_or("Untitled"),
            session
                .parent
                .as_deref()
                .map(|parent| format!("  (fork of {parent})"))
                .unwrap_or_default()
        );
    }
    Ok(())
}

fn sessions_render(sessions: &SessionStore, id: &str) -> Result<()> {
    let (session, events) = sessions.peek(id)?;
    let markdown = artist_session::render_markdown(&events);
    std::fs::write(&session.transcript, &markdown)?;
    println!("regenerated {}", session.transcript.display());
    Ok(())
}

fn sessions_gc(
    sessions: &SessionStore,
    keep: usize,
    older_than_days: u64,
    dry_run: bool,
) -> Result<()> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let cutoff = now.saturating_sub(older_than_days * 24 * 60 * 60 * 1000);
    let mut by_project: std::collections::BTreeMap<std::path::PathBuf, Vec<sessions::Session>> =
        Default::default();
    for session in sessions.list()? {
        by_project
            .entry(session.project.clone())
            .or_default()
            .push(session);
    }
    let mut removed = 0usize;
    let mut reclaimed = 0u64;
    for (_, mut entries) in by_project {
        entries.sort_by_key(|session| std::cmp::Reverse(session.created_at_ms));
        for session in entries.into_iter().skip(keep) {
            if session.created_at_ms >= cutoff {
                continue;
            }
            let size = dir_size(session.dir());
            removed += 1;
            reclaimed += size;
            if dry_run {
                println!(
                    "would delete {}  {:>8}  {}",
                    session.id,
                    format_size(size),
                    session.label.as_deref().unwrap_or("Untitled")
                );
            } else {
                sessions.remove(&session.id)?;
                println!("deleted {}  {:>8}", session.id, format_size(size));
            }
        }
    }
    println!(
        "{}{} session(s), {}",
        if dry_run { "would delete " } else { "deleted " },
        removed,
        format_size(reclaimed)
    );
    Ok(())
}

fn dir_size(dir: &std::path::Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![dir.to_owned()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                total += metadata.len();
            }
        }
    }
    total
}

fn format_size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1048575 => format!("{:.1} KiB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MiB", bytes as f64 / 1048576.0),
    }
}

/// `artist rules new <name>`: write a commented rule template into the
/// project's .artist/rules/ directory.
fn scaffold_rule(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        bail!("rule names are lowercase-kebab-case (got {name:?})");
    }
    let dir = std::env::current_dir()?.join(".artist/rules");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.md"));
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    let template = format!(
        r#"---
name: {name}
description: One line describing what this rule catches
# What to match against. Any of: assistant-text, tool-args, reasoning-summary.
targets: [assistant-text]
# Linear-time regexes; a match mid-stream aborts the request, injects the
# reminder below, and retries from the same point.
patterns:
  - 'REPLACE ME'
# Only for tool-args targets: restrict to these tools (empty = all).
# tools: [write, edit, bash]
# fire: once        # once per session (default) | per-turn
# persistence: session  # keep reminding every turn (default) | message
# scope: [main, delegate]
---
Write the reminder the model receives here. Say what NOT to do and what to
do instead.
"#
    );
    std::fs::write(&path, template)?;
    println!("created {}", path.display());
    println!(
        "test it against a session with: /rules dry-run {}",
        path.display()
    );
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
