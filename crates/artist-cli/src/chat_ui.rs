use anyhow::Result;
use ratatui::{
    Frame,
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::Text,
    widgets::{Block, Borders, Paragraph, Wrap},
};

#[derive(Default)]
struct ChatInput {
    text: String,
}

impl ChatInput {
    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return true;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => return false,
            (KeyCode::Char(character), _) => self.text.push(character),
            (KeyCode::Enter, _) => self.text.push('\n'),
            (KeyCode::Backspace, _) => {
                self.text.pop();
            }
            _ => {}
        }
        true
    }

    fn visual_lines(&self, inner_width: u16) -> u16 {
        let width = usize::from(inner_width.max(1));
        let mut lines = self.text.split('\n').collect::<Vec<_>>();
        let last = lines.pop().unwrap_or_default().chars().count();
        let previous = lines
            .into_iter()
            .map(|line| line.chars().count().max(1).div_ceil(width))
            .sum::<usize>();
        (previous + last / width + 1) as u16
    }

    fn cursor(&self, inner_width: u16) -> (u16, u16) {
        let width = usize::from(inner_width.max(1));
        let before_last = self
            .text
            .split('\n')
            .rev()
            .skip(1)
            .map(|line| line.chars().count().max(1).div_ceil(width))
            .sum::<usize>();
        let last = self
            .text
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .count();
        ((last % width) as u16, (before_last + last / width) as u16)
    }
}

/// Opens the input-only chat design surface. No prompts are submitted.
pub fn run() -> Result<()> {
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal);
    ratatui::restore();
    result
}

fn run_loop(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    let mut input = ChatInput::default();
    loop {
        terminal.draw(|frame| render(frame, &input))?;
        match event::read()? {
            Event::Key(key) if !input.handle_key(key) => return Ok(()),
            Event::Resize(_, _) => {}
            Event::Paste(text) => input.text.push_str(&text),
            _ => {}
        }
    }
}

fn render(frame: &mut Frame<'_>, input: &ChatInput) {
    let area = frame.area();
    let inner_width = area.width.saturating_sub(2).max(1);
    let desired_height = input.visual_lines(inner_width).saturating_add(2);
    let height = desired_height.min(area.height);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(height)])
        .split(area);
    let input_area = chunks[1];
    let style = Style::default().fg(Color::White).bg(Color::DarkGray);
    let paragraph = Paragraph::new(Text::raw(&input.text))
        .wrap(Wrap { trim: false })
        .style(style)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::White)),
        );
    frame.render_widget(paragraph, input_area);

    if input_area.width > 2 && input_area.height > 2 {
        let (x, y) = input.cursor(inner_width);
        frame.set_cursor_position((
            input_area.x + 1 + x.min(inner_width.saturating_sub(1)),
            input_area.y + 1 + y.min(input_area.height.saturating_sub(3)),
        ));
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
        input.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert_eq!(input.text, "a\nb");
        assert_eq!(input.visual_lines(10), 2);
        assert_eq!(
            ChatInput {
                text: "1234".into()
            }
            .visual_lines(4),
            2
        );
        input.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(input.text, "a\n");
    }

    #[test]
    fn renders_at_full_width() {
        let backend = TestBackend::new(20, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, &ChatInput::default()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 3)).unwrap().symbol(), "┌");
        assert_eq!(buffer.cell((19, 3)).unwrap().symbol(), "┐");
        assert_eq!(buffer.cell((1, 4)).unwrap().bg, Color::DarkGray);
    }
}
