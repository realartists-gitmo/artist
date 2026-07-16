use serde_json::Value;
use std::collections::HashMap;

/// Standardized presentation state for tool calls, independent of rendering.
#[derive(Default)]
pub struct ToolUi {
    calls: HashMap<String, bool>,
}

impl ToolUi {
    pub fn start(&mut self, id: String, name: &str, arguments: &Value) -> String {
        let title = title(name, arguments);
        self.calls.insert(id, false);
        title
    }

    /// Formats an output chunk. This API accepts chunks so future streaming
    /// tools use the same UI without changing the renderer.
    pub fn output(&mut self, id: &str, chunk: &str) -> String {
        let output_started = self.calls.entry(id.to_owned()).or_default();
        let prefix = if *output_started { "" } else { "= " };
        *output_started = true;
        format!("{prefix}{chunk}")
    }
}

fn title(name: &str, arguments: &Value) -> String {
    match name {
        "add_integers" => format!(
            "Add {} and {}",
            argument(arguments, "left"),
            argument(arguments, "right")
        ),
        _ => format!("{} {}", humanize(name), arguments),
    }
}

fn argument<'a>(arguments: &'a Value, name: &str) -> &'a Value {
    arguments.get(name).unwrap_or(&Value::Null)
}

fn humanize(name: &str) -> String {
    let mut words = name.split('_');
    let first = words.next().unwrap_or("tool");
    let mut result = first.to_owned();
    if let Some(initial) = result.get_mut(0..1) {
        initial.make_ascii_uppercase();
    }
    for word in words {
        result.push(' ');
        result.push_str(word);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_addition_and_output() {
        let mut ui = ToolUi::default();
        let title = ui.start(
            "call-1".into(),
            "add_integers",
            &serde_json::json!({
                "left": -199,
                "right": 201
            }),
        );
        assert_eq!(title, "Add -199 and 201");
        assert_eq!(ui.output("call-1", "2"), "= 2");
    }
}
