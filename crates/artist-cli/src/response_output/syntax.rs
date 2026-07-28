use std::{str::FromStr, sync::OnceLock};

use ratatui::{
    style::{Color as TerminalColor, Modifier, Style as TerminalStyle},
    text::Span,
};
use syntect::{
    easy::HighlightLines,
    highlighting::{
        Color, HighlightState, ScopeSelectors, Style, StyleModifier, Theme, ThemeItem,
        ThemeSettings,
    },
    parsing::{ParseState, SyntaxSet},
};

use crate::theme::{PASTEL_BLUE, PASTEL_MINT, PASTEL_PINK, PASTEL_WHITE, PASTEL_YELLOW};

const COMMENT: Color = Color {
    r: 0x80,
    g: 0x80,
    b: 0x80,
    a: 0xff,
};

pub(super) struct CodeHighlighter {
    inner: Option<SyntaxState>,
}

struct SyntaxState {
    parse: ParseState,
    highlight: HighlightState,
    pending_line: String,
}

impl CodeHighlighter {
    pub(super) fn new(language: &str) -> Self {
        let syntax_set = syntax_set();
        let inner = syntax_set
            .find_syntax_by_token(language.trim())
            .map(|syntax| {
                let highlighter = HighlightLines::new(syntax, artist_theme());
                let (highlight, parse) = highlighter.state();
                SyntaxState {
                    parse,
                    highlight,
                    pending_line: String::new(),
                }
            });
        Self { inner }
    }

    pub(super) fn highlight_line(&mut self, line: &str, ends_line: bool) -> Vec<Span<'static>> {
        let Some(state) = self.inner.as_mut() else {
            return fallback_span(line);
        };

        let fragment_start = state.pending_line.len();
        state.pending_line.push_str(line);
        let fragment_end = state.pending_line.len();
        let mut source = state.pending_line.clone();
        if ends_line {
            source.push('\n');
        }

        let mut highlighter = HighlightLines::from_state(
            artist_theme(),
            state.highlight.clone(),
            state.parse.clone(),
        );
        match highlighter.highlight_line(&source, syntax_set()) {
            Ok(regions) => {
                let spans = spans_in_range(&regions, fragment_start, fragment_end);
                if ends_line {
                    let (highlight, parse) = highlighter.state();
                    state.highlight = highlight;
                    state.parse = parse;
                    state.pending_line.clear();
                }
                spans
            }
            Err(_) => {
                if ends_line {
                    state.pending_line.clear();
                }
                fallback_span(line)
            }
        }
    }
}

fn spans_in_range(
    regions: &[(Style, &str)],
    range_start: usize,
    range_end: usize,
) -> Vec<Span<'static>> {
    let mut offset = 0usize;
    let mut spans = Vec::new();
    for (style, text) in regions {
        let region_start = offset;
        let region_end = region_start + text.len();
        offset = region_end;

        let start = region_start.max(range_start);
        let end = region_end.min(range_end);
        if start >= end {
            continue;
        }
        let text = &text[start - region_start..end - region_start];
        let is_comment = style.foreground == COMMENT;
        let foreground = if is_comment {
            TerminalColor::DarkGray
        } else {
            TerminalColor::Rgb(style.foreground.r, style.foreground.g, style.foreground.b)
        };
        let mut terminal_style = TerminalStyle::default().fg(foreground);
        if is_comment {
            terminal_style = terminal_style.add_modifier(Modifier::DIM);
        }
        spans.push(Span::styled(text.to_owned(), terminal_style));
    }
    spans
}

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn artist_theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| Theme {
        name: Some("Artist Pastel".to_owned()),
        settings: ThemeSettings {
            foreground: Some(syntect_color(PASTEL_WHITE)),
            ..ThemeSettings::default()
        },
        scopes: vec![
            theme_item("comment", COMMENT),
            theme_item("string, constant.character", syntect_color(PASTEL_YELLOW)),
            theme_item("constant.numeric", syntect_color(PASTEL_YELLOW)),
            theme_item("keyword, storage", syntect_color(PASTEL_PINK)),
            theme_item(
                "entity.name.function, support.function, variable.function",
                syntect_color(PASTEL_MINT),
            ),
            theme_item(
                "entity.name.type, entity.name.class, entity.name.struct, support.type",
                syntect_color(PASTEL_BLUE),
            ),
        ],
        ..Theme::default()
    })
}

fn theme_item(scopes: &str, foreground: Color) -> ThemeItem {
    ThemeItem {
        scope: ScopeSelectors::from_str(scopes).expect("Artist scope selectors are valid"),
        style: StyleModifier {
            foreground: Some(foreground),
            ..StyleModifier::default()
        },
    }
}

fn syntect_color(color: TerminalColor) -> Color {
    let TerminalColor::Rgb(r, g, b) = color else {
        unreachable!("Artist pastel colors are RGB")
    };
    Color { r, g, b, a: 0xff }
}

fn fallback_span(line: &str) -> Vec<Span<'static>> {
    vec![Span::styled(
        line.to_owned(),
        TerminalStyle::default().fg(PASTEL_WHITE),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color_for(spans: &[Span<'_>], token: &str) -> Option<TerminalColor> {
        spans
            .iter()
            .find(|span| span.content.contains(token))
            .and_then(|span| span.style.fg)
    }

    #[test]
    fn highlights_known_rust_syntax() {
        let spans =
            CodeHighlighter::new("rust").highlight_line("fn main() { let answer = 42; }", true);

        assert_eq!(
            spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "fn main() { let answer = 42; }"
        );
        assert_eq!(color_for(&spans, "fn"), Some(PASTEL_PINK));
        assert_eq!(color_for(&spans, "42"), Some(PASTEL_YELLOW));
    }

    #[test]
    fn highlights_known_haskell_syntax() {
        let spans = CodeHighlighter::new("haskell").highlight_line("twoSum 9 [2, 7, 11, 15]", true);

        assert_eq!(color_for(&spans, "9"), Some(PASTEL_YELLOW));
        assert_eq!(color_for(&spans, "15"), Some(PASTEL_YELLOW));
    }

    #[test]
    fn dims_comments() {
        let spans = CodeHighlighter::new("rust").highlight_line("// explanation", true);
        let comment = spans
            .iter()
            .find(|span| span.content.contains("explanation"))
            .expect("comment span");

        assert_eq!(comment.style.fg, Some(TerminalColor::DarkGray));
        assert!(comment.style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn unknown_language_falls_back_to_pastel_white() {
        let spans =
            CodeHighlighter::new("definitely-not-a-language").highlight_line("some code", true);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "some code");
        assert_eq!(spans[0].style.fg, Some(PASTEL_WHITE));
    }

    #[test]
    fn preserves_comment_state_across_visual_wraps() {
        let mut highlighter = CodeHighlighter::new("rust");
        let first = highlighter.highlight_line("// a long", false);
        let second = highlighter.highlight_line(" comment", true);

        for span in first.iter().chain(&second) {
            assert_eq!(span.style.fg, Some(TerminalColor::DarkGray));
            assert!(span.style.add_modifier.contains(Modifier::DIM));
        }
    }
}
