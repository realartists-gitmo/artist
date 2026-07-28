pub(super) fn numbered_diff(diff: &str) -> String {
    let mut old_line = 0usize;
    let mut new_line = 0usize;
    let mut output = Vec::new();
    for line in diff.lines() {
        if line.starts_with("@@") {
            let mut ranges = line.split_whitespace().skip(1);
            old_line = diff_range_start(ranges.next()).unwrap_or(old_line);
            new_line = diff_range_start(ranges.next()).unwrap_or(new_line);
            continue;
        }
        if line.starts_with("---") || line.starts_with("+++") {
            continue;
        }
        if line.starts_with('-') {
            output.push(format!("{old_line:>4}      │ {line}"));
            old_line += 1;
        } else if line.starts_with('+') {
            output.push(format!("     {new_line:>4} │ {line}"));
            new_line += 1;
        } else if line.starts_with(' ') {
            output.push(format!("{old_line:>4} {new_line:>4} │ {line}"));
            old_line += 1;
            new_line += 1;
        } else {
            output.push(format!("          │ {line}"));
        }
    }
    output.join("\n")
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
            "  10   20 │  context\n  11      │ -old\n       21 │ +new"
        );
    }
}
