use std::collections::{HashMap, HashSet};

use artist_core::{MessageId, SessionRecord, TranscriptEntryKind};

use crate::{ModelHistoryItem, ModelMessage};

pub fn project(record: &SessionRecord) -> (String, Vec<ModelHistoryItem>) {
    let context = record
        .initial_context
        .fragments
        .iter()
        .map(|fragment| fragment.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut inputs = HashMap::<MessageId, String>::new();
    let mut steering = HashMap::<MessageId, String>::new();
    for entry in &record.entries {
        match &entry.kind {
            TranscriptEntryKind::Input {
                message_id,
                content,
                ..
            } => {
                inputs.insert(message_id.clone(), content.clone());
            }
            TranscriptEntryKind::SteeringQueued {
                message_id,
                content,
                ..
            } => {
                steering.insert(message_id.clone(), content.clone());
            }
            _ => {}
        }
    }
    let latest_compaction = record
        .entries
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            TranscriptEntryKind::Compaction {
                through_sequence,
                artifact,
            } => Some((*through_sequence, artifact.clone())),
            _ => None,
        });
    let completed_calls: HashSet<_> = record
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            TranscriptEntryKind::ToolResult { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    let mut history = Vec::new();
    if let Some((sequence, artifact)) = &latest_compaction {
        history.push(ModelHistoryItem {
            sequence: *sequence,
            message: ModelMessage::Notification(format!("Earlier context: {artifact}")),
        });
    }

    for entry in &record.entries {
        if latest_compaction
            .as_ref()
            .is_some_and(|(through, _)| entry.sequence <= *through)
        {
            continue;
        }
        match &entry.kind {
            TranscriptEntryKind::Input { .. } | TranscriptEntryKind::SteeringQueued { .. } => {}
            TranscriptEntryKind::SteeringDelivered { message_ids, .. } => {
                history.extend(
                    message_ids
                        .iter()
                        .filter_map(|id| steering.get(id).cloned())
                        .map(|content| ModelHistoryItem {
                            sequence: entry.sequence,
                            message: ModelMessage::Notification(content),
                        }),
                );
            }
            TranscriptEntryKind::AssistantMessage { content, .. } => {
                history.push(ModelHistoryItem {
                    sequence: entry.sequence,
                    message: ModelMessage::Assistant(content.clone()),
                });
            }
            TranscriptEntryKind::Interrupted { cause, .. } => {
                history.push(ModelHistoryItem {
                    sequence: entry.sequence,
                    message: ModelMessage::Notification(format!(
                        "The preceding assistant response was interrupted: {cause:?}."
                    )),
                });
            }
            TranscriptEntryKind::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } if completed_calls.contains(call_id) => {
                history.push(ModelHistoryItem {
                    sequence: entry.sequence,
                    message: ModelMessage::ToolCall {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    },
                });
            }
            TranscriptEntryKind::ToolResult {
                call_id, result, ..
            } => {
                history.push(ModelHistoryItem {
                    sequence: entry.sequence,
                    message: ModelMessage::ToolResult {
                        call_id: call_id.clone(),
                        result: result.clone(),
                    },
                });
            }
            TranscriptEntryKind::Compaction { .. } => {}
            TranscriptEntryKind::RunStarted { input_id, .. } => {
                if let Some(content) = inputs.get(input_id) {
                    history.push(ModelHistoryItem {
                        sequence: entry.sequence,
                        message: ModelMessage::User(content.clone()),
                    });
                }
            }
            TranscriptEntryKind::ToolCall { .. } | TranscriptEntryKind::RunFailed { .. } => {}
        }
    }
    (context, history)
}

#[cfg(test)]
mod tests {
    use artist_core::{
        CallId, ContextFragment, InitialContext, MessageId, RunId, SessionId, SessionRecord, Source,
    };

    use super::*;

    #[test]
    fn joins_context_in_declared_order() {
        let record = SessionRecord::new(
            SessionId::from("s"),
            InitialContext {
                fragments: vec![
                    ContextFragment {
                        source: "SYSTEM.md".into(),
                        content: "system".into(),
                    },
                    ContextFragment {
                        source: "AGENTS.md".into(),
                        content: "agents".into(),
                    },
                ],
            },
        );
        assert_eq!(project(&record).0, "system\n\nagents");
    }

    #[test]
    fn compaction_changes_the_projection_without_rewriting_history() {
        let mut record =
            SessionRecord::new(SessionId::from("s"), InitialContext { fragments: vec![] });
        let first = MessageId::from("first");
        let second = MessageId::from("second");
        let run = RunId::from("run-1");
        for kind in [
            TranscriptEntryKind::Input {
                message_id: first.clone(),
                source: Source::User,
                content: "old question".into(),
            },
            TranscriptEntryKind::RunStarted {
                run_id: run.clone(),
                input_id: first,
            },
            TranscriptEntryKind::AssistantMessage {
                message_id: MessageId::from("answer"),
                run_id: run,
                content: "old answer".into(),
                complete: true,
            },
            TranscriptEntryKind::Compaction {
                through_sequence: 2,
                artifact: "old exchange summarized".into(),
            },
            TranscriptEntryKind::Input {
                message_id: second.clone(),
                source: Source::User,
                content: "new question".into(),
            },
            TranscriptEntryKind::RunStarted {
                run_id: RunId::from("run-2"),
                input_id: second,
            },
        ] {
            let entry = record.entry(kind);
            record.append(entry).unwrap();
        }
        let canonical = serde_json::to_vec(&record).unwrap();

        assert_eq!(
            project(&record).1,
            vec![
                ModelHistoryItem {
                    sequence: 2,
                    message: ModelMessage::Notification(
                        "Earlier context: old exchange summarized".into()
                    ),
                },
                ModelHistoryItem {
                    sequence: 5,
                    message: ModelMessage::User("new question".into()),
                },
            ]
        );
        assert_eq!(serde_json::to_vec(&record).unwrap(), canonical);
    }

    #[test]
    fn orphan_tool_calls_are_excluded_from_model_context() {
        let mut record =
            SessionRecord::new(SessionId::from("s"), InitialContext { fragments: vec![] });
        let input = MessageId::from("input");
        let run = RunId::from("run");
        for kind in [
            TranscriptEntryKind::Input {
                message_id: input.clone(),
                source: Source::User,
                content: "question".into(),
            },
            TranscriptEntryKind::RunStarted {
                run_id: run.clone(),
                input_id: input,
            },
            TranscriptEntryKind::ToolCall {
                run_id: run.clone(),
                call_id: CallId::from("orphan"),
                name: "read".into(),
                arguments: "{}".into(),
            },
            TranscriptEntryKind::ToolCall {
                run_id: run.clone(),
                call_id: CallId::from("paired"),
                name: "read".into(),
                arguments: "{}".into(),
            },
            TranscriptEntryKind::ToolResult {
                run_id: run.clone(),
                call_id: CallId::from("paired"),
                result: "ok".into(),
            },
            TranscriptEntryKind::AssistantMessage {
                message_id: MessageId::from("answer"),
                run_id: run,
                content: "done".into(),
                complete: true,
            },
        ] {
            let entry = record.entry(kind);
            record.append(entry).unwrap();
        }

        let messages: Vec<_> = project(&record)
            .1
            .into_iter()
            .map(|item| item.message)
            .collect();
        assert!(!messages.iter().any(|message| matches!(
            message,
            ModelMessage::ToolCall { call_id, .. } if call_id.as_str() == "orphan"
        )));
        assert!(messages.iter().any(|message| matches!(
            message,
            ModelMessage::ToolCall { call_id, .. } if call_id.as_str() == "paired"
        )));
    }

    #[test]
    fn resource_activity_replays_only_as_tool_call_and_result_entries() {
        let mut record =
            SessionRecord::new(SessionId::from("s"), InitialContext { fragments: vec![] });
        let run = RunId::from("run");
        for kind in [
            TranscriptEntryKind::Input {
                message_id: MessageId::from("input"),
                source: Source::User,
                content: "inspect it".into(),
            },
            TranscriptEntryKind::RunStarted {
                run_id: run.clone(),
                input_id: MessageId::from("input"),
            },
            TranscriptEntryKind::ToolCall {
                run_id: run.clone(),
                call_id: CallId::from("resource-call"),
                name: "read".into(),
                arguments: r#"{"uri":"file:///workspace/src/lib.rs?symbols/foo"}"#.into(),
            },
            TranscriptEntryKind::ToolResult {
                run_id: run.clone(),
                call_id: CallId::from("resource-call"),
                result: r#"{"text":"symbol foo\n"}"#.into(),
            },
            TranscriptEntryKind::AssistantMessage {
                message_id: MessageId::from("answer"),
                run_id: run,
                content: "found it".into(),
                complete: true,
            },
        ] {
            record.append(record.entry(kind)).unwrap();
        }

        let encoded = serde_json::to_vec(&record).unwrap();
        let replayed: SessionRecord = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(replayed, record);
        assert_eq!(
            project(&replayed).1[1..3]
                .iter()
                .map(|item| item.message.clone())
                .collect::<Vec<_>>(),
            [
                ModelMessage::ToolCall {
                    call_id: CallId::from("resource-call"),
                    name: "read".into(),
                    arguments: r#"{"uri":"file:///workspace/src/lib.rs?symbols/foo"}"#.into(),
                },
                ModelMessage::ToolResult {
                    call_id: CallId::from("resource-call"),
                    result: r#"{"text":"symbol foo\n"}"#.into(),
                },
            ]
        );
    }
}
