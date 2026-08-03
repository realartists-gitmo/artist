//! Syntax highlighting that yields token *kinds* rather than colours.
//!
//! This is Artist's original highlighter with one change: the syntect theme is
//! built from sentinel colours, one per [`TokenKind`], which are mapped straight
//! back to the kind. The scope selectors are untouched, so classification is
//! identical to what the terminal has always shown — but the palette now lives
//! in each frontend instead of here.

use std::{str::FromStr, sync::OnceLock};

use syntect::{
    easy::HighlightLines,
    highlighting::{
        Color, HighlightState, ScopeSelectors, Style, StyleModifier, Theme, ThemeItem,
        ThemeSettings,
    },
    parsing::{ParseState, SyntaxSet},
};

use crate::inline::{Inline, InlineKind, TokenKind};

/// Sentinel colours. The values are arbitrary and never reach a screen; they
/// exist only so syntect's scope matching can carry a `TokenKind` through its
/// `Style` struct, which has nowhere else to put one.
const SENTINEL_COMMENT: Color = sentinel(1);
const SENTINEL_STRING: Color = sentinel(2);
const SENTINEL_NUMBER: Color = sentinel(3);
const SENTINEL_KEYWORD: Color = sentinel(4);
const SENTINEL_FUNCTION: Color = sentinel(5);
const SENTINEL_TYPE: Color = sentinel(6);

const fn sentinel(tag: u8) -> Color {
    Color {
        r: tag,
        g: 0,
        b: 0,
        a: 0xff,
    }
}

fn kind_for(color: Color) -> TokenKind {
    match color {
        SENTINEL_COMMENT => TokenKind::Comment,
        SENTINEL_STRING => TokenKind::StringLit,
        SENTINEL_NUMBER => TokenKind::Number,
        SENTINEL_KEYWORD => TokenKind::Keyword,
        SENTINEL_FUNCTION => TokenKind::Function,
        SENTINEL_TYPE => TokenKind::Type,
        _ => TokenKind::Plain,
    }
}

pub(crate) struct CodeHighlighter {
    inner: Option<SyntaxState>,
}

struct SyntaxState {
    parse: ParseState,
    highlight: HighlightState,
    pending_line: String,
}

impl CodeHighlighter {
    pub(crate) fn new(language: &str) -> Self {
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

    /// Highlight one fragment. `ends_line` is false when the fragment is a
    /// partial line — a streaming chunk that stopped mid-line — in which case
    /// parser state is deliberately *not* committed, so that a comment opened in
    /// one chunk still reads as a comment in the next.
    pub(crate) fn highlight_line(&mut self, line: &str, ends_line: bool) -> Vec<Inline> {
        let Some(state) = self.inner.as_mut() else {
            return fallback(line);
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
                let inlines = inlines_in_range(&regions, fragment_start, fragment_end);
                if ends_line {
                    let (highlight, parse) = highlighter.state();
                    state.highlight = highlight;
                    state.parse = parse;
                    state.pending_line.clear();
                }
                inlines
            }
            Err(_) => {
                if ends_line {
                    state.pending_line.clear();
                }
                fallback(line)
            }
        }
    }
}

fn inlines_in_range(
    regions: &[(Style, &str)],
    range_start: usize,
    range_end: usize,
) -> Vec<Inline> {
    let mut offset = 0usize;
    let mut inlines = Vec::new();
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
        inlines.push(Inline::new(
            text.to_owned(),
            InlineKind::Syntax(kind_for(style.foreground)),
        ));
    }
    inlines
}

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn artist_theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| Theme {
        name: Some("Artist Semantic".to_owned()),
        settings: ThemeSettings::default(),
        scopes: vec![
            theme_item("comment", SENTINEL_COMMENT),
            theme_item("string, constant.character", SENTINEL_STRING),
            theme_item("constant.numeric", SENTINEL_NUMBER),
            theme_item("keyword, storage", SENTINEL_KEYWORD),
            theme_item(
                "entity.name.function, support.function, variable.function",
                SENTINEL_FUNCTION,
            ),
            theme_item(
                "entity.name.type, entity.name.class, entity.name.struct, support.type",
                SENTINEL_TYPE,
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

fn fallback(line: &str) -> Vec<Inline> {
    vec![Inline::new(
        line.to_owned(),
        InlineKind::Syntax(TokenKind::Plain),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_of(inlines: &[Inline], token: &str) -> Option<TokenKind> {
        inlines
            .iter()
            .find(|inline| inline.text.contains(token))
            .and_then(|inline| match inline.kind {
                InlineKind::Syntax(kind) => Some(kind),
                _ => None,
            })
    }

    #[test]
    fn classifies_known_rust_syntax() {
        let inlines =
            CodeHighlighter::new("rust").highlight_line("fn main() { let answer = 42; }", true);

        assert_eq!(
            inlines
                .iter()
                .map(|inline| inline.text.as_str())
                .collect::<String>(),
            "fn main() { let answer = 42; }"
        );
        assert_eq!(kind_of(&inlines, "fn"), Some(TokenKind::Keyword));
        assert_eq!(kind_of(&inlines, "42"), Some(TokenKind::Number));
    }

    #[test]
    fn classifies_known_haskell_syntax() {
        let inlines =
            CodeHighlighter::new("haskell").highlight_line("twoSum 9 [2, 7, 11, 15]", true);

        assert_eq!(kind_of(&inlines, "9"), Some(TokenKind::Number));
        assert_eq!(kind_of(&inlines, "15"), Some(TokenKind::Number));
    }

    #[test]
    fn marks_comments() {
        let inlines = CodeHighlighter::new("rust").highlight_line("// explanation", true);
        assert_eq!(kind_of(&inlines, "explanation"), Some(TokenKind::Comment));
    }

    #[test]
    fn unknown_language_falls_back_to_plain() {
        let inlines =
            CodeHighlighter::new("definitely-not-a-language").highlight_line("some code", true);

        assert_eq!(inlines.len(), 1);
        assert_eq!(inlines[0].text, "some code");
        assert_eq!(inlines[0].kind, InlineKind::Syntax(TokenKind::Plain));
    }

    #[test]
    fn preserves_comment_state_across_partial_lines() {
        let mut highlighter = CodeHighlighter::new("rust");
        let first = highlighter.highlight_line("// a long", false);
        let second = highlighter.highlight_line(" comment", true);

        for inline in first.iter().chain(&second) {
            assert_eq!(inline.kind, InlineKind::Syntax(TokenKind::Comment));
        }
    }
}
