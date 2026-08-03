//! The terminal's view of a streaming model response.
//!
//! Markdown parsing, fence tracking and syntax classification moved to
//! `artist-ui-core` so the gpui frontend could share them. What stays here is
//! everything that is genuinely about a terminal: the pastel palette, wrapping
//! to a column count, and the two-space indent.

use artist_ui_core::{InlineKind, InlineLine, MarkdownStream, TokenKind};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};
use unicode_width::UnicodeWidthChar;

const INDENT: &str = "    ";

#[derive(Default)]
pub(crate) struct Renderer {
    started: bool,
    stream: MarkdownStream,
}

impl Renderer {
    pub(crate) fn render(&mut self, output: &str, terminal_width: usize) -> Text<'static> {
        let content_width = terminal_width.saturating_sub(INDENT.len()).max(1);
        let mut lines = Vec::new();

        for logical in self.stream.push(output) {
            let code = logical.code;
            let styled = style_line(&logical);
            let wrapped = if code {
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
                        Span::styled(" ", Style::default().fg(crate::theme::PASTEL_BLUSH)),
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
        self.stream.reset();
    }

    #[cfg(test)]
    pub(crate) fn open_fence_language(&self) -> Option<&str> {
        self.stream.open_fence_language()
    }
}

fn style_line(line: &InlineLine) -> Vec<Span<'static>> {
    line.inlines
        .iter()
        .map(|inline| Span::styled(inline.text.clone(), style_for(inline.kind)))
        .collect()
}

/// The terminal half of rule 3: semantic kind in, pastel palette out. The gpui
/// frontend has its own copy of this function and is free to disagree with it.
fn style_for(kind: InlineKind) -> Style {
    use crate::theme::{PASTEL_BLUE, PASTEL_MINT, PASTEL_PINK, PASTEL_WHITE, PASTEL_YELLOW};

    match kind {
        InlineKind::Prose => Style::default().fg(PASTEL_WHITE),
        InlineKind::Code | InlineKind::FenceDelimiter => Style::default().fg(PASTEL_BLUE),
        InlineKind::StructuralMarker => Style::default().fg(PASTEL_MINT),
        InlineKind::Syntax(token) => match token {
            TokenKind::Plain => Style::default().fg(PASTEL_WHITE),
            // Comments are the one place the terminal uses a named colour rather
            // than an RGB pastel, because DIM needs a colour the terminal owns.
            TokenKind::Comment => Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
            TokenKind::StringLit | TokenKind::Number => Style::default().fg(PASTEL_YELLOW),
            TokenKind::Keyword => Style::default().fg(PASTEL_PINK),
            TokenKind::Function => Style::default().fg(PASTEL_MINT),
            TokenKind::Type => Style::default().fg(PASTEL_BLUE),
        },
    }
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
    let mut pending_is_leading = false;
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
            pending_is_leading = index == 0;
            index = end;
            continue;
        }

        let mut space_width = pending_space
            .iter()
            .map(|character| character.width)
            .sum::<usize>();
        if columns == 0 && pending_is_leading && space_width < width {
            append_characters(&mut line, &pending_space);
            columns = space_width;
            pending_space.clear();
            space_width = 0;
        }
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

        assert_eq!(lines, ["   **bold** and `code`"]);
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

        assert_eq!(lines, ["   123456", "    7"]);
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

        assert_eq!(lines, ["   hello,", "    world!", "    next"]);
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

        assert_eq!(lines, ["   ok", "    extrao", "    rdinar", "    y"]);
        assert!(lines.iter().all(|line| line.width() <= 10));
    }

    #[test]
    fn preserves_leading_indentation_in_prose() {
        let rendered = Renderer::default().render("  nested item", 20);
        assert_eq!(rendered.lines[0].to_string(), "     nested item");
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
            ["   before", "    `code`", "    after"]
        );
    }

    #[test]
    fn fence_state_survives_chunks_and_reset() {
        let mut renderer = Renderer::default();
        assert_eq!(
            renderer.render("```rust\n", 80).lines[0].to_string(),
            "   ```rust"
        );
        assert_eq!(renderer.open_fence_language(), Some("rust"));

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
        assert!(renderer.open_fence_language().is_none());

        renderer.render("~~~haskell", 80);
        renderer.render("    ~~~", 80);
        assert!(renderer.open_fence_language().is_some());
        renderer.reset();
        assert!(renderer.open_fence_language().is_none());
        assert_eq!(
            renderer.render("prose", 80).lines[0].to_string(),
            "   prose"
        );
    }

    #[test]
    fn syntax_highlighting_still_reaches_the_terminal_palette() {
        let mut renderer = Renderer::default();
        renderer.render("```rust\n", 80);
        let rendered = renderer.render("fn main() { let answer = 42; }", 80);
        let spans = rendered
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .collect::<Vec<_>>();

        let color_of = |token: &str| {
            spans
                .iter()
                .find(|span| span.content.contains(token))
                .and_then(|span| span.style.fg)
        };
        assert_eq!(color_of("fn"), Some(crate::theme::PASTEL_PINK));
        assert_eq!(color_of("42"), Some(crate::theme::PASTEL_YELLOW));
    }
}
