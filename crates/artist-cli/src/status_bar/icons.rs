pub(super) const PROJECT: &str = "";
pub(super) const BRANCH: &str = "";
pub(super) const MODEL: &str = "";
pub(super) const REASONING: &str = "";
pub(super) const CONTEXT: &str = "";
pub(super) const SESSION_TOKENS: &str = "";

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn built_in_icons_are_single_column_glyphs() {
        for icon in [PROJECT, BRANCH, MODEL, REASONING, CONTEXT, SESSION_TOKENS] {
            assert_eq!(icon.chars().count(), 1);
            assert_eq!(icon.width(), 1);
        }
    }
}
