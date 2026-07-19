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

const GRADIENT_START: (u8, u8, u8) = (64, 64, 64);
const GRADIENT_END: (u8, u8, u8) = (255, 255, 255);

/// Linearly interpolate the splash foreground from dark gray at the top to
/// white at the bottom.
fn gradient_color(row: usize) -> Color {
    let t = row as f32 / (HEIGHT - 1) as f32;
    let interpolate =
        |start: u8, end: u8| (start as f32 + (end as f32 - start as f32) * t).round() as u8;
    Color::Rgb(
        interpolate(GRADIENT_START.0, GRADIENT_END.0),
        interpolate(GRADIENT_START.1, GRADIENT_END.1),
        interpolate(GRADIENT_START.2, GRADIENT_END.2),
    )
}

fn splash_text() -> Text<'static> {
    Text::from(
        ART.iter()
            .enumerate()
            .map(|(row, text)| Line::styled(*text, Style::default().fg(gradient_color(row))))
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
    fn renders_art_with_dark_gray_to_white_gradient() {
        let mut terminal = Terminal::new(TestBackend::new(64, HEIGHT)).unwrap();
        terminal.draw(|frame| render(frame, frame.area())).unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((22, 0)).unwrap().symbol(), "█");
        let colors = (0..HEIGHT)
            .map(|y| buffer.cell((22, y)).unwrap().fg)
            .collect::<Vec<_>>();

        assert_eq!(colors.first(), Some(&Color::Rgb(64, 64, 64)));
        assert_eq!(colors.last(), Some(&Color::Rgb(255, 255, 255)));
        assert!(colors.windows(2).all(|pair| {
            let (Color::Rgb(previous, _, _), Color::Rgb(next, _, _)) = (pair[0], pair[1]) else {
                return false;
            };
            previous < next
        }));
    }

    #[test]
    fn clips_to_small_terminal_area() {
        let mut terminal = Terminal::new(TestBackend::new(20, 4)).unwrap();
        terminal
            .draw(|frame| render(frame, Rect::new(0, 0, 20, HEIGHT)))
            .unwrap();
    }
}
