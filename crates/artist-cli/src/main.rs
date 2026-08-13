mod args;
mod chat_ui;
mod command_ui;
mod herdr;
mod kernel;
mod login;
mod models;
mod prompt;
mod provider_commands;
mod sessions;
mod settings;
mod store;
mod termination;
mod test_provider;

use anyhow::{Context, Result, bail};
use args::{Cli, Command, SessionsCommand};
use clap::Parser;
use llm_provider::ChatGptOAuth;
use sessions::{ActiveSession, SessionStore};
use std::{
    io::IsTerminal,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use store::{ProviderStore, config_path};

#[tokio::main]
async fn main() {
    let herdr_runtime = herdr::Runtime::detect();
    let result = tokio::select! {
        result = run(herdr_runtime.lifecycle()) => result,
        _ = termination::requested() => Err(anyhow::anyhow!("interrupted")),
    };
    herdr_runtime.shutdown().await;
    if let Err(error) = result {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}

async fn run(herdr: herdr::Lifecycle) -> Result<()> {
    let mut cli = Cli::parse();
    enter_positional_project(&mut cli)?;
    if let Some(Command::Resource(args)) = cli.command.as_ref() {
        if cli.prompt.is_some() || cli.resume.is_some() || cli.print_prompt.is_some() {
            bail!("resource commands cannot be combined with prompts or --resume");
        }
        let root = std::env::current_dir().context("find current project directory")?;
        let kernel = kernel::build(&root).await?;
        let result = kernel::dispatch(&kernel, args.verb.verb(), &args.target, &args.args).await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        return if result.ok {
            Ok(())
        } else {
            bail!("resource operation failed")
        };
    }
    let path = config_path()?;
    let mut store = ProviderStore::load(&path)?;
    let config_root = path.parent().context("providers path has no parent")?;
    if cli.command.is_none() {
        enter_resume_project(&SessionStore::new(config_root), cli.resume.as_deref())?;
    }

    if let Some(prompt) = cli.print_prompt {
        if cli.command.is_some() {
            bail!("-p cannot be combined with a subcommand");
        }
        if let Some(extra) = cli.prompt {
            bail!("with -p, the positional argument must be a project directory: {extra}");
        }
        return execute_prompt(&mut store, &path, &prompt, cli.resume.as_deref(), &herdr).await;
    }
    match cli.command {
        Some(Command::Model) if cli.prompt.is_none() && cli.resume.is_none() => {
            let selected = default_index(&store)?;
            if refresh_if_needed(&mut store.providers[selected]).await? {
                store.save(&path)?;
            }
            models::select(&mut store.providers[selected]).await?;
            store.save(&path)?;
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
            let selected = (!store.providers.is_empty())
                .then(|| default_index(&store))
                .transpose()?;
            let project = std::env::current_dir().context("find current project directory")?;
            let kernel = kernel::build(&project).await?;
            // Layered settings resolve model/reasoning overrides.
            let effective =
                settings::load_effective(config_root, &project, &settings::Overrides::default())?;
            // Catch a missing effective model before the TUI takes over. Legacy
            // settings scalars supply ChatGPT only; other providers must have a
            // provider-local selection.
            if let Some(selected) = selected
                && effective
                    .apply_to(store.providers[selected].clone())
                    .model
                    .is_none()
            {
                // Legacy/incomplete provider records should recover through the
                // interactive model setup instead of referring to the removed
                // standalone model command.
                models::select(&mut store.providers[selected]).await?;
                store.save(&path)?;
            }
            // Resolve an interactive resume before entering inline TUI mode so the
            // selector cannot be painted underneath the splash and input viewport.
            // Draw the startup UI before loading the provider and session.
            let sessions = SessionStore::new(config_root);
            let resumed = load_resumed(&sessions, &project, cli.resume.as_deref())?;
            let show_splash = resumed.is_none() && cli.prompt.is_none();
            let mut refreshed_provider = selected.map(|index| store.providers[index].clone());
            let refreshed = match refreshed_provider.as_mut() {
                Some(provider) => refresh_if_needed(provider).await?,
                None => false,
            };
            let terminal =
                chat_ui::start_terminal(show_splash, cli.prompt.is_some(), selected.is_none())?;
            if refreshed {
                let selected = selected.expect("a refreshed provider is selected");
                store.providers[selected] = refreshed_provider.expect("refreshed provider exists");
                store.save(&path)?;
            }
            chat_ui::run(
                terminal,
                &mut store,
                selected,
                &path,
                chat_ui::ChatResources {
                    sessions: &sessions,
                    project: &project,
                    settings: &effective,
                    herdr: &herdr,
                    kernel: kernel.clone(),
                },
                resumed,
                cli.prompt,
                show_splash,
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

fn enter_resume_project(sessions: &SessionStore, resume: Option<&str>) -> Result<()> {
    let Some(id) = resume.filter(|id| !id.is_empty()) else {
        return Ok(());
    };
    let session = sessions.find(id)?;
    if !session.project.is_dir() {
        bail!(
            "session '{id}' project no longer exists: {}",
            session.project.display()
        );
    }
    std::env::set_current_dir(&session.project).with_context(|| {
        format!(
            "restore project directory for session '{id}': {}",
            session.project.display()
        )
    })
}

fn load_resumed(
    sessions: &SessionStore,
    project: &std::path::Path,
    resume: Option<&str>,
) -> Result<Option<(ActiveSession, Vec<artist_session::Envelope>)>> {
    let Some(requested) = resume else {
        return Ok(None);
    };
    if !requested.is_empty() {
        return sessions.open(requested).map(Some);
    }
    let mut available = sessions.list_project(project)?;
    available.sort_by_key(|session| std::cmp::Reverse(session.created_at_ms));
    let id = {
        if available.is_empty() {
            bail!("no sessions found for {}", project.display());
        }
        let items = available
            .iter()
            .map(|session| {
                format!(
                    "{}  {}{}",
                    session.id,
                    session
                        .label
                        .as_deref()
                        .and_then(|label| label.lines().next())
                        .unwrap_or("Untitled"),
                    session
                        .parent
                        .as_deref()
                        .map(|parent| format!("  (fork of {parent})"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>();
        available[prompt::select_paged("Session to resume", &items, 0, 10)?]
            .id
            .clone()
    };
    Ok(Some(sessions.open(&id)?))
}

async fn execute_prompt(
    store: &mut ProviderStore,
    path: &std::path::Path,
    input: &str,
    resume: Option<&str>,
    herdr: &herdr::Lifecycle,
) -> Result<()> {
    let selected = default_index(store)?;
    if refresh_if_needed(&mut store.providers[selected]).await? {
        store.save(path)?;
    }
    let config_root = path.parent().context("providers path has no parent")?;
    let sessions = SessionStore::new(config_root);
    let project = std::env::current_dir().context("find current project directory")?;
    let kernel = kernel::build(&project).await?;
    let effective =
        settings::load_effective(config_root, &project, &settings::Overrides::default())?;
    // Session-scoped provider carrying the settings model/reasoning override
    // (a throwaway clone, never persisted).
    let session_provider = effective.apply_to(store.providers[selected].clone());
    let (active, _) = match load_resumed(&sessions, &project, resume)? {
        Some(resumed) => resumed,
        None => (sessions.create(&project, Some(input))?, Vec::new()),
    };
    herdr.report_session(&active.session.id);
    let turn_lifecycle = herdr.start_turn();
    let steering = artist_agent::SteeringHandle::default();
    let cancel = tokio_util::sync::CancellationToken::new();
    let handles = artist_agent::SessionHandles {
        kernel,
        steering: steering.clone(),
        recorder: active.recorder.clone(),
        memory: Arc::new(active.memory.clone()),
        conversation_id: active.session.id.clone(),
        provider_context: active.provider_context.clone(),
        fast_mode: false,
        lifecycle: turn_lifecycle.emitter(),
        cancel: cancel.clone(),
    };
    let styled = std::io::stdout().is_terminal();
    let mut reasoning = false;
    let mut response = String::new();
    let agent_input = artist_agent::ChatInput::from(input.to_owned());
    let outcome = {
        let chat = artist_agent::stream_chat(&session_provider, &agent_input, handles, |event| {
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
        });
        chat.await
    };
    turn_lifecycle.finish(matches!(
        outcome.as_ref().ok(),
        Some(artist_agent::RunOutcome::Cancelled)
    ));
    let outcome = outcome?;
    let _ = outcome;
    println!();
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
        let identity = provider
            .chatgpt_auth()
            .ok()
            .map(|auth| auth.email.as_deref().unwrap_or(&auth.account_id))
            .unwrap_or("API key");
        println!("{marker} {}  {identity}", provider.name);
    }
}

fn choose(store: &ProviderStore, label: &str) -> Result<usize> {
    if store.providers.is_empty() {
        bail!("no accounts saved — run `artist provider --login chatgpt` to sign in");
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
    let config_root = path.parent().context("providers path has no parent")?;
    let project = std::env::current_dir().context("find current project directory")?;
    let effective =
        settings::load_effective(config_root, &project, &settings::Overrides::default())?;
    let provider = effective.apply_to(store.providers[selected].clone());
    print!("Testing {}... ", provider.name);
    std::io::Write::flush(&mut std::io::stdout())?;
    test_provider::test(&provider).await?;
    println!("OK");
    Ok(())
}
fn default_index(store: &ProviderStore) -> Result<usize> {
    if store.providers.is_empty() {
        bail!("no account configured — run `artist provider --login chatgpt` to sign in");
    }
    let id = store
        .default_provider
        .as_ref()
        .context("no default provider set — run `artist provider set` to choose one")?;
    store
        .providers
        .iter()
        .position(|provider| &provider.id == id)
        .context("default provider is missing")
}

pub(crate) async fn refresh_if_needed(provider: &mut llm_provider::SavedProvider) -> Result<bool> {
    if provider.provider != llm_provider::ProviderKind::Chatgpt {
        return Ok(false);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // A token with no known expiry is refreshed proactively rather than
    // trusted forever — otherwise it silently rots into an unrecoverable 401.
    // The refresh response populates `expires_at`, so this self-corrects.
    let needs_refresh = provider
        .chatgpt_auth()?
        .expires_at
        .map_or(true, |expiry| expiry <= now.saturating_add(60));
    if needs_refresh {
        return force_refresh(provider).await.map(|()| true);
    }
    Ok(false)
}

/// Refresh the access token unconditionally, ignoring `expires_at`. Used after
/// a mid-turn 401 (AUTH-2), where the token is known-bad regardless of its
/// recorded expiry.
pub(crate) async fn force_refresh(provider: &mut llm_provider::SavedProvider) -> Result<()> {
    if provider.provider != llm_provider::ProviderKind::Chatgpt {
        return Ok(());
    }
    let refreshed = ChatGptOAuth::default()
        .refresh(provider.chatgpt_auth()?)
        .await
        .context("refresh ChatGPT login")?
        .auth;
    *provider.chatgpt_auth_mut()? = refreshed;
    Ok(())
}

#[cfg(test)]
mod resume_tests {
    use super::*;

    #[test]
    fn explicit_unknown_resume_is_a_clear_error() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let sessions = SessionStore::new(root.path());
        let error = load_resumed(&sessions, &project, Some("missing-session"))
            .err()
            .expect("unknown resume should fail");
        assert!(error.to_string().contains("missing-session"));
        assert!(error.to_string().contains("not found"));
    }
}
