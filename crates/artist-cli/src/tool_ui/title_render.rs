use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{TitleSegment, ToolTitle};

pub(crate) fn title_spans(title: &ToolTitle, width: usize, accent: Color) -> Vec<Span<'static>> {
    let segments = title
        .segments
        .iter()
        .map(|segment| match segment {
            TitleSegment::Prose(text) => (flatten(text), false),
            TitleSegment::Input(text) => (flatten(text), true),
        })
        .collect::<Vec<_>>();
    let total_width = segments.iter().map(|(text, _)| text.width()).sum::<usize>();
    let truncated = total_width > width;
    let target = width.saturating_sub(usize::from(truncated));
    let mut used = 0;
    let mut spans = Vec::new();
    for (text, is_input) in segments {
        let mut visible = String::new();
        let mut clipped = false;
        for character in text.chars() {
            let next = used + character.width().unwrap_or(0);
            if next > target {
                clipped = true;
                break;
            }
            visible.push(character);
            used = next;
        }
        if !visible.is_empty() {
            let mut style = Style::default()
                .fg(if is_input {
                    accent
                } else {
                    crate::theme::PASTEL_WHITE
                })
                .bg(crate::theme::PANEL_BACKGROUND);
            if is_input {
                style = style.add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled(visible, style));
        }
        if clipped || used >= target {
            break;
        }
    }
    if truncated && width > 0 {
        spans.push(Span::styled(
            "…",
            Style::default()
                .fg(crate::theme::PASTEL_WHITE)
                .bg(crate::theme::PANEL_BACKGROUND),
        ));
    }
    spans
}

fn flatten(text: &str) -> String {
    text.replace('\t', "    ").replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn inputs_use_the_tool_accent_while_prose_stays_white() {
        let title = ToolTitle {
            segments: vec![
                TitleSegment::Prose("Read ".into()),
                TitleSegment::Input("src/lib.rs".into()),
            ],
        };
        let spans = title_spans(&title, 80, crate::theme::PASTEL_PINK);
        assert_eq!(text(&spans), "Read src/lib.rs");
        assert_eq!(spans[0].style.fg, Some(crate::theme::PASTEL_WHITE));
        assert_eq!(spans[1].style.fg, Some(crate::theme::PASTEL_PINK));
        assert!(spans[1].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn truncation_is_single_line_unicode_safe_and_width_bounded() {
        let title = ToolTitle {
            segments: vec![TitleSegment::Input("界\tcommand\ncontinued".into())],
        };
        let spans = title_spans(&title, 10, crate::theme::PASTEL_MINT);
        let rendered = text(&spans);
        assert!(rendered.width() <= 10);
        assert!(rendered.ends_with('…'));
        assert!(!rendered.contains(['\n', '\t']));
    }
}
