//! A streaming markdown reader that emits semantic lines.
//!
//! This is the incremental half of Artist's old `response_output::Renderer`,
//! with the terminal removed. It is fed model output in arbitrary chunks —
//! including chunks that stop mid-line — and carries fence and highlighter state
//! across them, so a code block opened in one token still highlights in the next.
//!
//! What it deliberately does *not* do is wrap, indent, or colour. Those were the
//! parts that assumed a cell grid.

use crate::{
    inline::{Inline, InlineKind, InlineLine},
    syntax::CodeHighlighter,
};

#[derive(Default)]
pub struct MarkdownStream {
    fence: Option<FencedBlock>,
}

struct FencedBlock {
    marker: char,
    marker_len: usize,
    highlighter: CodeHighlighter,
    language: String,
}

impl MarkdownStream {
    /// Consume a chunk of model output, returning its logical lines.
    ///
    /// A chunk that does not end in a newline leaves its final line open: the
    /// line is returned now (so streaming output appears immediately) and the
    /// next chunk continues it. Views are expected to replace the last line they
    /// were given rather than append, which is how the TUI has always behaved.
    pub fn push(&mut self, output: &str) -> Vec<InlineLine> {
        logical_lines(output)
            .into_iter()
            .map(|(source, ends_line)| {
                let code = self.fence.is_some();
                InlineLine {
                    inlines: self.line_inlines(source, ends_line),
                    code,
                }
            })
            .collect()
    }

    /// Forget fence and highlighter state, for the start of a new response.
    pub fn reset(&mut self) {
        self.fence = None;
    }

    /// The language of the fenced block currently open, if any. A GUI uses this
    /// to label a code block's header; the TUI ignores it.
    pub fn open_fence_language(&self) -> Option<&str> {
        self.fence.as_ref().map(|fence| fence.language.as_str())
    }

    fn line_inlines(&mut self, line: &str, ends_line: bool) -> Vec<Inline> {
        if self
            .fence
            .as_ref()
            .is_some_and(|fence| is_closing_fence(line, fence.marker, fence.marker_len))
        {
            self.fence = None;
            return fence_delimiter(line);
        }
        if let Some(fence) = self.fence.as_mut() {
            return fence.highlighter.highlight_line(line, ends_line);
        }
        if let Some((marker, marker_len, language)) = opening_fence(line) {
            self.fence = Some(FencedBlock {
                marker,
                marker_len,
                highlighter: CodeHighlighter::new(language),
                language: language.to_owned(),
            });
            return fence_delimiter(line);
        }
        prose_inlines(line)
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

fn fence_delimiter(line: &str) -> Vec<Inline> {
    vec![Inline::new(line.to_owned(), InlineKind::FenceDelimiter)]
}

fn prose_inlines(line: &str) -> Vec<Inline> {
    let marker_end = structural_marker_end(line);
    let mut inlines = Vec::new();
    if marker_end > 0 {
        inlines.push(Inline::new(
            line[..marker_end].to_owned(),
            InlineKind::StructuralMarker,
        ));
    }
    inlines.extend(inline_code_runs(&line[marker_end..]));
    inlines
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

fn inline_code_runs(text: &str) -> Vec<Inline> {
    let mut inlines = Vec::new();
    let mut rest = text;

    while let Some(start) = rest.find('`') {
        if start > 0 {
            inlines.push(Inline::prose(rest[..start].to_owned()));
        }
        let marker_len = rest[start..]
            .bytes()
            .take_while(|&value| value == b'`')
            .count();
        let marker = &rest[start..start + marker_len];
        let after_marker = &rest[start + marker_len..];
        let Some(end) = after_marker.find(marker) else {
            inlines.push(Inline::code(rest[start..].to_owned()));
            rest = "";
            break;
        };
        inlines.push(Inline::code(marker.to_owned()));
        inlines.push(Inline::code(after_marker[..end].to_owned()));
        inlines.push(Inline::code(marker.to_owned()));
        rest = &after_marker[end + marker_len..];
    }
    if !rest.is_empty() || inlines.is_empty() {
        inlines.push(Inline::prose(rest.to_owned()));
    }
    inlines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inline::TokenKind;

    fn plain(lines: &[InlineLine]) -> Vec<String> {
        lines.iter().map(InlineLine::plain).collect()
    }

    #[test]
    fn emits_logical_lines_without_wrapping_them() {
        // The core returns one line per source line no matter how long it is —
        // wrapping is the view's business.
        let lines = MarkdownStream::default().push("a very long line indeed");
        assert_eq!(plain(&lines), ["a very long line indeed"]);
    }

    #[test]
    fn marks_inline_code_including_its_backticks() {
        let lines = MarkdownStream::default().push("**bold** and `code`");
        assert_eq!(plain(&lines), ["**bold** and `code`"]);
        assert!(
            lines[0]
                .inlines
                .iter()
                .any(|inline| inline.text.contains("code") && inline.kind == InlineKind::Code)
        );
    }

    #[test]
    fn marks_structural_markers() {
        let lines = MarkdownStream::default().push("- a bullet");
        assert_eq!(lines[0].inlines[0].kind, InlineKind::StructuralMarker);
        assert_eq!(lines[0].inlines[0].text, "- ");
    }

    #[test]
    fn fence_state_survives_chunks_and_reset() {
        let mut stream = MarkdownStream::default();
        let opening = stream.push("```rust\n");
        assert_eq!(opening[0].inlines[0].kind, InlineKind::FenceDelimiter);
        assert_eq!(stream.open_fence_language(), Some("rust"));

        let comment = stream.push("// explanation");
        assert!(comment[0].code);
        assert_eq!(
            comment[0]
                .inlines
                .iter()
                .find_map(|inline| match inline.kind {
                    InlineKind::Syntax(kind) if inline.text.contains("explanation") => Some(kind),
                    _ => None,
                }),
            Some(TokenKind::Comment)
        );

        let closing = stream.push("```");
        assert_eq!(closing[0].inlines[0].kind, InlineKind::FenceDelimiter);
        assert!(stream.open_fence_language().is_none());

        stream.push("~~~haskell");
        stream.push("    ~~~");
        assert!(stream.open_fence_language().is_some());
        stream.reset();
        assert!(stream.open_fence_language().is_none());
    }

    #[test]
    fn preserves_leading_indentation() {
        let lines = MarkdownStream::default().push("  nested item");
        assert_eq!(plain(&lines), ["  nested item"]);
    }
}
