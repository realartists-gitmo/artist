pub const OUTPUT_CAP: usize = 50 * 1024;

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
