use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span, Text},
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

/// Gradient endpoints, top-left → bottom-right. Change these two to retheme.
const GRADIENT_START: (u8, u8, u8) = (129, 80, 223); // violet
const GRADIENT_END: (u8, u8, u8) = (255, 148, 66); // amber

fn lerp_color(t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let channel = |start: u8, end: u8| (start as f32 + (end as f32 - start as f32) * t) as u8;
    Color::Rgb(
        channel(GRADIENT_START.0, GRADIENT_END.0),
        channel(GRADIENT_START.1, GRADIENT_END.1),
        channel(GRADIENT_START.2, GRADIENT_END.2),
    )
}

fn splash_text() -> Text<'static> {
    let last_row = ART.len().saturating_sub(1).max(1);
    let last_col = ART
        .iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(1)
        .saturating_sub(1)
        .max(1);
    Text::from(
        ART.iter()
            .enumerate()
            .map(|(row, text)| {
                // Diagonal blend: equal weight to the row and column position.
                let row_t = row as f32 / last_row as f32;
                Line::from(
                    text.chars()
                        .enumerate()
                        .map(|(col, character)| {
                            let col_t = col as f32 / last_col as f32;
                            Span::styled(
                                character.to_string(),
                                Style::default().fg(lerp_color((row_t + col_t) / 2.0)),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
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
    fn renders_art_with_diagonal_color_gradient() {
        let mut terminal = Terminal::new(TestBackend::new(64, HEIGHT)).unwrap();
        terminal.draw(|frame| render(frame, frame.area())).unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((22, 0)).unwrap().symbol(), "█");
        // Top-left cell carries the pure start color…
        assert_eq!(
            buffer.cell((0, 0)).unwrap().fg,
            Color::Rgb(GRADIENT_START.0, GRADIENT_START.1, GRADIENT_START.2)
        );
        // …and the blend actually progresses toward the end color.
        let near = buffer.cell((1, 0)).unwrap().fg;
        let far = buffer.cell((50, HEIGHT - 1)).unwrap().fg;
        assert_ne!(near, far, "gradient must vary across the art");
        // The far cell's color matches the same diagonal blend the renderer
        // computes for that position.
        let last_col = ART.iter().map(|row| row.chars().count()).max().unwrap() - 1;
        let expected =
            lerp_color(((HEIGHT - 1) as f32 / (HEIGHT - 1) as f32 + 50.0 / last_col as f32) / 2.0);
        assert_eq!(far, expected);
    }

    #[test]
    fn clips_to_small_terminal_area() {
        let mut terminal = Terminal::new(TestBackend::new(20, 4)).unwrap();
        terminal
            .draw(|frame| render(frame, Rect::new(0, 0, 20, HEIGHT)))
            .unwrap();
    }
}
