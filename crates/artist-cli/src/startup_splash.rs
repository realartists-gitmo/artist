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

/// The transgender pride flag, top to bottom. Swap this table to retheme.
const STRIPES: [(u8, u8, u8); 5] = [
    (91, 206, 250),  // light blue
    (245, 169, 184), // pink
    (255, 255, 255), // white
    (245, 169, 184), // pink
    (91, 206, 250),  // light blue
];

/// The stripe covering `row`. Rows sample the flag at their midpoint, so an
/// even art height splits symmetrically (8 rows → 2/1/2/1/2).
fn stripe_color(row: usize) -> Color {
    let t = (row as f32 + 0.5) / HEIGHT as f32;
    let stripe = ((t * STRIPES.len() as f32) as usize).min(STRIPES.len() - 1);
    let (r, g, b) = STRIPES[stripe];
    Color::Rgb(r, g, b)
}

fn splash_text() -> Text<'static> {
    Text::from(
        ART.iter()
            .enumerate()
            .map(|(row, text)| Line::styled(*text, Style::default().fg(stripe_color(row))))
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
    fn renders_art_as_symmetric_trans_flag_stripes() {
        let mut terminal = Terminal::new(TestBackend::new(64, HEIGHT)).unwrap();
        terminal.draw(|frame| render(frame, frame.area())).unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((22, 0)).unwrap().symbol(), "█");
        let row = |y: u16| buffer.cell((22, y)).unwrap().fg;
        let blue = Color::Rgb(91, 206, 250);
        let pink = Color::Rgb(245, 169, 184);
        let white = Color::Rgb(255, 255, 255);
        // 8 rows → 2 blue, 1 pink, 2 white, 1 pink, 2 blue.
        assert_eq!(
            (0..HEIGHT).map(row).collect::<Vec<_>>(),
            [blue, blue, pink, white, white, pink, blue, blue]
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
