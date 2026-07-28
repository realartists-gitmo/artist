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
        _ => plain(humanize(name)),
    }
}

fn skill_title(arguments: &Value) -> ToolTitle {
    match string(arguments, "mode").as_str() {
        "" | "list" => plain("Listed Agent Skills"),
        "activate" => composed([
            prose("Loaded "),
            input(string(arguments, "name")),
            prose(" skill"),
        ]),
        "readResource" => composed([
            prose("Read "),
            input(string(arguments, "name")),
            prose(" skill resource "),
            input(string(arguments, "path")),
        ]),
        _ => plain("Skill"),
    }
}

fn bash_title(arguments: &Value) -> ToolTitle {
    match string(arguments, "mode").as_str() {
        "exec" if arguments.get("background").and_then(Value::as_bool) == Some(true) => {
            shell_command_title("Started shell: ", arguments)
        }
        "start" => shell_command_title("Started shell: ", arguments),
        "send" => plain("Sent input to shell"),
        "read" => plain("Checked shell output"),
        "stop" => plain("Stopped shell"),
        "list" => plain("Listed shell sessions"),
        _ => shell_command_title("Ran: ", arguments),
    }
}

fn shell_command_title(prefix: &str, arguments: &Value) -> ToolTitle {
    composed([
        prose(prefix),
        input(shortened(&string(arguments, "command"), 80)),
    ])
}

fn subagent_title(arguments: &Value) -> ToolTitle {
    match string(arguments, "mode").as_str() {
        "status" | "read" => subagent_task_title("Checked subagent ", arguments),
        "wait" => subagent_task_title("Waited for subagent ", arguments),
        "cancel" => subagent_task_title("Cancelled subagent ", arguments),
        "list" => plain("Listed subagent tasks"),
        _ if arguments.get("background").and_then(Value::as_bool) == Some(true)
            || string(arguments, "mode") == "start" =>
        {
            subagent_launch_title("Started ", arguments)
        }
        _ => subagent_launch_title("", arguments),
    }
}

fn subagent_task_title(prefix: &str, arguments: &Value) -> ToolTitle {
    composed([prose(prefix), input(string(arguments, "taskId"))])
}

fn subagent_launch_title(prefix: &str, arguments: &Value) -> ToolTitle {
    composed([
        prose(prefix),
        input(subagent_role(arguments)),
        prose(" subagent: "),
        input(shortened(&string(arguments, "prompt"), 80)),
    ])
}

fn subagent_role(arguments: &Value) -> String {
    let role = string(arguments, "agent");
    if role.is_empty() {
        "default".into()
    } else {
        role
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
    fn describes_each_skill_operation() {
        assert_eq!(
            title("skill", &serde_json::json!({})).plain_text(),
            "Listed Agent Skills"
        );
        assert_eq!(
            title(
                "skill",
                &serde_json::json!({"mode":"activate","name":"pdf"})
            )
            .plain_text(),
            "Loaded pdf skill"
        );
        assert_eq!(
            title(
                "skill",
                &serde_json::json!({
                    "mode":"readResource",
                    "name":"pdf",
                    "path":"references/forms.md"
                })
            )
            .plain_text(),
            "Read pdf skill resource references/forms.md"
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
            (
                "skill",
                serde_json::json!({"mode":"activate","name":"pdf"}),
                vec!["pdf"],
            ),
            (
                "skill",
                serde_json::json!({"mode":"readResource","name":"pdf","path":"forms.md"}),
                vec!["pdf", "forms.md"],
            ),
            (
                "subagent",
                serde_json::json!({"agent":"explorer","prompt":"inspect"}),
                vec!["explorer", "inspect"],
            ),
            (
                "subagent",
                serde_json::json!({"mode":"status","taskId":"a-blue-fox"}),
                vec!["a-blue-fox"],
            ),
        ] {
            assert_eq!(inputs(&title(name, &arguments)), expected, "{name}");
        }
        assert!(inputs(&title("find", &serde_json::json!({}))).is_empty());
    }
}
