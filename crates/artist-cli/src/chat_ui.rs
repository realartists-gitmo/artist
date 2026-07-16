use crate::{
    command_ui,
    interaction::{PromptHistory, SteeringQueue},
    models,
    sessions::{Role, Session, SessionStore, Turn},
    slash_commands,
    status_bar::{self, StatusBarConfig, StatusItem},
    store::ProviderStore,
    tool_ui::ToolUi,
};
use ansi_to_tui::IntoText;
use anyhow::{Context, Result};
use artist_tools::ToolBundle;
use llm_provider::SavedProvider;
use ratatui::{
    Frame, TerminalOptions, Viewport,
    buffer::Buffer,
    crossterm::{
        cursor::{Hide, MoveTo},
        event::{
            self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
            PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
        },
        execute,
        terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate},
    },
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};
use std::{collections::VecDeque, io::IsTerminal, path::Path};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Default)]
struct ChatInput {
    text: String,
    cursor: usize,
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
        self.text.insert_str(self.cursor, value);
        self.cursor += value.len();
    }
    fn backspace(&mut self) {
        if let Some((index, _)) = self.text[..self.cursor].char_indices().next_back() {
            self.text.drain(index..self.cursor);
            self.cursor = index;
        }
    }
    fn delete(&mut self) {
        if let Some(character) = self.text[self.cursor..].chars().next() {
            self.text
                .drain(self.cursor..self.cursor + character.len_utf8());
        }
    }
    fn move_left(&mut self) {
        if let Some((index, _)) = self.text[..self.cursor].char_indices().next_back() {
            self.cursor = index;
        }
    }
    fn move_right(&mut self) {
        if let Some(character) = self.text[self.cursor..].chars().next() {
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
    }

    fn visual_lines(&self, inner_width: u16) -> u16 {
        let width = usize::from(inner_width.max(1));
        let mut lines = self.text.split('\n').collect::<Vec<_>>();
        let last = UnicodeWidthStr::width(lines.pop().unwrap_or_default());
        let previous = lines
            .into_iter()
            .map(|line| UnicodeWidthStr::width(line).max(1).div_ceil(width))
            .sum::<usize>();
        (previous + last / width + 1) as u16
    }

    fn cursor_position(&self, inner_width: u16) -> (u16, u16) {
        let width = usize::from(inner_width.max(1));
        let prefix = &self.text[..self.cursor];
        let mut lines = prefix.split('\n').collect::<Vec<_>>();
        let current = UnicodeWidthStr::width(lines.pop().unwrap_or_default());
        let previous = lines
            .into_iter()
            .map(|line| UnicodeWidthStr::width(line).max(1).div_ceil(width))
            .sum::<usize>();
        (
            (current % width) as u16,
            (previous + current / width) as u16,
        )
    }
}

#[derive(Default)]
struct StatusRuntime {
    git_branch: Option<String>,
    used_tokens: Option<u64>,
    context_capacity: Option<u64>,
}

fn footer_line(
    config: &StatusBarConfig,
    provider: &SavedProvider,
    project: &Path,
    runtime: &StatusRuntime,
) -> Line<'static> {
    status_bar::render(&status_bar::segments(
        config,
        project,
        provider,
        runtime.git_branch.as_deref(),
        runtime.used_tokens,
        runtime.context_capacity,
    ))
}

struct SubmitContext<'a> {
    provider: &'a SavedProvider,
    sessions: &'a SessionStore,
    project: &'a Path,
    status_config: &'a StatusBarConfig,
    tools: &'a ToolBundle,
}

struct SubmitResult {
    viewport_height: u16,
    queued: Vec<String>,
}

struct StreamingControls<'a> {
    input: &'a ChatInput,
    steering: &'a SteeringQueue,
}

struct ChatContext<'a> {
    store: &'a mut ProviderStore,
    provider_index: usize,
    store_path: &'a Path,
    sessions: &'a SessionStore,
    project: &'a Path,
    tools: &'a ToolBundle,
}

pub struct ChatResources<'a> {
    pub sessions: &'a SessionStore,
    pub project: &'a Path,
    pub tools: &'a ToolBundle,
}

/// Runs an inline, persistent multi-turn chat. A session is created on first submission.
pub async fn run(
    store: &mut ProviderStore,
    provider_index: usize,
    store_path: &Path,
    resources: ChatResources<'_>,
    resumed: Option<(Session, Vec<Turn>)>,
    initial_prompt: Option<String>,
) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("interactive chat requires a terminal; use -p for non-interactive prompts");
    }
    let sessions = resources.sessions;
    let project = resources.project;
    let tools = resources.tools;
    let context_capacity = if store.status_bar.items.contains(&StatusItem::Context) {
        models::catalog(&store.providers[provider_index])
            .await
            .ok()
            .and_then(|catalog| {
                catalog
                    .iter()
                    .find(|model| {
                        Some(&model.slug) == store.providers[provider_index].model.as_ref()
                    })
                    .and_then(|model| model.effective_context_window())
            })
    } else {
        None
    };
    let status = StatusRuntime {
        git_branch: status_bar::git_branch(project),
        used_tokens: None,
        context_capacity,
    };
    let terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(3),
    });
    let keyboard_result = execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    let result = match keyboard_result {
        Ok(()) => {
            run_loop(
                terminal,
                ChatContext {
                    store,
                    provider_index,
                    store_path,
                    sessions,
                    project,
                    tools,
                },
                resumed,
                initial_prompt,
                status,
            )
            .await
        }
        Err(error) => Err(error.into()),
    };
    let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    ratatui::restore();
    result
}

async fn run_loop(
    mut terminal: ratatui::DefaultTerminal,
    context: ChatContext<'_>,
    resumed: Option<(Session, Vec<Turn>)>,
    mut pending: Option<String>,
    mut status: StatusRuntime,
) -> Result<()> {
    let resumed_session = resumed.is_some();
    let (mut session, mut turns) = resumed.map_or((None, Vec::new()), |(s, t)| (Some(s), t));
    let mut input = ChatInput::default();
    let mut prompt_history = PromptHistory::from_prompts(
        turns
            .iter()
            .filter(|turn| turn.role == Role::User)
            .map(|turn| turn.content.clone())
            .collect(),
    );
    let mut queued_prompts = VecDeque::new();
    let mut viewport_height = 3;
    let mut viewport_floor = 3;
    let mut command_panel = Vec::new();
    if resumed_session {
        let footer = footer_line(
            &context.store.status_bar,
            &context.store.providers[context.provider_index],
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
        )?;
        insert_history(&mut terminal, &turns)?;
    }
    loop {
        let suggestions = slash_commands::completions(&input.text)
            .into_iter()
            .map(|command| format!("{}  {}", command.name, command.description))
            .collect::<Vec<_>>();
        let panel = if suggestions.is_empty() {
            &command_panel
        } else {
            &suggestions
        };
        let footer = footer_line(
            &context.store.status_bar,
            &context.store.providers[context.provider_index],
            context.project,
            &status,
        );
        resize_and_draw(
            &mut terminal,
            &input,
            panel,
            &footer,
            &mut viewport_height,
            viewport_floor,
        )?;
        if let Some(prompt) = pending.take() {
            if let Some(command) = slash_commands::parse(&prompt) {
                command_panel = match command {
                    Ok(command) => {
                        let command_input = ChatInput::default();
                        match command_ui::run(
                            context.store,
                            context.provider_index,
                            context.store_path,
                            command,
                            |panel| {
                                resize_and_draw(
                                    &mut terminal,
                                    &command_input,
                                    panel,
                                    &footer,
                                    &mut viewport_height,
                                    3,
                                )
                            },
                        )
                        .await
                        {
                            Ok(output) => {
                                if output.model_changed {
                                    status.context_capacity = output.context_capacity;
                                    status.used_tokens = None;
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
                )?;
                prompt_history.push(prompt.clone());
                let result = submit(
                    &mut terminal,
                    SubmitContext {
                        provider: &context.store.providers[context.provider_index],
                        sessions: context.sessions,
                        project: context.project,
                        status_config: &context.store.status_bar,
                        tools: context.tools,
                    },
                    &mut session,
                    &mut turns,
                    &mut status,
                    prompt,
                )
                .await?;
                viewport_height = result.viewport_height;
                queued_prompts.extend(result.queued);
                pending = queued_prompts.pop_front();
                viewport_floor = 3;
            }
            continue;
        }
        match event::read()? {
            Event::Key(key)
                if key.kind == KeyEventKind::Press
                    && key.code == KeyCode::Enter
                    && !key.modifiers.contains(KeyModifiers::SHIFT)
                    && !input.text.trim().is_empty() =>
            {
                pending = Some(std::mem::take(&mut input.text));
                input.cursor = 0;
            }
            Event::Key(key)
                if key.kind == KeyEventKind::Press
                    && key.code == KeyCode::Tab
                    && !suggestions.is_empty() =>
            {
                input.text = slash_commands::completions(&input.text)[0].name.to_owned() + " ";
                input.cursor = input.text.len();
            }
            Event::Key(key)
                if key.kind == KeyEventKind::Press
                    && matches!(key.code, KeyCode::Up | KeyCode::Down)
                    && !input.text.contains('\n') =>
            {
                if let Some(prompt) = prompt_history.navigate(key.code == KeyCode::Up, &input.text)
                {
                    input.text = prompt;
                    input.cursor = input.text.len();
                }
            }
            Event::Key(key) if !input.handle_key(key) => {
                clear_inline(&mut terminal)?;
                return Ok(());
            }
            Event::Resize(_, _) => {}
            Event::Paste(text) => input.insert(&text),
            _ => {}
        }
    }
}

fn resize_and_draw(
    terminal: &mut ratatui::DefaultTerminal,
    input: &ChatInput,
    panel: &[String],
    footer: &Line<'_>,
    viewport_height: &mut u16,
    viewport_floor: u16,
) -> Result<()> {
    let width = terminal.size()?.width.saturating_sub(2).max(1);
    let panel_height = if panel.is_empty() {
        0
    } else {
        panel.len() as u16 + 2
    };
    let status_height = (!footer.spans.is_empty()) as u16;
    let desired = input
        .visual_lines(width)
        .saturating_add(2)
        .saturating_add(panel_height)
        .saturating_add(status_height)
        .max(viewport_floor);
    if desired != *viewport_height {
        *viewport_height = desired;
        execute!(std::io::stdout(), BeginSynchronizedUpdate)?;
        clear_inline(terminal)?;
        *terminal = ratatui::init_with_options(TerminalOptions {
            viewport: Viewport::Inline(desired),
        });
        terminal.draw(|frame| render_with_panel(frame, input, panel, footer))?;
        terminal.show_cursor()?;
        execute!(std::io::stdout(), EndSynchronizedUpdate)?;
    } else {
        terminal.draw(|frame| render_with_panel(frame, input, panel, footer))?;
    }
    Ok(())
}

async fn submit(
    terminal: &mut ratatui::DefaultTerminal,
    context: SubmitContext<'_>,
    session: &mut Option<Session>,
    turns: &mut Vec<Turn>,
    status: &mut StatusRuntime,
    prompt: String,
) -> Result<SubmitResult> {
    let started = std::time::Instant::now();
    let active = match session {
        Some(value) => value,
        None => session.insert(context.sessions.create(context.project, Some(&prompt))?),
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
    context.sessions.append(
        &active.id,
        &Turn {
            role: Role::User,
            content: prompt.clone(),
        },
    )?;
    if !turns.is_empty() {
        insert_blank(terminal)?;
    }
    insert_message(terminal, &prompt)?;
    let empty_input = ChatInput::default();
    let mut footer = footer_line(
        context.status_config,
        context.provider,
        context.project,
        status,
    );
    terminal.draw(|frame| render_with_panel(frame, &empty_input, &[], &footer))?;
    terminal.show_cursor()?;
    let mut response = String::new();
    let mut visible = String::new();
    let mut reasoning = String::new();
    let mut response_started = false;
    let mut response_output_started = false;
    let mut tools = ToolUi::default();
    let mut stream_height = 3;
    let mut phase = "thinking";
    let mut steering = SteeringQueue::default();
    let mut steering_input = ChatInput::default();
    let mut cancelled = false;
    let mut animation_frame = 0;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let task_provider = context.provider.clone();
    let task_prompt = prompt.clone();
    let task_history = history.clone();
    let task_tools = context.tools.clone();
    let task = tokio::spawn(async move {
        artist_agent::stream_chat(
            &task_provider,
            &task_prompt,
            &task_history,
            &task_tools,
            |event| {
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
        &visible,
        true,
        &status_line(phase, started.elapsed(), animation_frame),
        StreamingControls {
            input: &steering_input,
            steering: &steering,
        },
        &footer,
        &mut stream_height,
    )?;
    while !task.is_finished() || !rx.is_empty() {
        tokio::select! {
            _ = ticker.tick() => {
                animation_frame = animation_frame.wrapping_add(1);
                while event::poll(std::time::Duration::ZERO)? {
                    match event::read()? {
                        Event::Key(key) if key.kind == KeyEventKind::Press
                            && (key.code == KeyCode::Esc
                                || (key.code == KeyCode::Char('c')
                                    && key.modifiers.contains(KeyModifiers::CONTROL))) =>
                        {
                            task.abort();
                            cancelled = true;
                            break;
                        }
                        Event::Key(key) if key.kind == KeyEventKind::Press
                            && key.code == KeyCode::Enter
                            && !key.modifiers.contains(KeyModifiers::SHIFT)
                            && !steering_input.text.trim().is_empty() =>
                        {
                            steering.submit(std::mem::take(&mut steering_input.text));
                            steering_input.cursor = 0;
                        }
                        Event::Key(key) if key.kind == KeyEventKind::Press
                            && matches!(key.code, KeyCode::Up | KeyCode::Down) =>
                        {
                            if let Some(value) = steering.navigate(key.code == KeyCode::Up, &steering_input.text) {
                                steering_input.text = value;
                                steering_input.cursor = steering_input.text.len();
                            }
                        }
                        Event::Key(key) if key.kind == KeyEventKind::Press
                            && matches!(key.code, KeyCode::Backspace | KeyCode::Delete)
                            && steering.selected().is_some() =>
                        {
                            steering.remove_selected();
                            steering_input.text.clear();
                            steering_input.cursor = 0;
                        }
                        Event::Key(key) => { steering_input.handle_key(key); }
                        Event::Paste(text) => steering_input.insert(&text),
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
                            insert_reasoning(terminal, &reasoning)?;
                            reasoning.clear();
                        }
                        if !response_started {
                            response_started = true;
                        }
                        response.push_str(&delta);
                        visible.push_str(&delta);
                        let width = usize::from(terminal.size()?.width.saturating_sub(4).max(1));
                        while let Some(line) = take_visible_line(&mut visible, width) {
                            insert_response(terminal, &line, !response_output_started)?;
                            response_output_started = true;
                        }
                    }
                    artist_agent::PromptEvent::ReasoningSummaryDelta(delta) => {
                        phase = "thinking";
                        reasoning.push_str(&delta);
                    }
                    artist_agent::PromptEvent::ToolCall { id, name, arguments } => {
                        phase = "working";
                        if !reasoning.is_empty() {
                            insert_reasoning(terminal, &reasoning)?;
                            reasoning.clear();
                        }
                        let title = tools.start(id, &name, &arguments);
                        insert_tool_line(terminal, &title, true, false)?;
                    }
                    artist_agent::PromptEvent::ToolExecutionStart { .. } => phase = "working",
                    artist_agent::PromptEvent::ToolResult { id, content } => {
                        phase = "working";
                        let output = tools.output(&id, &content);
                        if !output.text.is_empty() {
                            insert_tool_line(terminal, &output.text, false, output.is_diff)?;
                        }
                        if output.batch_complete {
                            insert_blank(terminal)?;
                        }
                    }
                    artist_agent::PromptEvent::CompletionUsage { total_tokens } => {
                        if total_tokens > 0 {
                            status.used_tokens = Some(total_tokens);
                        }
                        footer = footer_line(
                            context.status_config,
                            context.provider,
                            context.project,
                            status,
                        );
                    }
                }
            }
        }
        draw_streaming(
            terminal,
            &visible,
            !response_output_started,
            &status_line(phase, started.elapsed(), animation_frame),
            StreamingControls {
                input: &steering_input,
                steering: &steering,
            },
            &footer,
            &mut stream_height,
        )?;
    }
    let stream_result = if cancelled {
        let _ = task.await;
        None
    } else {
        Some(task.await.context("join Artist agent")?)
    };
    if !reasoning.is_empty() {
        insert_reasoning(terminal, &reasoning)?;
    }
    if !visible.is_empty() {
        insert_response(terminal, &visible, !response_output_started)?;
    }
    insert_blank(terminal)?;
    insert_status(
        terminal,
        &if cancelled {
            format!("  stopped · {}", format_elapsed(started.elapsed()))
        } else {
            format!("  {}", format_elapsed(started.elapsed()))
        },
    )?;
    resize_and_draw(
        terminal,
        &ChatInput::default(),
        &[],
        &footer,
        &mut stream_height,
        3,
    )?;
    if let Some(result) = stream_result {
        result?;
    }
    turns.push(Turn {
        role: Role::User,
        content: prompt,
    });
    context.sessions.append(
        &active.id,
        &Turn {
            role: Role::Assistant,
            content: response.clone(),
        },
    )?;
    turns.push(Turn {
        role: Role::Assistant,
        content: response,
    });
    Ok(SubmitResult {
        viewport_height: stream_height,
        queued: steering.take(),
    })
}

fn take_visible_line(pending: &mut String, width: usize) -> Option<String> {
    if let Some(open) = pending.find("```") {
        let after_open = open + 3;
        let Some(close) = pending[after_open..].find("```") else {
            // Keep the complete fenced block together so Glamour retains the
            // language and can syntax-highlight content while it streams.
            return None;
        };
        let mut split = after_open + close + 3;
        if pending.as_bytes().get(split) == Some(&b'\n') {
            split += 1;
        }
        return Some(pending.drain(..split).collect());
    }
    let split = pending.find('\n').map(|index| index + 1).or_else(|| {
        let mut columns = 0;
        pending.char_indices().find_map(|(index, character)| {
            columns += character.width().unwrap_or(0);
            (columns > width).then_some(index)
        })
    })?;
    Some(pending.drain(..split).collect())
}

fn insert_history(terminal: &mut ratatui::DefaultTerminal, turns: &[Turn]) -> Result<()> {
    for turn in turns {
        match turn.role {
            Role::User => insert_message(terminal, &turn.content)?,
            Role::Assistant => {
                insert_response(terminal, &turn.content, true)?;
                insert_blank(terminal)?;
            }
        }
    }
    Ok(())
}

fn insert_message(terminal: &mut ratatui::DefaultTerminal, text: &str) -> Result<()> {
    let inner_width = usize::from(terminal.size()?.width.saturating_sub(2).max(1));
    let content_height = text
        .lines()
        .map(|line| UnicodeWidthStr::width(line).max(1).div_ceil(inner_width))
        .sum::<usize>()
        .max(1) as u16;
    terminal.insert_before(content_height.saturating_add(1), |buffer| {
        let text_area = Rect::new(
            buffer.area.x,
            buffer.area.y,
            buffer.area.width,
            content_height,
        );
        let highlighted_area = Rect::new(
            text_area.x.saturating_add(2),
            text_area.y,
            text_area.width.saturating_sub(2),
            text_area.height,
        );
        Paragraph::new(Text::styled(
            text,
            Style::default().fg(Color::Black).bg(Color::White),
        ))
        .wrap(Wrap { trim: false })
        .render(highlighted_area, buffer);
    })?;
    Ok(())
}

fn insert_blank(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    terminal.insert_before(1, |_| {})?;
    Ok(())
}

fn truncate_display_line(line: &str, width: usize) -> String {
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

fn insert_tool_line(
    terminal: &mut ratatui::DefaultTerminal,
    content: &str,
    first: bool,
    is_diff: bool,
) -> Result<()> {
    let prefix = if first { "  🛠  " } else { "    " };
    let width = usize::from(terminal.size()?.width.max(1));
    let text = content
        .lines()
        .enumerate()
        .map(|(index, line)| {
            // Tabs otherwise skip styled terminal cells. Tool lines are kept
            // to one terminal row so large diffs cannot dominate the UI.
            let line = line.replace('\t', "    ");
            let line_prefix = if index == 0 { prefix } else { "    " };
            let line =
                truncate_display_line(&line, width.saturating_sub(line_prefix.width()).max(1));
            let color = if first {
                Color::White
            } else if is_diff && line.starts_with('+') {
                Color::Rgb(120, 210, 140)
            } else if is_diff && line.starts_with('-') {
                Color::Rgb(235, 120, 120)
            } else if is_diff && line.starts_with("@@") {
                Color::Rgb(110, 190, 220)
            } else {
                Color::Rgb(175, 175, 175)
            };
            Line::styled(
                format!("{line_prefix}{line}"),
                Style::default().fg(color).bg(Color::Rgb(32, 32, 32)),
            )
        })
        .collect::<Vec<_>>();
    let height = text
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum::<usize>() as u16;
    terminal.insert_before(height.max(1), |buffer| {
        buffer.set_style(buffer.area, Style::default().bg(Color::Rgb(32, 32, 32)));
        Paragraph::new(Text::from(text))
            .wrap(Wrap { trim: false })
            .render(buffer.area, buffer);
    })?;
    Ok(())
}

fn insert_reasoning(terminal: &mut ratatui::DefaultTerminal, reasoning: &str) -> Result<()> {
    let text = reasoning_text(reasoning);
    let width = usize::from(terminal.size()?.width.max(1));
    let height = text
        .lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum::<usize>() as u16;
    terminal.insert_before(height.saturating_add(1), |buffer| {
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .render(buffer.area, buffer);
    })?;
    Ok(())
}

fn reasoning_text(reasoning: &str) -> Text<'static> {
    Text::from(
        reasoning
            .lines()
            .enumerate()
            .map(|(line_index, line)| {
                let mut spans = vec![Span::raw(if line_index == 0 { "  ◉ " } else { "    " })];
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

fn response_text(markdown: &str, first: bool, width: usize) -> Result<Text<'static>> {
    let mut style = glamour::Style::Dark.config();
    style.document.margin = Some(0);
    let rendered = glamour::Renderer::new()
        .with_style_config(style)
        .with_word_wrap(width.saturating_sub(4).max(1))
        .render(markdown);
    let mut text = rendered.into_text().context("parse Glamour output")?;
    while text.lines.first().is_some_and(line_is_blank) {
        text.lines.remove(0);
    }
    while text.lines.last().is_some_and(line_is_blank) {
        text.lines.pop();
    }
    for (index, line) in text.lines.iter_mut().enumerate() {
        let prefix = if first && index == 0 {
            "  ⋗ "
        } else {
            "    "
        };
        line.spans.insert(0, Span::raw(prefix));
    }
    Ok(text)
}

fn line_is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.trim().is_empty())
}

fn insert_response(
    terminal: &mut ratatui::DefaultTerminal,
    markdown: &str,
    first: bool,
) -> Result<()> {
    let width = usize::from(terminal.size()?.width.max(1));
    let text = response_text(markdown, first, width)?;
    let height = text.lines.len().max(1) as u16;
    terminal.insert_before(height, |buffer| {
        Paragraph::new(text).render(buffer.area, buffer);
    })?;
    Ok(())
}

fn status_line(phase: &str, elapsed: std::time::Duration, frame: usize) -> String {
    const FRAMES: [&str; 6] = ["▓", "▒", "░", " ", "░", "▒"];
    format!(
        "  {} {phase} [{} elapsed]",
        FRAMES[frame % FRAMES.len()],
        format_elapsed(elapsed)
    )
}

fn format_elapsed(elapsed: std::time::Duration) -> String {
    let seconds = elapsed.as_secs();
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours == 0 {
        format!("{minutes:02}:{seconds:02}")
    } else {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }
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
    response: &str,
    first: bool,
    status: &str,
    controls: StreamingControls<'_>,
    footer: &Line<'_>,
    viewport_height: &mut u16,
) -> Result<()> {
    let terminal_size = terminal.size()?;
    let width = terminal_size.width.max(1);
    let response_height = response
        .lines()
        .map(|line| {
            UnicodeWidthStr::width(line)
                .max(1)
                .div_ceil(usize::from(width))
        })
        .sum::<usize>()
        .max(1) as u16;
    // Keep a fixed one-row streaming tail above the input. Completed output is
    // inserted into scrollback, so the viewport never grows and repositions it.
    let visible_response_height = 1;
    let footer_height = (!footer.spans.is_empty()) as u16;
    let queued_height = controls.steering.entries().len() as u16;
    let input_height = controls
        .input
        .visual_lines(width.saturating_sub(2).max(1))
        .saturating_add(2);
    let desired = 3u16
        .saturating_add(queued_height)
        .saturating_add(input_height)
        .saturating_add(footer_height)
        .min(terminal_size.height);
    let resized = desired != *viewport_height;
    if resized {
        *viewport_height = desired;
        execute!(std::io::stdout(), BeginSynchronizedUpdate)?;
        clear_inline(terminal)?;
        *terminal = ratatui::init_with_options(TerminalOptions {
            viewport: Viewport::Inline(desired),
        });
    }
    terminal.draw(|frame| {
        let area = frame.area();
        let response_area = Rect::new(
            area.x,
            area.y,
            area.width,
            visible_response_height.min(area.height),
        );
        let text = response_text(response, first, usize::from(area.width))
            .unwrap_or_else(|_| Text::raw(response));
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .scroll((response_height.saturating_sub(visible_response_height), 0)),
            response_area,
        );
        let queued_area = Rect::new(
            area.x,
            response_area.bottom().saturating_add(1),
            area.width,
            queued_height.min(area.height),
        );
        let queued = controls
            .steering
            .entries()
            .iter()
            .enumerate()
            .map(|(index, prompt)| {
                Line::styled(
                    truncate_display_line(
                        &format!("  queued: {prompt}"),
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
        let status_area = Rect::new(area.x, queued_area.bottom(), area.width, 1);
        frame.render_widget(
            Paragraph::new(status).style(Style::default().fg(Color::DarkGray)),
            status_area,
        );
        let input_area = Rect::new(
            area.x,
            status_area.bottom(),
            area.width,
            area.height.saturating_sub(
                response_area
                    .height
                    .saturating_add(queued_height + 2 + footer_height),
            ),
        );
        render_input(frame, input_area, controls.input);
        if footer_height == 1 {
            frame.render_widget(
                Paragraph::new(footer.clone()),
                Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
            );
        }
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
    footer: &Line<'_>,
) {
    let area = frame.area();
    let status_height = (!footer.spans.is_empty()) as u16;
    if status_height == 1 {
        frame.render_widget(
            Paragraph::new(footer.clone()),
            Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
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
    frame.render_widget(
        Paragraph::new(panel.join("\n")).block(
            Block::default()
                .borders(Borders::TOP | Borders::BOTTOM)
                .border_style(Style::default().fg(Color::White)),
        ),
        panel_area,
    );
}

fn render_input(frame: &mut Frame<'_>, area: Rect, input: &ChatInput) {
    let inner_width = area.width.saturating_sub(2).max(1);
    let block = Block::default().borders(Borders::ALL);
    frame.render_widget(block, area);
    style_gradient_border(frame, area);

    let input_area = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let input_style = Style::default().fg(Color::White);
    let paragraph = Paragraph::new(Text::raw(&input.text))
        .wrap(Wrap { trim: false })
        .style(input_style);
    frame.render_widget(paragraph, input_area);

    if input_area.width > 0 && input_area.height > 0 {
        let (x, y) = input.cursor_position(inner_width);
        frame.set_cursor_position((
            input_area.x + x.min(inner_width.saturating_sub(1)),
            input_area.y + y.min(input_area.height.saturating_sub(1)),
        ));
    }
}

fn clear_inline(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    let top = terminal.get_frame().area().y;
    execute!(
        std::io::stdout(),
        Hide,
        MoveTo(0, top),
        Clear(ClearType::FromCursorDown)
    )?;
    Ok(())
}

fn style_gradient_border(frame: &mut Frame<'_>, area: Rect) {
    style_gradient_buffer(frame.buffer_mut(), area);
}

fn style_gradient_buffer(buffer: &mut Buffer, area: Rect) {
    if area.is_empty() {
        return;
    }
    let last_row = area.height.saturating_sub(1);
    for row in 0..area.height {
        // Keep the original three-row gradient stable as the box grows. New rows
        // continue with its final white shade instead of recoloring existing rows.
        let shade = match row {
            0 => 128,
            1 => 191,
            _ => 255,
        };
        let style = Style::default().fg(Color::Rgb(shade, shade, shade));
        let y = area.y + row;
        if row == 0 || row == last_row {
            for x in area.x..area.right() {
                buffer.cell_mut((x, y)).unwrap().set_style(style);
            }
        } else {
            buffer.cell_mut((area.x, y)).unwrap().set_style(style);
            if area.width > 1 {
                buffer
                    .cell_mut((area.right() - 1, y))
                    .unwrap()
                    .set_style(style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

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
    fn glamour_styles_and_indents_responses() {
        let text = response_text("**hello**", true, 80).unwrap();
        let rendered = text
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("⋗"));
        assert!(rendered.contains("hello"));
        assert!(!rendered.contains("**"));
    }

    #[test]
    fn glamour_syntax_highlights_fenced_code() {
        let text = response_text("```rust\nfn main() { let answer = 42; }\n```", true, 80).unwrap();
        let colors = text
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .filter_map(|span| span.style.fg)
            .collect::<std::collections::HashSet<_>>();
        assert!(
            colors.len() > 1,
            "expected multiple syntax colors: {colors:?}"
        );
    }

    #[test]
    fn truncates_tool_lines_to_terminal_columns() {
        assert_eq!(truncate_display_line("abcdef", 5), "abcd…");
        assert_eq!(truncate_display_line("ab界cd", 5), "ab界…");
        assert_eq!(truncate_display_line("short", 8), "short");
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
            "  ▓ thinking [00:00 elapsed]"
        );
        assert!(status_line("working", std::time::Duration::ZERO, 3).starts_with("    working"));
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
                    &Line::default(),
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
    fn status_bar_renders_below_input() {
        let backend = TestBackend::new(20, 4);
        let mut terminal = Terminal::new(backend).unwrap();
        let footer = Line::styled("model", Style::default().fg(Color::Black).bg(Color::Gray));
        terminal
            .draw(|frame| render_with_panel(frame, &ChatInput::default(), &[], &footer))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "┌");
        assert_eq!(buffer.cell((0, 2)).unwrap().symbol(), "└");
        assert_eq!(buffer.cell((0, 3)).unwrap().symbol(), "m");
        assert_eq!(buffer.cell((0, 3)).unwrap().bg, Color::Gray);
    }

    #[test]
    fn renders_at_full_width() {
        let backend = TestBackend::new(20, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_with_panel(frame, &ChatInput::default(), &[], &Line::default()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "┌");
        assert_eq!(buffer.cell((19, 0)).unwrap().symbol(), "┐");
        assert_eq!(buffer.cell((1, 1)).unwrap().bg, Color::Reset);
        assert_eq!(buffer.cell((0, 0)).unwrap().fg, Color::Rgb(128, 128, 128));
        assert_eq!(buffer.cell((0, 2)).unwrap().fg, Color::Rgb(255, 255, 255));
    }
}
