use crate::{
    activity_indicator::{activation_indicator, activity_status, format_elapsed},
    clipboard, command_ui,
    input_atoms::{ExpandedInput, InputAtoms},
    input_images::ImagePaste,
    interaction::{PromptHistory, SteeringQueue},
    models,
    sessions::{ActiveSession, SessionStore},
    slash_commands,
    status_bar::{self, StatusBarConfig, StatusItem},
    store::ProviderStore,
    subagent_ui::{self, SubagentStatuses},
    tool_ui::ToolUi,
};
use anyhow::{Context, Result};
use artist_rules::{RulesEngine, state::RulesHandle};
use artist_session::{Envelope, ReplayItem, SteeringDelivered};
use artist_tools::ToolBundle;
use llm_provider::SavedProvider;
use ratatui::{
    Frame, TerminalOptions, Viewport,
    buffer::Buffer,
    crossterm::{
        cursor::{Hide, MoveTo, Show},
        event::{
            self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent,
            KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
            PushKeyboardEnhancementFlags,
        },
        execute,
        style::Print,
        terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate},
    },
    layout::{Rect, Size},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};
use rig_core::completion::message::Message;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::IsTerminal,
    path::Path,
};
use tokio_util::sync::CancellationToken;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const NO_PROVIDER_NOTICE: &str = "No provider configured — run /login to connect one.";

#[derive(Default)]
pub(crate) struct ChatInput {
    text: String,
    cursor: usize,
    atoms: InputAtoms,
}

impl ChatInput {
    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return true;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') if !self.text.is_empty() => {
                    self.text.clear();
                    self.atoms.clear();
                    self.cursor = 0;
                    return true;
                }
                KeyCode::Char('c' | 'd' | 'z') => return false,
                _ => {}
            }
        }
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => return false,
            (KeyCode::Char('\n' | '\r'), modifiers) if modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert("\n");
            }
            (KeyCode::Char('\n' | '\r'), _) => {}
            (KeyCode::Char(character), _) => self.insert(&character.to_string()),
            (KeyCode::Enter, modifiers) if modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert("\n");
            }
            (KeyCode::Enter, _) => {}
            (KeyCode::Backspace, _) => self.backspace(),
            (KeyCode::Delete, _) => self.delete(),
            (KeyCode::Left, _) => self.move_left(),
            (KeyCode::Right, _) => self.move_right(),
            (KeyCode::Up, _) => self.move_vertical(false),
            (KeyCode::Down, _) => self.move_vertical(true),
            (KeyCode::Home, _) => self.cursor = self.line_start(),
            (KeyCode::End, _) => self.cursor = self.line_end(),
            _ => {}
        }
        true
    }

    fn insert(&mut self, value: &str) {
        self.cursor = self.atoms.insertion_point(self.cursor);
        self.atoms.insert_text(self.cursor, value.len());
        self.text.insert_str(self.cursor, value);
        self.cursor += value.len();
    }
    pub(crate) fn paste(&mut self, value: &str, allow_image: bool) {
        self.cursor = self.atoms.insertion_point(self.cursor);
        if allow_image {
            self.atoms
                .insert_paste(&mut self.text, &mut self.cursor, value);
        } else {
            self.atoms
                .insert_text_paste(&mut self.text, &mut self.cursor, value);
        }
    }
    fn replace_range(&mut self, range: std::ops::Range<usize>, value: &str) {
        self.text.replace_range(range.clone(), value);
        self.atoms.remove_text(range.start, range.end);
        self.atoms.insert_text(range.start, value.len());
        self.cursor = range.start + value.len();
    }
    fn take_expanded(&mut self) -> ExpandedInput {
        let expanded = self.atoms.expand(&self.text);
        self.text.clear();
        self.cursor = 0;
        self.atoms.clear();
        expanded
    }
    fn backspace(&mut self) {
        if self
            .atoms
            .remove_for_backspace(&mut self.text, &mut self.cursor)
        {
            return;
        }
        if let Some((index, _)) = self.text[..self.cursor].char_indices().next_back() {
            self.text.drain(index..self.cursor);
            self.atoms.remove_text(index, self.cursor);
            self.cursor = index;
        }
    }
    fn delete(&mut self) {
        if self
            .atoms
            .remove_for_delete(&mut self.text, &mut self.cursor)
        {
            return;
        }
        if let Some(character) = self.text[self.cursor..].chars().next() {
            let end = self.cursor + character.len_utf8();
            self.text.drain(self.cursor..end);
            self.atoms.remove_text(self.cursor, end);
        }
    }
    fn move_left(&mut self) {
        if let Some(cursor) = self.atoms.move_left(self.cursor) {
            self.cursor = cursor;
        } else if let Some((index, _)) = self.text[..self.cursor].char_indices().next_back() {
            self.cursor = index;
        }
    }
    fn move_right(&mut self) {
        if let Some(cursor) = self.atoms.move_right(self.cursor) {
            self.cursor = cursor;
        } else if let Some(character) = self.text[self.cursor..].chars().next() {
            self.cursor += character.len_utf8();
        }
    }
    fn line_start(&self) -> usize {
        self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1)
    }
    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |index| self.cursor + index)
    }
    fn move_vertical(&mut self, down: bool) {
        let start = self.line_start();
        let column = self.text[start..self.cursor].chars().count();
        let (target_start, target_end) = if down {
            let end = self.line_end();
            if end == self.text.len() {
                return;
            }
            let target_start = end + 1;
            let target_end = self.text[target_start..]
                .find('\n')
                .map_or(self.text.len(), |index| target_start + index);
            (target_start, target_end)
        } else {
            if start == 0 {
                return;
            }
            let target_end = start - 1;
            let target_start = self.text[..target_end]
                .rfind('\n')
                .map_or(0, |index| index + 1);
            (target_start, target_end)
        };
        self.cursor = self.text[target_start..target_end]
            .char_indices()
            .nth(column)
            .map_or(target_end, |(index, _)| target_start + index);
        self.cursor = self.atoms.insertion_point(self.cursor);
    }

    fn visual_lines(&self, inner_width: u16) -> u16 {
        crate::text_wrap::with_cursor(&self.text, self.text.len(), inner_width)
            .row
            .saturating_add(1)
    }
}

#[derive(Default)]
struct StatusRuntime {
    git_branch: Option<String>,
    /// Last completion's total — the current context size.
    used_tokens: Option<u64>,
    context_capacity: Option<u64>,
    /// Sum of all completion totals this session (billed volume).
    session_tokens: u64,
    fast_mode: bool,
    extension_values: Vec<(String, String)>,
}

impl StatusRuntime {
    fn refresh(&mut self, config: &StatusBarConfig, project: &Path) {
        self.refresh_git_branch_with(config, || status_bar::git_branch(project));
    }

    fn refresh_git_branch_with(
        &mut self,
        config: &StatusBarConfig,
        resolve: impl FnOnce() -> Option<String>,
    ) {
        self.git_branch = config
            .items
            .contains(&StatusItem::GitBranch)
            .then(resolve)
            .flatten();
    }
}

fn provider_with_session_selection(
    provider: SavedProvider,
    session_provider: &SavedProvider,
) -> SavedProvider {
    SavedProvider {
        model: session_provider.model.clone(),
        reasoning_effort: session_provider.reasoning_effort.clone(),
        ..provider
    }
}

fn footer_view(
    config: &StatusBarConfig,
    provider: Option<&SavedProvider>,
    project: &Path,
    runtime: &StatusRuntime,
) -> status_bar::StatusView {
    provider.map_or_else(status_bar::StatusView::default, |provider| {
        status_bar::view(status_bar::segments_with_fast_mode(
            config,
            project,
            provider,
            runtime.git_branch.as_deref(),
            runtime.used_tokens,
            runtime.context_capacity,
            runtime.session_tokens,
            runtime.fast_mode,
            &runtime.extension_values,
        ))
    })
}

struct SubmitContext<'a> {
    provider: &'a SavedProvider,
    sessions: &'a SessionStore,
    project: &'a Path,
    status_config: &'a StatusBarConfig,
    tools: &'a ToolBundle,
    mcp: &'a artist_agent::mcp::McpManager,
    extensions: &'a std::sync::Arc<artist_extensions::Manager>,
    extension_control: &'a crate::extension_control::ExtensionControl,
    herdr: &'a crate::herdr::Lifecycle,
    disabled_tools: &'a [String],
    compaction: crate::settings::CompactionConfig,
    show_splash: bool,
    rules_engine: &'a RulesEngine,
    rules_handle: &'a RulesHandle,
}

pub(crate) struct SubmittedPrompt {
    display: String,
    pub(crate) content: String,
    pub(crate) images: Vec<ImagePaste>,
    history_atoms: InputAtoms,
}

impl From<String> for SubmittedPrompt {
    fn from(content: String) -> Self {
        Self {
            display: content.clone(),
            content,
            images: Vec::new(),
            history_atoms: InputAtoms::default(),
        }
    }
}

struct SubmitResult {
    viewport_height: u16,
    queued: Vec<SubmittedPrompt>,
    delivered: Vec<String>,
    /// Text the user typed into the streaming box but never submitted, carried
    /// back so it isn't wiped when the turn ends.
    leftover_input: ChatInput,
    /// The turn failed with an authentication error (AUTH-2). The run loop
    /// force-refreshes the access token so a resend can succeed.
    auth_expired: bool,
}

/// Whether an error chain looks like an expired/invalid access token — a 401,
/// an explicit unauthorized, or an OAuth `invalid_grant`. Used to distinguish a
/// recoverable auth failure (refresh + resend) from a real error.
fn is_auth_error(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}").to_ascii_lowercase();
    text.contains("401")
        || text.contains("unauthorized")
        || text.contains("invalid_grant")
        || text.contains("invalid_token")
        || text.contains("token expired")
        || text.contains("token_expired")
}

struct PendingDelivery {
    display: String,
    content: String,
}

struct StreamingControls<'a> {
    input: &'a ChatInput,
    steering: &'a SteeringQueue,
    suggestions: &'a [String],
    subagents: &'a SubagentStatuses,
    animation_frame: usize,
    reasoning: &'a str,
    transcript_gap: bool,
}

struct StreamingViewport {
    height: u16,
    terminal_size: (u16, u16),
    resize_locked: bool,
}

impl StreamingViewport {
    fn new(height: u16, terminal_size: Size) -> Self {
        Self {
            height,
            terminal_size: (terminal_size.width, terminal_size.height),
            resize_locked: false,
        }
    }

    fn can_resize_viewport(&mut self, size: Size) -> bool {
        let size = (size.width, size.height);
        if size != self.terminal_size {
            self.terminal_size = size;
            self.resize_locked = true;
        }
        !self.resize_locked
    }
}

struct ChatContext<'a> {
    store: &'a mut ProviderStore,
    provider_index: Option<usize>,
    store_path: &'a Path,
    sessions: &'a SessionStore,
    project: &'a Path,
    tools: &'a ToolBundle,
    mcp: &'a artist_agent::mcp::McpManager,
    extensions: &'a std::sync::Arc<artist_extensions::Manager>,
    extension_control: &'a crate::extension_control::ExtensionControl,
    rules_engine: &'a RulesEngine,
    rules_handle: &'a RulesHandle,
    /// Resolved layered settings: model/reasoning overrides and denied tools.
    settings: &'a crate::settings::EffectiveSettings,
    herdr: &'a crate::herdr::Lifecycle,
}

pub struct ChatResources<'a> {
    pub sessions: &'a SessionStore,
    pub project: &'a Path,
    pub tools: &'a ToolBundle,
    pub mcp: &'a artist_agent::mcp::McpManager,
    pub extensions: &'a std::sync::Arc<artist_extensions::Manager>,
    pub extension_control: &'a crate::extension_control::ExtensionControl,
    pub rules_engine: &'a RulesEngine,
    pub rules_handle: &'a RulesHandle,
    pub settings: &'a crate::settings::EffectiveSettings,
    pub herdr: &'a crate::herdr::Lifecycle,
}

/// Compact inline viewport height: input(1) + borders(2) + status(2). The
/// splash is printed into scrollback (see `start_terminal`), never reserved
/// inside the viewport — so clearing it on the first message can't shrink and
/// re-init the viewport (which showed as a blink).
fn startup_viewport_height() -> u16 {
    1 + 2 + status_bar::HEIGHT
}

/// Draw the startup UI before loading models, extensions, indexes, or servers.
pub fn start_terminal(
    show_splash: bool,
    thinking: bool,
    extension_ids: &[String],
    needs_login: bool,
) -> Result<ratatui::DefaultTerminal> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("interactive chat requires a terminal; use -p for non-interactive prompts");
    }
    let mut terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(startup_viewport_height()),
    });
    terminal.draw(|frame| {
        if thinking {
            frame.render_widget(
                Paragraph::new(format!("  {} thinking", activation_indicator(0))),
                frame.area(),
            );
        } else {
            render_with_panel(
                frame,
                &ChatInput::default(),
                &[],
                &status_bar::StatusView::default(),
                false,
            );
        }
    })?;
    // Print the splash into scrollback above the compact input viewport rather
    // than reserving it inside — so it simply scrolls away as the chat grows and
    // never forces a viewport resize.
    if show_splash && !thinking {
        let notice_rows = u16::from(needs_login);
        terminal.insert_before(crate::startup_splash::HEIGHT + 1 + notice_rows, |buffer| {
            crate::startup_splash::render_buffer(buffer, extension_ids);
            if needs_login {
                buffer.set_string(
                    2,
                    crate::startup_splash::HEIGHT,
                    NO_PROVIDER_NOTICE,
                    Style::default().fg(crate::theme::PASTEL_YELLOW),
                );
            }
        })?;
    }
    terminal.show_cursor()?;
    Ok(terminal)
}

/// Restores the terminal modes the chat UI enables (bracketed paste + the kitty
/// keyboard-enhancement flags) on drop, so a panic or early `?` return can't
/// leave the user's shell with paste mode on and the protocol still pushed —
/// ratatui's own panic hook only restores raw mode and the alternate screen.
struct TerminalModeGuard;
impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        let _ = execute!(
            std::io::stdout(),
            DisableBracketedPaste,
            PopKeyboardEnhancementFlags
        );
    }
}

/// Runs an inline, persistent multi-turn chat. A session is created on first submission.
pub async fn run(
    mut terminal: ratatui::DefaultTerminal,
    store: &mut ProviderStore,
    provider_index: Option<usize>,
    store_path: &Path,
    resources: ChatResources<'_>,
    resumed: Option<(ActiveSession, Vec<Envelope>)>,
    initial_prompt: Option<String>,
    show_splash: bool,
) -> Result<()> {
    let sessions = resources.sessions;
    let project = resources.project;
    let tools = resources.tools;
    let mcp = resources.mcp;
    let extensions = resources.extensions;
    let extension_control = resources.extension_control;
    // Model metadata is optional startup work; fetch it only after the user
    // submits the first prompt so the input UI can appear immediately.
    let context_capacity = None;
    let mut status = StatusRuntime {
        git_branch: None,
        used_tokens: None,
        context_capacity,
        session_tokens: 0,
        fast_mode: false,
        extension_values: extensions.status_items(),
    };
    status.refresh(&store.status_bar, project);
    let keyboard_result = execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
        EnableBracketedPaste
    );
    let result = match keyboard_result {
        Ok(()) => {
            // Guard restores paste/keyboard modes on any exit, including panic.
            let _mode_guard = TerminalModeGuard;
            run_loop(
                &mut terminal,
                ChatContext {
                    store,
                    provider_index,
                    store_path,
                    sessions,
                    project,
                    tools,
                    mcp,
                    extensions,
                    extension_control,
                    rules_engine: resources.rules_engine,
                    rules_handle: resources.rules_handle,
                    settings: resources.settings,
                    herdr: resources.herdr,
                },
                resumed,
                initial_prompt,
                show_splash,
                status,
            )
            .await
        }
        Err(error) => Err(error.into()),
    };
    let viewport_bottom = terminal.get_frame().area().bottom();
    ratatui::restore();
    // Keep the final inline viewport visible and return the shell cursor to the
    // row immediately below it. Only emit a newline when the viewport reaches
    // the physical bottom and must scroll to make room for the prompt.
    if let Ok((_, height)) = ratatui::crossterm::terminal::size() {
        let last_row = height.saturating_sub(1);
        if viewport_bottom < height {
            let _ = execute!(std::io::stdout(), Show, MoveTo(0, viewport_bottom));
        } else {
            let _ = execute!(std::io::stdout(), Show, MoveTo(0, last_row), Print("\r\n"));
        }
    }
    result
}

fn extension_command_completions<'a>(
    input: &str,
    commands: &'a [artist_extensions::CommandDeclaration],
) -> Vec<&'a artist_extensions::CommandDeclaration> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('/') || trimmed.contains(char::is_whitespace) {
        return Vec::new();
    }
    commands
        .iter()
        .filter(|command| command.name.starts_with(trimmed))
        .collect()
}

fn extension_command<'a>(
    input: &'a str,
    commands: &[artist_extensions::CommandDeclaration],
) -> Option<(&'a str, &'a str)> {
    let trimmed = input.trim();
    let (name, arguments) = trimmed
        .split_once(char::is_whitespace)
        .unwrap_or((trimmed, ""));
    (!slash_commands::COMMANDS
        .iter()
        .any(|command| command.name == name)
        && commands.iter().any(|command| command.name == name))
    .then_some((name, arguments.trim_start()))
}

fn shell_command(input: &str) -> Option<&str> {
    input.strip_prefix('!').map(str::trim_start)
}

fn skill_completion_range(text: &str, cursor: usize) -> Option<std::ops::Range<usize>> {
    let prefix = text.get(..cursor)?;
    let start = prefix.rfind('$')?;
    let fragment = &prefix[start + 1..];
    if fragment.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    }) {
        Some(start..cursor)
    } else {
        None
    }
}

fn skill_completions<'a>(
    input: &ChatInput,
    skills: &'a [artist_agent::AvailableSkill],
) -> (
    Option<std::ops::Range<usize>>,
    Vec<&'a artist_agent::AvailableSkill>,
) {
    let Some(range) = skill_completion_range(&input.text, input.cursor) else {
        return (None, Vec::new());
    };
    let fragment = &input.text[range.start + 1..range.end];
    let matches = skills
        .iter()
        .filter(|skill| skill.name.starts_with(fragment))
        .collect();
    (Some(range), matches)
}

async fn run_loop(
    mut terminal: &mut ratatui::DefaultTerminal,
    mut context: ChatContext<'_>,
    resumed: Option<(ActiveSession, Vec<Envelope>)>,
    pending: Option<String>,
    mut show_splash: bool,
    mut status: StatusRuntime,
) -> Result<()> {
    let resumed_session = resumed.is_some();
    let (mut active, resumed_events) = resumed.map_or((None, Vec::new()), |(active, events)| {
        (Some(active), events)
    });
    // Full-fidelity model history (tool calls/results included), rebuilt
    // from the event log after each turn.
    let mut history: Vec<Message> = match &active {
        Some(active) => {
            context.rules_handle.restore_from_log(&resumed_events);
            artist_session::build_history(
                &resumed_events,
                &active.attachments,
                &artist_session::HistoryOptions::default(),
            )?
        }
        None => Vec::new(),
    };
    let mut input = ChatInput::default();
    let skills = artist_agent::available_skills(context.project);
    let custom_commands = crate::custom_commands::discover(context.project);
    let mcp_servers = context.mcp.server_names().await;
    let splash_extension_ids = context.extensions.extension_ids();
    let mut prompt_history =
        PromptHistory::from_prompts(artist_session::user_prompts(&resumed_events));
    let mut pending = pending.map(SubmittedPrompt::from);
    let mut queued_prompts = VecDeque::new();
    let mut viewport_height = startup_viewport_height();
    let mut viewport_floor = 3;
    let mut command_panel = Vec::new();
    // Tool selection can change during the session; keep the effective denylist
    // live rather than holding the startup settings snapshot forever.
    let mut denied_tools = context.settings.denied_tools.clone();
    let mut suggestion_index = 0usize;
    let mut suggestion_input = String::new();
    // The provider the session actually runs with: the selected account plus
    // any settings model/reasoning override, applied to a throwaway clone so
    // the override is never persisted. Rebuilt before a turn to carry a
    // freshly-refreshed token.
    let mut session_provider = context.provider_index.map(|index| {
        context
            .settings
            .apply_to(context.store.providers[index].clone())
    });
    if let Some(active) = &active {
        context.herdr.report_session(&active.session.id);
    }
    context.herdr.claim_idle();
    context
        .herdr
        .set_auth_blocked(context.provider_index.is_none());
    if resumed_session {
        let footer = footer_view(
            &context.store.status_bar,
            session_provider.as_ref(),
            context.project,
            &status,
        );
        resize_and_draw(
            &mut terminal,
            &input,
            &[],
            &footer,
            &mut viewport_height,
            viewport_floor,
            false,
            None,
        )?;
        insert_history(
            &mut terminal,
            &artist_session::replay_for_ui(&resumed_events),
            &context.extensions.tool_icons(),
        )?;
    }
    loop {
        context.extensions.update_context(|extension_context| {
            extension_context.model = session_provider.as_ref().and_then(|p| p.model.clone());
            extension_context.reasoning = session_provider
                .as_ref()
                .and_then(|p| p.reasoning_effort.clone());
        });
        status.extension_values = context.extensions.status_items();
        // Prompt execution can change any external status (notably the checked-out
        // branch), so refresh both before submission and after it returns.
        if pending.is_some() {
            status.refresh(&context.store.status_bar, context.project);
        }
        let slash_suggestions = slash_commands::completions(&input.text);
        let extension_commands = context.extensions.commands();
        let extension_suggestions = extension_command_completions(&input.text, &extension_commands);
        let custom_suggestions = crate::custom_commands::completions(&custom_commands, &input.text);
        let mcp_suggestions = slash_commands::mcp_completions(&input.text, &mcp_servers);
        let provider_suggestions: Vec<slash_commands::ArgumentCompletion> = Vec::new();
        let (skill_range, skill_suggestions) = skill_completions(&input, &skills);
        if suggestion_input != input.text {
            suggestion_index = 0;
            suggestion_input.clone_from(&input.text);
        }
        let mut suggestions = if !slash_suggestions.is_empty()
            || !custom_suggestions.is_empty()
            || !extension_suggestions.is_empty()
        {
            slash_suggestions
                .iter()
                .map(|command| format!("{}  {}", command.name, command.description))
                .chain(
                    custom_suggestions
                        .iter()
                        .map(|command| format!("{}  {}", command.name, command.description)),
                )
                .chain(
                    extension_suggestions
                        .iter()
                        .map(|command| format!("{}  {}", command.name, command.description)),
                )
                .collect()
        } else if !mcp_suggestions.is_empty() {
            mcp_suggestions.clone()
        } else {
            skill_suggestions
                .iter()
                .map(|skill| format!("${}  {}", skill.name, skill.description))
                .collect::<Vec<_>>()
        };
        suggestion_index = suggestion_index.min(suggestions.len().saturating_sub(1));
        if let Some(selected) = suggestions.get_mut(suggestion_index) {
            selected.insert_str(0, "› ");
        }
        let panel = if suggestions.is_empty() {
            &command_panel
        } else {
            &suggestions
        };
        let footer = footer_view(
            &context.store.status_bar,
            session_provider.as_ref(),
            context.project,
            &status,
        );
        // The splash is stored in scrollback until the first prompt. If Ratatui
        // clears the screen to repair a horizontal shrink, resize_and_draw puts
        // it back above the inline viewport.
        let splash_visible = show_splash && pending.is_none();
        // The splash itself lives above the viewport rather than inside it.
        let render_splash = false;
        resize_and_draw(
            &mut terminal,
            &input,
            panel,
            &footer,
            &mut viewport_height,
            viewport_floor,
            render_splash,
            splash_visible.then_some(splash_extension_ids.as_slice()),
        )?;
        if let Some(mut prompt) = pending.take() {
            show_splash = false;
            // Custom commands expand to prompt templates (once — an expanded
            // template is never re-expanded).
            if let Some(expanded) =
                crate::custom_commands::expand_invocation(&custom_commands, &prompt.content)
            {
                prompt.content = expanded;
            }
            if let Some(command) = shell_command(&prompt.content) {
                command_panel = match context.tools.bash.run_input(command).await {
                    Ok(output) => {
                        let mut lines = vec![format!("! {command}")];
                        lines.extend(output.lines().map(str::to_owned));
                        lines
                    }
                    Err(error) => vec![format!("Shell error: {error}")],
                };
            } else if let Some((name, arguments)) =
                extension_command(&prompt.content, &extension_commands)
            {
                command_panel = match context.extensions.invoke_command(name, arguments).await {
                    Ok(output) => output.lines().map(str::to_owned).collect(),
                    Err(error) => vec![format!("Error: {error:#}")],
                };
            } else if let Some(command) = slash_commands::parse(&prompt.content) {
                command_panel = match command {
                    Ok(slash_commands::ParsedCommand::Quit) => {
                        if let Some(active) = active.take() {
                            active.close().await?;
                        }
                        return Ok(());
                    }
                    Ok(slash_commands::ParsedCommand::Rewind { target, fork }) => handle_rewind(
                        &mut terminal,
                        context.sessions,
                        &mut active,
                        &mut history,
                        &mut input,
                        target,
                        fork,
                        context.herdr,
                    )
                    .await
                    .unwrap_or_else(|error| vec![format!("Error: {error:#}")]),
                    Ok(slash_commands::ParsedCommand::Compact { instructions }) => {
                        let Some(provider) = session_provider.as_ref() else {
                            command_panel = vec![NO_PROVIDER_NOTICE.into()];
                            continue;
                        };
                        let Some(active_session) = active.as_ref() else {
                            command_panel = vec!["Nothing to compact in a fresh session.".into()];
                            continue;
                        };
                        let progress = ["Compacting context…".to_owned()];
                        resize_and_draw(
                            &mut terminal,
                            &ChatInput::default(),
                            &progress,
                            &footer,
                            &mut viewport_height,
                            3,
                            false,
                            None,
                        )?;
                        match crate::compaction::compact(
                            active_session,
                            provider,
                            context.settings.compaction,
                            instructions,
                            "manual",
                            status.used_tokens,
                        )
                        .await
                        {
                            Ok(Some(result)) => {
                                let estimated_after =
                                    artist_session::compaction::estimate_messages_tokens(
                                        &result.history,
                                    );
                                history = result.history;
                                status.used_tokens = Some(estimated_after);
                                vec![format!(
                                    "Compacted {} messages ({} tokens before; ~{} retained).",
                                    result.summarized_messages,
                                    result.tokens_before,
                                    estimated_after
                                )]
                            }
                            Ok(None) => vec![
                                "Nothing to compact; the conversation fits inside the recent-context budget."
                                    .into(),
                            ],
                            Err(error) => vec![format!("Compaction failed: {error:#}")],
                        }
                    }
                    Ok(slash_commands::ParsedCommand::Fast) => {
                        if session_provider.as_ref().is_some_and(|provider| {
                            matches!(
                                provider.provider,
                                llm_provider::ProviderKind::Openai
                                    | llm_provider::ProviderKind::Chatgpt
                            )
                        }) {
                            status.fast_mode = !status.fast_mode;
                            vec![format!(
                                "Fast mode {}.",
                                if status.fast_mode {
                                    "enabled"
                                } else {
                                    "disabled"
                                }
                            )]
                        } else {
                            status.fast_mode = false;
                            vec!["Fast mode is not supported by the active provider.".into()]
                        }
                    }
                    Ok(slash_commands::ParsedCommand::Rules(action)) => handle_rules(
                        context.rules_engine,
                        context.rules_handle,
                        active.as_ref(),
                        action,
                    )
                    .await
                    .unwrap_or_else(|error| vec![format!("Error: {error:#}")]),
                    Ok(slash_commands::ParsedCommand::Sessions) => {
                        handle_sessions(context.sessions, context.project, &active)
                            .unwrap_or_else(|error| vec![format!("Error: {error:#}")])
                    }
                    Ok(slash_commands::ParsedCommand::New) => {
                        if let Some(old) = active.take() {
                            old.close().await?;
                        }
                        history.clear();
                        context.rules_handle.restore_from_log(&[]);
                        status.used_tokens = None;
                        status.session_tokens = 0;
                        status.fast_mode = false;
                        vec!["Started a fresh session — your next message begins it.".to_owned()]
                    }
                    Ok(slash_commands::ParsedCommand::Login) => match handle_login(
                        &mut terminal,
                        context.store,
                        context.store_path,
                        viewport_height,
                        &footer,
                    )
                    .await
                    {
                        Ok((lines, Some(index))) => {
                            context.provider_index = Some(index);
                            status.fast_mode = false;
                            session_provider = Some(
                                context
                                    .settings
                                    .apply_to(context.store.providers[index].clone()),
                            );
                            lines
                        }
                        Ok((lines, None)) => lines,
                        Err(error) => vec![format!("Login failed: {error:#}")],
                    },
                    Ok(slash_commands::ParsedCommand::Resume { id }) => handle_resume(
                        context.sessions,
                        context.project,
                        &mut active,
                        &mut history,
                        context.rules_handle,
                        id,
                        context.herdr,
                    )
                    .await
                    .unwrap_or_else(|error| vec![format!("Error: {error:#}")]),
                    Ok(command) => {
                        let Some(provider_index) = context.provider_index else {
                            command_panel = vec![NO_PROVIDER_NOTICE.into()];
                            continue;
                        };
                        let tools_changed = matches!(command, slash_commands::ParsedCommand::Tools);
                        let command_input = ChatInput::default();
                        match command_ui::run(
                            context.store,
                            provider_index,
                            context.store_path,
                            command,
                            &skills,
                            context.mcp,
                            &context.extensions.tool_names(),
                            &context.extensions.status_declarations(),
                            |panel| {
                                resize_and_draw(
                                    &mut terminal,
                                    &command_input,
                                    panel,
                                    &footer,
                                    &mut viewport_height,
                                    3,
                                    false,
                                    None,
                                )
                            },
                        )
                        .await
                        {
                            Ok(output) => {
                                if tools_changed {
                                    let config_root = context
                                        .store_path
                                        .parent()
                                        .context("providers path has no parent")?;
                                    denied_tools = crate::settings::load_effective(
                                        config_root,
                                        context.project,
                                        &crate::settings::Overrides::default(),
                                        &context.store.disabled_tools,
                                    )?
                                    .denied_tools;
                                }
                                if output.model_changed {
                                    if output.model_changed {
                                        status.context_capacity = output.context_capacity;
                                        status.used_tokens = None;
                                    }
                                    // `/model` is an explicit in-session override, including
                                    // when startup settings supplied model defaults. Use the
                                    // newly persisted choice directly rather than reapplying
                                    // those defaults over it.
                                    session_provider = if output.model_changed {
                                        Some(context.store.providers[provider_index].clone())
                                    } else {
                                        Some(context.settings.apply_to(
                                            context.store.providers[provider_index].clone(),
                                        ))
                                    };
                                }
                                output.lines
                            }
                            Err(error) => vec![format!("Error: {error:#}")],
                        }
                    }
                    Err(error) => vec![command_ui::format_parse_error(error)],
                };
            } else {
                command_panel.clear();
                resize_and_draw(
                    &mut terminal,
                    &ChatInput::default(),
                    &[],
                    &footer,
                    &mut viewport_height,
                    3,
                    false,
                    None,
                )?;
                let Some(provider_index) = context.provider_index else {
                    command_panel = vec![NO_PROVIDER_NOTICE.into()];
                    continue;
                };
                prompt_history.push(prompt.display.clone(), prompt.history_atoms.clone());
                // Refresh the access token at the turn boundary so a session
                // that outlives the token lifetime keeps working instead of
                // failing with an unrecoverable 401 (AUTH-1).
                if crate::refresh_if_needed(&mut context.store.providers[provider_index]).await? {
                    context
                        .store
                        .save(context.store_path)
                        .context("save refreshed ChatGPT login")?;
                }
                // Carry refreshed account credentials into the request without
                // clobbering a model/reasoning choice made via `/model`.
                session_provider = Some(provider_with_session_selection(
                    context.store.providers[provider_index].clone(),
                    session_provider.as_ref().expect("provider initialized"),
                ));
                let result = submit(
                    &mut terminal,
                    SubmitContext {
                        provider: session_provider.as_ref().expect("provider initialized"),
                        sessions: context.sessions,
                        project: context.project,
                        status_config: &context.store.status_bar,
                        tools: context.tools,
                        mcp: context.mcp,
                        extensions: context.extensions,
                        extension_control: context.extension_control,
                        herdr: context.herdr,
                        disabled_tools: &denied_tools,
                        compaction: context.settings.compaction,
                        show_splash,
                        rules_engine: context.rules_engine,
                        rules_handle: context.rules_handle,
                    },
                    &mut active,
                    &mut history,
                    &mut status,
                    prompt,
                    viewport_height,
                )
                .await?;
                viewport_height = result.viewport_height;
                // AUTH-2: the turn 401'd, so the token is stale regardless of
                // its recorded expiry. Force-refresh it now (non-blocking to the
                // rest of the loop's state) so the user's resend succeeds.
                if result.auth_expired {
                    match crate::force_refresh(&mut context.store.providers[provider_index]).await {
                        Ok(()) => match context.store.save(context.store_path) {
                            Ok(()) => insert_status(&mut terminal, "  ✓ login refreshed")?,
                            Err(error) => insert_status(
                                &mut terminal,
                                &format!("  ⚠ login refreshed but couldn't be saved: {error:#}"),
                            )?,
                        },
                        Err(error) => insert_status(
                            &mut terminal,
                            &format!("  ⚠ couldn't refresh login: {error:#} — run `artist login`"),
                        )?,
                    }
                }
                // Restore anything typed into the box mid-stream but not sent,
                // so finishing a turn no longer wipes an in-progress draft.
                if !result.leftover_input.text.is_empty() {
                    input = result.leftover_input;
                }
                for delivered in result.delivered {
                    prompt_history.push(delivered, InputAtoms::default());
                }
                queued_prompts.extend(result.queued);
                queued_prompts.extend(
                    context
                        .extension_control
                        .take_prompts()
                        .into_iter()
                        .map(SubmittedPrompt::from),
                );
                pending = queued_prompts.pop_front();
                viewport_floor = 3;
            }
            if pending.is_none() {
                queued_prompts.extend(
                    context
                        .extension_control
                        .take_prompts()
                        .into_iter()
                        .map(SubmittedPrompt::from),
                );
                pending = queued_prompts.pop_front();
            }
            status.refresh(&context.store.status_bar, context.project);
            continue;
        }
        if !event::poll(std::time::Duration::from_millis(120))? {
            continue;
        }
        match event::read()? {
            Event::Key(key)
                if key.kind == KeyEventKind::Press
                    && key.code == KeyCode::Enter
                    && !key.modifiers.contains(KeyModifiers::SHIFT)
                    && !input.text.trim().is_empty() =>
            {
                // Enter on an open suggestion menu completes the selected item
                // first, then sends it — Tab completes only.
                if !suggestions.is_empty() {
                    apply_selected_suggestion(
                        &mut input,
                        suggestion_index,
                        &slash_suggestions,
                        &custom_suggestions,
                        &extension_suggestions,
                        &provider_suggestions,
                        &mcp_suggestions,
                        &skill_range,
                        &skill_suggestions,
                    );
                }
                let display = input.text.clone();
                let history_atoms = input.atoms.clone();
                let expanded = input.take_expanded();
                pending = Some(SubmittedPrompt {
                    display,
                    content: expanded.text,
                    images: expanded.images,
                    history_atoms,
                });
            }
            Event::Key(key)
                if key.kind == KeyEventKind::Press
                    && key.code == KeyCode::Char('v')
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                let _ = clipboard::paste(&mut input, true);
            }
            Event::Key(key)
                if key.kind == KeyEventKind::Press
                    && matches!(key.code, KeyCode::Up | KeyCode::Down)
                    && !suggestions.is_empty() =>
            {
                suggestion_index = if key.code == KeyCode::Up {
                    suggestion_index
                        .checked_sub(1)
                        .unwrap_or(suggestions.len() - 1)
                } else {
                    (suggestion_index + 1) % suggestions.len()
                };
            }
            Event::Key(key)
                if key.kind == KeyEventKind::Press
                    && key.code == KeyCode::Tab
                    && !suggestions.is_empty() =>
            {
                apply_selected_suggestion(
                    &mut input,
                    suggestion_index,
                    &slash_suggestions,
                    &custom_suggestions,
                    &extension_suggestions,
                    &provider_suggestions,
                    &mcp_suggestions,
                    &skill_range,
                    &skill_suggestions,
                );
            }
            Event::Key(key)
                if key.kind == KeyEventKind::Press
                    && matches!(key.code, KeyCode::Up | KeyCode::Down)
                    && !input.text.contains('\n') =>
            {
                if let Some(prompt) =
                    prompt_history.navigate(key.code == KeyCode::Up, &input.text, &input.atoms)
                {
                    input.text = prompt.display;
                    input.atoms = prompt.atoms;
                    input.cursor = input.text.len();
                }
            }
            // First ctrl+c on a non-empty prompt clears it; make that legible
            // (otherwise it silently wipes typed text and looks like a no-op).
            Event::Key(key)
                if key.kind == KeyEventKind::Press
                    && key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                    && !input.text.is_empty() =>
            {
                input.text.clear();
                input.atoms.clear();
                input.cursor = 0;
                insert_status(&mut terminal, "  input cleared — ctrl+c again to quit")?;
            }
            Event::Key(key) if !input.handle_key(key) => {
                if let Some(active) = active.take() {
                    active.close().await?;
                }
                return Ok(());
            }

            Event::Resize(_, _) => {}
            Event::Paste(text) => input.paste(&text, true),
            _ => {}
        }
    }
}

/// `/rules`: the live rules panel and its actions. Listing shows every
/// loaded rule with armed/fired/disabled state and session hit counts;
/// `scan` retro-evaluates all rules over this session's log; `dry-run`
/// evaluates a candidate rule file without activating it.
async fn handle_rules(
    engine: &RulesEngine,
    handle: &RulesHandle,
    active: Option<&ActiveSession>,
    action: slash_commands::RulesAction<'_>,
) -> Result<Vec<String>> {
    use slash_commands::RulesAction;
    engine.reload_if_changed();
    let rules = engine.snapshot();
    let resolve = |name: &str| -> Option<artist_rules::types::RuleId> {
        let bare = artist_rules::types::RuleId(name.to_owned());
        let builtin = artist_rules::types::RuleId(format!("builtin:{name}"));
        if rules.get(&bare).is_some() {
            Some(bare)
        } else if rules.get(&builtin).is_some() {
            Some(builtin)
        } else {
            None
        }
    };
    match action {
        RulesAction::List => {
            let fired = handle.fired();
            let hits = handle.hits();
            let disabled = handle.disabled();
            let poisoned = rules.poisoned();
            let mut lines: Vec<String> = rules
                .rules
                .iter()
                .map(|compiled| {
                    let id = &compiled.rule.id;
                    let state = if poisoned.contains(id) {
                        "poisoned"
                    } else if disabled.contains(id) {
                        "disabled"
                    } else if fired.contains(id) {
                        "fired"
                    } else {
                        "armed"
                    };
                    let count = hits
                        .iter()
                        .find(|(rule, _)| rule == id)
                        .map(|(_, count)| format!(" ({count}\u{d7})"))
                        .unwrap_or_default();
                    format!("{state:>8}{count}  {id}  {}", compiled.rule.description)
                })
                .collect();
            if lines.is_empty() {
                lines.push("No rules loaded. Add markdown rules under .artist/rules/".to_owned());
            }
            for diagnostic in engine.diagnostics() {
                lines.push(format!("! {diagnostic}"));
            }
            Ok(lines)
        }
        RulesAction::Enable { rule } | RulesAction::Disable { rule } => {
            let disable = matches!(action, RulesAction::Disable { .. });
            match resolve(rule) {
                Some(id) => {
                    handle.set_disabled(id.clone(), disable);
                    Ok(vec![format!(
                        "rule {id} {}",
                        if disable { "disabled" } else { "enabled" }
                    )])
                }
                None => Ok(vec![format!("unknown rule: {rule}")]),
            }
        }
        RulesAction::Scan => {
            let Some(active) = active else {
                return Ok(vec!["No session yet — nothing to scan.".to_owned()]);
            };
            active.recorder.flush().await;
            let events = active.events()?;
            let findings = artist_rules::retro::scan(&rules, &events);
            if findings.is_empty() {
                return Ok(vec![
                    "No rule matches in this session's history.".to_owned(),
                ]);
            }
            let mut lines: Vec<String> = findings
                .iter()
                .take(15)
                .map(|finding| {
                    format!(
                        "{}  @{}  \"{}\"",
                        finding.rule, finding.seq, finding.excerpt
                    )
                })
                .collect();
            if findings.len() > 15 {
                lines.push(format!("… and {} more", findings.len() - 15));
            }
            let mut by_rule: Vec<(String, u64, Vec<String>)> = Vec::new();
            for finding in &findings {
                match by_rule
                    .iter_mut()
                    .find(|(rule, ..)| *rule == finding.rule.0)
                {
                    Some((_, count, examples)) => {
                        *count += 1;
                        if examples.len() < 3 {
                            examples.push(finding.excerpt.clone());
                        }
                    }
                    None => {
                        by_rule.push((finding.rule.0.clone(), 1, vec![finding.excerpt.clone()]))
                    }
                }
            }
            for (rule, count, examples) in by_rule {
                active.recorder.record(artist_session::RuleRetroFindings {
                    rule,
                    count,
                    examples,
                });
            }
            Ok(lines)
        }
        RulesAction::DryRun { file } => {
            let Some(active) = active else {
                return Ok(vec!["No session yet — nothing to scan against.".to_owned()]);
            };
            let rule = artist_rules::declarative::parse(std::path::Path::new(file))
                .map_err(|error| anyhow::anyhow!(error))?;
            let id = rule.id.clone();
            let candidate = artist_rules::matcher::RuleSet::compile(vec![rule]);
            active.recorder.flush().await;
            let events = active.events()?;
            let findings = artist_rules::retro::scan(&candidate, &events);
            let mut lines = vec![format!(
                "{id}: would have fired {}\u{d7} this session (not activated)",
                findings.len()
            )];
            lines.extend(
                findings
                    .iter()
                    .take(5)
                    .map(|finding| format!("  @{}  \"{}\"", finding.seq, finding.excerpt)),
            );
            Ok(lines)
        }
    }
}

/// `/rewind [n] [fork]`: list rewind targets, or mask history back to just
/// before the nth-most-recent user turn (append-only — nothing is deleted),
/// optionally forking into a new session that shares the prefix. The chosen
/// turn's text is pre-filled into the input for editing.
async fn handle_rewind(
    terminal: &mut ratatui::DefaultTerminal,
    sessions: &SessionStore,
    active: &mut Option<ActiveSession>,
    history: &mut Vec<Message>,
    input: &mut ChatInput,
    target: Option<usize>,
    fork: bool,
    herdr: &crate::herdr::Lifecycle,
) -> Result<Vec<String>> {
    let Some(current) = active.as_ref() else {
        return Ok(vec!["No session yet — nothing to rewind.".to_owned()]);
    };
    current.recorder.flush().await;
    let events = current.events()?;
    let targets = artist_session::rewind_targets(&events);
    if targets.is_empty() {
        return Ok(vec!["No user turns to rewind to.".to_owned()]);
    }
    let Some(n) = target else {
        let mut lines: Vec<String> = targets
            .iter()
            .rev()
            .take(10)
            .enumerate()
            .map(|(index, (_, display))| {
                let mut preview = display.lines().next().unwrap_or("").to_owned();
                if preview.chars().count() > 70 {
                    preview = preview.chars().take(69).chain(['\u{2026}']).collect();
                }
                format!("{}  {}", index + 1, preview)
            })
            .collect();
        lines.push("Rewind with /rewind <n>, or fork with /rewind <n> fork".to_owned());
        return Ok(lines);
    };
    if n == 0 || n > targets.len() {
        return Ok(vec![format!(
            "No such turn: {n} (1..{} available)",
            targets.len()
        )]);
    }
    let (seq, display) = targets[targets.len() - n].clone();
    let to_seq = seq.saturating_sub(1);
    let marker;
    if fork {
        let forked = sessions.fork(&current.session.id, to_seq)?;
        marker = format!(
            "  \u{23EA} forked session {} from {} (before \"{}\")",
            forked.session.id,
            current.session.id,
            display.lines().next().unwrap_or("")
        );
        let events = forked.events()?;
        *history = artist_session::build_history(
            &events,
            &forked.attachments,
            &artist_session::HistoryOptions::default(),
        )?;
        if let Some(old) = active.replace(forked) {
            old.close().await?;
        }
        if let Some(active) = active.as_ref() {
            herdr.report_session(&active.session.id);
        }
    } else {
        current.recorder.record(artist_session::HistoryRewind {
            to_seq,
            reason: "user rewind".to_owned(),
            by: "user".to_owned(),
        });
        current.recorder.flush().await;
        let events = current.events()?;
        current.memory.reload_from_events(&events)?;
        current.provider_context.restore_from_events(&events).await;
        *history = artist_session::build_history(
            &events,
            &current.attachments,
            &artist_session::HistoryOptions::default(),
        )?;
        marker = format!(
            "  \u{23EA} rewound to before \"{}\" \u{2014} history after this point is masked, not deleted",
            display.lines().next().unwrap_or("")
        );
    }
    insert_status(terminal, &marker)?;
    input.text = display;
    input.cursor = input.text.len();
    input.atoms.clear();
    Ok(Vec::new())
}

/// `/sessions`: list this project's sessions, newest first, marking the active
/// one. Read-only — switching is `/resume <id>`.
fn handle_sessions(
    sessions: &SessionStore,
    project: &Path,
    active: &Option<ActiveSession>,
) -> Result<Vec<String>> {
    let mut list = sessions.list_project(project)?;
    if list.is_empty() {
        return Ok(vec!["No sessions yet for this project.".to_owned()]);
    }
    // list_project is oldest-first; show newest first.
    list.reverse();
    let current = active.as_ref().map(|active| active.session.id.as_str());
    let mut lines: Vec<String> = list
        .iter()
        .take(15)
        .map(|session| {
            let marker = if current == Some(session.id.as_str()) {
                "*"
            } else {
                " "
            };
            let label = session.label.as_deref().unwrap_or("(no label)");
            let mut preview = label.lines().next().unwrap_or("").to_owned();
            if preview.chars().count() > 60 {
                preview = preview.chars().take(59).chain(['\u{2026}']).collect();
            }
            format!("{marker} {}  {preview}", session.id)
        })
        .collect();
    lines.push("Switch with /resume <id>.".to_owned());
    Ok(lines)
}

/// `/resume [id]`: switch the active session to another one by id. Without an
/// id, list the candidates. The current session is flushed and released first.
async fn handle_resume(
    sessions: &SessionStore,
    project: &Path,
    active: &mut Option<ActiveSession>,
    history: &mut Vec<Message>,
    rules_handle: &RulesHandle,
    id: Option<&str>,
    herdr: &crate::herdr::Lifecycle,
) -> Result<Vec<String>> {
    let Some(id) = id else {
        return handle_sessions(sessions, project, active);
    };
    if active.as_ref().map(|active| active.session.id.as_str()) == Some(id) {
        return Ok(vec![format!("Already on session {id}.")]);
    }
    let (opened, events) = sessions
        .open(id)
        .with_context(|| format!("no such session: {id}"))?;
    rules_handle.restore_from_log(&events);
    *history = artist_session::build_history(
        &events,
        &opened.attachments,
        &artist_session::HistoryOptions::default(),
    )?;
    let label = opened.session.label.clone();
    if let Some(old) = active.replace(opened) {
        old.close().await?;
    }
    herdr.report_session(id);
    Ok(vec![format!(
        "Resumed session {id}{}.",
        label
            .as_deref()
            .map(|label| format!(" — {label}"))
            .unwrap_or_default()
    )])
}

async fn handle_provider(
    terminal: &mut ratatui::DefaultTerminal,
    store: &mut ProviderStore,
    store_path: &Path,
    current: &mut usize,
    action: slash_commands::ProviderAction<'_>,
    viewport_height: u16,
    settings: &crate::settings::EffectiveSettings,
) -> Result<(Vec<String>, bool)> {
    use slash_commands::ProviderAction;
    if matches!(action, ProviderAction::List) {
        return Ok((crate::provider_commands::list_lines(store, *current), false));
    }
    finish_inline(terminal)?;
    let _ = execute!(
        std::io::stdout(),
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste
    );
    ratatui::restore();
    let outcome: Result<(Vec<String>, bool)> = async {
        match action {
            ProviderAction::Add { kind } => {
                crate::provider_commands::add_kind(store, kind)?;
                store.save(store_path)?;
                Ok((vec!["Provider added and saved.".into()], false))
            }
            ProviderAction::Edit { id } => {
                crate::provider_commands::edit(store, id)?;
                store.save(store_path)?;
                Ok((vec!["Provider updated and saved.".into()], false))
            }
            ProviderAction::Remove { id } => {
                if store.providers.len() == 1 {
                    anyhow::bail!("cannot remove the only provider while chat is running");
                }
                let active_id = store.providers.get(*current).map(|p| p.id.clone());
                crate::provider_commands::remove(store, id)?;
                store.save(store_path)?;
                *current = active_id
                    .and_then(|id| store.providers.iter().position(|p| p.id == id))
                    .unwrap_or(0);
                Ok((vec!["Provider removal completed.".into()], true))
            }
            ProviderAction::Set { id } => {
                *current = crate::provider_commands::set_default(store, id)?;
                store.save(store_path)?;
                Ok((
                    vec![format!("Switched to {}.", store.providers[*current].name)],
                    true,
                ))
            }
            ProviderAction::Test { id } => {
                let index = crate::provider_commands::select_index(store, id)?;
                if crate::refresh_if_needed(&mut store.providers[index]).await? {
                    store.save(store_path)?;
                }
                let provider = settings.apply_to(store.providers[index].clone());
                crate::test_provider::test(&provider).await?;
                Ok((vec![format!("{}: OK", store.providers[index].name)], false))
            }
            ProviderAction::List => unreachable!(),
        }
    }
    .await;
    *terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(viewport_height),
    });
    let _ = execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
        EnableBracketedPaste
    );
    terminal.show_cursor()?;
    outcome
}

/// `/accounts [id]`: list logged-in accounts, or return the index to switch to.
/// Returns the panel plus `Some(new_index)` when a switch was requested.
fn handle_accounts(
    store: &mut ProviderStore,
    store_path: &Path,
    current: usize,
    id: Option<&str>,
) -> Result<(Vec<String>, Option<usize>)> {
    let Some(id) = id else {
        let mut lines = crate::provider_commands::list_lines(store, current);
        lines.push("Switch with /accounts <id>, or add one with /login.".to_owned());
        return Ok((lines, None));
    };
    match store
        .providers
        .iter()
        .position(|provider| provider.id.as_str() == id)
    {
        Some(index) => {
            store.default_provider = Some(store.providers[index].id.clone());
            store.save(store_path)?;
            let line = if index == current {
                format!("Already using account {id}; saved as default.")
            } else {
                format!(
                    "Switched to {} ({}), and saved as default.",
                    store.providers[index].id.as_str(),
                    store.providers[index].name
                )
            };
            Ok((vec![line], Some(index)))
        }
        None => Ok((
            vec![format!(
                "No such account: {id} — see /accounts for the list."
            )],
            None,
        )),
    }
}

/// `/login`: select an authentication method and provider for an additional
/// account. The inline viewport and its input modes are suspended for the flow,
/// then restored.
async fn handle_login(
    terminal: &mut ratatui::DefaultTerminal,
    store: &mut ProviderStore,
    store_path: &Path,
    mut viewport_height: u16,
    footer: &status_bar::StatusView,
) -> Result<(Vec<String>, Option<usize>)> {
    let input = ChatInput::default();
    let mut draw = |panel: &[String]| {
        resize_and_draw(
            terminal,
            &input,
            panel,
            footer,
            &mut viewport_height,
            3,
            false,
            None,
        )
    };
    let selected = crate::login::select(&mut draw)?;
    let previous = store.clone();
    let before = store.providers.len();
    let attempted: Result<Option<usize>> = if let Some(provider) = selected {
        // Only credential entry / browser OAuth leaves inline mode.
        finish_inline(terminal)?;
        let _ = execute!(
            std::io::stdout(),
            PopKeyboardEnhancementFlags,
            DisableBracketedPaste
        );
        ratatui::restore();
        let credential_result = crate::login::execute(provider, store).await;
        *terminal = ratatui::init_with_options(TerminalOptions {
            viewport: Viewport::Inline(viewport_height),
        });
        let _ = execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
            EnableBracketedPaste
        );
        terminal.show_cursor()?;
        async {
            credential_result?;
            let index = (store.providers.len() > before).then_some(store.providers.len() - 1);
            if let Some(index) = index
                && store.providers[index].model.is_none()
            {
                let mut draw = |panel: &[String]| {
                    resize_and_draw(
                        terminal,
                        &input,
                        panel,
                        footer,
                        &mut viewport_height,
                        3,
                        false,
                        None,
                    )
                };
                command_ui::select_model(&mut store.providers[index], &mut draw).await?;
            }
            Ok(index)
        }
        .await
    } else {
        Err(anyhow::anyhow!("login cancelled"))
    };
    let outcome = finish_login_transaction(store, previous, store_path, attempted);
    match outcome {
        Ok(Some(index)) => Ok((
            vec!["Logged in and selected for this session.".to_owned()],
            Some(index),
        )),
        Ok(None) => Ok((vec!["Login completed.".to_owned()], None)),
        Err(error) => Ok((vec![format!("Login failed: {error:#}")], None)),
    }
}

fn finish_login_transaction(
    store: &mut ProviderStore,
    previous: ProviderStore,
    store_path: &Path,
    attempted: Result<Option<usize>>,
) -> Result<Option<usize>> {
    match attempted {
        Ok(index) => {
            if let Err(error) = store.save(store_path) {
                *store = previous;
                return Err(error);
            }
            Ok(index)
        }
        Err(error) => {
            *store = previous;
            Err(error)
        }
    }
}

fn resize_and_draw(
    terminal: &mut ratatui::DefaultTerminal,
    input: &ChatInput,
    panel: &[String],
    footer: &status_bar::StatusView,
    viewport_height: &mut u16,
    viewport_floor: u16,
    show_splash: bool,
    restore_scrollback_splash: Option<&[String]>,
) -> Result<()> {
    // A command panel hides the splash (see render_with_panel); keep the height
    // math consistent so no blank splash rows are reserved behind the panel.
    let show_splash = show_splash && panel.is_empty();
    let terminal_size = terminal.size()?;
    let previous_width = terminal.get_frame().area().width;
    let width_changed = previous_width != terminal_size.width;
    let width_shrank = terminal_size.width < previous_width;
    let width = terminal_size.width.saturating_sub(2).max(1);
    let panel_height = if panel.is_empty() {
        0
    } else {
        panel.len() as u16 + 2
    };
    let status_height = footer.height(terminal.size()?.width);
    let desired = input
        .visual_lines(width)
        .saturating_add(2)
        .saturating_add(panel_height)
        .saturating_add(status_height)
        .saturating_add(if show_splash {
            crate::startup_splash::HEIGHT + 1
        } else {
            0
        })
        .max(viewport_floor)
        // Never reserve an inline viewport taller than the terminal itself,
        // which ratatui cannot place (garbled reservation / scroll).
        .min(terminal.size()?.height);
    if width_changed {
        // A terminal width change only changes the existing viewport's area.
        // Do not recreate `Viewport::Inline`: terminal reflow can commit the
        // previous composer rows into scrollback, producing duplicate boxes.
        terminal.autoresize()?;
        *viewport_height = desired;
        terminal.draw(|frame| render_with_panel(frame, input, panel, footer, show_splash))?;
        if width_shrank && let Some(extension_ids) = restore_scrollback_splash {
            terminal.insert_before(crate::startup_splash::HEIGHT + 1, |buffer| {
                crate::startup_splash::render_buffer(buffer, extension_ids);
            })?;
        }
        terminal.show_cursor()?;
    } else if desired != *viewport_height {
        *viewport_height = desired;
        execute!(std::io::stdout(), BeginSynchronizedUpdate)?;
        clear_inline(terminal)?;
        *terminal = ratatui::init_with_options(TerminalOptions {
            viewport: Viewport::Inline(desired),
        });
        // Reinitializing ratatui resets terminal modes, including bracketed paste.
        // Restore it so terminal-provided image paths continue to arrive as
        // `Event::Paste` rather than being typed into the prompt.
        execute!(std::io::stdout(), EnableBracketedPaste)?;
        terminal.draw(|frame| render_with_panel(frame, input, panel, footer, show_splash))?;
        terminal.show_cursor()?;
        execute!(std::io::stdout(), EndSynchronizedUpdate)?;
    } else {
        terminal.draw(|frame| render_with_panel(frame, input, panel, footer, show_splash))?;
    }
    Ok(())
}

fn collect_messages(
    messages: Vec<String>,
    queue: &mut SteeringQueue,
    delivered: &mut Vec<PendingDelivery>,
) -> bool {
    let selected = queue.selected();
    for message in messages {
        let display = queue
            .mark_delivered(&message)
            .unwrap_or_else(|| message.clone());
        delivered.push(PendingDelivery {
            display,
            content: message,
        });
    }
    selected.is_some() && queue.selected().is_none()
}

fn collect_delivered(
    handle: &artist_agent::SteeringHandle,
    queue: &mut SteeringQueue,
    delivered: &mut Vec<PendingDelivery>,
) -> bool {
    collect_messages(handle.take_delivered(), queue, delivered)
}

async fn submit(
    terminal: &mut ratatui::DefaultTerminal,
    context: SubmitContext<'_>,
    session: &mut Option<ActiveSession>,
    history: &mut Vec<Message>,
    status: &mut StatusRuntime,
    prompt: SubmittedPrompt,
    viewport_height: u16,
) -> Result<SubmitResult> {
    let started = std::time::Instant::now();
    let first_turn = history.is_empty();
    let active = match session {
        Some(value) => value,
        None => session.insert(
            context
                .sessions
                .create(context.project, Some(&prompt.display))?,
        ),
    };
    context.herdr.report_session(&active.session.id);
    let turn_lifecycle = context.herdr.start_turn();
    // Rules hot-reload between turns; the run holds the snapshot.
    context.rules_engine.reload_if_changed();
    let rule_set = context.rules_engine.snapshot();
    let agent_input_probe = clipboard::agent_input(&prompt)?;
    // A resumed session has no in-memory usage/capacity yet. Resolve capacity
    // before its first new turn so threshold compaction still works after a
    // process restart; fresh sessions keep the existing concurrent fetch.
    if status.context_capacity.is_none()
        && !history.is_empty()
        && let Ok(catalog) = models::catalog(context.provider).await
    {
        status.context_capacity = catalog
            .iter()
            .find(|model| Some(&model.slug) == context.provider.model.as_ref())
            .and_then(|model| model.effective_context_window());
    }
    let projected_tokens = crate::compaction::projected_context_tokens(
        history,
        status.used_tokens,
        &prompt.content,
        prompt.images.len(),
    );
    if let Some(capacity) = status.context_capacity
        && crate::compaction::should_compact(projected_tokens, capacity, context.compaction)
    {
        insert_status(terminal, "  compacting context…")?;
        match crate::compaction::compact(
            active,
            context.provider,
            context.compaction,
            None,
            "threshold",
            status.used_tokens,
        )
        .await
        {
            Ok(Some(result)) => {
                let estimated_after =
                    artist_session::compaction::estimate_messages_tokens(&result.history);
                *history = result.history;
                status.used_tokens = Some(estimated_after);
                insert_status(
                    terminal,
                    &format!(
                        "  compacted {} messages · ~{} tokens retained",
                        result.summarized_messages, estimated_after
                    ),
                )?;
            }
            Ok(None) => {}
            Err(error) => insert_status(
                terminal,
                &format!("  ⚠ automatic compaction failed: {error:#}"),
            )?,
        }
    }
    if context.show_splash {
        // Add separation only when moving the splash into scrollback. The live
        // startup layout already reserves its own gap above the input box.
        let extension_ids = context.extensions.extension_ids();
        terminal.insert_before(crate::startup_splash::HEIGHT + 1, |buffer| {
            crate::startup_splash::render_buffer(buffer, &extension_ids);
        })?;
    } else if !first_turn {
        insert_blank(terminal)?;
    }
    insert_message(terminal, &prompt.display)?;
    let empty_input = ChatInput::default();
    let mut footer = footer_view(
        context.status_config,
        Some(context.provider),
        context.project,
        status,
    );
    terminal.draw(|frame| render_with_panel(frame, &empty_input, &[], &footer, false))?;
    terminal.show_cursor()?;
    // Fetch context capacity concurrently on the first turn for the optional
    // status readout.
    let mut catalog_fut = status
        .context_capacity
        .is_none()
        .then(|| Box::pin(models::catalog(context.provider)));
    let mut response = String::new();
    let mut visible = String::new();
    let mut reasoning = String::new();
    let mut response_started = false;
    let mut response_renderer = crate::response_output::Renderer::default();
    let mut response_since_tool = false;
    let mut transcript_gap = false;
    let mut tools = ToolUi::with_icons(context.extensions.tool_icons());
    let mut subagents = SubagentStatuses::default();
    let mut subagent_launch_calls = HashSet::new();
    // Keep the existing viewport on entry so submitting does not consume the screen.
    let terminal_size = terminal.size()?;
    let mut stream_viewport = StreamingViewport::new(viewport_height, terminal_size);
    let mut phase = "thinking";
    let mut steering = SteeringQueue::default();
    let steering_handle = artist_agent::SteeringHandle::default();
    context
        .extension_control
        .set_steering(Some(steering_handle.clone()));
    let task_steering = steering_handle.clone();
    let mut delivered_steering: Vec<String> = Vec::new();
    let mut pending_delivered = Vec::new();
    let mut steering_input = ChatInput::default();
    let mut deferred_commands = Vec::new();
    let mut suggestion_index = 0usize;
    let mut cancelled = false;
    let mut animation_frame = 0;
    let cancel = CancellationToken::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let task_provider = context.provider.clone();
    let task_prompt = agent_input_probe;
    let task_tools = context.tools.clone();
    let task_mcp = context.mcp.clone();
    let task_disabled_tools = context.disabled_tools.to_vec();
    let task_extensions = context.extensions.clone();
    let event_extensions = task_extensions.clone();
    let lifecycle_extensions = task_extensions.clone();
    task_extensions
        .update_context(|value| value.agent_state = serde_json::json!({"state":"thinking"}));
    let _ = task_extensions.publish(artist_extensions::Event {
        kind: "state_transition".into(),
        payload: serde_json::json!({"state":"thinking"}),
    });
    let task_handles = artist_agent::SessionHandles {
        steering: task_steering,
        rules: context.rules_handle.clone(),
        rule_set,
        recorder: active.recorder.clone(),
        memory: std::sync::Arc::new(active.memory.clone()),
        conversation_id: active.session.id.clone(),
        provider_context: active.provider_context.clone(),
        effective_context_window: status.context_capacity,
        fast_mode: status.fast_mode,
        lifecycle: turn_lifecycle.emitter(),
        cancel: cancel.clone(),
    };
    let task = tokio::spawn(async move {
        artist_agent::stream_chat(
            &task_provider,
            &task_prompt,
            artist_agent::ToolContext {
                native: &task_tools,
                mcp: &task_mcp,
                extensions: Some(&task_extensions),
                disabled: &task_disabled_tools,
            },
            task_handles,
            |event| {
                crate::publish_prompt_event(&event_extensions, &event);
                tx.send(event)
                    .map_err(|_| anyhow::anyhow!("chat UI closed"))
            },
        )
        .await
    });
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(120));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    draw_streaming(
        terminal,
        &status_line(phase, started.elapsed(), animation_frame),
        &footer,
        StreamingControls {
            input: &steering_input,
            steering: &steering,
            suggestions: &[],
            subagents: &subagents,
            animation_frame,
            reasoning: &reasoning,
            transcript_gap: false,
        },
        &mut stream_viewport,
    )?;
    while !task.is_finished() || !rx.is_empty() {
        let slash_suggestions = slash_commands::completions(&steering_input.text);
        suggestion_index = suggestion_index.min(slash_suggestions.len().saturating_sub(1));
        let suggestions = slash_suggestions
            .iter()
            .enumerate()
            .map(|(index, command)| {
                format!(
                    "{}{}  {}",
                    if index == suggestion_index {
                        "› "
                    } else {
                        ""
                    },
                    command.name,
                    command.description
                )
            })
            .collect::<Vec<_>>();
        tokio::select! {
            // The context-size fetch resolves concurrently; update the readout
            // when it lands. Disabled once done via the `if` guard.
            catalog = async { catalog_fut.as_mut().unwrap().await }, if catalog_fut.is_some() => {
                catalog_fut = None;
                status.context_capacity = catalog.ok().and_then(|catalog| {
                    catalog
                        .iter()
                        .find(|model| Some(&model.slug) == context.provider.model.as_ref())
                        .and_then(|model| model.effective_context_window())
                });
                footer = footer_view(
                    context.status_config,
                    Some(context.provider),
                    context.project,
                    status,
                );
            }
            _ = ticker.tick() => {
                animation_frame = animation_frame.wrapping_add(1);
                if context.extension_control.take_stop() {
                    cancel.cancel();
                    cancelled = true;
                }
                while event::poll(std::time::Duration::ZERO)? {
                    match event::read()? {
                        Event::Key(key) if key.kind == KeyEventKind::Press
                            && (key.code == KeyCode::Esc
                                || (key.code == KeyCode::Char('c')
                                    && key.modifiers.contains(KeyModifiers::CONTROL))) =>
                        {
                            // Cooperative cancel: the driver's select! arm
                            // returns RunOutcome::Cancelled and records
                            // run.finished — no abandoned MCP calls, no
                            // partial-turn loss.
                            cancel.cancel();
                            cancelled = true;
                            break;
                        }
                        Event::Key(key) if key.kind == KeyEventKind::Press
                            && key.code == KeyCode::Char('v')
                            && key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            let _ = clipboard::paste(&mut steering_input, false);
                        }
                        Event::Key(key) if key.kind == KeyEventKind::Press
                            && key.code == KeyCode::Enter
                            && !key.modifiers.contains(KeyModifiers::SHIFT)
                            && !steering_input.text.trim().is_empty() =>
                        {
                            if !slash_suggestions.is_empty() {
                                steering_input.text = slash_suggestions[suggestion_index].name.into();
                                steering_input.cursor = steering_input.text.len();
                            }
                            let display = steering_input.text.clone();
                            let history_atoms = steering_input.atoms.clone();
                            let expanded = steering_input.take_expanded();
                            let prompt = SubmittedPrompt {
                                display: display.clone(),
                                content: expanded.text,
                                images: expanded.images,
                                history_atoms,
                            };
                            if prompt.content.trim_start().starts_with('/') {
                                deferred_commands.push(prompt);
                            } else {
                                let applied = if let Some(index) = steering.selected() {
                                    let mutation = steering_handle
                                        .edit_pending(index, prompt.content.clone());
                                    collect_messages(
                                        mutation.delivered,
                                        &mut steering,
                                        &mut pending_delivered,
                                    );
                                    mutation.applied
                                } else {
                                    steering_handle.enqueue(prompt.content.clone());
                                    true
                                };
                                if applied {
                                    steering.submit(
                                        display,
                                        prompt.content,
                                        prompt.images,
                                        prompt.history_atoms,
                                    );
                                }
                            }
                        }
                        Event::Key(key) if key.kind == KeyEventKind::Press
                            && matches!(key.code, KeyCode::Up | KeyCode::Down)
                            && !slash_suggestions.is_empty() =>
                        {
                            suggestion_index = if key.code == KeyCode::Up {
                                suggestion_index.checked_sub(1).unwrap_or(slash_suggestions.len() - 1)
                            } else {
                                (suggestion_index + 1) % slash_suggestions.len()
                            };
                        }
                        Event::Key(key) if key.kind == KeyEventKind::Press
                            && key.code == KeyCode::Tab
                            && !slash_suggestions.is_empty() =>
                        {
                            steering_input.text = slash_suggestions[suggestion_index].name.into();
                            steering_input.cursor = steering_input.text.len();
                        }
                        Event::Key(key) if key.kind == KeyEventKind::Press
                            && matches!(key.code, KeyCode::Up | KeyCode::Down) =>
                        {
                            if let Some(value) = steering.navigate(
                                key.code == KeyCode::Up,
                                &steering_input.text,
                                &steering_input.atoms,
                            ) {
                                steering_input.text = value.display;
                                steering_input.atoms = value.atoms;
                                steering_input.cursor = steering_input.text.len();
                            }
                        }
                        Event::Key(key) if key.kind == KeyEventKind::Press
                            && matches!(key.code, KeyCode::Backspace | KeyCode::Delete)
                            && steering.selected().is_some() =>
                        {
                            let index = steering.selected().expect("selected steering");
                            let mutation = steering_handle.remove_pending(index);
                            collect_messages(
                                mutation.delivered,
                                &mut steering,
                                &mut pending_delivered,
                            );
                            if mutation.applied {
                                steering.remove_selected();
                            }
                            steering_input.text.clear();
                            steering_input.atoms.clear();
                            steering_input.cursor = 0;
                        }
                        Event::Key(key) => { steering_input.handle_key(key); }
                        Event::Paste(text) => steering_input.paste(&text, false),
                        _ => {}
                    }
                }
                if cancelled { break; }
            }
            event = rx.recv() => if let Some(event) = event {
                match event {
                    artist_agent::PromptEvent::TextDelta(delta) => {
                        phase = "responding";
                        if !reasoning.is_empty() {
                            if transcript_gap {
                                insert_blank(terminal)?;
                            }
                            insert_reasoning(terminal, &reasoning)?;
                            reasoning.clear();
                            transcript_gap = false;
                        }
                        if !response_started {
                            response_started = true;
                        }
                        response.push_str(&delta);
                        visible.push_str(&delta);
                        response_since_tool = true;
                        let width = usize::from(terminal.size()?.width.saturating_sub(4).max(1));
                        while let Some(line) = take_visible_line(&mut visible, width) {
                            insert_response(terminal, &line, &mut response_renderer)?;
                            transcript_gap = true;
                        }
                    }
                    artist_agent::PromptEvent::ReasoningSummaryDelta(delta) => {
                        phase = "thinking";
                        reasoning.push_str(&delta);
                    }
                    artist_agent::PromptEvent::ToolCall { id, name, arguments } => {
                        phase = "working";
                        if response_since_tool {
                            if !visible.is_empty() {
                                insert_response(terminal, &visible, &mut response_renderer)?;
                                visible.clear();
                            }
                            insert_blank(terminal)?;
                            transcript_gap = false;
                            response_since_tool = false;
                        }
                        if !reasoning.is_empty() {
                            if transcript_gap {
                                insert_blank(terminal)?;
                            }
                            insert_reasoning(terminal, &reasoning)?;
                            reasoning.clear();
                            transcript_gap = false;
                        }
                        let is_subagent_launch =
                            name == "subagent" && subagent_ui::is_launch(&arguments);
                        let title = if is_subagent_launch {
                            subagent_launch_calls.insert(id.clone());
                            tools.start_silent(id, &name, &arguments);
                            None
                        } else {
                            tools.start(id, &name, &arguments)
                        };
                        if let Some(title) = title {
                            insert_tool_line(
                                terminal,
                                &title.text,
                                title.title.as_ref(),
                                true,
                                false,
                                title.icon.as_deref(),
                            )?;
                            transcript_gap = true;
                        }
                    }
                    artist_agent::PromptEvent::ToolExecutionStart { .. } => phase = "working",
                    artist_agent::PromptEvent::ToolResult { id, content, images, .. } => {
                        phase = "thinking";
                        let is_subagent_launch = subagent_launch_calls.remove(&id);
                        let silent_running = is_subagent_launch
                            && subagent_ui::is_running_output(&content);
                        let output = tools.output(
                            &id,
                            if silent_running { "" } else { &content },
                        );
                        for line in output.lines {
                            insert_tool_line(
                                terminal,
                                &line.text,
                                line.title.as_ref(),
                                line.first,
                                line.is_diff,
                                line.icon.as_deref(),
                            )?;
                            transcript_gap = true;
                        }
                        if images > 0 {
                            insert_tool_line(
                                terminal,
                                &format!("[{images} image result(s) not shown]"),
                                None,
                                false,
                                false,
                                None,
                            )?;
                            transcript_gap = true;
                        }
                        if collect_delivered(
                            &steering_handle,
                            &mut steering,
                            &mut pending_delivered,
                        ) {
                            steering_input.text.clear();
                            steering_input.atoms.clear();
                            steering_input.cursor = 0;
                        }
                        if output.batch_complete {
                            if silent_running {
                                transcript_gap = true;
                            } else {
                                insert_blank(terminal)?;
                                transcript_gap = false;
                            }
                            for message in pending_delivered.drain(..) {
                                insert_message(terminal, &message.display)?;
                                transcript_gap = true;
                                active.recorder.record(SteeringDelivered {
                                    content: message.content.clone(),
                                    after_internal_call_id: id.clone(),
                                });
                                delivered_steering.push(message.content);
                            }
                        }
                    }
                    artist_agent::PromptEvent::SubagentStarted { id, role, prompt } => {
                        phase = "working";
                        subagents.start_card(id, role, prompt);
                        transcript_gap = true;
                    }
                    artist_agent::PromptEvent::SubagentEvent { id, event } => {
                        phase = "working";
                        subagents.event(&id, &event);
                    }
                    artist_agent::PromptEvent::SubagentFinished { id, outcome } => {
                        if let Some(status) = subagents.settle(&id, outcome) {
                            subagent_ui::insert_settled(terminal, status)?;
                            transcript_gap = true;
                        }
                    }
                    artist_agent::PromptEvent::CompletionUsage { total_tokens } => {
                        if total_tokens > 0 {
                            status.used_tokens = Some(total_tokens);
                            status.session_tokens += total_tokens;
                        }
                        footer = footer_view(
                            context.status_config,
                            Some(context.provider),
                            context.project,
                            status,
                        );
                    }
                    artist_agent::PromptEvent::RuleFired { rule, matched } => {
                        // The aborted partial output never entered the model's
                        // context; drop it from the pending buffers too.
                        // (Already-flushed scrollback lines remain — full
                        // clean-rewind rendering lands with the rules UX pass.)
                        phase = "rewinding";
                        visible.clear();
                        reasoning.clear();
                        response.clear();
                        response_renderer.reset();
                        response_started = false;
                        response_since_tool = false;
                        let excerpt: String = matched.chars().take(60).collect();
                        insert_status(
                            terminal,
                            &format!("  ⚠ rule {rule} fired on \"{excerpt}\" — rewound, retrying"),
                        )?;
                        transcript_gap = true;
                    }
                }
            }
        }
        draw_streaming(
            terminal,
            &status_line(phase, started.elapsed(), animation_frame),
            &footer,
            StreamingControls {
                input: &steering_input,
                steering: &steering,
                suggestions: &suggestions,
                subagents: &subagents,
                animation_frame,
                reasoning: &reasoning,
                transcript_gap,
            },
            &mut stream_viewport,
        )?;
    }
    let stream_result = if cancelled {
        let _ = task.await;
        None
    } else {
        Some(task.await.context("join Artist agent"))
    };
    let unfinished_subagents = subagents.finish_turn(cancelled);
    turn_lifecycle.finish(cancelled);
    lifecycle_extensions
        .update_context(|value| value.agent_state = serde_json::json!({"state":"idle"}));
    let _ = lifecycle_extensions.publish(artist_extensions::Event {
        kind: "state_transition".into(),
        payload: serde_json::json!({"state":"idle", "cancelled": cancelled}),
    });
    context.extension_control.set_steering(None);
    collect_delivered(&steering_handle, &mut steering, &mut pending_delivered);
    for message in pending_delivered.drain(..) {
        insert_message(terminal, &message.display)?;
        active.recorder.record(SteeringDelivered {
            content: message.content.clone(),
            after_internal_call_id: String::new(),
        });
        delivered_steering.push(message.content);
    }
    if !reasoning.is_empty() {
        if transcript_gap {
            insert_blank(terminal)?;
        }
        insert_reasoning(terminal, &reasoning)?;
    }
    if !visible.is_empty() {
        insert_response(terminal, &visible, &mut response_renderer)?;
    }
    for status in unfinished_subagents {
        subagent_ui::insert_settled(terminal, status)?;
    }
    // Compose the failure and elapsed time as one transcript block. This mirrors
    // component-based TUIs (Codex/Pi), where related rows are laid out together
    // instead of relying on the ordering of successive inline insertions.
    let mut auth_expired = false;
    let error_message = if let Some(result) = stream_result
        && let Err(error) = result.and_then(|result| result)
    {
        if is_auth_error(&error) {
            // AUTH-2: a token that expired mid-turn (or was already stale).
            // Flag it so the run loop force-refreshes the login; auto-resending
            // isn't safe because tool calls in the failed turn may have already
            // run, so ask the user to resend instead.
            auth_expired = true;
            Some(
                "Your login expired mid-turn — refreshing it now. Resend your message to continue."
                    .to_owned(),
            )
        } else {
            Some(format!("Error: {error:#}"))
        }
    } else {
        None
    };
    let elapsed_status = if cancelled {
        format!("  stopped · {}", format_elapsed(started.elapsed()))
    } else {
        format!("  {}", format_elapsed(started.elapsed()))
    };
    insert_turn_result(terminal, error_message.as_deref(), &elapsed_status)?;
    resize_and_draw(
        terminal,
        &ChatInput::default(),
        &[],
        &footer,
        &mut stream_viewport.height,
        3,
        false,
        None,
    )?;
    // Rebuild the model-facing history from the log — the single source of
    // truth, including tool round-trips and any TTSR rule turns.
    active.recorder.flush().await;
    if !active.recorder.is_healthy() {
        insert_status(
            terminal,
            "  ⚠ session log write failed (disk full?) — history may be incomplete",
        )?;
    }
    *history = artist_session::build_history(
        &active.events()?,
        &active.attachments,
        &artist_session::HistoryOptions::default(),
    )?;
    let delivered = delivered_steering;
    Ok(SubmitResult {
        viewport_height: stream_viewport.height,
        queued: deferred_commands
            .into_iter()
            .chain(steering.take().into_iter().map(|entry| SubmittedPrompt {
                display: entry.display,
                content: entry.content,
                images: entry.images,
                history_atoms: entry.atoms,
            }))
            .collect(),
        delivered,
        leftover_input: steering_input,
        auth_expired,
    })
}

fn take_visible_line(pending: &mut String, width: usize) -> Option<String> {
    let split = pending.find('\n').map(|index| index + 1).or_else(|| {
        let mut columns = 0;
        pending.char_indices().find_map(|(index, character)| {
            columns += character.width().unwrap_or(0);
            (columns > width).then_some(index)
        })
    })?;
    Some(pending.drain(..split).collect())
}

fn insert_history(
    terminal: &mut ratatui::DefaultTerminal,
    items: &[ReplayItem],
    custom_icons: &HashMap<String, String>,
) -> Result<()> {
    for item in items {
        match item {
            ReplayItem::User(text) => insert_message(terminal, text)?,
            ReplayItem::Assistant(text) => {
                let mut renderer = crate::response_output::Renderer::default();
                insert_response(terminal, text, &mut renderer)?;
                insert_blank(terminal)?;
            }
            ReplayItem::Reasoning(text) => insert_reasoning(terminal, text)?,
            ReplayItem::Tool { name, preview } => {
                let line = if preview.is_empty() {
                    name.clone()
                } else {
                    format!("{name} · {preview}")
                };
                insert_tool_line(
                    terminal,
                    &line,
                    None,
                    true,
                    false,
                    crate::tool_ui::icon_for(name, custom_icons),
                )?;
            }
            ReplayItem::Steering(text) => insert_message(terminal, text)?,
            ReplayItem::RuleFired { rule, matched } => {
                let excerpt: String = matched.chars().take(60).collect();
                insert_status(
                    terminal,
                    &format!("  ⚠ rule {rule} fired on \"{excerpt}\" — rewound and retried"),
                )?;
            }
        }
    }
    Ok(())
}

fn insert_message(terminal: &mut ratatui::DefaultTerminal, text: &str) -> Result<()> {
    let frame_height = crate::message_box::frame_height(text, terminal.size()?.width);
    terminal.insert_before(frame_height.saturating_add(1), |buffer| {
        let message_area = Rect::new(
            buffer.area.x,
            buffer.area.y,
            buffer.area.width,
            frame_height,
        );
        crate::message_box::render(buffer, message_area, text);
    })?;
    Ok(())
}
fn insert_turn_result(
    terminal: &mut ratatui::DefaultTerminal,
    error: Option<&str>,
    status: &str,
) -> Result<()> {
    let width = usize::from(terminal.size()?.width.max(1));
    let error_height = error.map_or(0, |message| {
        message
            .lines()
            .map(|line| UnicodeWidthStr::width(line).max(1).div_ceil(width))
            .sum::<usize>()
            .max(1) as u16
    });
    // Keep one spacer between a provider error and the stopwatch. A successful
    // turn retains the existing blank row before its stopwatch.
    let total_height = error_height.saturating_add(2);
    terminal.insert_before(total_height, |buffer| {
        if let Some(message) = error {
            let area = Rect::new(
                buffer.area.x,
                buffer.area.y,
                buffer.area.width,
                error_height,
            );
            let style = Style::default().fg(Color::White).bg(Color::Red);
            buffer.set_style(area, style);
            Paragraph::new(Text::styled(message, style))
                .wrap(Wrap { trim: false })
                .render(area, buffer);
            // Make blank cells explicit for terminals without background-color
            // erase support, so the red panel spans the complete row.
            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    let cell = buffer.cell_mut((x, y)).expect("error panel cell");
                    if cell.symbol() == " " {
                        cell.set_symbol("\u{00a0}");
                    }
                }
            }
        }
        let status_area = Rect::new(
            buffer.area.x,
            buffer.area.bottom().saturating_sub(1),
            buffer.area.width,
            1,
        );
        Paragraph::new(status)
            .style(Style::default().fg(Color::DarkGray))
            .render(status_area, buffer);
    })?;
    Ok(())
}

fn insert_blank(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    terminal.insert_before(1, |_| {})?;
    Ok(())
}

pub(crate) fn truncate_display_line(line: &str, width: usize) -> String {
    if line.width() <= width {
        return line.to_owned();
    }
    let target = width.saturating_sub(1);
    let mut used = 0;
    let mut output = String::new();
    for character in line.chars() {
        let character_width = character.width().unwrap_or(0);
        if used + character_width > target {
            break;
        }
        output.push(character);
        used += character_width;
    }
    output.push('…');
    output
}

pub(crate) fn fill_panel_background(buffer: &mut Buffer, area: Rect) {
    // A printable, one-column blank prevents ratatui's backend from replacing a
    // run of trailing spaces with EraseToEndOfLine. NBSP is still treated as
    // whitespace by some terminal layers, while the blank braille pattern is a
    // regular glyph and therefore reliably carries its cell background.
    const EXPLICIT_BLANK: &str = "\u{2800}";
    let area = area.intersection(buffer.area);
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let cell = buffer.cell_mut((x, y)).expect("panel cell");
            if cell.symbol() == " " {
                cell.set_symbol(EXPLICIT_BLANK);
            }
        }
    }
}

fn tool_prefix(first: bool, icon: Option<&str>) -> String {
    if first {
        format!("  {}  ", icon.unwrap_or("󰒓"))
    } else {
        "    ".to_owned()
    }
}

fn insert_tool_line(
    terminal: &mut ratatui::DefaultTerminal,
    content: &str,
    title: Option<&crate::tool_ui::ToolTitle>,
    first: bool,
    is_diff: bool,
    icon: Option<&str>,
) -> Result<()> {
    let prefix = tool_prefix(first, icon);
    let width = usize::from(terminal.size()?.width.max(1));
    let text = if first && let Some(title) = title {
        let icon = icon.unwrap_or("󰒓");
        let base = Style::default()
            .fg(crate::theme::PASTEL_WHITE)
            .bg(crate::theme::PANEL_BACKGROUND);
        let mut spans = vec![
            Span::styled("  ", base),
            Span::styled(
                icon.to_owned(),
                Style::default()
                    .fg(crate::tool_ui::accent_color(icon))
                    .bg(crate::theme::PANEL_BACKGROUND)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", base),
        ];
        spans.extend(crate::tool_ui::title_spans(
            title,
            width.saturating_sub(prefix.width()).max(1),
            crate::tool_ui::accent_color(icon),
        ));
        vec![Line::from(spans)]
    } else {
        content
            .lines()
            .enumerate()
            .map(|(index, line)| {
                // Tabs otherwise skip styled terminal cells. Tool lines are kept
                // to one terminal row so large diffs cannot dominate the UI.
                let line = line.replace('\t', "    ");
                let line_prefix = if index == 0 { prefix.as_str() } else { "    " };
                let diff_content = line
                    .split_once("│ ")
                    .map_or(line.as_str(), |(_, content)| content);
                let color = tool_line_color(first, is_diff, diff_content);
                let line =
                    truncate_display_line(&line, width.saturating_sub(line_prefix.width()).max(1));
                let style = Style::default()
                    .fg(color)
                    .bg(crate::theme::PANEL_BACKGROUND);
                if index == 0 && first {
                    let icon = icon.unwrap_or("󰒓");
                    Line::from(vec![
                        Span::styled("  ", style),
                        Span::styled(
                            icon.to_owned(),
                            Style::default()
                                .fg(crate::tool_ui::accent_color(icon))
                                .bg(crate::theme::PANEL_BACKGROUND)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(format!("  {line}"), style),
                    ])
                } else {
                    Line::styled(format!("{line_prefix}{line}"), style)
                }
            })
            .collect::<Vec<_>>()
    };
    let height = text
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum::<usize>() as u16;
    terminal.insert_before(height.max(1), |buffer| {
        let background = Style::default().bg(crate::theme::PANEL_BACKGROUND);
        buffer.set_style(buffer.area, background);
        Paragraph::new(Text::from(text))
            .wrap(Wrap { trim: false })
            .render(buffer.area, buffer);
        // Ratatui may optimize trailing ordinary spaces into an erase-to-EOL
        // sequence, which paints with the default background in terminals that
        // do not support background-color erase (notably herdr). Emit a real
        // blank glyph in every otherwise-empty panel cell instead.
        fill_panel_background(buffer, buffer.area);
    })?;
    Ok(())
}

fn tool_line_color(first: bool, is_diff: bool, content: &str) -> Color {
    if first {
        crate::theme::PASTEL_WHITE
    } else if is_diff && content.starts_with('+') {
        crate::theme::PASTEL_MINT
    } else if is_diff && content.starts_with('-') {
        crate::theme::PASTEL_PINK
    } else if is_diff && content.starts_with('~') {
        crate::theme::PASTEL_YELLOW
    } else {
        Color::Rgb(175, 175, 175)
    }
}

fn insert_reasoning(terminal: &mut ratatui::DefaultTerminal, reasoning: &str) -> Result<()> {
    insert_reasoning_chunk(terminal, reasoning, true)?;
    insert_blank(terminal)
}

fn insert_reasoning_chunk(
    terminal: &mut ratatui::DefaultTerminal,
    reasoning: &str,
    first: bool,
) -> Result<()> {
    let width = terminal.size()?.width.max(1);
    let text = Text::from(wrapped_reasoning_lines_with_prefix(
        reasoning,
        usize::from(width),
        first,
    ));
    let height = text.lines.len().max(1) as u16;
    terminal.insert_before(height, |buffer| {
        Paragraph::new(text).render(buffer.area, buffer);
    })?;
    Ok(())
}

#[cfg(test)]
fn reasoning_text(reasoning: &str) -> Text<'static> {
    reasoning_chunk_text(reasoning, true)
}

pub(crate) fn reasoning_chunk_text(reasoning: &str, first: bool) -> Text<'static> {
    Text::from(
        reasoning
            // Providers commonly stream adjacent bold summary headings without
            // a newline between the closing and opening markers.
            .replace("****", "**\n**")
            .lines()
            .enumerate()
            .map(|(line_index, line)| {
                let mut spans = vec![Span::raw(if first && line_index == 0 {
                    "  ◉ "
                } else {
                    "    "
                })];
                let mut rest = line;
                while let Some(start) = rest.find("**") {
                    spans.push(Span::styled(
                        rest[..start].to_owned(),
                        Style::default().fg(Color::DarkGray),
                    ));
                    rest = &rest[start + 2..];
                    let Some(end) = rest.find("**") else { break };
                    spans.push(Span::styled(
                        rest[..end].to_owned(),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ));
                    rest = &rest[end + 2..];
                }
                spans.push(Span::styled(
                    rest.to_owned(),
                    Style::default().fg(Color::DarkGray),
                ));
                Line::from(spans)
            })
            .collect::<Vec<_>>(),
    )
}

fn wrapped_reasoning_lines(reasoning: &str, width: usize) -> Vec<Line<'static>> {
    wrapped_reasoning_lines_with_prefix(reasoning, width, true)
}

fn wrapped_reasoning_lines_with_prefix(
    reasoning: &str,
    width: usize,
    first: bool,
) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(4).max(1) as u16;
    let wrapped = crate::text_wrap::plain(reasoning, content_width);
    reasoning_chunk_text(&wrapped, first).lines
}
fn insert_response(
    terminal: &mut ratatui::DefaultTerminal,
    output: &str,
    renderer: &mut crate::response_output::Renderer,
) -> Result<()> {
    let width = usize::from(terminal.size()?.width.max(1));
    let text = renderer.render(output, width);
    let height = text.lines.len().max(1) as u16;
    terminal.insert_before(height, |buffer| {
        Paragraph::new(text).render(buffer.area, buffer);
    })?;
    Ok(())
}

fn streaming_viewport_height(
    input_height: u16,
    subagent_height: u16,
    queued_height: u16,
    reasoning_height: u16,
    footer_height: u16,
    transcript_gap: bool,
    terminal_height: u16,
) -> u16 {
    input_height
        .saturating_add(subagent_height)
        .saturating_add(queued_height)
        .saturating_add(reasoning_height)
        // The transcript and timer separators are distinct rows.
        .saturating_add(u16::from(transcript_gap))
        .saturating_add(1)
        .saturating_add(1)
        .saturating_add(footer_height)
        .min(terminal_height)
}

fn status_line(phase: &str, elapsed: std::time::Duration, frame: usize) -> String {
    format!(
        "  {} · esc to interrupt",
        activity_status(phase, elapsed, frame)
    )
}

fn insert_status(terminal: &mut ratatui::DefaultTerminal, status: &str) -> Result<()> {
    terminal.insert_before(1, |buffer| {
        Paragraph::new(status)
            .style(Style::default().fg(Color::DarkGray))
            .render(buffer.area, buffer);
    })?;
    Ok(())
}

fn draw_streaming(
    terminal: &mut ratatui::DefaultTerminal,
    status: &str,
    footer: &status_bar::StatusView,
    controls: StreamingControls<'_>,
    viewport: &mut StreamingViewport,
) -> Result<()> {
    let terminal_size = terminal.size()?;
    let can_resize = viewport.can_resize_viewport(terminal_size);
    let layout_height = if can_resize {
        terminal_size.height
    } else {
        viewport.height.min(terminal_size.height)
    };
    let width = terminal_size.width.max(1);
    let footer_height = footer.height(width);
    let queued_height = controls.steering.displays().count() as u16;
    let suggestions_height = if controls.suggestions.is_empty() {
        0
    } else {
        controls.suggestions.len() as u16 + 2
    };
    let input_height = controls
        .input
        .visual_lines(width.saturating_sub(2).max(1))
        .saturating_add(2);
    let has_live_content =
        controls.subagents.height() > 0 || queued_height > 0 || !controls.reasoning.is_empty();
    // When there is no live content, the timer's own leading gap already
    // separates it from scrollback; do not stack a second blank row.
    let transcript_gap_height =
        u16::from(has_live_content && (controls.transcript_gap || !controls.reasoning.is_empty()));
    const TIMER_GAP_HEIGHT: u16 = 1;
    let base_fixed_height = input_height
        .saturating_add(queued_height)
        .saturating_add(suggestions_height)
        .saturating_add(transcript_gap_height)
        .saturating_add(TIMER_GAP_HEIGHT)
        .saturating_add(1)
        .saturating_add(footer_height);
    let subagent_height = controls
        .subagents
        .height()
        .min(layout_height.saturating_sub(base_fixed_height))
        / 2
        * 2;
    let fixed_height = base_fixed_height.saturating_add(subagent_height);
    const MAX_LIVE_REASONING_ROWS: u16 = 8;
    let mut reasoning_lines = wrapped_reasoning_lines(controls.reasoning, usize::from(width));
    let reasoning_height = (reasoning_lines.len() as u16)
        .min(layout_height.saturating_sub(fixed_height))
        .min(MAX_LIVE_REASONING_ROWS);
    let keep_from = reasoning_lines
        .len()
        .saturating_sub(usize::from(reasoning_height));
    reasoning_lines.drain(..keep_from);
    let desired = streaming_viewport_height(
        input_height,
        subagent_height,
        queued_height.saturating_add(suggestions_height),
        reasoning_height,
        footer_height,
        transcript_gap_height > 0,
        layout_height,
    );
    let resized = desired != viewport.height && can_resize;
    if resized {
        viewport.height = desired;
        execute!(std::io::stdout(), BeginSynchronizedUpdate)?;
        clear_inline(terminal)?;
        *terminal = ratatui::init_with_options(TerminalOptions {
            viewport: Viewport::Inline(desired),
        });
        execute!(std::io::stdout(), EnableBracketedPaste)?;
    }
    terminal.draw(|frame| {
        let area = frame.area();
        let live_top = area.y.saturating_add(transcript_gap_height);
        let subagent_area = Rect::new(
            area.x,
            live_top,
            area.width,
            subagent_height.min(area.height.saturating_sub(transcript_gap_height)),
        );
        controls
            .subagents
            .render(frame.buffer_mut(), subagent_area, controls.animation_frame);
        let queued_area = Rect::new(
            area.x,
            subagent_area.bottom(),
            area.width,
            queued_height.min(area.height.saturating_sub(subagent_height)),
        );
        let queued = controls
            .steering
            .displays()
            .enumerate()
            .map(|(index, prompt)| {
                Line::styled(
                    truncate_display_line(
                        &format!("  ⌊  {prompt}"),
                        usize::from(area.width.max(1)),
                    ),
                    if controls.steering.selected() == Some(index) {
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                )
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(queued), queued_area);
        let reasoning_area = Rect::new(
            area.x,
            queued_area.bottom(),
            area.width,
            reasoning_height.min(area.height.saturating_sub(subagent_height + queued_height)),
        );
        if reasoning_height > 0 {
            frame.render_widget(Paragraph::new(reasoning_lines), reasoning_area);
        }
        // Gaps have one owner each: transcript_gap_height is above every live
        // element, while TIMER_GAP_HEIGHT is immediately above the timer.
        let status_top = reasoning_area.bottom().saturating_add(TIMER_GAP_HEIGHT);
        let status_area = Rect::new(
            area.x,
            status_top,
            area.width,
            area.height
                .saturating_sub(
                    transcript_gap_height
                        + subagent_height
                        + queued_height
                        + reasoning_height
                        + TIMER_GAP_HEIGHT,
                )
                .min(1),
        );
        frame.render_widget(
            Paragraph::new(status).style(Style::default().fg(Color::DarkGray)),
            status_area,
        );
        let suggestions_area = Rect::new(
            area.x,
            status_area.bottom(),
            area.width,
            suggestions_height.min(area.height.saturating_sub(footer_height)),
        );
        if suggestions_height > 0 {
            let text = controls
                .suggestions
                .iter()
                .map(|option| Line::styled(option.clone(), panel_option_style(option)))
                .collect::<Vec<_>>();
            frame.render_widget(
                Paragraph::new(text)
                    .block(Block::bordered().border_style(Style::default().fg(Color::DarkGray))),
                suggestions_area,
            );
        }
        let input_area = Rect::new(
            area.x,
            suggestions_area.bottom(),
            area.width,
            area.height.saturating_sub(
                transcript_gap_height
                    + subagent_height
                    + queued_height
                    + suggestions_height
                    + TIMER_GAP_HEIGHT
                    + 1
                    + footer_height,
            ),
        );
        render_input(frame, input_area, controls.input);
        footer.render(
            frame.buffer_mut(),
            Rect::new(
                area.x,
                area.bottom().saturating_sub(footer_height),
                area.width,
                footer_height,
            ),
        );
    })?;
    terminal.show_cursor()?;
    if resized {
        execute!(std::io::stdout(), EndSynchronizedUpdate)?;
    }
    Ok(())
}

fn render_with_panel(
    frame: &mut Frame<'_>,
    input: &ChatInput,
    panel: &[String],
    footer: &status_bar::StatusView,
    show_splash: bool,
) {
    let area = frame.area();
    let status_height = footer.height(area.width).min(area.height);
    // The splash is a startup affordance only. Suppress it whenever a command
    // panel (e.g. /help) is open: a Paragraph doesn't clear its background, so
    // the splash would otherwise bleed through the panel's empty cells and eat
    // the rows the panel needs.
    if show_splash
        && panel.is_empty()
        && area.height >= crate::startup_splash::HEIGHT.saturating_add(3)
    {
        crate::startup_splash::render(
            frame,
            Rect::new(area.x, area.y, area.width, crate::startup_splash::HEIGHT),
            &[],
        );
    }
    if status_height > 0 {
        footer.render(
            frame.buffer_mut(),
            Rect::new(
                area.x,
                area.bottom().saturating_sub(status_height),
                area.width,
                status_height,
            ),
        );
    }
    let input_height = input
        .visual_lines(area.width.saturating_sub(2).max(1))
        .saturating_add(2)
        .min(area.height.saturating_sub(status_height));
    let input_area = Rect::new(
        area.x,
        area.bottom().saturating_sub(status_height + input_height),
        area.width,
        input_height,
    );
    render_input(frame, input_area, input);
    if panel.is_empty() {
        return;
    }
    let panel_height =
        (panel.len() as u16 + 2).min(area.height.saturating_sub(status_height + input_height));
    let panel_area = Rect::new(
        area.x,
        input_area.y.saturating_sub(panel_height),
        area.width,
        panel_height,
    );
    let panel_text = Text::from(
        panel
            .iter()
            .map(|option| Line::styled(option.clone(), panel_option_style(option)))
            .collect::<Vec<_>>(),
    );
    frame.render_widget(
        Paragraph::new(panel_text).block(
            Block::default()
                .borders(Borders::TOP | Borders::BOTTOM)
                .border_style(Style::default().fg(crate::theme::PASTEL_PINK)),
        ),
        panel_area,
    );
}

/// Complete the currently-selected suggestion into `input`, returning true if
/// one was applied. Shared by Tab (complete) and Enter (complete, then send).
/// The suggestion lists are concatenated [slash][custom][extension] for the
/// slash family, or a standalone mcp/skill list; empty lists yield `None` from
/// `.get`, so the index simply falls through to the active family.
#[allow(clippy::too_many_arguments)]
fn apply_selected_suggestion(
    input: &mut ChatInput,
    index: usize,
    slash: &[&slash_commands::SlashCommand],
    custom: &[&crate::custom_commands::CustomCommand],
    extension: &[&artist_extensions::CommandDeclaration],
    provider: &[slash_commands::ArgumentCompletion],
    mcp: &[String],
    skill_range: &Option<std::ops::Range<usize>>,
    skills: &[&artist_agent::AvailableSkill],
) -> bool {
    if let Some(command) = slash.get(index) {
        input.text = command.name.to_owned() + " ";
        input.atoms.clear();
        input.cursor = input.text.len();
    } else if let Some(command) = custom.get(index.saturating_sub(slash.len())) {
        input.text = command.name.clone() + " ";
        input.atoms.clear();
        input.cursor = input.text.len();
    } else if let Some(command) = extension.get(index.saturating_sub(slash.len() + custom.len())) {
        input.text = command.name.clone() + " ";
        input.atoms.clear();
        input.cursor = input.text.len();
    } else if let Some(completion) = provider.get(index) {
        input.text = completion.value.clone() + " ";
        input.atoms.clear();
        input.cursor = input.text.len();
    } else if let Some(completion) = mcp.get(index) {
        input.text = completion.clone();
        // A fully-specified command can be sent as-is; a partial one keeps a
        // trailing space so the user (or Enter) can still add an argument.
        if completion != "/mcp status" && completion.split_whitespace().count() != 3 {
            input.text.push(' ');
        }
        input.atoms.clear();
        input.cursor = input.text.len();
    } else if let (Some(range), Some(skill)) = (skill_range.clone(), skills.get(index)) {
        input.replace_range(range, &format!("${}", skill.name));
    } else {
        return false;
    }
    true
}

fn panel_option_style(option: &str) -> Style {
    // The selected suggestion is marked with a leading "› "; only it is
    // highlighted. (Previously index 0 was also highlighted, so navigating away
    // left two items blue.)
    if option.trim_start().starts_with('›') {
        Style::default()
            .fg(crate::theme::PASTEL_BLUE)
            .add_modifier(Modifier::BOLD)
    } else if option.contains("[x]") {
        Style::default().fg(crate::theme::PASTEL_MINT)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn render_input(frame: &mut Frame<'_>, area: Rect, input: &ChatInput) {
    let inner_width = area.width.saturating_sub(2).max(1);
    crate::input_border::render(frame.buffer_mut(), area);

    let input_area = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let input_style = Style::default().fg(crate::theme::PASTEL_WHITE);
    let wrapped = crate::text_wrap::with_cursor(&input.text, input.cursor, inner_width);
    let paragraph = Paragraph::new(Text::raw(wrapped.text.clone())).style(input_style);
    frame.render_widget(paragraph, input_area);

    if input_area.width > 0 && input_area.height > 0 {
        let (x, y) = (wrapped.column, wrapped.row);
        frame.set_cursor_position((
            input_area.x + x.min(inner_width.saturating_sub(1)),
            input_area.y + y.min(input_area.height.saturating_sub(1)),
        ));
    }
}

pub(crate) fn finish_inline(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    clear_inline(terminal)
}

fn clear_inline(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    // Preserve both positions: shrinking/growing the terminal can move the
    // inline viewport in either direction. Clearing only from the post-resize
    // top leaves whichever part of the old viewport sits above it in scrollback,
    // which appears as duplicated input boxes after resizing back.
    let old_top = terminal.get_frame().area().y;
    terminal.autoresize()?;
    let new_top = terminal.get_frame().area().y;
    let clear_from = old_top.min(new_top);
    execute!(
        std::io::stdout(),
        Hide,
        MoveTo(0, clear_from),
        Clear(ClearType::FromCursorDown)
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_provider::{Auth, ProviderId, SavedProvider, Secret};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn onboarding_notice_is_actionable_and_tui_only() {
        assert!(NO_PROVIDER_NOTICE.contains("/login"));
        assert!(!NO_PROVIDER_NOTICE.contains("artist provider"));
    }

    #[test]
    fn empty_provider_footer_is_safe() {
        let runtime = StatusRuntime {
            git_branch: None,
            used_tokens: None,
            context_capacity: None,
            session_tokens: 0,
            fast_mode: false,
            extension_values: Vec::new(),
        };
        assert_eq!(
            footer_view(&StatusBarConfig::default(), None, Path::new("."), &runtime).height(80),
            0
        );
    }

    fn test_account(id: &str) -> SavedProvider {
        SavedProvider::chatgpt(
            ProviderId::new(id).unwrap(),
            id,
            Auth {
                access_token: Secret::new("access"),
                refresh_token: Secret::new("refresh"),
                account_id: id.into(),
                email: None,
                expires_at: None,
            },
        )
    }

    fn test_footer() -> status_bar::StatusView {
        status_bar::view(vec![
            status_bar::StatusSegment {
                item: StatusItem::ProjectDirectory,
                text: " artist".into(),
                compact: None,
                palette_index: 0,
            },
            status_bar::StatusSegment {
                item: StatusItem::GitBranch,
                text: " main".into(),
                compact: None,
                palette_index: 1,
            },
            status_bar::StatusSegment {
                item: StatusItem::Model,
                text: " gpt-5.4".into(),
                compact: None,
                palette_index: 2,
            },
            status_bar::StatusSegment {
                item: StatusItem::Reasoning,
                text: " high".into(),
                compact: None,
                palette_index: 3,
            },
            status_bar::StatusSegment {
                item: StatusItem::Context,
                text: " ctx ██████░░ 75% · 200k".into(),
                compact: Some(" ctx 75%".into()),
                palette_index: 4,
            },
            status_bar::StatusSegment {
                item: StatusItem::SessionTokens,
                text: " 1.5k total".into(),
                compact: None,
                palette_index: 5,
            },
        ])
    }

    #[test]
    fn accounts_switch_persists_selected_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        let mut store = ProviderStore::default();
        store.add(test_account("one"));
        store.add(test_account("two"));

        let (_, switched) = handle_accounts(&mut store, &path, 0, Some("two")).unwrap();

        assert_eq!(switched, Some(1));
        assert_eq!(store.default_provider.as_ref().unwrap().as_str(), "two");
        let loaded = ProviderStore::load(&path).unwrap();
        assert_eq!(loaded.default_provider.unwrap().as_str(), "two");
    }

    #[test]
    fn login_selection_cancel_restores_exact_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        let mut store = ProviderStore::default();
        store.add(test_account("existing"));
        let before = toml::to_string(&store).unwrap();
        let previous = store.clone();
        store.add(test_account("partial"));
        store.default_provider = Some(store.providers[1].id.clone());

        let result = finish_login_transaction(
            &mut store,
            previous,
            &path,
            Err(anyhow::anyhow!("selection cancelled")),
        );

        assert!(result.is_err());
        assert_eq!(toml::to_string(&store).unwrap(), before);
        assert!(!path.exists());
    }

    #[test]
    fn successful_login_selection_is_saved_once_complete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        let mut store = ProviderStore::default();
        let previous = store.clone();
        store.add(test_account("selected"));
        store.providers[0].model = Some("chosen-model".to_owned());

        let index = finish_login_transaction(&mut store, previous, &path, Ok(Some(0))).unwrap();

        assert_eq!(index, Some(0));
        let loaded = ProviderStore::load(&path).unwrap();
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers[0].id.as_str(), "selected");
        assert!(loaded.providers[0].model.is_some());
    }

    #[test]
    fn edits_and_expands_input() {
        let mut input = ChatInput::default();
        input.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        input.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(input.text, "a");
        input.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        input.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert_eq!(input.text, "a\nb");
        assert_eq!(input.visual_lines(10), 2);
        assert_eq!(
            ChatInput {
                text: "1234".into(),
                cursor: 4,
                atoms: InputAtoms::default(),
            }
            .visual_lines(4),
            2
        );
        input.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(input.text, "a\n");
        input.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(input.text, "a");
    }

    #[test]
    fn streaming_viewport_stays_locked_after_terminal_resize() {
        let mut viewport = StreamingViewport::new(6, Size::new(80, 24));
        assert!(viewport.can_resize_viewport(Size::new(80, 24)));
        assert!(!viewport.can_resize_viewport(Size::new(20, 24)));
        assert!(!viewport.can_resize_viewport(Size::new(20, 24)));
        assert_eq!(viewport.height, 6);
    }

    #[test]
    fn navigates_and_edits_at_the_cursor() {
        let mut input = ChatInput::default();
        input.insert("abc\ndef");
        input.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(input.cursor, 3);
        input.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        input.handle_key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));
        assert_eq!(input.text, "ab!c\ndef");
        assert!(input.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert!(input.text.is_empty());
        assert!(!input.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn reasoning_markers_become_italics() {
        let text = reasoning_text("**Planning** the answer");
        assert!(text.lines[0].spans[0].content.contains("◉"));
        assert!(
            text.lines[0]
                .spans
                .iter()
                .any(|span| span.style.add_modifier.contains(Modifier::ITALIC))
        );
        assert!(
            text.lines[0]
                .spans
                .iter()
                .all(|span| !span.content.contains("**"))
        );
    }

    #[test]
    fn live_reasoning_wraps_for_the_available_viewport_width() {
        let rows = wrapped_reasoning_lines("**Planning** a deliberately long trace", 12);
        assert!(rows.len() > 1);
        assert!(rows.iter().all(|line| line.width() <= 12));
        let tail = rows
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(tail.trim_end().ends_with('e'));
        let wrapped_words = wrapped_reasoning_lines("one two", 8);
        assert_eq!(wrapped_words.len(), 2);
        assert!(wrapped_words[0].to_string().trim_end().ends_with("one"));
        assert!(wrapped_words[1].to_string().trim_end().ends_with("two"));
    }

    #[test]
    fn adjacent_reasoning_headings_get_separate_rows() {
        let text = reasoning_text("**Diagnosing****Evaluating****Identifying**");
        assert_eq!(text.lines.len(), 3);
        let rows = text
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(rows[0].contains("Diagnosing"));
        assert!(rows[1].contains("Evaluating"));
        assert!(rows[2].contains("Identifying"));
    }

    #[test]
    fn markdown_fences_stream_as_plain_lines() {
        let mut pending = "```rust\nfn main() {}".to_owned();

        assert_eq!(
            take_visible_line(&mut pending, 80).as_deref(),
            Some("```rust\n")
        );
        assert_eq!(pending, "fn main() {}");
    }

    #[test]
    fn truncates_tool_lines_to_terminal_columns() {
        assert_eq!(truncate_display_line("abcdef", 5), "abcd…");
        assert_eq!(truncate_display_line("ab界cd", 5), "ab界…");
        assert_eq!(truncate_display_line("short", 8), "short");
        assert_eq!(tool_prefix(true, Some("$")), "  $  ");
        assert_eq!(tool_prefix(true, None), "  󰒓  ");
        assert_eq!(tool_prefix(false, Some("$")), "    ");
    }

    #[test]
    fn colors_compact_replacements_with_the_pastel_change_accent() {
        assert_eq!(
            tool_line_color(false, true, "~- [x] item"),
            crate::theme::PASTEL_YELLOW
        );
    }

    #[test]
    fn panel_background_uses_printable_blank_cells() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
        buffer.cell_mut((1, 0)).unwrap().set_symbol("x");

        let area = buffer.area;
        fill_panel_background(&mut buffer, area);

        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "\u{2800}");
        assert_eq!(buffer.cell((1, 0)).unwrap().symbol(), "x");
        assert_eq!(buffer.cell((3, 0)).unwrap().symbol(), "\u{2800}");
    }

    #[test]
    fn formats_activity_elapsed_time() {
        assert_eq!(format_elapsed(std::time::Duration::from_secs(65)), "01:05");
        assert_eq!(
            format_elapsed(std::time::Duration::from_secs(3_661)),
            "01:01:01"
        );
        assert_eq!(
            status_line("thinking", std::time::Duration::ZERO, 0),
            "  ⋮·⋮·⋮ thinking [00:00 elapsed] · esc to interrupt"
        );
        assert!(
            status_line("working", std::time::Duration::ZERO, 3).starts_with("  ⋮··⋮ working")
        );
    }

    #[test]
    fn streaming_viewport_accounts_for_independent_transcript_and_timer_gaps() {
        let without_output = streaming_viewport_height(3, 0, 0, 0, status_bar::HEIGHT, false, 20);
        let with_output = streaming_viewport_height(3, 0, 0, 0, status_bar::HEIGHT, true, 20);
        let with_live_reasoning =
            streaming_viewport_height(3, 0, 0, 1, status_bar::HEIGHT, true, 20);
        let with_subagent = streaming_viewport_height(3, 2, 0, 0, status_bar::HEIGHT, false, 20);

        assert_eq!(without_output, 3 + 1 + 1 + status_bar::HEIGHT);
        assert_eq!(with_output, without_output + 1);
        assert_eq!(with_live_reasoning, without_output + 2);
        assert_eq!(with_subagent, without_output + 2);
    }

    #[test]
    fn command_panel_has_only_horizontal_walls() {
        let backend = TestBackend::new(20, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_with_panel(
                    frame,
                    &ChatInput::default(),
                    &["/help  Show commands".into()],
                    &status_bar::StatusView::default(),
                    false,
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "─");
        assert_ne!(buffer.cell((0, 1)).unwrap().symbol(), "│");
        assert_eq!(buffer.cell((0, 2)).unwrap().symbol(), "─");
        assert_eq!(buffer.cell((0, 3)).unwrap().symbol(), "┌");
    }

    #[test]
    fn interactive_selection_uses_color() {
        let selected = panel_option_style("› model");
        assert_eq!(selected.fg, Some(crate::theme::PASTEL_BLUE));
        assert!(selected.add_modifier.contains(Modifier::BOLD));
        assert_eq!(panel_option_style("Select model").fg, Some(Color::DarkGray));
        assert_eq!(
            panel_option_style("  [x] branch").fg,
            Some(crate::theme::PASTEL_MINT)
        );
    }

    #[test]
    fn completes_embedded_skill_mentions() {
        let mut input = ChatInput::default();
        input.insert("please use $lin");
        let skills = vec![artist_agent::AvailableSkill {
            name: "linear".into(),
            description: "Manage Linear issues".into(),
        }];
        let (range, matches) = skill_completions(&input, &skills);
        assert_eq!(range, Some(11..15));
        assert_eq!(matches[0].name, "linear");
        input.replace_range(range.unwrap(), "$linear");
        assert_eq!(input.text, "please use $linear");
    }

    #[test]
    fn tab_completion_option_uses_pastel_blue() {
        let backend = TestBackend::new(20, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut input = ChatInput::default();
        input.insert("/");
        terminal
            .draw(|frame| {
                render_with_panel(
                    frame,
                    &input,
                    // The selected suggestion carries a leading "› " marker.
                    &[
                        "› /help  Show commands".into(),
                        "/model  Select model".into(),
                    ],
                    &status_bar::StatusView::default(),
                    false,
                )
            })
            .unwrap();
        assert_eq!(
            terminal.backend().buffer().cell((0, 1)).unwrap().fg,
            crate::theme::PASTEL_BLUE
        );
    }

    #[test]
    fn status_bar_renders_two_rows_below_input() {
        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let footer = test_footer();
        terminal
            .draw(|frame| render_with_panel(frame, &ChatInput::default(), &[], &footer, false))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "┌");
        assert_eq!(buffer.cell((0, 2)).unwrap().symbol(), "└");
        assert_eq!(buffer.cell((1, 3)).unwrap().symbol(), "");
        assert_eq!(buffer.cell((11, 3)).unwrap().symbol(), "");
        assert_eq!(buffer.cell((1, 4)).unwrap().symbol(), "");
        assert_eq!(buffer.cell((13, 4)).unwrap().symbol(), "");
    }

    #[test]
    fn status_bar_stays_two_rows_at_narrow_widths() {
        let backend = TestBackend::new(6, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let footer = test_footer();
        terminal
            .draw(|frame| render_with_panel(frame, &ChatInput::default(), &[], &footer, false))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "┌");
        assert_eq!(buffer.cell((0, 2)).unwrap().symbol(), "└");
        assert_ne!(buffer.cell((0, 3)).unwrap().symbol(), "┌");
        assert_ne!(buffer.cell((0, 4)).unwrap().symbol(), "└");
        assert_eq!(buffer.cell((0, 3)).unwrap().symbol(), "");
        assert_eq!(buffer.cell((1, 4)).unwrap().symbol(), "");
    }

    #[test]
    fn renders_at_full_width() {
        let backend = TestBackend::new(20, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_with_panel(
                    frame,
                    &ChatInput::default(),
                    &[],
                    &status_bar::StatusView::default(),
                    false,
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "┌");
        assert_eq!(buffer.cell((19, 0)).unwrap().symbol(), "┐");
        assert_eq!(buffer.cell((1, 1)).unwrap().bg, Color::Reset);
        assert_eq!(buffer.cell((0, 0)).unwrap().fg, crate::theme::PASTEL_PINK);
        assert_eq!(buffer.cell((7, 0)).unwrap().fg, crate::theme::PASTEL_WHITE);
        assert_eq!(buffer.cell((14, 0)).unwrap().fg, crate::theme::PASTEL_MINT);
        assert_eq!(
            buffer.cell((19, 2)).unwrap().fg,
            crate::theme::PASTEL_YELLOW
        );
        assert_eq!(buffer.cell((12, 2)).unwrap().fg, crate::theme::PASTEL_BLUE);
        assert_eq!(buffer.cell((0, 2)).unwrap().fg, crate::theme::PASTEL_BLUSH);
    }
}
