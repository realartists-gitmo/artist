pub(super) fn numbered_diff(diff: &str) -> String {
    let mut old_line = 0usize;
    let mut new_line = 0usize;
    let mut output = Vec::new();
    let mut removals = Vec::new();
    let mut additions = Vec::new();
    let mut in_hunk = false;
    for line in diff.lines() {
        if line.starts_with("@@") {
            flush_changes(&mut output, &mut removals, &mut additions);
            let mut ranges = line.split_whitespace().skip(1);
            old_line = diff_range_start(ranges.next()).unwrap_or(old_line);
            new_line = diff_range_start(ranges.next()).unwrap_or(new_line);
            in_hunk = true;
            continue;
        }
        if !in_hunk && (line.starts_with("---") || line.starts_with("+++")) {
            flush_changes(&mut output, &mut removals, &mut additions);
            continue;
        }
        if line == "\\ No newline at end of file" {
            continue;
        }
        if line.starts_with('-') {
            if !additions.is_empty() {
                flush_changes(&mut output, &mut removals, &mut additions);
            }
            removals.push((old_line, line.to_owned()));
            old_line += 1;
        } else if line.starts_with('+') {
            additions.push((new_line, line.to_owned()));
            new_line += 1;
        } else {
            flush_changes(&mut output, &mut removals, &mut additions);
            if line.starts_with(' ') {
                output.push(format!("{old_line:>4} {new_line:>4} │ {line}"));
                old_line += 1;
                new_line += 1;
            } else {
                output.push(format!("          │ {line}"));
            }
        }
    }
    flush_changes(&mut output, &mut removals, &mut additions);
    output.join("\n")
}

fn flush_changes(
    output: &mut Vec<String>,
    removals: &mut Vec<(usize, String)>,
    additions: &mut Vec<(usize, String)>,
) {
    let paired = removals.len().min(additions.len());
    for (removed, added) in removals.iter().zip(additions.iter()) {
        let content = added.1.strip_prefix('+').unwrap_or(&added.1);
        output.push(format!("{:>4} {:>4} │ ~{content}", removed.0, added.0));
    }
    for (line, content) in removals.drain(paired..) {
        output.push(format!("{line:>4}      │ {content}"));
    }
    for (line, content) in additions.drain(paired..) {
        output.push(format!("     {line:>4} │ {content}"));
    }
    removals.clear();
    additions.clear();
}

fn diff_range_start(range: Option<&str>) -> Option<usize> {
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

    #[test]
    fn numbers_unified_diff_without_file_or_hunk_headers() {
        assert_eq!(
            numbered_diff("@@ -10,2 +20,2 @@\n context\n-old\n+new\n"),
            "  10   20 │  context\n  11   21 │ ~new"
        );
    }

    #[test]
    fn leaves_unpaired_insertions_and_deletions_on_separate_rows() {
        assert_eq!(
            numbered_diff("@@ -4,2 +4,3 @@\n-old a\n-old b\n+new a\n+new b\n+new c\n"),
            "   4    4 │ ~new a\n   5    5 │ ~new b\n        6 │ +new c"
        );
        assert_eq!(
            numbered_diff("@@ -7,2 +7,1 @@\n-old a\n-old b\n+new a\n"),
            "   7    7 │ ~new a\n   8      │ -old b"
        );
    }

    #[test]
    fn separates_change_runs_and_never_pairs_across_hunks() {
        assert_eq!(
            numbered_diff(concat!(
                "@@ -1,2 +1,3 @@\n-old-a\n+new-a\n+insert-a\n-old-b\n+new-b\n",
                "@@ -8 +8,0 @@\n-old-c\n@@ -20,0 +30 @@\n+new-c\n"
            )),
            concat!(
                "   1    1 │ ~new-a\n        2 │ +insert-a\n   2    3 │ ~new-b\n",
                "   8      │ -old-c\n       30 │ +new-c"
            )
        );
    }

    #[test]
    fn preserves_marker_like_content_and_compacts_missing_newline_changes() {
        assert_eq!(
            numbered_diff(concat!(
                "--- a/file\n+++ b/file\n@@ -4 +9 @@\n--- old\n",
                "\\ No newline at end of file\n+++ new\n\\ No newline at end of file\n"
            )),
            "   4    9 │ ~++ new"
        );
    }
}
