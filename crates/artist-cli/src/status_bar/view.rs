use super::row::{plain_line, plain_segment, plain_text, powerline_line, render_row};
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
        let mut left = powerline_line(&repo);
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
            left = powerline_line(&repo[1..]);
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
