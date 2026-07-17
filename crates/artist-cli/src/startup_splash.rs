use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Text},
    widgets::{Paragraph, Widget},
};

pub(crate) const HEIGHT: u16 = 8;

const ART: [&str; HEIGHT as usize] = [
    "                      █████     ███           █████",
    "                     ▒▒███     ▒▒▒           ▒▒███",
    "  ██████   ████████  ███████   ████   █████  ███████",
    " ▒▒▒▒▒███ ▒▒███▒▒███▒▒▒███▒   ▒▒███  ███▒▒  ▒▒▒███▒",
    "  ███████  ▒███ ▒▒▒   ▒███     ▒███ ▒▒█████   ▒███",
    " ███▒▒███  ▒███       ▒███ ███ ▒███  ▒▒▒▒███  ▒███ ███",
    "▒▒████████ █████      ▒▒█████  █████ ██████   ▒▒█████",
    " ▒▒▒▒▒▒▒▒ ▒▒▒▒▒        ▒▒▒▒▒  ▒▒▒▒▒ ▒▒▒▒▒▒     ▒▒▒▒▒",
];

fn splash_text() -> Text<'static> {
    let last_row = ART.len().saturating_sub(1).max(1);
    Text::from(
        ART.iter()
            .enumerate()
            .map(|(row, text)| {
                let shade = 128 + (127 * row / last_row) as u8;
                Line::styled(*text, Style::default().fg(Color::Rgb(shade, shade, shade)))
            })
            .collect::<Vec<_>>(),
    )
}

pub(crate) fn render(frame: &mut Frame<'_>, area: Rect) {
    let area = area.intersection(frame.area());
    if !area.is_empty() {
        frame.render_widget(Paragraph::new(splash_text()), area);
    }
}

pub(crate) fn render_buffer(buffer: &mut Buffer) {
    Paragraph::new(splash_text()).render(buffer.area, buffer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn renders_art_with_top_down_grayscale_gradient() {
        let mut terminal = Terminal::new(TestBackend::new(64, HEIGHT)).unwrap();
        terminal.draw(|frame| render(frame, frame.area())).unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((22, 0)).unwrap().symbol(), "█");
        assert_eq!(buffer.cell((22, 0)).unwrap().fg, Color::Rgb(128, 128, 128));
        assert_eq!(buffer.cell((1, HEIGHT - 1)).unwrap().symbol(), "▒");
        assert_eq!(
            buffer.cell((1, HEIGHT - 1)).unwrap().fg,
            Color::Rgb(255, 255, 255)
        );
    }

    #[test]
    fn clips_to_small_terminal_area() {
        let mut terminal = Terminal::new(TestBackend::new(20, 4)).unwrap();
        terminal
            .draw(|frame| render(frame, Rect::new(0, 0, 20, HEIGHT)))
            .unwrap();
    }
}
