use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::{
    domain::{Event, EventData, RunId},
    store::{SqliteStore, StoreError},
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextRole {
    System,
    User,
    Assistant,
    Tool,
    Harness,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ContextEntry {
    pub role: ContextRole,
    pub content: Value,
}

#[derive(Debug, Error)]
pub enum ContextError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("fork ancestry is too deep or cyclic")]
    ForkCycle,
}

pub struct ContextBuilder<'a> {
    store: &'a SqliteStore,
}

impl<'a> ContextBuilder<'a> {
    pub fn new(store: &'a SqliteStore) -> Self {
        Self { store }
    }

    pub fn build(&self, run_id: RunId) -> Result<Vec<ContextEntry>, ContextError> {
        self.build_inner(run_id, None, 0)
    }

    fn build_inner(
        &self,
        run_id: RunId,
        through: Option<u64>,
        depth: usize,
    ) -> Result<Vec<ContextEntry>, ContextError> {
        if depth > 64 {
            return Err(ContextError::ForkCycle);
        }
        let events = self.store.events_through(run_id, through)?;
        let Some(first) = events.first() else {
            return Ok(Vec::new());
        };
        let EventData::RunCreated {
            input,
            initial_context,
            fork_point,
            ..
        } = &first.data
        else {
            return Ok(Vec::new());
        };

        let mut context = if let Some(point) = fork_point {
            let mut inherited =
                self.build_inner(point.run_id, Some(point.through_sequence), depth + 1)?;
            inherited.push(ContextEntry {
                role: ContextRole::Harness,
                content: json!({
                    "event": "fork_started",
                    "input": input,
                    "note": "This fork has a fresh Python REPL. Files remain shared."
                }),
            });
            inherited
        } else {
            vec![
                ContextEntry {
                    role: ContextRole::System,
                    content: Value::String(initial_context.system.clone()),
                },
                ContextEntry {
                    role: ContextRole::System,
                    content: Value::String(initial_context.agents.clone()),
                },
                ContextEntry {
                    role: ContextRole::System,
                    content: Value::String(initial_context.node.clone()),
                },
                ContextEntry {
                    role: ContextRole::User,
                    content: json!({"input": input}),
                },
            ]
        };

        for event in &events[1..] {
            if let Some(entry) = event_entry(event) {
                context.push(entry);
            }
        }
        Ok(context)
    }
}

fn event_entry(event: &Event) -> Option<ContextEntry> {
    let (role, content) = match &event.data {
        EventData::ContextAppended { text } => (
            ContextRole::Harness,
            json!({"event": "context_diff", "text": text}),
        ),
        EventData::ModelResponse { content, .. } => (ContextRole::Assistant, content.clone()),
        EventData::PythonRequested { request_id, code } => (
            ContextRole::Assistant,
            json!({"tool": "python", "request_id": request_id, "code": code}),
        ),
        EventData::PythonCompleted { request_id, output } => (
            ContextRole::Tool,
            json!({"tool": "python", "request_id": request_id, "output": output}),
        ),
        EventData::Asked { question } => (
            ContextRole::Assistant,
            json!({"tool": "ask", "question": question}),
        ),
        EventData::Answered { answer } => {
            (ContextRole::Tool, json!({"tool": "ask", "answer": answer}))
        }
        EventData::ChildrenJoined { child_ids, results } => (
            ContextRole::Tool,
            json!({"operation": "join", "children": child_ids, "results": results}),
        ),
        EventData::ReplRestarted { reason } => (
            ContextRole::Harness,
            json!({
                "event": "python_restarted",
                "reason": reason,
                "note": "Python memory was lost. Durable files and recorded history remain."
            }),
        ),
        EventData::Interrupted { action, reason } => (
            ContextRole::Harness,
            json!({"event": "interrupted", "action": action, "reason": reason}),
        ),
        EventData::ChildrenWaiting { .. }
        | EventData::ModelRequested { .. }
        | EventData::RunCreated { .. }
        | EventData::RunStarted
        | EventData::Yielded { .. }
        | EventData::Replaced { .. }
        | EventData::Cancelled { .. } => return None,
    };
    Some(ContextEntry { role, content })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::domain::{ChildKind, ForkPoint, InitialContext};

    #[test]
    fn fork_keeps_exact_parent_prefix() {
        let store = SqliteStore::memory().unwrap();
        let parent = Uuid::new_v4();
        let node = Uuid::new_v4();
        let initial = InitialContext {
            system: "SYSTEM".into(),
            agents: "AGENTS".into(),
            node: "NODE".into(),
        };
        store
            .append(
                parent,
                EventData::RunCreated {
                    node_id: node,
                    input: json!({"task": "x"}),
                    initial_context: initial.clone(),
                    parent_id: None,
                    child_kind: None,
                    fork_point: None,
                },
            )
            .unwrap();
        store
            .append(parent, EventData::ContextAppended { text: "hot".into() })
            .unwrap();
        let through = store.run(parent).unwrap().last_sequence;
        let child = Uuid::new_v4();
        store
            .append(
                child,
                EventData::RunCreated {
                    node_id: node,
                    input: json!({"branch": 1}),
                    initial_context: initial,
                    parent_id: Some(parent),
                    child_kind: Some(ChildKind::Fork),
                    fork_point: Some(ForkPoint {
                        run_id: parent,
                        through_sequence: through,
                    }),
                },
            )
            .unwrap();
        let builder = ContextBuilder::new(&store);
        let parent_context = builder.build(parent).unwrap();
        let child_context = builder.build(child).unwrap();
        assert_eq!(
            &child_context[..parent_context.len()],
            parent_context.as_slice()
        );
    }
}
