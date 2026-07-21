use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use unicode_width::UnicodeWidthStr;

const DISPLAY_OUTPUT_LIMIT: usize = 1200;

/// Standardized presentation state for tool calls, independent of rendering.
#[derive(Default)]
pub struct ToolUi {
    calls: HashMap<String, CallState>,
    order: VecDeque<String>,
    pending: HashSet<String>,
    custom_icons: HashMap<String, String>,
}

pub struct ToolOutput {
    pub lines: Vec<ToolLine>,
    pub batch_complete: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ToolLine {
    pub text: String,
    pub first: bool,
    pub is_diff: bool,
    pub icon: Option<String>,
}

struct CallState {
    name: String,
    title: String,
    icon: Option<String>,
    title_displayed: bool,
    completed_output: Option<(String, bool)>,
}

impl ToolUi {
    pub fn with_icons(mut custom_icons: HashMap<String, String>) -> Self {
        custom_icons.retain(|_, icon| valid_icon(icon));
        Self {
            custom_icons,
            ..Self::default()
        }
    }

    /// Registers a call and returns its title only when it is the next transcript slot.
    /// Later calls run concurrently, but remain buffered behind earlier call output.
    pub fn start(&mut self, id: String, name: &str, arguments: &Value) -> Option<ToolLine> {
        let show_now = self.order.is_empty();
        let call_title = title(name, arguments);
        let icon = icon_for(name, &self.custom_icons).map(str::to_owned);
        self.pending.insert(id.clone());
        self.order.push_back(id.clone());
        self.calls.insert(
            id,
            CallState {
                name: name.to_owned(),
                title: call_title.clone(),
                icon: icon.clone(),
                title_displayed: show_now,
                completed_output: None,
            },
        );
        show_now.then_some(ToolLine {
            text: call_title,
            first: true,
            is_diff: false,
            icon,
        })
    }

    /// Stores a completed result in its call slot, then emits every contiguous
    /// ready slot in call order. Results may arrive in any execution order.
    pub fn output(&mut self, id: &str, chunk: &str) -> ToolOutput {
        let call = self
            .calls
            .entry(id.to_owned())
            .or_insert_with(|| CallState {
                name: "tool".into(),
                title: "Tool".into(),
                icon: None,
                title_displayed: true,
                completed_output: None,
            });
        let compact = compact_output(&call.name, chunk);
        let mut end = compact.len().min(DISPLAY_OUTPUT_LIMIT);
        while end > 0 && !compact.is_char_boundary(end) {
            end -= 1;
        }
        let was_truncated = end < compact.len();
        let is_diff = matches!(call.name.as_str(), "edit" | "write");
        let prefix = if is_diff { "" } else { "= " };
        call.completed_output = Some((
            format!(
                "{prefix}{}{}",
                &compact[..end],
                if was_truncated { "…" } else { "" }
            ),
            is_diff,
        ));
        self.pending.remove(id);

        let mut lines = Vec::new();
        while let Some(front_id) = self.order.front().cloned() {
            let Some(front) = self.calls.get_mut(&front_id) else {
                self.order.pop_front();
                continue;
            };
            let Some((text, is_diff)) = front.completed_output.take() else {
                break;
            };
            if !text.is_empty() {
                lines.push(ToolLine {
                    text,
                    first: false,
                    is_diff,
                    icon: None,
                });
            }
            self.order.pop_front();
            self.calls.remove(&front_id);
            if let Some(next_id) = self.order.front()
                && let Some(next) = self.calls.get_mut(next_id)
                && !next.title_displayed
            {
                next.title_displayed = true;
                lines.push(ToolLine {
                    text: next.title.clone(),
                    first: true,
                    is_diff: false,
                    icon: next.icon.clone(),
                });
            }
        }
        ToolOutput {
            lines,
            batch_complete: self.pending.is_empty() && self.order.is_empty(),
        }
    }
}

pub fn icon_for<'a>(name: &str, custom_icons: &'a HashMap<String, String>) -> Option<&'a str> {
    builtin_icon(name).or_else(|| custom_icons.get(name).map(String::as_str))
}

fn builtin_icon(name: &str) -> Option<&'static str> {
    match name {
        "bash" => Some("$"),
        "delegate" => Some("♟"),
        "edit" => Some("🖉"),
        "find" => Some("🗁"),
        "grep" => Some("⌕"),
        "read" => Some("🕮"),
        "skill" => Some("🗡"),
        "write" => Some("🗎"),
        _ => None,
    }
}

fn valid_icon(icon: &str) -> bool {
    !icon.is_empty()
        && icon.chars().count() <= 8
        && !icon
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        && matches!(UnicodeWidthStr::width(icon), 1..=2)
}

fn title(name: &str, arguments: &Value) -> String {
    let path = string(arguments, "path");
    let query = string(arguments, "query");
    match name {
        "read" => format!("Read {path}"),
        "find" => {
            if query.is_empty() {
                "Listed project files".into()
            } else {
                format!("Searched files for “{query}”")
            }
        }
        "grep" => format!("Searched code for “{query}”"),
        "web_search" => {
            let query = if query.is_empty() {
                arguments
                    .get("queries")
                    .and_then(Value::as_array)
                    .map(|queries| {
                        queries
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                    .unwrap_or_default()
            } else {
                query
            };
            format!("Web searched for “{}”", shortened(&query, 100))
        }
        "edit" => format!("Edited {path}"),
        "write" => format!("Wrote {path}"),
        "bash" => match string(arguments, "mode").as_str() {
            "exec" if arguments.get("background").and_then(Value::as_bool) == Some(true) => {
                format!(
                    "Started shell: {}",
                    shortened(&string(arguments, "command"), 80)
                )
            }
            "start" => format!(
                "Started shell: {}",
                shortened(&string(arguments, "command"), 80)
            ),
            "send" => "Sent input to shell".into(),
            "read" => "Checked shell output".into(),
            "stop" => "Stopped shell".into(),
            "list" => "Listed shell sessions".into(),
            _ => format!("Ran: {}", shortened(&string(arguments, "command"), 80)),
        },
        "delegate" => match string(arguments, "mode").as_str() {
            "status" | "read" => format!("Checked delegate {}", string(arguments, "taskId")),
            "wait" => format!("Waited for delegate {}", string(arguments, "taskId")),
            "cancel" => format!("Cancelled delegate {}", string(arguments, "taskId")),
            "list" => "Listed delegate tasks".into(),
            _ if arguments.get("background").and_then(Value::as_bool) == Some(true)
                || string(arguments, "mode") == "start" =>
            {
                format!(
                    "Started delegate: {}",
                    shortened(&string(arguments, "prompt"), 80)
                )
            }
            _ => format!("Delegated: {}", shortened(&string(arguments, "prompt"), 80)),
        },
        _ => humanize(name),
    }
}

fn compact_output(name: &str, output: &str) -> String {
    match name {
        "find" => format!("Found {} files", result_count(output)),
        "grep" => format!(
            "Found {} matches",
            output.lines().filter(|line| line.contains(':')).count()
        ),
        "read" => format!(
            "Read {} lines",
            output.lines().filter(|line| line.contains(" | ")).count()
        ),
        "edit" | "write" => output
            .split_once("Diff:\n")
            .map(|(_, diff)| numbered_diff(diff))
            .unwrap_or_else(|| output.lines().next().unwrap_or("Completed").to_owned()),
        "delegate" => compact_delegate_output(output),
        _ => shortened(output.trim(), DISPLAY_OUTPUT_LIMIT),
    }
}

fn numbered_diff(diff: &str) -> String {
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

fn compact_delegate_output(output: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(output) else {
        return shortened(output.trim(), DISPLAY_OUTPUT_LIMIT);
    };
    if let Some(tasks) = value.as_array() {
        if tasks.is_empty() {
            return "No delegate tasks".into();
        }
        return tasks
            .iter()
            .map(|task| {
                let id = task
                    .get("taskId")
                    .and_then(Value::as_str)
                    .unwrap_or("delegate");
                let status = task
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let prompt = task.get("prompt").and_then(Value::as_str).unwrap_or("");
                format!(
                    "{id} · {status}{}",
                    if prompt.is_empty() {
                        String::new()
                    } else {
                        format!(" · {prompt}")
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if let Some(result) = value.get("output").and_then(Value::as_str) {
        return format!("Completed\n{}", truncate_delegate_text(result));
    }
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return format!("Failed\n{}", truncate_delegate_text(error));
    }
    let id = value
        .get("taskId")
        .and_then(Value::as_str)
        .unwrap_or("delegate");
    format!("{id} · {status}")
}

fn truncate_delegate_text(output: &str) -> String {
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

fn result_count(output: &str) -> usize {
    output
        .lines()
        .filter(|line| !line.starts_with('[') && *line != "No files found.")
        .count()
}
fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}
fn shortened(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_owned();
    }
    let mut end = max.saturating_sub(1);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}
fn humanize(name: &str) -> String {
    let mut result = name.replace('_', " ");
    if let Some(initial) = result.get_mut(0..1) {
        initial.make_ascii_uppercase();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_standard_tool_titles_and_compact_output() {
        let mut ui = ToolUi::default();
        assert_eq!(
            ui.start("f".into(), "find", &serde_json::json!({"query":"config"})),
            Some(ToolLine {
                text: "Searched files for “config”".into(),
                first: true,
                is_diff: false,
                icon: Some("🗁".into()),
            })
        );
        let first = ui.output("f", "src/config.rs\nconfig.toml");
        assert_eq!(first.lines[0].text, "= Found 2 files");
        assert!(first.batch_complete);

        assert_eq!(
            ui.start(
                "w".into(),
                "web_search",
                &serde_json::json!({"query":"rust async runtimes"})
            ),
            Some(ToolLine {
                text: "Web searched for “rust async runtimes”".into(),
                first: true,
                is_diff: false,
                icon: None,
            })
        );
        ui.output("w", "results");
        assert_eq!(
            ui.start(
                "e".into(),
                "edit",
                &serde_json::json!({"path":"src/lib.rs"})
            ),
            Some(ToolLine {
                text: "Edited src/lib.rs".into(),
                first: true,
                is_diff: false,
                icon: Some("🖉".into()),
            })
        );
        assert_eq!(
            ui.output("e", "Applied edit.\n\nDiff:\n@@ -1 +1 @@\n-old\n+new\n")
                .lines[0]
                .text,
            "   1      │ -old\n        1 │ +new"
        );

        ui.start(
            "d".into(),
            "delegate",
            &serde_json::json!({"mode":"read","taskId":"delegate-1"}),
        );
        assert_eq!(
            ui.output(
                "d",
                r#"{"taskId":"delegate-1","status":"completed","output":"Found the bug."}"#,
            )
            .lines[0]
                .text,
            "= Completed\nFound the bug."
        );
        assert_eq!(
            numbered_diff("@@ -10,2 +20,2 @@\n context\n-old\n+new\n"),
            "  10   20 │  context\n  11      │ -old\n       21 │ +new"
        );
        assert_eq!(
            truncate_delegate_text("1\n2\n3\n4\n5\n6\n7\n8\n9"),
            "1\n2\n3\n4\n5\n6\n7\n8\n…"
        );
    }

    #[test]
    fn emits_concurrent_calls_and_results_in_call_order() {
        let mut ui = ToolUi::default();
        assert!(
            ui.start("a".into(), "find", &serde_json::json!({"query":"a"}))
                .is_some()
        );
        assert!(
            ui.start("b".into(), "find", &serde_json::json!({"query":"b"}))
                .is_none()
        );

        let early_second = ui.output("b", "b.rs");
        assert!(early_second.lines.is_empty());
        assert!(!early_second.batch_complete);

        let released = ui.output("a", "a.rs");
        assert!(released.batch_complete);
        assert_eq!(released.lines.len(), 3);
        assert_eq!(released.lines[0].text, "= Found 1 files");
        assert_eq!(released.lines[1].text, "Searched files for “b”");
        assert!(released.lines[1].first);
        assert_eq!(released.lines[1].icon.as_deref(), Some("🗁"));
        assert_eq!(released.lines[2].text, "= Found 1 files");
    }

    #[test]
    fn built_in_icons_are_stable() {
        let icons = HashMap::new();
        for (name, expected) in [
            ("bash", "$"),
            ("delegate", "♟"),
            ("edit", "🖉"),
            ("find", "🗁"),
            ("grep", "⌕"),
            ("read", "🕮"),
            ("skill", "🗡"),
            ("write", "🗎"),
        ] {
            assert_eq!(icon_for(name, &icons), Some(expected));
        }
        assert_eq!(icon_for("unknown", &icons), None);
    }

    #[test]
    fn uses_valid_custom_icons_and_falls_back_for_invalid_ones() {
        let mut ui = ToolUi::with_icons(HashMap::from([
            ("deploy".into(), "🚀".into()),
            ("broken".into(), "two words".into()),
        ]));
        assert_eq!(
            ui.start("a".into(), "deploy", &serde_json::json!({}))
                .unwrap()
                .icon
                .as_deref(),
            Some("🚀")
        );
        ui.output("a", "done");
        assert_eq!(
            ui.start("b".into(), "broken", &serde_json::json!({}))
                .unwrap()
                .icon,
            None
        );
    }
}
