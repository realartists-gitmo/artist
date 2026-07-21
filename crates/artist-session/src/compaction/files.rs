use std::collections::BTreeSet;

use rig_core::completion::Message;
use rig_core::completion::message::AssistantContent;

#[derive(Default)]
pub(super) struct FileOperations {
    read: BTreeSet<String>,
    modified: BTreeSet<String>,
}

impl FileOperations {
    pub(super) fn observe(&mut self, message: &Message) {
        let Message::Assistant { content, .. } = message else {
            return;
        };
        for item in content.iter() {
            let AssistantContent::ToolCall(call) = item else {
                continue;
            };
            let Some(path) = call
                .function
                .arguments
                .get("path")
                .and_then(|path| path.as_str())
            else {
                continue;
            };
            match call.function.name.as_str() {
                "read" => {
                    self.read.insert(path.to_owned());
                }
                "write" | "edit" => {
                    self.modified.insert(path.to_owned());
                }
                _ => {}
            }
        }
    }

    pub(super) fn extend_summary(&mut self, summary: &str) {
        self.read.extend(tag_lines(summary, "read-files"));
        self.modified.extend(tag_lines(summary, "modified-files"));
    }

    pub(super) fn finish(mut self) -> (Vec<String>, Vec<String>) {
        for path in &self.modified {
            self.read.remove(path);
        }
        (
            self.read.into_iter().collect(),
            self.modified.into_iter().collect(),
        )
    }
}

fn tag_lines(summary: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    summary
        .split_once(&open)
        .and_then(|(_, rest)| rest.split_once(&close).map(|(body, _)| body))
        .map(|body| {
            body.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
