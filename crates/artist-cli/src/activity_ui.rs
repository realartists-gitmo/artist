use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
const FALLBACK_ICON: &str = "•";
const ICON_WIDTH: usize = 2;
const MUTED: Color = Color::Rgb(175, 175, 175);
const ADDED: Color = Color::Rgb(120, 210, 140);
const REMOVED: Color = Color::Rgb(235, 120, 120);
pub(crate) fn tool_text(
    content: &str,
    first: bool,
    is_diff: bool,
    icon: Option<&str>,
    width: usize,
    accent: Color,
) -> Text<'static> {
    let rows = content.split('\n').enumerate().map(|(index, row)| {
        let row = row.trim_end_matches('\r').replace('\t', "    ");
        let shown_icon = (first && index == 0).then_some(icon.unwrap_or(FALLBACK_ICON));
        let icon = shown_icon.map(|value| truncate(value, ICON_WIDTH));
        let icon_padding =
            ICON_WIDTH.saturating_sub(icon.as_deref().map_or(0, UnicodeWidthStr::width));
        let content_color = if first {
            crate::theme::PASTEL_WHITE
        } else if is_diff && diff_body(&row).starts_with('+') {
            ADDED
        } else if is_diff && diff_body(&row).starts_with('-') {
            REMOVED
        } else {
            MUTED
        };
        let mut spans = Vec::new();
        let mut remaining = width;

        push(
            &mut spans,
            "  │ ",
            Style::default().fg(accent),
            &mut remaining,
        );
        if let Some(icon) = icon {
            push(
                &mut spans,
                &icon,
                Style::default().fg(accent),
                &mut remaining,
            );
        }
        push(
            &mut spans,
            &" ".repeat(icon_padding + 1),
            Style::default(),
            &mut remaining,
        );
        push(
            &mut spans,
            &row,
            Style::default().fg(content_color),
            &mut remaining,
        );
        Line::from(spans)
    });

    Text::from(rows.collect::<Vec<_>>())
}

fn diff_body(row: &str) -> &str {
    row.split_once("│ ").map_or(row, |(_, body)| body)
}

fn push(spans: &mut Vec<Span<'static>>, value: &str, style: Style, remaining: &mut usize) {
    if *remaining == 0 {
        return;
    }
    let value = truncate(value, *remaining);
    *remaining = remaining.saturating_sub(value.width());
    if !value.is_empty() {
        spans.push(Span::styled(value, style));
    }
}

fn truncate(value: &str, width: usize) -> String {
    let mut used = 0;
    value
        .chars()
        .take_while(|character| {
            let next = used + character.width().unwrap_or(0);
            let fits = next <= width;
            if fits {
                used = next;
            }
            fits
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_accent_rail_without_backgrounds() {
        let text = tool_text("Title", true, false, None, 40, crate::theme::PASTEL_PINK);
        assert_eq!(text.lines[0].spans[0].content, "  │ ");
        assert_eq!(
            text.lines[0].spans[0].style.fg,
            Some(crate::theme::PASTEL_PINK)
        );
        assert!(
            text.lines
                .iter()
                .flat_map(|line| &line.spans)
                .all(|span| span.style.bg.is_none())
        );
    }

    #[test]
    fn one_and_two_column_icons_align_titles() {
        let narrow = tool_text("Title", true, false, Some("•"), 40, Color::Cyan);
        let wide = tool_text("Title", true, false, Some("界"), 40, Color::Cyan);
        assert_eq!(narrow.lines[0].to_string(), "  │ •  Title");
        assert_eq!(wide.lines[0].to_string(), "  │ 界 Title");
        assert_eq!(
            narrow.lines[0].to_string()[.."  │ •  ".len()].width(),
            wide.lines[0].to_string()[.."  │ 界 ".len()].width()
        );
    }

    #[test]
    fn continuations_align_and_diffs_keep_color() {
        let text = tool_text("+one\n-two", false, true, None, 40, Color::Cyan);
        assert_eq!(text.lines[0].to_string(), "  │    +one");
        assert_eq!(text.lines[1].to_string(), "  │    -two");
        assert_eq!(text.lines[0].spans.last().unwrap().style.fg, Some(ADDED));
        assert_eq!(text.lines[1].spans.last().unwrap().style.fg, Some(REMOVED));
        assert_eq!(
            text.lines[0].to_string().find('+'),
            text.lines[1].to_string().find('-')
        );
    }

    #[test]
    fn tabs_expand_and_narrow_rows_are_truncated() {
        let text = tool_text("a\t界long", true, false, Some("•"), 10, Color::Cyan);
        assert_eq!(text.lines[0].width(), 10);
        assert!(!text.lines[0].to_string().contains('\t'));
    }
}
