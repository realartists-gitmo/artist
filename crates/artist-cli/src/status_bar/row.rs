use super::StatusSegment;
use crate::theme::cycle_color;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
};

pub(super) fn plain_segment(segment: &StatusSegment) -> Line<'static> {
    plain_text(segment, &segment.text)
}

pub(super) fn plain_text(segment: &StatusSegment, text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_owned(),
        Style::default().fg(cycle_color(segment.palette_index)),
    ))
}

pub(super) fn plain_line<'a>(segments: impl Iterator<Item = &'a StatusSegment>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (index, segment) in segments.enumerate() {
        if index > 0 {
            spans.push(Span::styled(" • ", Style::default().fg(Color::DarkGray)));
        }
        spans.extend(plain_segment(segment).spans);
    }
    Line::from(spans)
}

pub(super) fn plain_segments(segments: &[&StatusSegment]) -> Line<'static> {
    plain_line(segments.iter().copied())
}

pub(super) fn render_row(
    buffer: &mut Buffer,
    area: Rect,
    left: &Line<'_>,
    right: &Line<'_>,
    right_priority: bool,
) {
    let width = usize::from(area.width);
    let show_right =
        !right.spans.is_empty() && (right_priority || left.width() + right.width() < width);
    let right_width = if show_right {
        right.width().min(width)
    } else {
        0
    };
    if right_width > 0 {
        buffer.set_line(
            area.right().saturating_sub(right_width as u16),
            area.y,
            right,
            right_width as u16,
        );
    }
    let gap = usize::from(right_width > 0 && !left.spans.is_empty());
    let left_width = width.saturating_sub(right_width + gap);
    if left_width > 0 {
        buffer.set_line(area.x, area.y, left, left_width as u16);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        status_bar::StatusItem,
        theme::{PASTEL_BLUE, PASTEL_BLUSH, PASTEL_MINT, PASTEL_PINK, PASTEL_WHITE, PASTEL_YELLOW},
    };

    fn segment(item: StatusItem, text: &str, palette_index: usize) -> StatusSegment {
        StatusSegment {
            item,
            text: text.into(),
            compact: None,
            palette_index,
        }
    }

    #[test]
    fn plain_line_uses_bullets_and_cycle_colors_without_backgrounds() {
        let segments = [
            segment(StatusItem::ProjectDirectory, " artist", 0),
            segment(StatusItem::GitBranch, " main", 1),
            segment(StatusItem::Model, " gpt-5.4", 2),
            segment(StatusItem::Reasoning, " high", 3),
            segment(StatusItem::Context, " ctx 75%", 4),
            segment(StatusItem::SessionTokens, " 1.5k total", 5),
        ];
        let line = plain_segments(&segments.iter().collect::<Vec<_>>());

        assert_eq!(
            line.to_string(),
            "  artist •  main •  gpt-5.4 •  high •  ctx 75% •  1.5k total"
        );
        let expected_colors = [
            PASTEL_PINK,
            PASTEL_WHITE,
            PASTEL_MINT,
            PASTEL_YELLOW,
            PASTEL_BLUE,
            PASTEL_BLUSH,
        ];
        for (span, expected) in line.spans.iter().skip(1).step_by(2).zip(expected_colors) {
            assert_eq!(span.style.fg, Some(expected));
        }
        assert!(
            line.spans
                .iter()
                .skip(2)
                .step_by(2)
                .all(|span| span.style.fg == Some(Color::DarkGray))
        );
        assert!(line.spans.iter().all(|span| span.style.bg.is_none()));
    }

    #[test]
    fn plain_segments_keep_indices_after_regrouping_and_wrap() {
        let model = segment(StatusItem::Model, " gpt", 2);
        let context = segment(StatusItem::Context, " ctx", 4);
        let extension = segment(StatusItem::Model, "quota", 6);

        assert_eq!(plain_segment(&model).spans[0].style.fg, Some(PASTEL_MINT));
        assert_eq!(plain_segment(&context).spans[0].style.fg, Some(PASTEL_BLUE));
        assert_eq!(
            plain_segment(&extension).spans[0].style.fg,
            Some(PASTEL_PINK)
        );
    }

    #[test]
    fn row_right_aligns_without_overlapping_left_content() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buffer = Buffer::empty(area);
        let left = Line::raw("left");
        let right = Line::raw("right");

        render_row(&mut buffer, area, &left, &right, true);

        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "l");
        assert_eq!(buffer.cell((15, 0)).unwrap().symbol(), "r");
        assert_eq!(buffer.cell((19, 0)).unwrap().symbol(), "t");
    }
}
