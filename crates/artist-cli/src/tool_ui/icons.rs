use std::collections::HashMap;

use ratatui::style::Color;
use unicode_width::UnicodeWidthStr;

pub const FALLBACK: &str = "󰒓";

pub fn icon_for<'a>(name: &str, custom_icons: &'a HashMap<String, String>) -> Option<&'a str> {
    Some(
        builtin_icon(name)
            .or_else(|| custom_icons.get(name).map(String::as_str))
            .unwrap_or(FALLBACK),
    )
}

pub(crate) fn accent_color(icon: &str) -> Color {
    match icon {
        "" | "" => crate::theme::PASTEL_MINT,
        "" | "" => crate::theme::PASTEL_BLUSH,
        "" | "" => crate::theme::PASTEL_YELLOW,
        "" | FALLBACK => crate::theme::PASTEL_BLUE,
        "" => crate::theme::PASTEL_PINK,
        _ => {
            let index = icon
                .chars()
                .fold(0usize, |value, character| value ^ character as usize);
            crate::theme::cycle_color(index)
        }
    }
}

fn builtin_icon(name: &str) -> Option<&'static str> {
    match name {
        "bash" => Some(""),
        "subagent" => Some(""),
        "edit" => Some(""),
        "find" => Some(""),
        "grep" => Some(""),
        "read" => Some(""),
        "skill" => Some(""),
        "write" => Some(""),
        _ => None,
    }
}

pub(super) fn valid_icon(icon: &str) -> bool {
    !icon.is_empty()
        && icon.chars().count() <= 8
        && !icon
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        && matches!(UnicodeWidthStr::width(icon), 1..=2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_and_fallback_are_single_column_nerd_glyphs() {
        let custom = HashMap::new();
        for (name, expected) in [
            ("bash", ""),
            ("subagent", ""),
            ("edit", ""),
            ("find", ""),
            ("grep", ""),
            ("read", ""),
            ("skill", ""),
            ("write", ""),
            ("unknown", FALLBACK),
        ] {
            let icon = icon_for(name, &custom).expect("all tools have an icon");
            assert_eq!(icon, expected);
            assert_eq!(icon.chars().count(), 1);
            assert_eq!(UnicodeWidthStr::width(icon), 1);
        }
    }

    #[test]
    fn builtin_accents_follow_the_pastel_tool_palette() {
        for (icon, expected) in [
            ("", crate::theme::PASTEL_MINT),
            ("", crate::theme::PASTEL_MINT),
            ("", crate::theme::PASTEL_BLUSH),
            ("", crate::theme::PASTEL_BLUSH),
            ("", crate::theme::PASTEL_YELLOW),
            ("", crate::theme::PASTEL_YELLOW),
            ("", crate::theme::PASTEL_BLUE),
            ("", crate::theme::PASTEL_PINK),
        ] {
            assert_eq!(accent_color(icon), expected);
        }
    }
}
