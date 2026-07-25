use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget},
};

pub(crate) const HEIGHT: u16 = 1;

const ART: [&str; HEIGHT as usize] = ["Artist"];

fn splash_text(extension_ids: &[String]) -> Text<'static> {
    let mut lines = ART
        .iter()
        .map(|text| Line::styled((*text).to_owned(), Style::default().fg(Color::White)))
        .collect::<Vec<_>>();

    if !extension_ids.is_empty() {
        lines[HEIGHT as usize - 1].push_span(Span::styled(
            format!(" + {}", extension_ids.join(", ")),
            Style::default().fg(Color::DarkGray),
        ));
    }

    Text::from(lines)
}

pub(crate) fn render(frame: &mut Frame<'_>, area: Rect, extension_ids: &[String]) {
    let area = area.intersection(frame.area());
    if !area.is_empty() {
        frame.render_widget(Paragraph::new(splash_text(extension_ids)), area);
    }
}

pub(crate) fn render_buffer(buffer: &mut Buffer, extension_ids: &[String]) {
    Paragraph::new(splash_text(extension_ids)).render(buffer.area, buffer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn renders_plain_name_and_dark_extension_list() {
        let mut terminal = Terminal::new(TestBackend::new(80, HEIGHT)).unwrap();
        let extensions = vec!["extension1".to_owned(), "extension2".to_owned()];
        terminal
            .draw(|frame| render(frame, frame.area(), &extensions))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "A");
        assert_eq!(buffer.cell((0, 0)).unwrap().fg, Color::White);
        assert_eq!(buffer.cell((7, 0)).unwrap().symbol(), "+");
        assert_eq!(buffer.cell((7, 0)).unwrap().fg, Color::DarkGray);
    }

    #[test]
    fn clips_to_small_terminal_area() {
        let mut terminal = Terminal::new(TestBackend::new(20, 4)).unwrap();
        terminal
            .draw(|frame| render(frame, Rect::new(0, 0, 20, HEIGHT), &[]))
            .unwrap();
    }
}
