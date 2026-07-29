mod syntax;

use ratatui::{
    style::Style,
    text::{Line, Span, Text},
};
use unicode_width::UnicodeWidthChar;

const INDENT: &str = "    ";

#[derive(Default)]
pub(crate) struct Renderer {
    started: bool,
    fence: Option<FencedBlock>,
}

struct FencedBlock {
    marker: char,
    marker_len: usize,
    highlighter: syntax::CodeHighlighter,
}

impl Renderer {
    pub(crate) fn render(&mut self, output: &str, terminal_width: usize) -> Text<'static> {
        let content_width = terminal_width.saturating_sub(INDENT.len()).max(1);
        let mut lines = Vec::new();

        for (source_line, ends_line) in logical_lines(output) {
            let code_line = self.fence.is_some();
            let styled = self.style_line(source_line, ends_line);
            let wrapped = if code_line {
                wrap_spans_by_character(styled, content_width)
            } else {
                wrap_spans(styled, content_width)
            };
            for mut spans in wrapped {
                let mut prefixed = if self.started {
                    vec![Span::raw(INDENT)]
                } else {
                    self.started = true;
                    vec![
                        Span::raw("  "),
                        Span::styled(" ", Style::default().fg(crate::theme::PASTEL_BLUSH)),
                    ]
                };
                prefixed.append(&mut spans);
                lines.push(Line::from(prefixed));
            }
        }
        Text::from(lines)
    }

    pub(crate) fn reset(&mut self) {
        self.started = false;
        self.fence = None;
    }

    fn style_line(&mut self, line: &str, ends_line: bool) -> Vec<Span<'static>> {
        if self
            .fence
            .as_ref()
            .is_some_and(|fence| is_closing_fence(line, fence.marker, fence.marker_len))
        {
            self.fence = None;
            return fence_spans(line);
        }
        if let Some(fence) = self.fence.as_mut() {
            return fence.highlighter.highlight_line(line, ends_line);
        }
        if let Some((marker, marker_len, language)) = opening_fence(line) {
            self.fence = Some(FencedBlock {
                marker,
                marker_len,
                highlighter: syntax::CodeHighlighter::new(language),
            });
            return fence_spans(line);
        }
        markdown_spans(line)
    }
}

fn logical_lines(output: &str) -> Vec<(&str, bool)> {
    if output.is_empty() {
        return vec![("", false)];
    }
    output
        .split_inclusive('\n')
        .map(|line| {
            line.strip_suffix('\n')
                .map_or((line, false), |line| (line, true))
        })
        .collect()
}

fn fence_candidate(line: &str) -> Option<&str> {
    let indent = line.bytes().take_while(|&value| value == b' ').count();
    (indent <= 3).then_some(&line[indent..])
}

fn opening_fence(line: &str) -> Option<(char, usize, &str)> {
    let trimmed = fence_candidate(line)?;
    let marker = trimmed.chars().next()?;
    if !matches!(marker, '`' | '~') {
        return None;
    }
    let marker_len = trimmed.chars().take_while(|&value| value == marker).count();
    if marker_len < 3 {
        return None;
    }
    let language = trimmed[marker.len_utf8() * marker_len..]
        .trim()
        .split_ascii_whitespace()
        .next()
        .unwrap_or_default();
    Some((marker, marker_len, language))
}

fn is_closing_fence(line: &str, marker: char, minimum_len: usize) -> bool {
    let Some(trimmed) = fence_candidate(line) else {
        return false;
    };
    let marker_len = trimmed.chars().take_while(|&value| value == marker).count();
    marker_len >= minimum_len && trimmed[marker.len_utf8() * marker_len..].trim().is_empty()
}

fn fence_spans(line: &str) -> Vec<Span<'static>> {
    vec![Span::styled(
        line.to_owned(),
        Style::default().fg(crate::theme::PASTEL_BLUE),
    )]
}

fn markdown_spans(line: &str) -> Vec<Span<'static>> {
    let marker_end = structural_marker_end(line);
    let mut spans = Vec::new();
    if marker_end > 0 {
        spans.push(Span::styled(
            line[..marker_end].to_owned(),
            Style::default().fg(crate::theme::PASTEL_MINT),
        ));
    }
    spans.extend(inline_spans(&line[marker_end..]));
    spans
}

fn structural_marker_end(line: &str) -> usize {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[indent..];
    if rest.starts_with("> ") || rest.starts_with("- ") || rest.starts_with("+ ") {
        return indent + 2;
    }
    if rest.starts_with("* ") {
        return indent + 2;
    }
    if rest.starts_with('#') {
        let hashes = rest.bytes().take_while(|&value| value == b'#').count();
        if rest.as_bytes().get(hashes) == Some(&b' ') {
            return indent + hashes + 1;
        }
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0
        && matches!(rest.as_bytes().get(digits), Some(b'.' | b')'))
        && rest.as_bytes().get(digits + 1) == Some(&b' ')
    {
        return indent + digits + 2;
    }
    0
}

fn inline_spans(text: &str) -> Vec<Span<'static>> {
    let prose = Style::default().fg(crate::theme::PASTEL_WHITE);
    let code = Style::default().fg(crate::theme::PASTEL_BLUE);
    let mut spans = Vec::new();
    let mut rest = text;

    while let Some(start) = rest.find('`') {
        if start > 0 {
            spans.push(Span::styled(rest[..start].to_owned(), prose));
        }
        let marker_len = rest[start..]
            .bytes()
            .take_while(|&value| value == b'`')
            .count();
        let marker = &rest[start..start + marker_len];
        let after_marker = &rest[start + marker_len..];
        let Some(end) = after_marker.find(marker) else {
            spans.push(Span::styled(rest[start..].to_owned(), code));
            rest = "";
            break;
        };
        spans.push(Span::styled(marker.to_owned(), code));
        spans.push(Span::styled(after_marker[..end].to_owned(), code));
        spans.push(Span::styled(marker.to_owned(), code));
        rest = &after_marker[end + marker_len..];
    }
    if !rest.is_empty() || spans.is_empty() {
        spans.push(Span::styled(rest.to_owned(), prose));
    }
    spans
}

#[derive(Clone)]
struct StyledCharacter {
    character: char,
    style: Style,
    width: usize,
}

pub(crate) fn wrap_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    let characters = styled_characters(spans, width);
    let mut lines = Vec::new();
    let mut line = Vec::new();
    let mut columns = 0usize;
    let mut pending_space: Vec<StyledCharacter> = Vec::new();
    let mut index = 0;

    while index < characters.len() {
        let whitespace = characters[index].character.is_whitespace();
        let end = characters[index..]
            .iter()
            .position(|character| character.character.is_whitespace() != whitespace)
            .map_or(characters.len(), |offset| index + offset);
        let token = &characters[index..end];
        let token_width = token.iter().map(|character| character.width).sum::<usize>();

        if whitespace {
            pending_space.clear();
            pending_space.extend_from_slice(token);
            index = end;
            continue;
        }

        let space_width = pending_space
            .iter()
            .map(|character| character.width)
            .sum::<usize>();
        if token_width <= width {
            if columns > 0 && columns.saturating_add(space_width + token_width) > width {
                lines.push(std::mem::take(&mut line));
                columns = 0;
            }
            if columns > 0 {
                append_characters(&mut line, &pending_space);
                columns += space_width;
            }
            append_characters(&mut line, token);
            columns += token_width;
        } else {
            if columns > 0 {
                lines.push(std::mem::take(&mut line));
                columns = 0;
            }
            for character in token {
                if columns > 0 && columns.saturating_add(character.width) > width {
                    lines.push(std::mem::take(&mut line));
                    columns = 0;
                }
                append_character(&mut line, character);
                columns += character.width;
            }
        }
        pending_space.clear();
        index = end;
    }

    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

fn styled_characters(spans: Vec<Span<'static>>, width: usize) -> Vec<StyledCharacter> {
    spans
        .into_iter()
        .flat_map(|span| {
            let style = span.style;
            span.content
                .chars()
                .map(move |character| {
                    let character_width = character.width().unwrap_or(0);
                    if character_width > width {
                        StyledCharacter {
                            character: '�',
                            style,
                            width: 1,
                        }
                    } else {
                        StyledCharacter {
                            character,
                            style,
                            width: character_width,
                        }
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn append_characters(line: &mut Vec<Span<'static>>, characters: &[StyledCharacter]) {
    for character in characters {
        append_character(line, character);
    }
}

fn append_character(line: &mut Vec<Span<'static>>, character: &StyledCharacter) {
    if let Some(span) = line.last_mut()
        && span.style == character.style
    {
        span.content.to_mut().push(character.character);
    } else {
        line.push(Span::styled(
            character.character.to_string(),
            character.style,
        ));
    }
}

fn wrap_spans_by_character(spans: Vec<Span<'static>>, width: usize) -> Vec<Vec<Span<'static>>> {
    let mut lines = Vec::new();
    let mut line = Vec::new();
    let mut columns = 0usize;

    for span in spans {
        let style = span.style;
        let mut chunk = String::new();
        for character in span.content.chars() {
            let mut character_width = character.width().unwrap_or(0);
            if columns > 0 && columns.saturating_add(character_width) > width {
                push_chunk(&mut line, &mut chunk, style);
                lines.push(std::mem::take(&mut line));
                columns = 0;
            }
            if character_width > width {
                character_width = 1;
                chunk.push('�');
            } else {
                chunk.push(character);
            }
            columns = columns.saturating_add(character_width);
        }
        push_chunk(&mut line, &mut chunk, style);
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

fn push_chunk(line: &mut Vec<Span<'static>>, chunk: &mut String, style: Style) {
    if !chunk.is_empty() {
        line.push(Span::styled(std::mem::take(chunk), style));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn preserves_markdown_and_accents_inline_code() {
        let rendered = Renderer::default().render("**bold** and `code`", 80);
        let lines = rendered
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert_eq!(lines, ["   **bold** and `code`"]);
        assert!(
            rendered
                .lines
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| span.content.contains("code")
                    && span.style.fg == Some(crate::theme::PASTEL_BLUE))
        );
    }

    #[test]
    fn wraps_content_with_the_existing_four_column_indent() {
        let rendered = Renderer::default().render("1234567", 10);
        let lines = rendered
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert_eq!(lines, ["   123456", "    7"]);
        assert!(lines.iter().all(|line| line.width() <= 10));
    }

    #[test]
    fn wraps_prose_at_word_boundaries_and_keeps_punctuation_attached() {
        let rendered = Renderer::default().render("hello, world! next", 14);
        let lines = rendered
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert_eq!(lines, ["   hello,", "    world!", "    next"]);
        assert!(lines.iter().all(|line| line.width() <= 14));
    }

    #[test]
    fn splits_only_words_that_are_wider_than_a_content_line() {
        let rendered = Renderer::default().render("ok extraordinary", 10);
        let lines = rendered
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert_eq!(lines, ["   ok", "    extrao", "    rdinar", "    y"]);
        assert!(lines.iter().all(|line| line.width() <= 10));
    }

    #[test]
    fn word_wrapping_preserves_inline_styles_across_lines() {
        let rendered = Renderer::default().render("before `code` after", 15);
        assert!(
            rendered
                .lines
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| span.content.contains("code")
                    && span.style.fg == Some(crate::theme::PASTEL_BLUE))
        );
        assert_eq!(
            rendered
                .lines
                .iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>(),
            ["   before", "    `code`", "    after"]
        );
    }

    #[test]
    fn fence_state_survives_chunks_and_reset() {
        let mut renderer = Renderer::default();
        assert_eq!(
            renderer.render("```rust\n", 80).lines[0].to_string(),
            "   ```rust"
        );
        assert!(renderer.fence.is_some());

        let comment = renderer.render("// explanation", 80);
        let comment_span = comment
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content.contains("explanation"))
            .expect("highlighted comment span");
        assert_eq!(comment_span.style.fg, Some(ratatui::style::Color::DarkGray));
        assert!(
            comment_span
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::DIM)
        );

        assert_eq!(renderer.render("```", 80).lines[0].to_string(), "    ```");
        assert!(renderer.fence.is_none());

        renderer.render("~~~haskell", 80);
        renderer.render("    ~~~", 80);
        assert!(renderer.fence.is_some());
        renderer.reset();
        assert!(renderer.fence.is_none());
        assert_eq!(
            renderer.render("prose", 80).lines[0].to_string(),
            "   prose"
        );
    }
}
