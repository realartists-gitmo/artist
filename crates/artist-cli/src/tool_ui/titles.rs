use serde_json::Value;

pub(super) fn title(name: &str, arguments: &Value) -> String {
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
        "bash" => bash_title(arguments),
        "skill" => skill_title(arguments),
        "subagent" => subagent_title(arguments),
        _ => humanize(name),
    }
}

fn skill_title(arguments: &Value) -> String {
    match string(arguments, "mode").as_str() {
        "" | "list" => "Listed Agent Skills".into(),
        "activate" => format!("Loaded {} skill", string(arguments, "name")),
        "readResource" => format!(
            "Read {} skill resource {}",
            string(arguments, "name"),
            string(arguments, "path")
        ),
        _ => "Skill".into(),
    }
}

fn bash_title(arguments: &Value) -> String {
    match string(arguments, "mode").as_str() {
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
    }
}

fn subagent_title(arguments: &Value) -> String {
    match string(arguments, "mode").as_str() {
        "status" | "read" => format!("Checked subagent {}", string(arguments, "taskId")),
        "wait" => format!("Waited for subagent {}", string(arguments, "taskId")),
        "cancel" => format!("Cancelled subagent {}", string(arguments, "taskId")),
        "list" => "Listed subagent tasks".into(),
        _ if arguments.get("background").and_then(Value::as_bool) == Some(true)
            || string(arguments, "mode") == "start" =>
        {
            format!(
                "Started {} subagent: {}",
                subagent_role(arguments),
                shortened(&string(arguments, "prompt"), 80)
            )
        }
        _ => format!(
            "{} subagent: {}",
            subagent_role(arguments),
            shortened(&string(arguments, "prompt"), 80)
        ),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_each_skill_operation() {
        assert_eq!(
            title("skill", &serde_json::json!({})),
            "Listed Agent Skills"
        );
        assert_eq!(
            title(
                "skill",
                &serde_json::json!({"mode":"activate","name":"pdf"})
            ),
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
            ),
            "Read pdf skill resource references/forms.md"
        );
    }
}
