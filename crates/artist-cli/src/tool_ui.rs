mod diff;
mod icons;
mod previews;
mod titles;

use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

use icons::valid_icon;
pub use icons::{FALLBACK as FALLBACK_ICON, icon_for};
use previews::present;
use titles::title;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRecord {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    pub raw_output: String,
    /// Full semantic output used to derive the bounded transcript preview.
    /// This may differ from raw tool protocol text (for example, bash headers
    /// are stripped and write exposes the requested file content).
    pub semantic_output: String,
}

impl ToolRecord {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: Value,
        raw_output: impl Into<String>,
    ) -> Self {
        let name = name.into();
        let raw_output = raw_output.into();
        let semantic_output = present(&name, &arguments, &raw_output).semantic_output;
        Self {
            id: id.into(),
            name,
            arguments,
            raw_output,
            semantic_output,
        }
    }

    pub fn title(&self) -> String {
        title(&self.name, &self.arguments)
    }
}

/// Render every semantic output line for the supervise view. Unlike the live
/// transcript preview, this applies neither line nor byte truncation.
pub fn expanded_lines(record: &ToolRecord) -> Vec<ToolLine> {
    let is_diff = record.name == "edit";
    record
        .semantic_output
        .lines()
        .map(|line| ToolLine {
            text: line.to_owned(),
            first: false,
            is_diff,
            icon: None,
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolLine {
    pub text: String,
    pub first: bool,
    pub is_diff: bool,
    pub icon: Option<String>,
}

struct CallState {
    name: String,
    arguments: Value,
    title: String,
    icon: Option<String>,
    title_displayed: bool,
    completed: Option<CompletedCall>,
}

struct CompletedCall {
    preview: String,
    is_diff: bool,
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
                arguments: arguments.clone(),
                title: call_title.clone(),
                icon: icon.clone(),
                title_displayed: show_now,
                completed: None,
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
        self.ensure_call_slot(id);
        let call = self.calls.get_mut(id).expect("call slot exists");
        let presented = present(&call.name, &call.arguments, chunk);
        call.completed = Some(CompletedCall {
            preview: presented.preview,
            is_diff: presented.is_diff,
        });
        self.pending.remove(id);

        let mut lines = Vec::new();
        while let Some(front_id) = self.order.front().cloned() {
            let Some(front) = self.calls.get_mut(&front_id) else {
                self.order.pop_front();
                continue;
            };
            let Some(completed) = front.completed.take() else {
                break;
            };
            if !completed.preview.is_empty() {
                let prefix = if completed.is_diff || is_structured_output(&front.name) {
                    ""
                } else {
                    "= "
                };
                lines.push(ToolLine {
                    text: format!("{prefix}{}", completed.preview),
                    first: false,
                    is_diff: completed.is_diff,
                    icon: None,
                });
            }
            self.order.pop_front();
            self.calls.remove(&front_id);
            self.release_next_title(&mut lines);
        }
        ToolOutput {
            lines,
            batch_complete: self.pending.is_empty() && self.order.is_empty(),
        }
    }

    fn ensure_call_slot(&mut self, id: &str) {
        if self.calls.contains_key(id) {
            return;
        }
        self.order.push_back(id.to_owned());
        self.calls.insert(
            id.to_owned(),
            CallState {
                name: "tool".into(),
                arguments: Value::Object(Default::default()),
                title: "Tool".into(),
                icon: icon_for("tool", &self.custom_icons).map(str::to_owned),
                title_displayed: true,
                completed: None,
            },
        );
    }

    fn release_next_title(&mut self, lines: &mut Vec<ToolLine>) {
        let Some(next_id) = self.order.front() else {
            return;
        };
        let Some(next) = self.calls.get_mut(next_id) else {
            return;
        };
        if next.title_displayed {
            return;
        }
        next.title_displayed = true;
        lines.push(ToolLine {
            text: next.title.clone(),
            first: true,
            is_diff: false,
            icon: next.icon.clone(),
        });
    }
}

fn is_structured_output(name: &str) -> bool {
    matches!(
        name,
        "bash" | "find" | "grep" | "read" | "skill" | "subagent" | "write"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_titles_and_truncated_previews() {
        let mut ui = ToolUi::default();
        assert_eq!(
            ui.start("f".into(), "find", &serde_json::json!({"query":"config"})),
            Some(ToolLine {
                text: "Searched files for “config”".into(),
                first: true,
                is_diff: false,
                icon: Some("".into()),
            })
        );
        let raw = (1..=12)
            .map(|index| format!("src/config-{index}.rs\t(score {index})"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = ui.output("f", &raw);
        assert_eq!(output.lines[0].text.lines().count(), 11);
        assert!(
            output.lines[0]
                .text
                .ends_with("[2 lines omitted · /supervise]")
        );
        assert!(output.batch_complete);
    }

    #[test]
    fn emits_concurrent_calls_results_and_records_in_call_order() {
        let mut ui = ToolUi::default();
        assert!(
            ui.start("a".into(), "find", &serde_json::json!({"query":"a"}))
                .is_some()
        );
        assert!(
            ui.start("b".into(), "read", &serde_json::json!({"path":"b.rs"}))
                .is_none()
        );

        let early_second = ui.output("b", "abc: b");
        assert!(early_second.lines.is_empty());
        assert!(!early_second.batch_complete);

        let released = ui.output("a", "a.rs\t(score 1)");
        assert!(released.batch_complete);
        assert_eq!(released.lines.len(), 3);
        assert_eq!(released.lines[0].text, "a.rs\t(score 1)");
        assert_eq!(released.lines[1].text, "Read b.rs");
        assert!(released.lines[1].first);
        assert_eq!(released.lines[1].icon.as_deref(), Some(""));
        assert_eq!(released.lines[2].text, "abc: b");
    }

    #[test]
    fn keeps_edit_rendering_unchanged() {
        let mut ui = ToolUi::default();
        ui.start(
            "e".into(),
            "edit",
            &serde_json::json!({"path":"src/lib.rs"}),
        );
        let output = ui.output("e", "Applied edit.\n\nDiff:\n@@ -1 +1 @@\n-old\n+new\n");
        assert_eq!(output.lines[0].text, "   1      │ -old\n        1 │ +new");
        assert!(output.lines[0].is_diff);
    }

    #[test]
    fn valid_custom_icons_survive_and_invalid_icons_use_fallback() {
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
                .icon
                .as_deref(),
            Some(FALLBACK_ICON)
        );
    }

    #[test]
    fn orphan_result_still_produces_output() {
        let mut ui = ToolUi::default();
        let output = ui.output("missing", "result");
        assert_eq!(output.lines[0].text, "= result");
        assert!(output.batch_complete);
    }

    #[test]
    fn session_parts_build_the_same_untruncated_expanded_record() {
        let content = (1..=31)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let record = ToolRecord::new(
            "session-call",
            "write",
            serde_json::json!({"path":"src/new.rs","content":content}),
            "Written src/new.rs.\n\nDiff:\n+ignored",
        );

        let expanded = expanded_lines(&record);
        assert_eq!(expanded.len(), 31);
        assert_eq!(expanded.first().unwrap().text, "line 1");
        assert_eq!(expanded.last().unwrap().text, "line 31");
        assert!(expanded.iter().all(|line| !line.is_diff));
        assert_eq!(record.title(), "Wrote src/new.rs");
    }
}
