use super::row::{plain_line, plain_segment, plain_segments, plain_text, render_row};
use super::{StatusItem, StatusSegment};
use ratatui::{buffer::Buffer, layout::Rect};

pub(crate) const HEIGHT: u16 = 2;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct StatusView {
    pub(super) segments: Vec<StatusSegment>,
}

impl StatusView {
    pub fn height(&self, width: u16) -> u16 {
        if self.segments.is_empty() || width == 0 {
            0
        } else {
            HEIGHT
        }
    }

    pub fn render(&self, buffer: &mut Buffer, area: Rect) {
        let height = self.height(area.width).min(area.height);
        if height == 0 {
            return;
        }
        self.render_top(buffer, Rect::new(area.x, area.y, area.width, 1));
        if height > 1 {
            self.render_bottom(buffer, Rect::new(area.x, area.y + 1, area.width, 1));
        }
    }

    fn render_top(&self, buffer: &mut Buffer, area: Rect) {
        let repo = self
            .segments
            .iter()
            .filter(|segment| {
                matches!(
                    segment.item,
                    StatusItem::ProjectDirectory | StatusItem::GitBranch
                )
            })
            .collect::<Vec<_>>();
        let context = self
            .segments
            .iter()
            .find(|segment| segment.item == StatusItem::Context);
        let full_right = context.map(plain_segment).unwrap_or_default();
        let mut right = full_right.clone();
        let mut left = plain_segments(&repo);
        if left.width() + right.width() + 1 > usize::from(area.width) {
            right = context
                .and_then(|segment| {
                    segment
                        .compact
                        .as_ref()
                        .map(|text| plain_text(segment, text))
                })
                .unwrap_or(full_right);
        }
        let right_space = right
            .width()
            .saturating_add(usize::from(!right.spans.is_empty()));
        let left_space = usize::from(area.width).saturating_sub(right_space);
        if left.width() > left_space && repo.len() > 1 {
            left = plain_segments(&repo[1..]);
        }
        render_row(buffer, area, &left, &right, true);
    }

    fn render_bottom(&self, buffer: &mut Buffer, area: Rect) {
        let left = plain_line(self.segments.iter().filter(|segment| {
            !matches!(
                segment.item,
                StatusItem::ProjectDirectory
                    | StatusItem::GitBranch
                    | StatusItem::Context
                    | StatusItem::SessionTokens
            )
        }));
        let right = self
            .segments
            .iter()
            .find(|segment| segment.item == StatusItem::SessionTokens)
            .map(plain_segment)
            .unwrap_or_default();
        render_row(buffer, area, &left, &right, false);
    }
}

pub(crate) fn view(segments: Vec<StatusSegment>) -> StatusView {
    StatusView { segments }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        status_bar::StatusItem,
        theme::{PASTEL_BLUE, PASTEL_BLUSH, PASTEL_WHITE},
    };

    fn segment(
        item: StatusItem,
        text: &str,
        compact: Option<&str>,
        palette_index: usize,
    ) -> StatusSegment {
        StatusSegment {
            item,
            text: text.into(),
            compact: compact.map(str::to_owned),
            palette_index,
        }
    }

    fn row_text(buffer: &Buffer, width: u16, row: u16) -> String {
        (0..width)
            .map(|column| buffer.cell((column, row)).unwrap().symbol())
            .collect::<String>()
    }

    #[test]
    fn renders_two_right_aligned_rows() {
        let width = 60;
        let view = view(vec![
            segment(StatusItem::ProjectDirectory, " artist", None, 0),
            segment(StatusItem::GitBranch, " main", None, 1),
            segment(StatusItem::Model, " gpt-5.4", None, 2),
            segment(StatusItem::Reasoning, " high", None, 3),
            segment(
                StatusItem::Context,
                " ctx ██████░░ 75% · 200k",
                Some(" ctx 75%"),
                4,
            ),
            segment(StatusItem::SessionTokens, " 1.5k total", None, 5),
        ]);
        let area = Rect::new(0, 0, width, HEIGHT);
        let mut buffer = Buffer::empty(area);

        view.render(&mut buffer, area);

        let top = row_text(&buffer, width, 0);
        assert!(top.starts_with("  artist •  main"));
        assert!(top.ends_with(" ctx ██████░░ 75% · 200k"));
        let bottom = row_text(&buffer, width, 1);
        assert!(bottom.starts_with("  gpt-5.4 •  high"));
        assert!(bottom.ends_with(" 1.5k total"));
        assert!(!top.contains(''));
        assert!((0..HEIGHT).all(|row| {
            (0..width).all(|column| {
                buffer.cell((column, row)).unwrap().bg == ratatui::style::Color::Reset
            })
        }));
        assert_eq!(buffer.cell((width - 1, 0)).unwrap().fg, PASTEL_BLUE);
        assert_eq!(buffer.cell((width - 1, 1)).unwrap().fg, PASTEL_BLUSH);
    }

    #[test]
    fn narrow_layout_drops_project_without_recoloring_branch() {
        let width = 20;
        let view = view(vec![
            segment(
                StatusItem::ProjectDirectory,
                " an-impossibly-long-project",
                None,
                0,
            ),
            segment(StatusItem::GitBranch, " main", None, 1),
            segment(
                StatusItem::Context,
                " ctx ██████░░ 75% · 100",
                Some(" ctx 75%"),
                4,
            ),
        ]);
        let area = Rect::new(0, 0, width, HEIGHT);
        let mut buffer = Buffer::empty(area);

        view.render(&mut buffer, area);

        let top = row_text(&buffer, width, 0);
        assert_eq!(top, "  main     ctx 75%");
        assert!(!top.contains(''));
        assert_eq!(buffer.cell((1, 0)).unwrap().fg, PASTEL_WHITE);
        assert_eq!(buffer.cell((11, 0)).unwrap().fg, PASTEL_BLUE);
        assert_eq!(
            buffer.cell((1, 0)).unwrap().bg,
            ratatui::style::Color::Reset
        );
    }

    #[test]
    fn empty_view_reserves_no_rows() {
        assert_eq!(StatusView::default().height(80), 0);
        assert_eq!(
            view(vec![segment(StatusItem::Model, "gpt", None, 0)]).height(80),
            HEIGHT
        );
    }
}
