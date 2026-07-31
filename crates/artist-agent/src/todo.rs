//! Harness-owned todo lists.
//!
//! The list lives outside the model context and is recorded as session events,
//! so it survives a handoff verbatim rather than being summarized into one.
//! That is the whole reason it is harness-owned: everything else crossing a
//! handoff boundary is lossy prose.
//!
//! One list per session. A subagent gets its own and can read its parent's,
//! but cannot write to it — a fan-out of concurrent children writing one list
//! is a data race with no obvious merge.

use artist_session::{Recorder, TodoItem, TodoStatus, TodoUpdated};
use dashmap::DashMap;
use rig_core::tool::PortableTool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// Every list in a session, keyed by owner.
#[derive(Clone, Default)]
pub struct TodoStore {
    lists: Arc<DashMap<String, Vec<TodoItem>>>,
}

impl TodoStore {
    pub fn get(&self, owner: &str) -> Vec<TodoItem> {
        self.lists
            .get(owner)
            .map(|list| list.clone())
            .unwrap_or_default()
    }

    pub fn set(&self, owner: &str, items: Vec<TodoItem>) {
        self.lists.insert(owner.to_owned(), items);
    }

    /// Restore from the session log on resume. Snapshots are last-writer-wins,
    /// so replaying in order leaves the newest list per owner.
    pub fn restore(&self, events: &[artist_session::Envelope]) {
        for envelope in artist_session::visible_events(events) {
            if let artist_session::SessionEvent::TodoUpdated(update) = envelope.event() {
                self.lists.insert(update.owner, update.items);
            }
        }
    }
}

/// Render a list for injection into an agent's context.
pub fn render(items: &[TodoItem]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let lines: String = items
        .iter()
        .map(|item| {
            let mark = match item.status {
                TodoStatus::Pending => " ",
                TodoStatus::InProgress => "~",
                TodoStatus::Done => "x",
            };
            format!("[{mark}] {}\n", item.text)
        })
        .collect();
    format!("<todos>\n{lines}</todos>")
}

#[derive(Clone)]
pub(crate) struct TodoTool {
    store: TodoStore,
    recorder: Recorder,
    owner: String,
    /// Present for a subagent: the list it may read but not write.
    parent: Option<String>,
}

impl TodoTool {
    pub fn new(
        store: TodoStore,
        recorder: Recorder,
        owner: String,
        parent: Option<String>,
    ) -> Self {
        Self {
            store,
            recorder,
            owner,
            parent,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TodoArgs {
    mode: Option<String>,
    #[serde(default)]
    items: Vec<TodoItem>,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct TodoError(String);

impl PortableTool for TodoTool {
    const NAME: &'static str = "todo";
    type Error = TodoError;
    type Args = TodoArgs;
    type Output = String;

    fn description(&self) -> String {
        let parent = if self.parent.is_some() {
            " Use mode=parent to read the list of the agent that delegated to you; you cannot write to it."
        } else {
            ""
        };
        format!(
            "Track multi-step work. mode=write replaces the whole list, so send every item each \
             time, not just the changed one. The list is kept outside your context and survives \
             compaction and handoff.{parent}"
        )
    }

    fn parameters(&self) -> Value {
        let mut modes = vec!["write", "read"];
        if self.parent.is_some() {
            modes.push("parent");
        }
        json!({"type":"object","properties":{
            "mode":{"enum":modes,"default":"write"},
            "items":{"type":"array","description":"The complete list, in order.","items":{
                "type":"object",
                "properties":{
                    "text":{"type":"string"},
                    "status":{"enum":["pending","in_progress","done"],"default":"pending"}
                },
                "required":["text"],
                "additionalProperties":false
            }}
        },"additionalProperties":false})
    }

    async fn call(&self, args: TodoArgs) -> Result<String, TodoError> {
        match args.mode.as_deref().unwrap_or("write") {
            "read" => Ok(summarize(&self.store.get(&self.owner))),
            "parent" => {
                let parent = self
                    .parent
                    .as_deref()
                    .ok_or_else(|| TodoError("this agent has no parent list".into()))?;
                Ok(summarize(&self.store.get(parent)))
            }
            "write" => {
                if args.items.is_empty() {
                    return Err(TodoError(
                        "items is required for mode=write; send the complete list".into(),
                    ));
                }
                self.store.set(&self.owner, args.items.clone());
                self.recorder.record(TodoUpdated {
                    owner: self.owner.clone(),
                    items: args.items.clone(),
                });
                Ok(summarize(&args.items))
            }
            other => Err(TodoError(format!("unknown todo mode: {other}"))),
        }
    }
}

fn summarize(items: &[TodoItem]) -> String {
    if items.is_empty() {
        return "The list is empty.".into();
    }
    let done = items
        .iter()
        .filter(|item| item.status == TodoStatus::Done)
        .count();
    format!("{}\n{done}/{} done.", render(items), items.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str, status: TodoStatus) -> TodoItem {
        TodoItem {
            text: text.into(),
            status,
        }
    }

    fn tool(owner: &str, parent: Option<&str>) -> (TodoStore, TodoTool) {
        let store = TodoStore::default();
        let tool = TodoTool::new(
            store.clone(),
            Recorder::noop(),
            owner.to_owned(),
            parent.map(str::to_owned),
        );
        (store, tool)
    }

    fn args(mode: &str, items: Vec<TodoItem>) -> TodoArgs {
        TodoArgs {
            mode: Some(mode.into()),
            items,
        }
    }

    #[tokio::test]
    async fn a_write_replaces_the_whole_list() {
        let (store, tool) = tool("session", None);
        tool.call(args(
            "write",
            vec![
                item("read the code", TodoStatus::Done),
                item("write the fix", TodoStatus::InProgress),
            ],
        ))
        .await
        .unwrap();
        tool.call(args("write", vec![item("ship it", TodoStatus::Pending)]))
            .await
            .unwrap();

        let list = store.get("session");
        assert_eq!(list.len(), 1, "a write is a snapshot, not an append");
        assert_eq!(list[0].text, "ship it");
    }

    #[tokio::test]
    async fn an_empty_write_is_rejected_rather_than_silently_clearing() {
        let (store, tool) = tool("session", None);
        tool.call(args("write", vec![item("keep me", TodoStatus::Pending)]))
            .await
            .unwrap();
        assert!(tool.call(args("write", Vec::new())).await.is_err());
        assert_eq!(store.get("session").len(), 1);
    }

    /// A subagent reads its parent's list but writes only its own, so a
    /// concurrent fan-out cannot race on one list.
    #[tokio::test]
    async fn a_subagent_reads_the_parent_list_but_writes_its_own() {
        let store = TodoStore::default();
        store.set("session", vec![item("parent work", TodoStatus::InProgress)]);
        let child = TodoTool::new(
            store.clone(),
            Recorder::noop(),
            "a-child".into(),
            Some("session".into()),
        );

        let seen = child.call(args("parent", Vec::new())).await.unwrap();
        assert!(seen.contains("parent work"));

        child
            .call(args("write", vec![item("child work", TodoStatus::Pending)]))
            .await
            .unwrap();
        assert_eq!(store.get("session").len(), 1, "parent list untouched");
        assert_eq!(store.get("session")[0].text, "parent work");
        assert_eq!(store.get("a-child")[0].text, "child work");
    }

    #[tokio::test]
    async fn a_root_agent_has_no_parent_mode() {
        let (_, tool) = tool("session", None);
        let modes = tool.parameters()["properties"]["mode"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert!(!modes.contains(&"parent".to_owned()));
        assert!(tool.call(args("parent", Vec::new())).await.is_err());
    }

    #[test]
    fn rendering_marks_each_state() {
        let rendered = render(&[
            item("done thing", TodoStatus::Done),
            item("current thing", TodoStatus::InProgress),
            item("later thing", TodoStatus::Pending),
        ]);
        assert!(rendered.contains("[x] done thing"));
        assert!(rendered.contains("[~] current thing"));
        assert!(rendered.contains("[ ] later thing"));
    }

    #[test]
    fn an_empty_list_renders_to_nothing_rather_than_an_empty_block() {
        assert_eq!(render(&[]), "");
    }
}
