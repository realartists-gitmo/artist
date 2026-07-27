use hashline_tools::AnchoredLine;

pub const OUTPUT_CAP: usize = 50 * 1024;

/// Render a unified diff with a mnemonic-anchor gutter instead of line numbers —
/// the same anchors the model edits by, so a reviewer sees a consistent view.
/// Removed lines take their pre-edit anchor, added/context lines the post-edit
/// anchor.
///
/// A run of removals immediately followed by additions is a replacement, so the
/// pairs collapse onto one `~` row carrying the new text; whatever is left over
/// keeps its `-`/`+` prefix. The prefix survives after the `│` so the TUI can
/// still color the row.
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
    let mut removals: Vec<(String, String)> = Vec::new();
    let mut additions: Vec<(String, String)> = Vec::new();
    let mut in_hunk = false;
    for line in diff.lines() {
        if let Some(header) = line.strip_prefix("@@") {
            flush_changes(&mut rows, &mut removals, &mut additions);
            let mut ranges = header.split_whitespace();
            old_line = range_start(ranges.next()).unwrap_or(old_line);
            new_line = range_start(ranges.next()).unwrap_or(new_line);
            in_hunk = true;
            continue;
        }
        // Inside a hunk these are content, not file headers.
        if !in_hunk && (line.starts_with("---") || line.starts_with("+++")) {
            flush_changes(&mut rows, &mut removals, &mut additions);
            continue;
        }
        if line == "\\ No newline at end of file" {
            continue;
        }
        if line.starts_with('-') {
            // A removal after additions starts a new run rather than pairing
            // across the boundary.
            if !additions.is_empty() {
                flush_changes(&mut rows, &mut removals, &mut additions);
            }
            removals.push((anchor_at(before, old_line), line.to_owned()));
            old_line += 1;
        } else if line.starts_with('+') {
            additions.push((anchor_at(after, new_line), line.to_owned()));
            new_line += 1;
        } else {
            flush_changes(&mut rows, &mut removals, &mut additions);
            if line.starts_with(' ') {
                rows.push((anchor_at(after, new_line), line.to_owned()));
                old_line += 1;
                new_line += 1;
            } else {
                rows.push((String::new(), line.to_owned()));
            }
        }
    }
    flush_changes(&mut rows, &mut removals, &mut additions);
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

/// Drain a buffered removal/addition run, pairing as far as both sides go.
fn flush_changes(
    rows: &mut Vec<(String, String)>,
    removals: &mut Vec<(String, String)>,
    additions: &mut Vec<(String, String)>,
) {
    let paired = removals.len().min(additions.len());
    for (_, added) in removals.iter().zip(additions.iter()) {
        let content = added.1.strip_prefix('+').unwrap_or(&added.1);
        rows.push((added.0.clone(), format!("~{content}")));
    }
    rows.extend(removals.drain(paired..));
    rows.extend(additions.drain(paired..));
    removals.clear();
    additions.clear();
}

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

    /// Anchors named `oNN` come from the pre-edit file, `nNN` from the post-edit
    /// file, so a gutter entry names which side it was read from.
    fn anchors(prefix: &str, numbers: &[usize]) -> Vec<AnchoredLine> {
        numbers
            .iter()
            .map(|n| line(*n, &format!("{prefix}{n:02}"), ""))
            .collect()
    }

    #[test]
    fn anchored_diff_uses_anchors_for_the_gutter() {
        let before = [line(1, "alfa", "let x = 1"), line(2, "bravo", "done")];
        let after = [line(1, "delta", "let x = 2"), line(2, "bravo", "done")];
        let diff = "@@ -1,2 +1,2 @@\n-let x = 1\n+let x = 2\n done\n";

        let rendered = anchored_diff(diff, &before, &after);

        // The removal/addition pair collapses onto the post-edit anchor.
        assert_eq!(rendered, "delta │ ~let x = 2\nbravo │  done");
    }

    #[test]
    fn new_file_content_lands_on_the_post_edit_anchors() {
        let after = anchors("n", &[1, 2]);
        let rendered = anchored_diff("@@ -0,0 +1,2 @@\n+first\n+second\n", &[], &after);

        assert_eq!(rendered, "n01 │ +first\nn02 │ +second");
    }

    #[test]
    fn leaves_unpaired_insertions_and_deletions_on_separate_rows() {
        let before = anchors("o", &[4, 5]);
        let after = anchors("n", &[4, 5, 6]);
        assert_eq!(
            anchored_diff(
                "@@ -4,2 +4,3 @@\n-old a\n-old b\n+new a\n+new b\n+new c\n",
                &before,
                &after,
            ),
            "n04 │ ~new a\nn05 │ ~new b\nn06 │ +new c"
        );

        let before = anchors("o", &[7, 8]);
        let after = anchors("n", &[7]);
        assert_eq!(
            anchored_diff("@@ -7,2 +7,1 @@\n-old a\n-old b\n+new a\n", &before, &after),
            "n07 │ ~new a\no08 │ -old b"
        );
    }

    #[test]
    fn separates_change_runs_and_never_pairs_across_hunks() {
        let before = anchors("o", &[1, 2, 8]);
        let after = anchors("n", &[1, 2, 3, 30]);
        assert_eq!(
            anchored_diff(
                concat!(
                    "@@ -1,2 +1,3 @@\n-old-a\n+new-a\n+insert-a\n-old-b\n+new-b\n",
                    "@@ -8 +8,0 @@\n-old-c\n@@ -20,0 +30 @@\n+new-c\n"
                ),
                &before,
                &after,
            ),
            concat!(
                "n01 │ ~new-a\nn02 │ +insert-a\nn03 │ ~new-b\n",
                "o08 │ -old-c\nn30 │ +new-c"
            )
        );
    }

    #[test]
    fn preserves_marker_like_content_and_compacts_missing_newline_changes() {
        let before = anchors("o", &[4]);
        let after = anchors("n", &[9]);
        assert_eq!(
            anchored_diff(
                concat!(
                    "--- a/file\n+++ b/file\n@@ -4 +9 @@\n--- old\n",
                    "\\ No newline at end of file\n+++ new\n\\ No newline at end of file\n"
                ),
                &before,
                &after,
            ),
            "n09 │ ~++ new"
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
