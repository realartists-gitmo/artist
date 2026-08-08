use serde_json::Value;

use super::{TitleSegment, ToolTitle};

pub(super) fn title(name: &str, arguments: &Value) -> ToolTitle {
    let path = string(arguments, "path");
    let query = string(arguments, "query");
    match name {
        "read" => composed([prose("Read "), input(path)]),
        "find" => {
            if query.is_empty() {
                plain("Listed project files")
            } else {
                composed([prose("Searched files for “"), input(query), prose("”")])
            }
        }
        "grep" => composed([prose("Searched code for “"), input(query), prose("”")]),
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
            composed([
                prose("Web searched for “"),
                input(shortened(&query, 100)),
                prose("”"),
            ])
        }
        "edit" => composed([prose("Edited "), input(path)]),
        "write" => composed([prose("Wrote "), input(path)]),
        "bash" => bash_title(arguments),
        "skill" => skill_title(arguments),
        "subagent" => subagent_title(arguments),
        "computer" => computer_title(arguments),
        "canvas" => canvas_title(arguments),
        _ => plain(humanize(name)),
    }
}

/// Name a canvas action by mode and slug.
///
/// Every mode rendered as a bare `Canvas` before, so creating an app looked
/// identical to reading its status — and the slug, the one thing that lets the
/// user find it again, was nowhere.
fn canvas_title(arguments: &Value) -> ToolTitle {
    let query = string(arguments, "query");
    if query.is_empty() {
        plain("Searched canvases")
    } else {
        composed([prose("Canvas query “"), input(query), prose("”")])
    }
}

fn computer_title(arguments: &Value) -> ToolTitle {
    let action = string(arguments, "action");
    if action.is_empty() {
        return plain("Computer");
    }
    let session = string(arguments, "session");
    let args = arguments.get("args").unwrap_or(&Value::Null);
    match action.as_str() {
        "launch" => composed([prose("Launched "), input(string(args, "command"))]),
        "attach" => composed([prose("Attached to "), input(string(args, "endpoint"))]),
        "observe" => composed([prose("Observed "), input(session)]),
        "screenshot" => composed([prose("Captured "), input(session)]),
        "watch" => composed([prose("Showed "), input(session)]),
        "do" => {
            let steps = args.get("steps").and_then(Value::as_array);
            match steps.map(Vec::as_slice) {
                Some(steps) if !steps.is_empty() => {
                    let named: Vec<String> = steps.iter().filter_map(step_summary).collect();
                    if named.is_empty() {
                        composed([
                            prose(format!("Ran {} steps on ", steps.len())),
                            input(session),
                        ])
                    } else {
                        composed([
                            prose("Ran "),
                            input(named.join(", ")),
                            prose(" on "),
                            input(session),
                        ])
                    }
                }
                _ => composed([prose("Used computer "), input(session)]),
            }
        }
        other => composed([prose(format!("Computer {other} on ")), input(session)]),
    }
}

fn step_summary(step: &Value) -> Option<String> {
    let (action, body) = step.as_object()?.iter().next()?;
    let verb = action.as_str();
    let detail = match verb {
        "key" => match body {
            // Both wire forms: a bare chord, or a chord that names its target.
            Value::String(chord) => chord.clone(),
            _ => {
                let chord = string(body, "chord");
                let label = string(body, "label");
                if label.is_empty() {
                    chord
                } else {
                    format!("{chord} on {label}")
                }
            }
        },
        "navigate" => string(body, "url"),
        "scroll" | "back" | "forward" => String::new(),
        _ => string(body, "label"),
    };
    let detail: String = detail.chars().take(40).collect();
    Some(if detail.is_empty() {
        verb.to_owned()
    } else {
        format!("{verb} {detail}")
    })
}

fn skill_title(arguments: &Value) -> ToolTitle {
    let query = string(arguments, "query");
    if query.is_empty() {
        plain("Listed Agent Skills")
    } else {
        composed([prose("Skill query “"), input(query), prose("”")])
    }
}

fn bash_title(arguments: &Value) -> ToolTitle {
    shell_command_title("Started shell: ", arguments)
}

fn shell_command_title(prefix: &str, arguments: &Value) -> ToolTitle {
    composed([
        prose(prefix),
        input(shortened(&string(arguments, "command"), 80)),
    ])
}

fn subagent_title(arguments: &Value) -> ToolTitle {
    composed([
        prose("Started "),
        input(subagent_role(arguments)),
        prose(" subagent: "),
        input(shortened(&string(arguments, "prompt"), 80)),
    ])
}

fn subagent_role(arguments: &Value) -> String {
    let profile = string(arguments, "profile");
    if profile.is_empty() {
        "default".into()
    } else {
        profile
    }
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

pub(super) fn shortened(value: &str, max: usize) -> String {
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

fn plain(text: impl Into<String>) -> ToolTitle {
    ToolTitle {
        segments: vec![prose(text)],
    }
}

fn composed(segments: impl IntoIterator<Item = TitleSegment>) -> ToolTitle {
    ToolTitle {
        segments: segments
            .into_iter()
            .filter(|segment| match segment {
                TitleSegment::Prose(text) | TitleSegment::Input(text) => !text.is_empty(),
            })
            .collect(),
    }
}

fn prose(text: impl Into<String>) -> TitleSegment {
    TitleSegment::Prose(text.into())
}

fn input(text: impl Into<String>) -> TitleSegment {
    TitleSegment::Input(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(title: &ToolTitle) -> Vec<&str> {
        title
            .segments
            .iter()
            .filter_map(|segment| match segment {
                TitleSegment::Input(text) => Some(text.as_str()),
                TitleSegment::Prose(_) => None,
            })
            .collect()
    }

    #[test]
    fn canvas_title_reflects_the_query_only_surface() {
        assert_eq!(
            title("canvas", &serde_json::json!({"query":"test-dashboard"})).plain_text(),
            "Canvas query “test-dashboard”"
        );
        assert_eq!(
            title("canvas", &serde_json::json!({"query":""})).plain_text(),
            "Searched canvases"
        );
    }

    #[test]
    fn skill_title_reflects_the_query_only_surface() {
        assert_eq!(
            title("skill", &serde_json::json!({"query":"pdf"})).plain_text(),
            "Skill query “pdf”"
        );
        assert_eq!(
            title("skill", &serde_json::json!({"query":""})).plain_text(),
            "Listed Agent Skills"
        );
    }

    #[test]
    fn marks_only_displayed_input_values_for_accenting() {
        for (name, arguments, expected) in [
            (
                "read",
                serde_json::json!({"path":"src/lib.rs"}),
                vec!["src/lib.rs"],
            ),
            (
                "find",
                serde_json::json!({"query":"config"}),
                vec!["config"],
            ),
            ("grep", serde_json::json!({"query":"TODO"}), vec!["TODO"]),
            (
                "web_search",
                serde_json::json!({"queries":["rust","tui"]}),
                vec!["rust; tui"],
            ),
            ("edit", serde_json::json!({"path":"old.rs"}), vec!["old.rs"]),
            (
                "write",
                serde_json::json!({"path":"new.rs"}),
                vec!["new.rs"],
            ),
            (
                "bash",
                serde_json::json!({"command":"cargo test"}),
                vec!["cargo test"],
            ),
            ("skill", serde_json::json!({"query":"pdf"}), vec!["pdf"]),
            (
                "subagent",
                serde_json::json!({"profile":"explorer","prompt":"inspect"}),
                vec!["explorer", "inspect"],
            ),
        ] {
            assert_eq!(inputs(&title(name, &arguments)), expected, "{name}");
        }
        assert!(inputs(&title("find", &serde_json::json!({}))).is_empty());
    }
}
