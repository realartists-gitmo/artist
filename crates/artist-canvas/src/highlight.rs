//! Syntax highlighting for canvases, done in Rust.
//!
//! A coding agent's canvas shows code constantly, so `<Code>` and `<Diff>` are
//! the components that earn their place fastest. Highlighting them here rather
//! than shipping a JS highlighter costs nothing — `syntect` is already in the
//! workspace for the TUI — and means the browser downloads no grammar files and
//! a canvas renders Rust the same way the transcript does.
//!
//! Output is spans rather than HTML: the page decides how to draw them, so a
//! canvas can restyle code without the server knowing about its theme.

use serde::Serialize;
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Theme, ThemeSet},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

/// One run of characters sharing a style.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Span {
    pub text: String,
    /// `#rrggbb`, so the page can use it directly.
    pub color: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
}

/// A highlighted document, one vector of spans per line.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Highlighted {
    pub language: String,
    pub lines: Vec<Vec<Span>>,
}

fn syntaxes() -> &'static SyntaxSet {
    static SET: std::sync::OnceLock<SyntaxSet> = std::sync::OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme(dark: bool) -> &'static Theme {
    static THEMES: std::sync::OnceLock<(Theme, Theme)> = std::sync::OnceLock::new();
    let (light, dark_theme) = THEMES.get_or_init(|| {
        let set = ThemeSet::load_defaults();
        (
            set.themes["InspiredGitHub"].clone(),
            set.themes["base16-eighties.dark"].clone(),
        )
    });
    if dark { dark_theme } else { light }
}

/// Highlight `source` as `language`.
///
/// `language` is matched by token (`rust`, `rs`) then by file extension, and
/// finally falls back to plain text — an unknown language must render as
/// unstyled code, never as an error.
pub fn highlight(source: &str, language: &str, dark: bool) -> Highlighted {
    let set = syntaxes();
    let syntax = set
        .find_syntax_by_token(language)
        .or_else(|| set.find_syntax_by_extension(language))
        .unwrap_or_else(|| set.find_syntax_plain_text());

    let mut highlighter = HighlightLines::new(syntax, theme(dark));
    let mut lines = Vec::new();
    for line in LinesWithEndings::from(source) {
        let styled = highlighter.highlight_line(line, set).unwrap_or_default();
        lines.push(
            styled
                .into_iter()
                .map(|(style, text)| Span {
                    text: text.trim_end_matches('\n').to_owned(),
                    color: format!(
                        "#{:02x}{:02x}{:02x}",
                        style.foreground.r, style.foreground.g, style.foreground.b
                    ),
                    bold: style.font_style.contains(FontStyle::BOLD),
                    italic: style.font_style.contains(FontStyle::ITALIC),
                })
                // A trailing newline becomes an empty span that renders as
                // nothing but still costs a DOM node on every line.
                .filter(|span| !span.text.is_empty())
                .collect(),
        );
    }

    Highlighted {
        language: syntax.name.to_lowercase(),
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(highlighted: &Highlighted) -> String {
        highlighted
            .lines
            .iter()
            .map(|line| {
                line.iter()
                    .map(|span| span.text.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Highlighting must never lose or reorder a character — the page renders
    /// the spans as the code itself.
    #[test]
    fn spans_reconstruct_the_source_exactly() {
        let source = "fn main() {\n    let x = \"hi\";\n}";
        let highlighted = highlight(source, "rust", true);
        assert_eq!(text_of(&highlighted), source);
    }

    #[test]
    fn a_keyword_is_styled_differently_from_a_string() {
        let highlighted = highlight("let x = \"hi\";", "rust", true);
        let colors: Vec<_> = highlighted.lines[0]
            .iter()
            .map(|span| span.color.as_str())
            .collect();
        assert!(
            colors
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                > 1,
            "everything came out one colour: {colors:?}"
        );
    }

    #[test]
    fn language_matches_by_name_or_extension() {
        assert_eq!(highlight("let x = 1;", "rust", true).language, "rust");
        assert_eq!(highlight("let x = 1;", "rs", true).language, "rust");
    }

    /// An unknown language is a typo in a canvas, not a failure.
    #[test]
    fn an_unknown_language_renders_as_plain_text() {
        let highlighted = highlight("some words", "kobol", true);
        assert_eq!(text_of(&highlighted), "some words");
        assert_eq!(highlighted.language, "plain text");
    }

    #[test]
    fn light_and_dark_produce_different_colours() {
        let source = "fn main() {}";
        assert_ne!(
            highlight(source, "rust", true).lines,
            highlight(source, "rust", false).lines
        );
    }

    #[test]
    fn an_empty_source_is_not_an_error() {
        assert!(highlight("", "rust", true).lines.is_empty());
    }
}
