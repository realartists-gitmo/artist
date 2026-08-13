//! Minimal interactive shell used while the VFS tool surface is rebuilt.

use crate::{
    herdr::Lifecycle,
    sessions::{ActiveSession, SessionStore},
    settings::EffectiveSettings,
    store::ProviderStore,
};
use anyhow::{Context, Result};
use artist_session::Envelope;
use ratatui::{TerminalOptions, Viewport, backend::CrosstermBackend};
use std::{
    io::{self, IsTerminal, Write},
    path::Path,
};
use tokio_util::sync::CancellationToken;

pub struct ChatResources<'a> {
    pub sessions: &'a SessionStore,
    pub project: &'a Path,
    pub settings: &'a EffectiveSettings,
    pub herdr: &'a Lifecycle,
    pub kernel: artist_kernel::Kernel,
}

pub fn start_terminal(
    _show_splash: bool,
    _thinking: bool,
    _needs_login: bool,
) -> Result<ratatui::Terminal<CrosstermBackend<io::Stdout>>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("interactive chat requires a terminal; use -p for non-interactive prompts");
    }
    Ok(ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(3),
    }))
}

pub async fn run(
    mut terminal: ratatui::Terminal<CrosstermBackend<io::Stdout>>,
    store: &mut ProviderStore,
    provider_index: Option<usize>,
    _store_path: &Path,
    resources: ChatResources<'_>,
    resumed: Option<(ActiveSession, Vec<Envelope>)>,
    initial_prompt: Option<String>,
    _show_splash: bool,
) -> Result<()> {
    let result = run_inner(
        &mut terminal,
        store,
        provider_index,
        resources,
        resumed,
        initial_prompt,
    )
    .await;
    ratatui::restore();
    result
}

async fn run_inner(
    terminal: &mut ratatui::Terminal<CrosstermBackend<io::Stdout>>,
    store: &mut ProviderStore,
    provider_index: Option<usize>,
    resources: ChatResources<'_>,
    resumed: Option<(ActiveSession, Vec<Envelope>)>,
    initial_prompt: Option<String>,
) -> Result<()> {
    let Some(provider_index) = provider_index else {
        terminal.draw(|frame| {
            frame.render_widget(
                ratatui::widgets::Paragraph::new("No provider configured."),
                frame.area(),
            )
        })?;
        return Ok(());
    };
    let provider = resources
        .settings
        .apply_to(store.providers[provider_index].clone());
    let (mut active, _) = match resumed {
        Some(session) => (Some(session.0), session.1),
        None => (None, Vec::new()),
    };
    let mut first = initial_prompt;
    loop {
        let prompt = match first.take() {
            Some(prompt) => prompt,
            None => {
                terminal.draw(|frame| {
                    frame.render_widget(ratatui::widgets::Paragraph::new("artist> "), frame.area())
                })?;
                let mut line = String::new();
                io::stdin().read_line(&mut line).context("read prompt")?;
                let line = line.trim().to_owned();
                if line == "/quit" || line == "/exit" {
                    break;
                }
                if line.is_empty() {
                    continue;
                }
                line
            }
        };
        let session = match active.take() {
            Some(session) => session,
            None => resources
                .sessions
                .create(resources.project, Some(&prompt))?,
        };
        resources.herdr.report_session(&session.session.id);
        let lifecycle = resources.herdr.start_turn();
        let cancel = CancellationToken::new();
        let handles = artist_agent::SessionHandles {
            kernel: resources.kernel.clone(),
            steering: artist_agent::SteeringHandle::default(),
            recorder: session.recorder.clone(),
            memory: std::sync::Arc::new(session.memory.clone()),
            conversation_id: session.session.id.clone(),
            provider_context: session.provider_context.clone(),
            fast_mode: false,
            lifecycle: lifecycle.emitter(),
            cancel: cancel.clone(),
        };
        let input = artist_agent::ChatInput::from(prompt);
        let result = artist_agent::stream_chat(&provider, &input, handles, |event| {
            use artist_agent::PromptEvent;
            match event {
                PromptEvent::TextDelta(text) | PromptEvent::ReasoningSummaryDelta(text) => {
                    print!("{text}")
                }
                PromptEvent::ToolCall { name, .. } => eprintln!("\nCalling {name}…"),
                PromptEvent::ToolResult { .. } => eprintln!("Tool completed."),
                PromptEvent::ToolExecutionStart { .. } | PromptEvent::CompletionUsage { .. } => {}
            }
            io::stdout().flush().map_err(anyhow::Error::from)
        })
        .await;
        lifecycle.finish(matches!(
            result.as_ref().ok(),
            Some(artist_agent::RunOutcome::Cancelled)
        ));
        result?;
        println!();
        active = Some(session);
    }
    if let Some(session) = active {
        session.close().await?;
    }
    Ok(())
}
