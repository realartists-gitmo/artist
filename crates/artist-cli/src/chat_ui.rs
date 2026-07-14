use anyhow::Result;
use ratatui::{
    Frame, TerminalOptions, Viewport,
    crossterm::{
        cursor::MoveTo,
        event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
        execute,
        terminal::{Clear, ClearType},
    },
    layout::Rect,
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
            (KeyCode::Enter, modifiers) if modifiers.contains(KeyModifiers::SHIFT) => {
                self.text.push('\n');
            }
            (KeyCode::Enter, _) => {}
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
    let terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(3),
    });
    let result = run_loop(terminal);
    ratatui::restore();
    result
}

fn run_loop(mut terminal: ratatui::DefaultTerminal) -> Result<()> {
    let mut input = ChatInput::default();
    let mut viewport_height = 3;
    loop {
        let width = terminal.size()?.width.saturating_sub(2).max(1);
        let desired_height = input.visual_lines(width).saturating_add(2);
        if desired_height > viewport_height {
            viewport_height = desired_height;
            let top = terminal.get_frame().area().y;
            execute!(
                std::io::stdout(),
                MoveTo(0, top),
                Clear(ClearType::FromCursorDown)
            )?;
            terminal = ratatui::init_with_options(TerminalOptions {
                viewport: Viewport::Inline(viewport_height),
            });
        }
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
        let (x, y) = input.cursor(inner_width);
        frame.set_cursor_position((
            input_area.x + x.min(inner_width.saturating_sub(1)),
            input_area.y + y.min(input_area.height.saturating_sub(1)),
        ));
    }
}

fn style_gradient_border(frame: &mut Frame<'_>, area: Rect) {
    if area.is_empty() {
        return;
    }
    let last_row = area.height.saturating_sub(1);
    for row in 0..area.height {
        let shade = (128 + (127 * row).checked_div(last_row).unwrap_or(127)) as u8;
        let style = Style::default().fg(Color::Rgb(shade, shade, shade));
        let y = area.y + row;
        if row == 0 || row == last_row {
            for x in area.x..area.right() {
                frame
                    .buffer_mut()
                    .cell_mut((x, y))
                    .unwrap()
                    .set_style(style);
            }
        } else {
            frame
                .buffer_mut()
                .cell_mut((area.x, y))
                .unwrap()
                .set_style(style);
            if area.width > 1 {
                frame
                    .buffer_mut()
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
        let backend = TestBackend::new(20, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, &ChatInput::default()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "┌");
        assert_eq!(buffer.cell((19, 0)).unwrap().symbol(), "┐");
        assert_eq!(buffer.cell((1, 1)).unwrap().bg, Color::Reset);
        assert_eq!(buffer.cell((0, 0)).unwrap().fg, Color::Rgb(128, 128, 128));
        assert_eq!(buffer.cell((0, 2)).unwrap().fg, Color::Rgb(255, 255, 255));
    }
}
