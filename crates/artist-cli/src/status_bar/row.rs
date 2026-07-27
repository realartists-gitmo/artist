use super::StatusSegment;
use crate::theme::cycle_color;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
};

const POWERLINE_SEPARATOR: &str = "";

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

pub(super) fn powerline_line(segments: &[&StatusSegment]) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        let color = cycle_color(segment.palette_index);
        let next = segments
            .get(index + 1)
            .map(|next| cycle_color(next.palette_index))
            .unwrap_or(Color::Reset);
        spans.push(Span::styled(
            format!(" {} ", segment.text),
            Style::default().fg(Color::Black).bg(color),
        ));
        spans.push(Span::styled(
            POWERLINE_SEPARATOR,
            Style::default().fg(color).bg(next),
        ));
    }
    Line::from(spans)
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
    let right_width = show_right.then(|| right.width().min(width)).unwrap_or(0);
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
        theme::{PASTEL_BLUE, PASTEL_MINT, PASTEL_PINK, PASTEL_WHITE},
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
    fn powerline_transitions_follow_global_cycle_indices() {
        let project = segment(StatusItem::ProjectDirectory, "artist", 0);
        let branch = segment(StatusItem::GitBranch, "main", 1);
        let line = powerline_line(&[&project, &branch]);

        assert_eq!(line.spans[0].style.bg, Some(PASTEL_PINK));
        assert_eq!(line.spans[0].style.fg, Some(Color::Black));
        assert_eq!(line.spans[1].style.fg, Some(PASTEL_PINK));
        assert_eq!(line.spans[1].style.bg, Some(PASTEL_WHITE));
        assert_eq!(line.spans[2].style.bg, Some(PASTEL_WHITE));
        assert_eq!(line.spans[3].style.fg, Some(PASTEL_WHITE));
        assert_eq!(line.spans[3].style.bg, Some(Color::Reset));
    }

    #[test]
    fn plain_segments_keep_indices_after_regrouping_and_wrap() {
        let model = segment(StatusItem::Model, "gpt", 2);
        let context = segment(StatusItem::Context, "ctx", 4);
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
