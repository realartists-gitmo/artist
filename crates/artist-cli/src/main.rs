mod args;
mod sessions;

use anyhow::{Result, bail};
use args::{Cli, Command, RulesCommand, SessionsCommand};
use clap::Parser;
use sessions::SessionStore;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    if let Err(error) = run() {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let config_root = config_root()?;
    match cli.command {
        Command::Rules(args) => match args.action {
            RulesCommand::New { name } => scaffold_rule(&name),
        },
        Command::Sessions(args) => {
            let sessions = SessionStore::new(config_root);
            match args.action {
                SessionsCommand::List => sessions_list(&sessions),
                SessionsCommand::Render { id } => sessions_render(&sessions, &id),
                SessionsCommand::Gc {
                    keep,
                    older_than_days,
                    dry_run,
                } => sessions_gc(&sessions, keep, older_than_days, dry_run),
            }
        }
    }
}

fn config_root() -> Result<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("ARTIST_CONFIG_DIR") {
        return Ok(path.into());
    }
    dirs::config_dir()
        .map(|path| path.join("artist"))
        .ok_or_else(|| anyhow::anyhow!("could not determine config directory"))
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
        println!(
            "{}  {:>8}  {}{}",
            session.id,
            format_size(dir_size(session.dir())),
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
    std::fs::write(
        &session.transcript,
        artist_session::render_markdown(&events),
    )?;
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
    let mut by_project: std::collections::BTreeMap<_, Vec<sessions::Session>> = Default::default();
    for session in sessions.list()? {
        by_project
            .entry(session.project.clone())
            .or_default()
            .push(session);
    }
    let (mut removed, mut reclaimed) = (0usize, 0u64);
    for (_, mut entries) in by_project {
        entries.sort_by_key(|session| std::cmp::Reverse(session.created_at_ms));
        for session in entries
            .into_iter()
            .skip(keep)
            .filter(|session| session.created_at_ms < cutoff)
        {
            let size = dir_size(session.dir());
            removed += 1;
            reclaimed += size;
            if dry_run {
                println!("would delete {}  {:>8}", session.id, format_size(size));
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
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(entry.path())
            } else {
                total += metadata.len()
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

fn scaffold_rule(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        bail!("rule names are lowercase-kebab-case (got {name:?})");
    }
    let dir = std::env::current_dir()?.join(".artist/rules");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.md"));
    if path.exists() {
        bail!("{} already exists", path.display())
    }
    std::fs::write(
        &path,
        format!(
            r#"---
name: {name}
description: One line describing what this rule catches
targets: [assistant-text]
patterns:
  - 'REPLACE ME'
# tools: [write, edit, bash]
# fire: once
# persistence: session
# scope: [main, delegate]
---
Write the reminder here.
"#
        ),
    )?;
    println!("created {}", path.display());
    Ok(())
}
