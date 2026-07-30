use unicode_width::UnicodeWidthStr;

const CURSOR_MARKER: char = '\0';

pub(crate) struct WrappedCursorText {
    pub text: String,
    pub column: u16,
    pub row: u16,
}

/// Wrap prose at Unicode line-breaking opportunities while retaining a precise
/// cursor position. The zero-width marker participates in the same wrapping
/// pass as the complete input, so a partially typed word cannot disagree with
/// where that word is rendered.
pub(crate) fn with_cursor(text: &str, cursor: usize, width: u16) -> WrappedCursorText {
    debug_assert!(text.is_char_boundary(cursor));
    let mut marked = String::with_capacity(text.len() + 1);
    marked.push_str(&text[..cursor]);
    marked.push(CURSOR_MARKER);
    marked.push_str(&text[cursor..]);

    let options = textwrap::Options::new(usize::from(width.max(1)))
        .break_words(true)
        .word_separator(textwrap::WordSeparator::UnicodeBreakProperties);
    let mut wrapped = fill_preserving_newlines(&marked, options);
    let marker = wrapped
        .find(CURSOR_MARKER)
        .expect("cursor marker must survive wrapping");
    let before = &wrapped[..marker];
    let row = before.bytes().filter(|byte| *byte == b'\n').count() as u16;
    let mut column = before
        .rsplit_once('\n')
        .map_or(before, |(_, line)| line)
        .width() as u16;
    let mut row = row;
    if column == width.max(1) {
        column = 0;
        row = row.saturating_add(1);
    }
    wrapped.remove(marker);

    WrappedCursorText {
        text: wrapped,
        column,
        row,
    }
}

pub(crate) fn plain(text: &str, width: u16) -> String {
    let options = textwrap::Options::new(usize::from(width.max(1)))
        .break_words(true)
        .word_separator(textwrap::WordSeparator::UnicodeBreakProperties);
    fill_preserving_newlines(text, options)
}

fn fill_preserving_newlines(text: &str, options: textwrap::Options<'_>) -> String {
    text.split('\n')
        .map(|line| textwrap::fill(line, options.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_complete_words_and_splits_only_overlong_words() {
        assert_eq!(plain("hello world", 8), "hello\nworld");
        assert_eq!(plain("extraordinary", 5), "extra\nordin\nary");
        assert_eq!(plain("hello, world!", 8), "hello,\nworld!");
    }

    #[test]
    fn cursor_uses_the_complete_words_wrap_decision() {
        let wrapped = with_cursor("hello world", 8, 8);
        assert_eq!((wrapped.column, wrapped.row), (2, 1));

        let exact = with_cursor("1234", 4, 4);
        assert_eq!((exact.column, exact.row), (0, 1));
    }
}
