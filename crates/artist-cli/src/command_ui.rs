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
            let current_model = store.providers[provider_index]
                .model
                .as_ref()
                .and_then(|slug| catalog.iter().position(|model| &model.slug == slug))
                .unwrap_or(0);
            let model_index = match model {
                Some(slug) => catalog
                    .iter()
                    .position(|candidate| candidate.slug == slug)
                    .with_context(|| format!("model `{slug}` is not selectable"))?,
                None => pick_model(&catalog, current_model, &mut draw)?
                    .context("model selection cancelled")?,
            };
            let selected = &catalog[model_index];
            let reasoning = match reasoning {
                Some(effort) => Some(effort.to_owned()),
                None if selected.supported_reasoning_levels.is_empty() => None,
                None => {
                    let preferred = store.providers[provider_index]
                        .reasoning_effort
                        .as_ref()
                        .or(selected.default_reasoning_level.as_ref());
                    let current = preferred
                        .and_then(|effort| {
                            selected
                                .supported_reasoning_levels
                                .iter()
                                .position(|level| &level.effort == effort)
                        })
                        .unwrap_or(0);
                    Some(
                        selected.supported_reasoning_levels[pick_reasoning(
                            selected, current, &mut draw,
                        )?
                        .context("reasoning selection cancelled")?]
                        .effort
                        .clone(),
                    )
                }
            };
            let previous = store.providers[provider_index].clone();
            models::apply_selection(
                &mut store.providers[provider_index],
                &catalog,
                &selected.slug,
                reasoning.as_deref(),
            )?;
            if let Err(error) = store.save(store_path) {
                store.providers[provider_index] = previous;
                return Err(error);
            }
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
    current: usize,
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
    pick("Select model", &labels, current, draw)
}

fn pick_reasoning(
    model: &SelectableModel,
    current: usize,
    draw: &mut impl FnMut(&[String]) -> Result<()>,
) -> Result<Option<usize>> {
    let labels = model
        .supported_reasoning_levels
        .iter()
        .map(|level| format!("{}  {}", level.effort, level.description))
        .collect::<Vec<_>>();
    pick("Select reasoning", &labels, current, draw)
}

fn pick(
    title: &str,
    labels: &[String],
    current: usize,
    draw: &mut impl FnMut(&[String]) -> Result<()>,
) -> Result<Option<usize>> {
    let mut selected = current.min(labels.len().saturating_sub(1));
    loop {
        let mut panel = vec![title.to_owned()];
        let start = selected
            .saturating_sub(3)
            .min(labels.len().saturating_sub(7));
        panel.extend(
            labels
                .iter()
                .enumerate()
                .skip(start)
                .take(7)
                .map(|(index, label)| {
                    format!("{} {label}", if index == selected { "›" } else { " " })
                }),
        );
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
