use std::collections::{HashMap, HashSet};

use artist_core::{ContextRole, MessageId, RunOutcome, SessionRecord, TranscriptEntryKind};

use crate::{ModelHistoryItem, ModelMessage};

pub fn project(record: &SessionRecord) -> (String, Vec<ModelHistoryItem>) {
    let activation = record
        .entries()
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            TranscriptEntryKind::ProfileActivated { profile, .. } => {
                Some((entry.sequence, profile))
            }
            _ => None,
        });
    let boundary = activation.map(|(sequence, _)| sequence);
    let context = compose_context(
        record,
        activation.map(|(_, profile)| profile.instructions.as_str()),
    );
    let mut inputs = HashMap::<MessageId, Vec<artist_core::ContentPart>>::new();
    let mut steering = HashMap::<MessageId, Vec<artist_core::ContentPart>>::new();
    for entry in record
        .entries()
        .iter()
        .filter(|entry| boundary.is_none_or(|boundary| entry.sequence > boundary))
    {
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
        .entries()
        .iter()
        .filter(|entry| boundary.is_none_or(|boundary| entry.sequence > boundary))
        .rev()
        .find_map(|entry| match &entry.kind {
            TranscriptEntryKind::Compaction {
                through_sequence,
                artifact,
            } => Some((*through_sequence, artifact.clone())),
            _ => None,
        });
    let completed_calls: HashSet<_> = record
        .entries()
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
            message: ModelMessage::Notification(artifact.content.clone()),
        });
    }

    for entry in record.entries() {
        if boundary.is_some_and(|boundary| entry.sequence <= boundary) {
            continue;
        }
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
            TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Interrupted { cause, .. },
                ..
            } => {
                history.push(ModelHistoryItem {
                    sequence: entry.sequence,
                    message: ModelMessage::Notification(vec![artist_core::ContentPart::text(
                        format!("The preceding assistant response was interrupted: {cause:?}."),
                    )]),
                });
            }
            TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Failed { error, .. },
                ..
            } => {
                history.push(ModelHistoryItem {
                    sequence: entry.sequence,
                    message: ModelMessage::Notification(vec![artist_core::ContentPart::text(
                        format!("The preceding assistant response failed: {error}."),
                    )]),
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
                call_id, content, ..
            } => {
                history.push(ModelHistoryItem {
                    sequence: entry.sequence,
                    message: ModelMessage::ToolResult {
                        call_id: call_id.clone(),
                        content: content.clone(),
                    },
                });
            }
            TranscriptEntryKind::Compaction { .. } => {}
            TranscriptEntryKind::ProfileActivated { .. }
            | TranscriptEntryKind::InputsSuperseded { .. }
            | TranscriptEntryKind::SlashCommand { .. }
            | TranscriptEntryKind::PluginEventSchemaRegistered { .. }
            | TranscriptEntryKind::PluginEvent { .. } => {}
            TranscriptEntryKind::RunStarted { input_id, .. } => {
                if let Some(content) = inputs.get(input_id) {
                    history.push(ModelHistoryItem {
                        sequence: entry.sequence,
                        message: ModelMessage::User(content.clone()),
                    });
                }
            }
            TranscriptEntryKind::ToolCall { .. }
            | TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Completed { .. },
                ..
            }
            | TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Yielded { .. } | RunOutcome::HandedOff { .. },
                ..
            } => {}
        }
    }
    (context, history)
}

fn compose_context(record: &SessionRecord, profile: Option<&str>) -> String {
    let mut fragments = Vec::new();
    let initial = &record.initial_context().fragments;
    let identity = initial
        .iter()
        .position(|fragment| fragment.role == ContextRole::Identity)
        .unwrap_or(initial.len());
    fragments.extend(
        initial[..identity]
            .iter()
            .map(|fragment| fragment.content.as_str()),
    );
    if let Some(profile) = profile.filter(|profile| !profile.trim().is_empty()) {
        fragments.push(profile);
    }
    fragments.extend(
        initial[identity..]
            .iter()
            .map(|fragment| fragment.content.as_str()),
    );
    fragments.join("\n\n")
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
                        role: ContextRole::System,
                    },
                    ContextFragment {
                        source: "AGENTS.md".into(),
                        content: "agents".into(),
                        role: ContextRole::Agents,
                    },
                ],
            },
        );
        assert_eq!(project(&record).0, "system\n\nagents");
    }

    #[test]
    fn profile_instructions_are_between_agents_and_identity() {
        let mut record = SessionRecord::new(
            SessionId::from("s"),
            InitialContext {
                fragments: vec![
                    ContextFragment {
                        source: "system".into(),
                        content: "system".into(),
                        role: ContextRole::System,
                    },
                    ContextFragment {
                        source: "agents".into(),
                        content: "agents".into(),
                        role: ContextRole::Agents,
                    },
                    ContextFragment {
                        source: "identity".into(),
                        content: "identity".into(),
                        role: ContextRole::Identity,
                    },
                ],
            },
        );
        let profile = artist_core::ProfileSnapshot {
            name: "planner".into(),
            instructions: "profile".into(),
            yield_schema: artist_core::default_yield_schema(),
            policy: artist_core::ProfilePolicy::default(),
            models: Vec::new(),
            catalog: vec!["planner".into()],
        };
        let activation = record.entry(TranscriptEntryKind::ProfileActivated {
            profile,
            brief: None,
            steering_message_ids: Vec::new(),
        });
        record.append(activation).unwrap();
        assert_eq!(
            project(&record).0,
            "system\n\nagents\n\nprofile\n\nidentity"
        );
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
                content: vec![artist_core::ContentPart::text("old question")],
            },
            TranscriptEntryKind::RunStarted {
                run_id: run.clone(),
                input_id: first,
            },
            TranscriptEntryKind::AssistantMessage {
                message_id: MessageId::from("answer"),
                run_id: run.clone(),
                content: vec![artist_core::ContentPart::text("old answer")],
            },
            TranscriptEntryKind::RunFinished {
                run_id: run,
                outcome: RunOutcome::Completed {
                    message_id: MessageId::from("answer"),
                },
            },
            TranscriptEntryKind::Compaction {
                through_sequence: 3,
                artifact: artist_core::ProjectionArtifact::text("old exchange summarized", 3),
            },
            TranscriptEntryKind::Input {
                message_id: second.clone(),
                source: Source::User,
                content: vec![artist_core::ContentPart::text("new question")],
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
                    sequence: 3,
                    message: ModelMessage::Notification(vec![artist_core::ContentPart::text(
                        "old exchange summarized",
                    )]),
                },
                ModelHistoryItem {
                    sequence: 6,
                    message: ModelMessage::User(vec![artist_core::ContentPart::text(
                        "new question",
                    )]),
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
                content: vec![artist_core::ContentPart::text("question")],
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
                content: vec![artist_core::ContentPart::text("ok")],
            },
            TranscriptEntryKind::AssistantMessage {
                message_id: MessageId::from("answer"),
                run_id: run,
                content: vec![artist_core::ContentPart::text("done")],
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
                content: vec![artist_core::ContentPart::text("inspect it")],
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
                content: vec![artist_core::ContentPart::Json {
                    value: serde_json::json!({"text": "symbol foo\n"}),
                }],
            },
            TranscriptEntryKind::AssistantMessage {
                message_id: MessageId::from("answer"),
                run_id: run.clone(),
                content: vec![artist_core::ContentPart::text("found it")],
            },
            TranscriptEntryKind::RunFinished {
                run_id: run,
                outcome: RunOutcome::Completed {
                    message_id: MessageId::from("answer"),
                },
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
                    content: vec![artist_core::ContentPart::Json {
                        value: serde_json::json!({"text": "symbol foo\n"}),
                    }],
                },
            ]
        );
    }

    #[test]
    fn failed_runs_replay_the_partial_message_and_failure_reason() {
        let mut record =
            SessionRecord::new(SessionId::from("s"), InitialContext { fragments: vec![] });
        let run = RunId::from("run");
        for kind in [
            TranscriptEntryKind::Input {
                message_id: MessageId::from("input"),
                source: Source::User,
                content: vec![artist_core::ContentPart::text("question")],
            },
            TranscriptEntryKind::RunStarted {
                run_id: run.clone(),
                input_id: MessageId::from("input"),
            },
            TranscriptEntryKind::AssistantMessage {
                message_id: MessageId::from("answer"),
                run_id: run.clone(),
                content: vec![artist_core::ContentPart::text("partial")],
            },
            TranscriptEntryKind::RunFinished {
                run_id: run,
                outcome: RunOutcome::Failed {
                    message_id: Some(MessageId::from("answer")),
                    error: "provider unavailable".into(),
                },
            },
        ] {
            record.append(record.entry(kind)).unwrap();
        }

        assert_eq!(
            project(&record)
                .1
                .into_iter()
                .map(|item| item.message)
                .collect::<Vec<_>>(),
            [
                ModelMessage::User(vec![artist_core::ContentPart::text("question")]),
                ModelMessage::Assistant(vec![artist_core::ContentPart::text("partial")]),
                ModelMessage::Notification(vec![artist_core::ContentPart::text(
                    "The preceding assistant response failed: provider unavailable.",
                )]),
            ]
        );
    }
}
