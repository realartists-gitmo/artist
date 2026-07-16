use crate::{
    models::{self, SelectableModel},
    slash_commands::{self, ParseError, ParsedCommand},
    store::ProviderStore,
};
use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event, KeyCode};
use std::path::Path;

pub fn format_parse_error(error: ParseError<'_>) -> String {
    match error {
        ParseError::UnknownCommand(command) => format!("Unknown command: {command}"),
        ParseError::InvalidUsage { usage, .. } => format!("Usage: {usage}"),
    }
}

pub async fn run(
    store: &mut ProviderStore,
    provider_index: usize,
    store_path: &Path,
    command: ParsedCommand<'_>,
    mut draw: impl FnMut(&[String]) -> Result<()>,
) -> Result<Vec<String>> {
    match command {
        ParsedCommand::Help => Ok(slash_commands::COMMANDS
            .iter()
            .map(|command| format!("{}  {}", command.usage, command.description))
            .collect()),
        ParsedCommand::Model { model, reasoning } => {
            draw(&["Loading models…".to_owned()])?;
            let catalog = models::catalog(&store.providers[provider_index]).await?;
            let model_index = match model {
                Some(slug) => catalog
                    .iter()
                    .position(|candidate| candidate.slug == slug)
                    .with_context(|| format!("model `{slug}` is not selectable"))?,
                None => pick_model(&catalog, &mut draw)?.context("model selection cancelled")?,
            };
            let selected = &catalog[model_index];
            let reasoning = match reasoning {
                Some(effort) => Some(effort.to_owned()),
                None if selected.supported_reasoning_levels.is_empty() => None,
                None => Some(
                    selected.supported_reasoning_levels[pick_reasoning(selected, &mut draw)?
                        .context("reasoning selection cancelled")?]
                    .effort
                    .clone(),
                ),
            };
            models::apply_selection(
                &mut store.providers[provider_index],
                &catalog,
                &selected.slug,
                reasoning.as_deref(),
            )?;
            store.save(store_path)?;
            Ok(vec![format!(
                "model set to {} with {} reasoning.",
                selected.display_name,
                store.providers[provider_index]
                    .reasoning_effort
                    .as_deref()
                    .unwrap_or("default")
            )])
        }
    }
}

fn pick_model(
    models: &[SelectableModel],
    draw: &mut impl FnMut(&[String]) -> Result<()>,
) -> Result<Option<usize>> {
    let labels = models
        .iter()
        .map(|model| {
            format!(
                "{}  {}",
                model.display_name,
                model.description.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>();
    pick("Select model", &labels, draw)
}

fn pick_reasoning(
    model: &SelectableModel,
    draw: &mut impl FnMut(&[String]) -> Result<()>,
) -> Result<Option<usize>> {
    let labels = model
        .supported_reasoning_levels
        .iter()
        .map(|level| format!("{}  {}", level.effort, level.description))
        .collect::<Vec<_>>();
    pick("Select reasoning", &labels, draw)
}

fn pick(
    title: &str,
    labels: &[String],
    draw: &mut impl FnMut(&[String]) -> Result<()>,
) -> Result<Option<usize>> {
    let mut selected = 0;
    loop {
        let mut panel = vec![title.to_owned()];
        panel.extend(labels.iter().enumerate().map(|(index, label)| {
            format!("{} {label}", if index == selected { "›" } else { " " })
        }));
        draw(&panel)?;
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down => selected = (selected + 1).min(labels.len().saturating_sub(1)),
                KeyCode::Enter => return Ok(Some(selected)),
                KeyCode::Esc => return Ok(None),
                _ => {}
            }
        }
    }
}
