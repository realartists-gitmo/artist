//! Semantic inline content — the vocabulary both frontends render.
//!
//! Nothing here carries a colour, a font, or a width. A `Span` in ratatui and a
//! `StyledText` run in gpui are both *views* of an [`Inline`]: the terminal maps
//! [`InlineKind`] onto `Style`, the GUI maps it onto weight, family and hue. The
//! rule that keeps the two honest is that this module never learns which.

use serde::{Deserialize, Serialize};

/// A syntax-highlighting classification, produced by scope matching over a
/// fenced code block.
///
/// These are deliberately coarse. They are the groups Artist's highlighter has
/// always distinguished — widening the set is a core change that both frontends
/// must then answer for, which is the point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenKind {
    /// Unclassified code, and the fallback for languages syntect cannot parse.
    #[default]
    Plain,
    Comment,
    /// String and character literals.
    StringLit,
    Number,
    /// Keywords and storage classes.
    Keyword,
    /// Function names, at definition and at call site.
    Function,
    /// Type, class and struct names.
    Type,
}

/// What a run of text *is*, as opposed to how it looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum InlineKind {
    /// Ordinary prose.
    #[default]
    Prose,
    /// Inline code, including the backticks that delimit it. The backticks are
    /// kept because the TUI has always shown them; a GUI is free to drop them
    /// and draw a background instead, which it can only do if it can see them.
    Code,
    /// A list bullet, heading hashes, blockquote arrow or ordered-list number —
    /// the leading run that gives a line its block role.
    StructuralMarker,
    /// A ``` or ~~~ line opening or closing a fenced block.
    FenceDelimiter,
    /// Text inside a fenced code block, classified by the highlighter.
    Syntax(TokenKind),
}

/// A run of text with a single semantic kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inline {
    pub text: String,
    pub kind: InlineKind,
}

impl Inline {
    pub fn new(text: impl Into<String>, kind: InlineKind) -> Self {
        Self {
            text: text.into(),
            kind,
        }
    }

    pub fn prose(text: impl Into<String>) -> Self {
        Self::new(text, InlineKind::Prose)
    }

    pub fn code(text: impl Into<String>) -> Self {
        Self::new(text, InlineKind::Code)
    }
}

/// One *logical* line — a line of the source, before any wrapping.
///
/// Wrapping is a view concern and deliberately absent: the terminal wraps to a
/// column count, and gpui wraps to a pixel width after shaping a proportional
/// font, where a column count means nothing. `code` is carried because the two
/// wrap differently — prose breaks at word boundaries, code breaks anywhere —
/// and that distinction is semantic even though the wrapping itself is not.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineLine {
    pub inlines: Vec<Inline>,
    /// Whether this line sits inside a fenced code block.
    pub code: bool,
}

impl InlineLine {
    /// The line's text with all styling dropped. Used by tests and by any view
    /// that needs a plain-text projection (clipboard, accessibility label).
    pub fn plain(&self) -> String {
        self.inlines
            .iter()
            .map(|inline| inline.text.as_str())
            .collect()
    }
}
