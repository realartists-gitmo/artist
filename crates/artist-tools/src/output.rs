use hashline_tools::AnchoredLine;

pub const OUTPUT_CAP: usize = 50 * 1024;

/// Render a unified diff with a mnemonic-anchor gutter instead of line numbers —
/// the same anchors the model edits by, so a reviewer sees a consistent view.
/// Removed lines take their pre-edit anchor, added/context lines the post-edit
/// anchor; the `+`/`-` prefix is preserved after the `│` so the TUI still colors
/// the diff.
pub fn anchored_diff(diff: &str, before: &[AnchoredLine], after: &[AnchoredLine]) -> String {
    let anchor_at = |lines: &[AnchoredLine], number: usize| {
        lines
            .iter()
            .find(|line| line.line_number == number)
            .map(|line| line.anchor.clone())
            .unwrap_or_default()
    };
    let mut old_line = 0usize;
    let mut new_line = 0usize;
    let mut rows: Vec<(String, String)> = Vec::new();
    for line in diff.lines() {
        if let Some(header) = line.strip_prefix("@@") {
            let mut ranges = header.split_whitespace();
            old_line = range_start(ranges.next()).unwrap_or(old_line);
            new_line = range_start(ranges.next()).unwrap_or(new_line);
            continue;
        }
        if line.starts_with("---") || line.starts_with("+++") {
            continue;
        }
        let anchor = if line.starts_with('-') {
            let anchor = anchor_at(before, old_line);
            old_line += 1;
            anchor
        } else if line.starts_with('+') {
            let anchor = anchor_at(after, new_line);
            new_line += 1;
            anchor
        } else if line.starts_with(' ') {
            let anchor = anchor_at(after, new_line);
            old_line += 1;
            new_line += 1;
            anchor
        } else {
            String::new()
        };
        rows.push((anchor, line.to_owned()));
    }
    let width = rows
        .iter()
        .map(|(anchor, _)| anchor.chars().count())
        .max()
        .unwrap_or(0);
    rows.iter()
        .map(|(anchor, line)| format!("{anchor:>width$} │ {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse the starting line of a unified-diff hunk range (`-10,3` / `+12` → 10).
fn range_start(range: Option<&str>) -> Option<usize> {
    range?
        .trim_start_matches(['-', '+'])
        .split(',')
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(n: usize, anchor: &str, text: &str) -> AnchoredLine {
        AnchoredLine {
            line_number: n,
            anchor: anchor.into(),
            text: text.into(),
        }
    }

    #[test]
    fn anchored_diff_uses_anchors_for_the_gutter() {
        // Old line 1 "let x = 1" replaced by new line 1 "let x = 2", with a
        // context line after.
        let before = [line(1, "alfa", "let x = 1"), line(2, "bravo", "done")];
        let after = [line(1, "delta", "let x = 2"), line(2, "bravo", "done")];
        let diff = "@@ -1,2 +1,2 @@\n-let x = 1\n+let x = 2\n done\n";

        let rendered = anchored_diff(diff, &before, &after);

        assert_eq!(
            rendered,
            " alfa │ -let x = 1\ndelta │ +let x = 2\nbravo │  done"
        );
    }
}

pub fn head(mut value: String, cap: usize) -> String {
    if value.len() <= cap {
        return value;
    }
    let mut end = cap.saturating_sub(64).min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value.push_str("\n[truncated: visible output limit reached]");
    value
}

pub fn tail(value: String, cap: usize) -> (String, bool) {
    if value.len() <= cap {
        return (value, false);
    }
    let mut start = value.len().saturating_sub(cap.saturating_sub(64));
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    (
        format!("[truncated: showing recent output]\n{}", &value[start..]),
        true,
    )
}
