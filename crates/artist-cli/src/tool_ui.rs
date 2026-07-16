use serde_json::Value;
use std::collections::HashMap;

const DISPLAY_OUTPUT_LIMIT: usize = 1200;

/// Standardized presentation state for tool calls, independent of rendering.
#[derive(Default)]
pub struct ToolUi {
    calls: HashMap<String, CallState>,
}

struct CallState {
    name: String,
    output_started: bool,
    displayed_bytes: usize,
}

impl ToolUi {
    pub fn start(&mut self, id: String, name: &str, arguments: &Value) -> String {
        let title = title(name, arguments);
        self.calls.insert(
            id,
            CallState {
                name: name.to_owned(),
                output_started: false,
                displayed_bytes: 0,
            },
        );
        title
    }

    /// Formats a bounded output chunk. Future streaming tools can call this
    /// repeatedly without allowing tool output to dominate the transcript.
    pub fn output(&mut self, id: &str, chunk: &str) -> String {
        let call = self
            .calls
            .entry(id.to_owned())
            .or_insert_with(|| CallState {
                name: "tool".into(),
                output_started: false,
                displayed_bytes: 0,
            });
        let compact = compact_output(&call.name, chunk);
        let remaining = DISPLAY_OUTPUT_LIMIT.saturating_sub(call.displayed_bytes);
        if remaining == 0 {
            return String::new();
        }
        let mut end = compact.len().min(remaining);
        while end > 0 && !compact.is_char_boundary(end) {
            end -= 1;
        }
        let was_truncated = end < compact.len();
        let prefix = if call.output_started { "" } else { "= " };
        call.output_started = true;
        call.displayed_bytes += end;
        format!(
            "{prefix}{}{}",
            &compact[..end],
            if was_truncated { "…" } else { "" }
        )
    }
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
        "edit" => format!("Edited {path}"),
        "write" => format!("Wrote {path}"),
        "bash" => match string(arguments, "mode").as_str() {
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
        "delegate" => format!("Delegated: {}", shortened(&string(arguments, "prompt"), 80)),
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
        "edit" | "write" => output.lines().next().unwrap_or("Completed").to_owned(),
        _ => shortened(output.trim(), DISPLAY_OUTPUT_LIMIT),
    }
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
            "Searched files for “config”"
        );
        assert_eq!(
            ui.output("f", "src/config.rs\nconfig.toml"),
            "= Found 2 files"
        );
        assert_eq!(
            ui.start(
                "e".into(),
                "edit",
                &serde_json::json!({"path":"src/lib.rs"})
            ),
            "Edited src/lib.rs"
        );
    }
}
