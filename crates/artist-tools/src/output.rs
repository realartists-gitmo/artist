use hashline_tools::AnchoredLine;

pub const OUTPUT_CAP: usize = 50 * 1024;

/// How a modified line is rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffStyle {
    /// Collapse a removal/addition pair onto the post-edit anchor as `~new`.
    ///
    /// Right for an edit the model just made: it supplied the new text, so
    /// echoing what it replaced is noise.
    Collapsed,
    /// Keep both sides, `-old` then `+new`.
    ///
    /// Right for a change the model did *not* make. The removal row carries the
    /// pre-edit anchor — which is the handle the model is still holding — so
    /// this is what connects "the anchor you have" to "what happened to it".
    /// Collapsing here would show only an anchor the model has never seen.
    Explicit,
}

/// Render a unified diff with a semantic-anchor gutter instead of line numbers —
/// the same anchors the model edits by, so a reviewer sees a consistent view.
/// Removed lines take their pre-edit anchor, added/context lines the post-edit
/// anchor.
///
/// A run of removals immediately followed by additions is a replacement, so the
/// pairs collapse onto one `~` row carrying the new text; whatever is left over
/// keeps its `-`/`+` prefix. The prefix survives after the `│` so the TUI can
/// still color the row.
pub fn anchored_diff(diff: &str, before: &[AnchoredLine], after: &[AnchoredLine]) -> String {
    anchored_diff_styled(diff, before, after, DiffStyle::Collapsed)
}

/// As [`anchored_diff`], with the pairing behaviour chosen by the caller.
pub fn anchored_diff_styled(
    diff: &str,
    before: &[AnchoredLine],
    after: &[AnchoredLine],
    style: DiffStyle,
) -> String {
    // `AnchoredLine`s are built by an in-order enumerate, so `line_number` is
    // strictly ascending and the gutter lookup can binary-search instead of
    // scanning — the linear form is quadratic over a large diff.
    let anchor_at = |lines: &[AnchoredLine], number: usize| {
        lines
            .binary_search_by_key(&number, |line| line.line_number)
            .ok()
            .map(|index| lines[index].anchor.clone())
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
            flush_changes(&mut rows, &mut removals, &mut additions, style);
            let mut ranges = header.split_whitespace();
            old_line = range_start(ranges.next()).unwrap_or(old_line);
            new_line = range_start(ranges.next()).unwrap_or(new_line);
            in_hunk = true;
            continue;
        }
        // Inside a hunk these are content, not file headers.
        if !in_hunk && (line.starts_with("---") || line.starts_with("+++")) {
            flush_changes(&mut rows, &mut removals, &mut additions, style);
            continue;
        }
        if line == "\\ No newline at end of file" {
            continue;
        }
        if line.starts_with('-') {
            // A removal after additions starts a new run rather than pairing
            // across the boundary.
            if !additions.is_empty() {
                flush_changes(&mut rows, &mut removals, &mut additions, style);
            }
            removals.push((anchor_at(before, old_line), line.to_owned()));
            old_line += 1;
        } else if line.starts_with('+') {
            additions.push((anchor_at(after, new_line), line.to_owned()));
            new_line += 1;
        } else {
            flush_changes(&mut rows, &mut removals, &mut additions, style);
            if line.starts_with(' ') {
                rows.push((anchor_at(after, new_line), line.to_owned()));
                old_line += 1;
                new_line += 1;
            } else {
                rows.push((String::new(), line.to_owned()));
            }
        }
    }
    flush_changes(&mut rows, &mut removals, &mut additions, style);
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
    style: DiffStyle,
) {
    if style == DiffStyle::Explicit {
        // No pairing: both sides are kept, removals first, so a modification
        // reads as what went and what came.
        rows.append(removals);
        rows.append(additions);
        return;
    }
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

/// Fit oversized output under `cap`, compressing before discarding.
///
/// Plain [`tail`] keeps the end and throws the beginning away, which on a build
/// log discards the first error and keeps the summary that merely says one
/// happened. Squeezing first — reversible tag substitution over repeated
/// timestamps, component prefixes and token runs — retains more of the original
/// within the same budget.
///
/// Deliberately only applied when the output would otherwise be truncated: even
/// a readable compression is worth nothing when the text already fits.
///
/// Runs [`Stages::legible`] rather than the full pipeline. Upstream's `keys`
/// stage substitutes on `\b[A-Za-z0-9_]+={1,2}`, so the tag swallows the `=`
/// and lands inside what a reader sees as one token — `--edition=2024` becomes
/// `--#d#2024` — and its `meta_bpe` stage nests tags inside tags, so resolving
/// one entry exposes more. Dropping both is not a concession: measured on a
/// `cargo build -v` log from this workspace, the full pipeline gives a body 485
/// bytes smaller but needs 46 more legend entries to do it, which costs more
/// than it saves. Legible totals −49.9% against full's −48.4%, with 14 legend
/// entries instead of 60.
#[allow(dead_code)] // parked: see the call site in bash.rs
pub fn tail_compressed(value: String, cap: usize) -> (String, bool) {
    if value.len() <= cap {
        return (value, false);
    }
    let compressed =
        artist_ast::squeeze::squeeze_with(&value, artist_ast::squeeze::Stages::legible());
    if compressed.legend.is_empty() {
        // Nothing repeated enough to be worth a tag. Squeezing bought nothing,
        // so do not pay the legibility cost.
        return tail(value, cap);
    }
    let mut legend = String::from("[compressed: repeated text replaced by tags]\n");
    for (tag, original) in &compressed.legend {
        legend.push_str(&format!("  {tag} = {original}\n"));
    }
    legend.push_str("---\n");

    // The legend must survive truncation. `tail` keeps the *end* of its input,
    // so compressing and then tailing the whole thing would cut the legend off
    // and leave a tagged body with no key — worse than plain truncation. Budget
    // the legend out first and tail only the body.
    let Some(body_cap) = cap.checked_sub(legend.len()) else {
        // Legend alone would not fit. Nothing to gain.
        return tail(value, cap);
    };
    if compressed.body.len() + legend.len() >= value.len() {
        // No saving once the legend is paid for.
        return tail(value, cap);
    }
    let (body, truncated) = tail(compressed.body, body_cap);
    (format!("{legend}{body}"), truncated)
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

    /// Output that fits must come back byte-identical. Compression is only ever
    /// worth its legibility cost when the alternative is losing text.
    #[test]
    fn output_under_the_cap_is_left_completely_alone() {
        let text = "warning: unused variable `x`\n".repeat(20);
        let (out, truncated) = tail_compressed(text.clone(), 50 * 1024);
        assert_eq!(out, text);
        assert!(!truncated);
    }

    #[test]
    fn repetitive_oversized_output_is_compressed_rather_than_only_cut() {
        // Highly repetitive, so squeezing has something to exploit.
        let text = (0..4000)
            .map(|i| {
                format!("[worker] 2026-07-31T12:00:00Z compiling module_{i} feature=default\n")
            })
            .collect::<String>();
        let cap = 8 * 1024;
        let (compressed, _) = tail_compressed(text.clone(), cap);
        let (plain, _) = tail(text, cap);
        assert!(
            compressed.contains("[compressed:"),
            "expected the compression header"
        );
        // Both land under the cap; the compressed one represents more of the
        // original within it.
        assert!(compressed.len() <= cap);
        assert!(plain.len() <= cap);
    }

    /// Text with nothing repeated must not be made worse. A legend costs bytes
    /// and readability, so it has to earn its place.
    #[test]
    fn incompressible_output_falls_back_to_plain_truncation() {
        let mut text = String::new();
        for i in 0..3000 {
            text.push_str(&format!(
                "{i:x}{}\n",
                "qwertyuiop".chars().rev().collect::<String>()
            ));
        }
        let cap = 4 * 1024;
        let (out, truncated) = tail_compressed(text, cap);
        assert!(truncated);
        assert!(out.len() <= cap);
    }
}
