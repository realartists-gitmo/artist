use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::Text,
    widgets::{Paragraph, Widget},
};

pub(crate) fn frame_height(text: &str, width: u16) -> u16 {
    let inner_width = width.saturating_sub(2).max(1);
    let content_height = crate::text_wrap::plain(text, inner_width)
        .split('\n')
        .count()
        .max(1) as u16;

    content_height.saturating_add(2)
}

pub(crate) fn render(buffer: &mut Buffer, area: Rect, text: &str) {
    crate::input_border::render(buffer, area);

    let content_area = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let wrapped = crate::text_wrap::plain(text, content_area.width.max(1));
    Paragraph::new(Text::styled(
        wrapped,
        Style::default().fg(crate::theme::PASTEL_WHITE),
    ))
    .render(content_area, buffer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{buffer::Buffer, style::Color};

    #[test]
    fn sent_message_matches_input_box_styling() {
        let area = Rect::new(0, 0, 12, 3);
        let mut buffer = Buffer::empty(area);

        render(&mut buffer, area, "hello");

        assert_eq!(buffer[(0, 0)].symbol(), "┌");
        assert_eq!(buffer[(11, 0)].symbol(), "┐");
        assert_eq!(buffer[(0, 2)].symbol(), "└");
        assert_eq!(buffer[(11, 2)].symbol(), "┘");
        assert_eq!(buffer[(0, 0)].fg, crate::theme::PASTEL_PINK);
        assert_eq!(buffer[(1, 1)].fg, crate::theme::PASTEL_WHITE);
        assert_eq!(buffer[(1, 1)].bg, Color::Reset);
        assert_eq!(buffer[(1, 1)].symbol(), "h");
    }

    #[test]
    fn frame_height_accounts_for_borders_wrapping_and_newlines() {
        assert_eq!(frame_height("hello", 12), 3);
        assert_eq!(frame_height("12345678901", 12), 4);
        assert_eq!(frame_height("one\ntwo", 12), 4);
        assert_eq!(frame_height("one\n", 12), 4);
    }
}
