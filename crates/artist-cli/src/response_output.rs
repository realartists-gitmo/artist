use ratatui::text::{Line, Span, Text};
use unicode_width::UnicodeWidthChar;

const PREFIX: &str = "   ";
const INDENT: &str = "    ";

pub(crate) fn text(output: &str, first: bool, terminal_width: usize) -> Text<'static> {
    let content_width = terminal_width.saturating_sub(INDENT.len()).max(1);
    Text::from(
        wrapped_lines(output, content_width)
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                let prefix = if first && index == 0 { PREFIX } else { INDENT };
                Line::from(vec![Span::raw(prefix), Span::raw(line)])
            })
            .collect::<Vec<_>>(),
    )
}

fn wrapped_lines(output: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut columns = 0usize;

    for character in output.chars() {
        if character == '\n' {
            lines.push(std::mem::take(&mut line));
            columns = 0;
            continue;
        }

        let character_width = character.width().unwrap_or(0);
        if character_width > width {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                columns = 0;
            }
            lines.push("�".to_owned());
            continue;
        }
        if columns > 0 && columns.saturating_add(character_width) > width {
            lines.push(std::mem::take(&mut line));
            columns = 0;
        }
        line.push(character);
        columns = columns.saturating_add(character_width);
    }

    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn preserves_markdown_as_unstyled_literal_text() {
        let rendered = text("**bold** and `code`\n```rust", true, 80);
        let lines = rendered
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert_eq!(lines, ["   **bold** and `code`", "    ```rust"]);
        assert!(
            rendered
                .lines
                .iter()
                .flat_map(|line| &line.spans)
                .all(|span| span.style == Style::default())
        );
    }

    #[test]
    fn wraps_content_with_the_existing_four_column_indent() {
        let rendered = text("1234567", true, 10);
        let lines = rendered
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert_eq!(lines, ["   123456", "    7"]);
        assert!(lines.iter().all(|line| line.width() <= 10));
    }
}
