use serde_json::Value;

use super::{diff::numbered_diff, titles::shortened};

const DISPLAY_OUTPUT_LIMIT: usize = 1200;

pub(super) struct PresentedOutput {
    pub preview: String,
    pub is_diff: bool,
}

pub(super) fn present(name: &str, arguments: &Value, raw_output: &str) -> PresentedOutput {
    match name {
        "bash" => line_preview(bash_semantic(raw_output), 5),
        "edit" => edit_preview(raw_output),
        "find" | "grep" | "read" => line_preview(raw_output.to_owned(), 10),
        "skill" => line_preview(raw_output.to_owned(), 30),
        "write" => line_preview(
            arguments
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            30,
        ),
        "subagent" => PresentedOutput {
            preview: compact_subagent_output(raw_output),
            is_diff: false,
        },
        _ => PresentedOutput {
            preview: bounded_bytes(raw_output.trim(), DISPLAY_OUTPUT_LIMIT),
            is_diff: false,
        },
    }
}

fn line_preview(semantic_output: String, limit: usize) -> PresentedOutput {
    PresentedOutput {
        preview: limited_lines(semantic_output.trim(), limit),
        is_diff: false,
    }
}

fn limited_lines(output: &str, limit: usize) -> String {
    let lines = output.lines().collect::<Vec<_>>();
    if lines.len() <= limit {
        return lines.join("\n");
    }
    let omitted = lines.len() - limit;
    let mut preview = lines[..limit].join("\n");
    if !preview.is_empty() {
        preview.push('\n');
    }
    let noun = if omitted == 1 { "line" } else { "lines" };
    preview.push_str(&format!("[{omitted} {noun} omitted]"));
    preview
}

fn bash_semantic(output: &str) -> String {
    let lines = output.lines().collect::<Vec<_>>();
    let Some(status) = lines.first().filter(|line| line.starts_with("status:")) else {
        return output.trim().to_owned();
    };
    let payload_start = lines
        .iter()
        .position(|line| !is_bash_header(line))
        .unwrap_or(lines.len());
    let payload = lines[payload_start..].join("\n");
    if payload.trim().is_empty() {
        (*status).to_owned()
    } else {
        payload.trim().to_owned()
    }
}

fn is_bash_header(line: &str) -> bool {
    [
        "status:",
        "exitCode:",
        "timeout:",
        "truncated:",
        "sessionId:",
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
}

fn edit_preview(output: &str) -> PresentedOutput {
    let semantic_output = output
        .split_once("Diff:\n")
        .map(|(_, diff)| numbered_diff(diff))
        .unwrap_or_else(|| output.lines().next().unwrap_or("Completed").to_owned());
    PresentedOutput {
        preview: truncate_edit(&semantic_output),
        is_diff: true,
    }
}

/// Preserve the edit panel's pre-existing byte cap and ellipsis behavior.
fn truncate_edit(output: &str) -> String {
    let mut end = output.len().min(DISPLAY_OUTPUT_LIMIT);
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}{}",
        &output[..end],
        if end < output.len() { "…" } else { "" }
    )
}

fn bounded_bytes(output: &str, limit: usize) -> String {
    if output.len() <= limit {
        return output.to_owned();
    }
    const MARKER: &str = "…";
    let mut end = limit.saturating_sub(MARKER.len());
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{MARKER}", &output[..end])
}

fn compact_subagent_output(output: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(output) else {
        return shortened(output.trim(), DISPLAY_OUTPUT_LIMIT);
    };
    if let Some(tasks) = value.as_array() {
        if tasks.is_empty() {
            return "No subagent tasks".into();
        }
        return tasks
            .iter()
            .map(compact_subagent_task)
            .collect::<Vec<_>>()
            .join("\n");
    }
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let role = value
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("default");
    if let Some(result) = value.get("output").and_then(Value::as_str) {
        return format!("{role} · completed\n{}", truncate_subagent_text(result));
    }
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return format!("{role} · failed\n{}", truncate_subagent_text(error));
    }
    let id = value
        .get("taskId")
        .and_then(Value::as_str)
        .unwrap_or("delegate");
    format!("{id} · {role} · {status}")
}

fn compact_subagent_task(task: &Value) -> String {
    let id = task
        .get("taskId")
        .and_then(Value::as_str)
        .unwrap_or("delegate");
    let status = task
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let role = task
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("default");
    let prompt = task.get("prompt").and_then(Value::as_str).unwrap_or("");
    format!(
        "{id} · {role} · {status}{}",
        if prompt.is_empty() {
            String::new()
        } else {
            format!(" · {prompt}")
        }
    )
}

fn truncate_subagent_text(output: &str) -> String {
    const MAX_LINES: usize = 8;
    const MAX_BYTES: usize = 600;
    let lines = output.trim().lines().collect::<Vec<_>>();
    let limited = lines
        .iter()
        .take(MAX_LINES)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let was_truncated = lines.len() > MAX_LINES || limited.len() > MAX_BYTES;
    let mut result = shortened(&limited, MAX_BYTES);
    if was_truncated && !result.ends_with('…') {
        result.push_str("\n…");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered_lines(count: usize) -> String {
        (1..=count)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn strips_bash_protocol_and_limits_terminal_output_to_five_lines() {
        let presented = present(
            "bash",
            &serde_json::json!({"mode":"exec"}),
            &format!(
                "status: completed\nexitCode: Some(0)\ntruncated: false\n{}",
                numbered_lines(7)
            ),
        );
        assert_eq!(
            presented.preview,
            "line 1\nline 2\nline 3\nline 4\nline 5\n[2 lines omitted]"
        );
    }

    #[test]
    fn bash_without_terminal_output_falls_back_to_status() {
        let presented = present(
            "bash",
            &serde_json::json!({"mode":"exec"}),
            "status: completed\nexitCode: Some(0)\ntruncated: false\n",
        );
        assert_eq!(presented.preview, "status: completed");
    }

    #[test]
    fn applies_exact_line_limits_and_omission_markers() {
        for (name, limit) in [("find", 10), ("grep", 10), ("read", 10), ("skill", 30)] {
            let exact = numbered_lines(limit);
            let exact_presented = present(name, &serde_json::json!({}), &exact);
            assert_eq!(exact_presented.preview, exact);

            let output = numbered_lines(limit + 1);
            let presented = present(name, &serde_json::json!({}), &output);
            assert_eq!(presented.preview.lines().count(), limit + 1);
            assert!(presented.preview.ends_with("[1 line omitted]"));
        }
    }

    #[test]
    fn write_previews_argument_content_instead_of_result_diff() {
        let content = numbered_lines(31);
        let presented = present(
            "write",
            &serde_json::json!({"path":"new.rs","content":content}),
            "Written new.rs.\n\nDiff:\n+not the preview",
        );
        assert_eq!(presented.preview.lines().count(), 31);
        assert!(presented.preview.ends_with("[1 line omitted]"));
        assert!(!presented.is_diff);
    }

    #[test]
    fn edit_rendering_remains_numbered_and_diff_styled() {
        let presented = present(
            "edit",
            &serde_json::json!({}),
            "Applied edit.\n\nDiff:\n@@ -1 +1 @@\n-old\n+new\n",
        );
        assert_eq!(presented.preview, "   1      │ -old\n        1 │ +new");
        assert!(presented.is_diff);
    }

    #[test]
    fn generic_preview_is_utf8_safe_and_strictly_byte_bounded() {
        let output = "界".repeat(DISPLAY_OUTPUT_LIMIT);
        let presented = present("extension_tool", &serde_json::json!({}), &output);
        assert!(presented.preview.len() <= DISPLAY_OUTPUT_LIMIT);
        assert!(presented.preview.ends_with('…'));
    }
}
