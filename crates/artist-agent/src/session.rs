//! Replay and projection contracts for a durable session.
//!
//! The event log is authoritative. This module only turns the durable,
//! provider-neutral events back into a provider-neutral conversation; it
//! never applies a local compaction or context-retention policy.

use std::collections::BTreeMap;

use artist_session::Snapshot;
use llm_provider::{ContentPart, Message, ModelResponse, Role, ToolResult};

use crate::AgentEvent;

pub trait ContextProjector: Send + Sync {
    fn project(&self, events: &[AgentEvent]) -> Vec<Message>;
}

/// Projects provider-neutral composition state into ordinary model messages.
/// The durable contribution identity remains in the context log; provider
/// adapters receive only the active ordered text prefix here.
pub trait SnapshotProjector: Send + Sync {
    fn project_snapshot(&self, snapshot: &Snapshot) -> Vec<Message>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SlotSnapshotProjector;

impl SnapshotProjector for SlotSnapshotProjector {
    fn project_snapshot(&self, snapshot: &Snapshot) -> Vec<Message> {
        snapshot
            .contributions
            .iter()
            .map(|contribution| Message::text(Role::System, contribution.content.clone()))
            .collect()
    }
}

pub fn project_snapshot(snapshot: &Snapshot) -> Vec<Message> {
    SlotSnapshotProjector.project_snapshot(snapshot)
}

/// Rebuild the durable conversational portion of a session. A completed
/// tool result is always reused. A request with no durable result is rendered
/// as an interrupted tool result so recovery cannot blindly repeat a
/// potentially mutating operation.
pub fn project_conversation(events: &[AgentEvent]) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut requested: BTreeMap<String, String> = BTreeMap::new();

    for event in events {
        match event {
            AgentEvent::ContextCompacted { context, .. } => {
                messages = context.clone();
                requested.clear();
            }
            AgentEvent::UserMessage { message } => messages.push(message.clone()),
            AgentEvent::SteeringAccepted { message } => messages.push(message.clone()),
            AgentEvent::SurfaceMessage { message } => messages.push(message.clone()),
            AgentEvent::Handoff { brief, .. } => {
                messages.clear();
                requested.clear();
                messages.push(Message::text(Role::User, brief.clone()));
            }
            AgentEvent::ForkCompleted { fork_id, payload } => messages.push(Message::text(
                Role::User,
                format!("Fork {fork_id} completed with structured result: {payload}"),
            )),
            AgentEvent::ModelEvent {
                event: llm_provider::ModelEvent::Completed { response },
            } => {
                // The assistant response is durable before the harness writes
                // each separate ToolRequested record. If the daemon dies in
                // that narrow window, recover the calls from the completed
                // response itself so the next provider request still receives
                // a deterministic interrupted result instead of an assistant
                // message with an unresolved tool call.
                for call in &response.tool_calls {
                    requested.insert(call.id.clone(), call.name.clone());
                }
                messages.push(assistant_message(response));
            }
            AgentEvent::ToolRequested { call } => {
                requested.insert(call.id.clone(), call.name.clone());
            }
            AgentEvent::ToolCompleted { result } => {
                requested.remove(&result.call_id);
                messages.push(tool_message(result.clone()));
            }
            AgentEvent::HarnessCompleted {
                result, operation, ..
            } => {
                requested.remove(&result.call_id);
                match operation {
                    Some(artist_component::HarnessOperation::Handoff { brief, .. }) => {
                        messages.clear();
                        requested.clear();
                        messages.push(Message::text(Role::User, brief.clone()));
                    }
                    _ => messages.push(tool_message(result.clone())),
                }
            }
            _ => {}
        }
    }

    for (call_id, name) in requested {
        messages.push(tool_message(ToolResult {
            call_id,
            content: vec![ContentPart::Text {
                text: format!("tool {name} was interrupted before a durable result was recorded"),
            }],
            is_error: true,
        }));
    }
    messages
}

fn assistant_message(response: &ModelResponse) -> Message {
    let mut content = response.content.clone();
    for item in &response.continuation_items {
        content.push(ContentPart::ProviderExtension {
            provider: "openai".into(),
            value: item.clone(),
        });
    }
    if let Some(refusal) = &response.refusal {
        content.push(ContentPart::ProviderExtension {
            provider: "model".into(),
            value: serde_json::json!({"kind": "refusal", "text": refusal}),
        });
    }
    if response.incomplete {
        content.push(ContentPart::ProviderExtension {
            provider: "model".into(),
            value: serde_json::json!({"kind": "incomplete"}),
        });
    }
    Message {
        role: Role::Assistant,
        content,
        name: None,
        tool_calls: response.tool_calls.clone(),
        tool_results: Vec::new(),
    }
}

fn tool_message(result: ToolResult) -> Message {
    Message {
        role: Role::Tool,
        content: result.content.clone(),
        name: None,
        tool_calls: Vec::new(),
        tool_results: vec![result],
    }
}

/// Rebuild the complete provider request prefix from the current context
/// projection and the durable conversation. This is intentionally called at
/// every provider boundary so replace/remove mutations cannot leave stale
/// system messages in a mutable vector.
pub fn project_request(snapshot: &Snapshot, events: &[AgentEvent]) -> Vec<Message> {
    // A compaction result is the complete provider-neutral request context.
    // Do not prepend the pre-compaction snapshot again: doing so would turn a
    // component-owned replacement into an implicit host preservation rule.
    let latest_compaction = events
        .iter()
        .rposition(|event| matches!(event, AgentEvent::ContextCompacted { .. }));
    let latest_handoff = events
        .iter()
        .rposition(|event| matches!(event, AgentEvent::Handoff { .. }));
    if latest_compaction.is_some_and(|index| latest_handoff.is_none_or(|handoff| index > handoff)) {
        return project_conversation(events);
    }
    let mut messages = project_snapshot(snapshot);
    messages.extend(project_conversation(events));
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_provider::{ModelEvent, ToolCall, Usage};

    fn response(tool_calls: Vec<ToolCall>) -> ModelResponse {
        ModelResponse {
            content: vec![ContentPart::Text {
                text: "answer".into(),
            }],
            tool_calls,
            finish_reason: Some("stop".into()),
            usage: Usage::default(),
            refusal: None,
            incomplete: false,
            continuation_items: Vec::new(),
        }
    }

    #[test]
    fn replay_preserves_images_and_does_not_rerun_incomplete_tools() {
        let call = ToolCall {
            id: "call-1".into(),
            name: "read".into(),
            arguments: serde_json::json!({"uri":"file:///x"}),
        };
        let events = vec![
            AgentEvent::UserMessage {
                message: Message {
                    role: Role::User,
                    content: vec![ContentPart::Image {
                        mime_type: "image/png".into(),
                        data_base64: "bytes".into(),
                    }],
                    name: None,
                    tool_calls: Vec::new(),
                    tool_results: Vec::new(),
                },
            },
            AgentEvent::ModelEvent {
                event: ModelEvent::Completed {
                    response: response(vec![call.clone()]),
                },
            },
            AgentEvent::ToolRequested { call },
        ];
        let messages = project_conversation(&events);
        assert!(matches!(messages[0].content[0], ContentPart::Image { .. }));
        assert!(messages[2].tool_results[0].is_error);
    }

    #[test]
    fn replay_recovers_calls_durable_only_in_completed_response() {
        let messages = project_conversation(&[AgentEvent::ModelEvent {
            event: ModelEvent::Completed {
                response: response(vec![ToolCall {
                    id: "call-1".into(),
                    name: "write".into(),
                    arguments: serde_json::json!({"uri":"file:///x"}),
                }]),
            },
        }]);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].tool_calls[0].id, "call-1");
        assert!(messages[1].tool_results[0].is_error);
        assert_eq!(messages[1].tool_results[0].call_id, "call-1");
    }

    #[test]
    fn compaction_replaces_the_complete_request_context() {
        let compacted = Message::text(Role::User, "compacted");
        let messages = project_request(
            &Snapshot::new([artist_session::Contribution::new(
                "system",
                "system",
                "must not be duplicated",
            )]),
            &[AgentEvent::ContextCompacted {
                context: vec![compacted.clone()],
                metadata: serde_json::Value::Null,
            }],
        );
        assert_eq!(messages, vec![compacted]);
    }
}
