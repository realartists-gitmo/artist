use std::collections::HashMap;

use unicode_width::UnicodeWidthStr;

pub const FALLBACK: &str = "󰒓";

pub fn icon_for<'a>(name: &str, custom_icons: &'a HashMap<String, String>) -> Option<&'a str> {
    Some(
        builtin_icon(name)
            .or_else(|| custom_icons.get(name).map(String::as_str))
            .unwrap_or(FALLBACK),
    )
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
}
