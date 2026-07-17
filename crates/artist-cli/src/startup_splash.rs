use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Text},
    widgets::Paragraph,
};

pub(crate) const HEIGHT: u16 = 8;

const ART: [&str; HEIGHT as usize] = [
    "█████     ███           █████",
    "                     ▒▒███     ▒▒▒           ▒▒███",
    "  ██████   ████████  ███████   ████   █████  ███████",
    " ▒▒▒▒▒███ ▒▒███▒▒███▒▒▒███▒   ▒▒███  ███▒▒  ▒▒▒███▒",
    "  ███████  ▒███ ▒▒▒   ▒███     ▒███ ▒▒█████   ▒███",
    " ███▒▒███  ▒███       ▒███ ███ ▒███  ▒▒▒▒███  ▒███ ███",
    "▒▒████████ █████      ▒▒█████  █████ ██████   ▒▒█████",
    " ▒▒▒▒▒▒▒▒ ▒▒▒▒▒        ▒▒▒▒▒  ▒▒▒▒▒ ▒▒▒▒▒▒     ▒▒▒▒▒",
];

pub(crate) fn render(frame: &mut Frame<'_>, area: Rect) {
    let last_row = ART.len().saturating_sub(1).max(1);
    let lines = ART
        .iter()
        .enumerate()
        .map(|(row, text)| {
            let shade = 128 + (127 * row / last_row) as u8;
            Line::styled(*text, Style::default().fg(Color::Rgb(shade, shade, shade)))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
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
        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "█");
        assert_eq!(buffer.cell((0, 0)).unwrap().fg, Color::Rgb(128, 128, 128));
        assert_eq!(buffer.cell((1, HEIGHT - 1)).unwrap().symbol(), "▒");
        assert_eq!(
            buffer.cell((1, HEIGHT - 1)).unwrap().fg,
            Color::Rgb(255, 255, 255)
        );
    }
}
