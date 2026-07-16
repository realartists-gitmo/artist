use crate::sessions::{Role, Session, SessionStore, Turn};
use anyhow::Result;
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
    style::{Color, Style},
    text::Text,
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};
use std::{io::IsTerminal, path::Path};
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

/// Runs an inline, persistent multi-turn chat. A session is created on first submission.
pub async fn run(
    provider: &SavedProvider,
    sessions: &SessionStore,
    project: &Path,
    resumed: Option<(Session, Vec<Turn>)>,
    initial_prompt: Option<String>,
) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("interactive chat requires a terminal; use -p for non-interactive prompts");
    }
    let terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(4),
    });
    let keyboard_result = execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    let result = match keyboard_result {
        Ok(()) => {
            run_loop(
                terminal,
                provider,
                sessions,
                project,
                resumed,
                initial_prompt,
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
    provider: &SavedProvider,
    sessions: &SessionStore,
    project: &Path,
    resumed: Option<(Session, Vec<Turn>)>,
    mut pending: Option<String>,
) -> Result<()> {
    let (mut session, mut turns) = resumed.map_or((None, Vec::new()), |(s, t)| (Some(s), t));
    let mut input = ChatInput::default();
    let mut viewport_height = 4;
    loop {
        resize_and_draw(&mut terminal, &input, &mut viewport_height)?;
        if let Some(prompt) = pending.take() {
            submit(
                &mut terminal,
                provider,
                sessions,
                project,
                &mut session,
                &mut turns,
                prompt,
            )
            .await?;
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
    viewport_height: &mut u16,
) -> Result<()> {
    let width = terminal.size()?.width.saturating_sub(2).max(1);
    // Reserve the top row for streamed output so the input frame never moves.
    let desired = input.visual_lines(width).saturating_add(3);
    if desired != *viewport_height {
        *viewport_height = desired;
        execute!(std::io::stdout(), BeginSynchronizedUpdate)?;
        clear_inline(terminal)?;
        *terminal = ratatui::init_with_options(TerminalOptions {
            viewport: Viewport::Inline(desired),
        });
        terminal.draw(|frame| render(frame, input))?;
        terminal.show_cursor()?;
        execute!(std::io::stdout(), EndSynchronizedUpdate)?;
    } else {
        terminal.draw(|frame| render(frame, input))?;
    }
    Ok(())
}

async fn submit(
    terminal: &mut ratatui::DefaultTerminal,
    provider: &SavedProvider,
    sessions: &SessionStore,
    project: &Path,
    session: &mut Option<Session>,
    turns: &mut Vec<Turn>,
    prompt: String,
) -> Result<()> {
    let active = match session {
        Some(value) => value,
        None => session.insert(sessions.create(project, Some(&prompt))?),
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
        &active.id,
        &Turn {
            role: Role::User,
            content: prompt.clone(),
        },
    )?;
    insert_message(terminal, &prompt)?;
    let mut response = String::new();
    let mut visible = String::new();
    let mut stream_height = 4;
    artist_agent::stream_chat(provider, &prompt, &history, |event| {
        match event {
            artist_agent::PromptEvent::TextDelta(delta) => {
                response.push_str(&delta);
                visible.push_str(&delta);
                let width = usize::from(terminal.size()?.width.max(1));
                while let Some(line) = take_visible_line(&mut visible, width) {
                    insert_text(terminal, &line, Color::White)?;
                }
                draw_streaming(terminal, &visible, &mut stream_height)?;
            }
            artist_agent::PromptEvent::ReasoningSummaryDelta(delta) => {
                insert_text(terminal, &delta, Color::DarkGray)?;
            }
        }
        Ok(())
    })
    .await?;
    if !visible.is_empty() {
        insert_text(terminal, &visible, Color::White)?;
    }
    resize_and_draw(terminal, &ChatInput::default(), &mut stream_height)?;
    turns.push(Turn {
        role: Role::User,
        content: prompt,
    });
    sessions.append(
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
    Ok(())
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

fn insert_message(terminal: &mut ratatui::DefaultTerminal, text: &str) -> Result<()> {
    let inner_width = usize::from(terminal.size()?.width.saturating_sub(2).max(1));
    let content_height = text
        .lines()
        .map(|line| UnicodeWidthStr::width(line).max(1).div_ceil(inner_width))
        .sum::<usize>()
        .max(1) as u16;
    terminal.insert_before(content_height.saturating_add(2), |buffer| {
        let area = buffer.area;
        Block::default().borders(Borders::ALL).render(area, buffer);
        style_gradient_buffer(buffer, area);
        let inner = Rect::new(
            area.x.saturating_add(1),
            area.y.saturating_add(1),
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        Paragraph::new(text)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: false })
            .render(inner, buffer);
    })?;
    Ok(())
}

fn insert_text(terminal: &mut ratatui::DefaultTerminal, text: &str, color: Color) -> Result<()> {
    let width = usize::from(terminal.size()?.width.max(1));
    let height = text
        .lines()
        .map(|line| UnicodeWidthStr::width(line).max(1).div_ceil(width))
        .sum::<usize>()
        .max(1) as u16;
    terminal.insert_before(height, |buffer| {
        Paragraph::new(text)
            .style(Style::default().fg(color))
            .wrap(Wrap { trim: false })
            .render(buffer.area, buffer);
    })?;
    Ok(())
}

fn draw_streaming(
    terminal: &mut ratatui::DefaultTerminal,
    response: &str,
    viewport_height: &mut u16,
) -> Result<()> {
    let width = terminal.size()?.width.max(1);
    let response_height = response
        .lines()
        .map(|line| {
            UnicodeWidthStr::width(line)
                .max(1)
                .div_ceil(usize::from(width))
        })
        .sum::<usize>()
        .max(1) as u16;
    let desired = response_height.saturating_add(3);
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
        let response_area = Rect::new(area.x, area.y, area.width, response_height.min(area.height));
        frame.render_widget(
            Paragraph::new(response)
                .style(Style::default().fg(Color::White))
                .wrap(Wrap { trim: false }),
            response_area,
        );
        let input_area = Rect::new(
            area.x,
            response_area.bottom(),
            area.width,
            area.height.saturating_sub(response_area.height),
        );
        render_input(frame, input_area, &ChatInput::default());
    })?;
    terminal.show_cursor()?;
    if resized {
        execute!(std::io::stdout(), EndSynchronizedUpdate)?;
    }
    Ok(())
}

fn render(frame: &mut Frame<'_>, input: &ChatInput) {
    let area = frame.area();
    let input_area = Rect::new(
        area.x,
        area.y.saturating_add(1),
        area.width,
        area.height.saturating_sub(1),
    );
    render_input(frame, input_area, input);
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
    fn renders_at_full_width() {
        let backend = TestBackend::new(20, 4);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, &ChatInput::default()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 1)).unwrap().symbol(), "┌");
        assert_eq!(buffer.cell((19, 1)).unwrap().symbol(), "┐");
        assert_eq!(buffer.cell((1, 2)).unwrap().bg, Color::Reset);
        assert_eq!(buffer.cell((0, 1)).unwrap().fg, Color::Rgb(128, 128, 128));
        assert_eq!(buffer.cell((0, 3)).unwrap().fg, Color::Rgb(255, 255, 255));
    }
}
