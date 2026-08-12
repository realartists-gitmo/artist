use super::*;
use rig_core::OneOrMany;
use rig_core::completion::message::{
    AssistantContent, ToolCall, ToolFunction, ToolResult, ToolResultContent, UserContent,
};
use serde_json::json;

fn tool_call(name: &str, path: &str) -> Message {
    Message::Assistant {
        id: None,
        content: OneOrMany::one(AssistantContent::ToolCall(ToolCall {
            id: format!("{name}-id"),
            call_id: None,
            function: ToolFunction {
                name: name.into(),
                arguments: json!({"path": path}),
            },
            signature: None,
            additional_params: None,
        })),
    }
}

fn tool_result(id: &str, text: &str) -> Message {
    Message::User {
        content: OneOrMany::one(UserContent::ToolResult(ToolResult {
            id: id.into(),
            call_id: None,
            content: OneOrMany::one(ToolResultContent::text(text)),
        })),
    }
}

fn wire_canonical(messages: &[Message]) -> Vec<Message> {
    serde_json::from_value(serde_json::to_value(messages).unwrap()).unwrap()
}

#[test]
fn compacts_at_turn_boundary_and_keeps_recent_suffix() {
    let messages = vec![
        Message::user("old request"),
        Message::assistant("old answer"),
        Message::user("recent request"),
        Message::assistant("recent answer"),
    ];
    let keep = estimate_messages_tokens(&messages[2..]);
    let plan = prepare_compaction(&messages, keep).unwrap();

    assert!(!plan.is_split_turn);
    assert_eq!(plan.messages_to_summarize, messages[..2]);
    assert_eq!(plan.kept_messages, messages[2..]);
}

#[test]
fn split_turn_never_cuts_at_tool_result() {
    let messages = vec![
        Message::user("one large request"),
        tool_call("read", "src/lib.rs"),
        tool_result("read-id", &"x".repeat(200)),
        Message::assistant("newest answer"),
    ];
    let keep = estimate_message_tokens(&messages[3]);
    let plan = prepare_compaction(&messages, keep).unwrap();

    assert!(plan.is_split_turn);
    assert_eq!(plan.turn_prefix_messages, messages[..3]);
    assert_eq!(plan.kept_messages, messages[3..]);
    assert_eq!(plan.read_files, ["src/lib.rs"]);
}

#[test]
fn repeated_compaction_updates_prior_summary_and_file_lists() {
    let previous = "## Goal\nOld\n\n<read-files>\na.rs\n</read-files>";
    let messages = vec![
        summary_message(previous),
        Message::user("older"),
        tool_call("edit", "a.rs"),
        tool_result("edit-id", "done"),
        Message::assistant("done"),
        Message::user("recent"),
        Message::assistant("answer"),
    ];
    let keep = estimate_messages_tokens(&messages[5..]);
    let plan = prepare_compaction(&messages, keep).unwrap();

    assert_eq!(plan.previous_summary.as_deref(), Some(previous));
    assert!(plan.read_files.is_empty());
    assert_eq!(plan.modified_files, ["a.rs"]);
    let snapshot = plan.snapshot("updated");
    assert_eq!(compaction_summary(&snapshot[0]).as_deref(), Some("updated"));
    assert_eq!(snapshot[1..], messages[5..]);
}

#[test]
fn serialization_truncates_large_tool_results() {
    let source = serialize_conversation(&[tool_result("read-id", &"x".repeat(2_100))]);
    assert!(source.contains("100 more characters truncated"));
    assert!(source.len() < 2_100);
}

#[test]
fn exact_checkpoint_round_trips_large_tool_results_without_truncation() {
    let messages = vec![
        Message::user("request"),
        tool_call("read", "src/lib.rs"),
        tool_result("read-id", &"x".repeat(12_000)),
    ];
    let checkpoint = exact_context_message(&messages);
    let recovered = exact_context_messages(&checkpoint).expect("valid exact checkpoint");
    assert_eq!(recovered, wire_canonical(&messages));
    let Message::User { content } = checkpoint else {
        panic!("checkpoint is a user message")
    };
    let rendered = content
        .iter()
        .find_map(|item| match item {
            UserContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(!rendered.contains("truncated"));
}

#[test]
fn repeated_exact_compaction_rehydrates_the_prior_checkpoint() {
    let earlier = vec![Message::user("first"), Message::assistant("first answer")];
    let checkpoint = exact_context_message(&earlier);
    let messages = vec![
        checkpoint,
        Message::user("second"),
        Message::assistant("second answer"),
        Message::user("recent"),
        Message::assistant("recent answer"),
    ];
    let keep = estimate_messages_tokens(&messages[3..]);
    let plan = prepare_compaction(&messages, keep).unwrap();
    let prefix = plan.exact_prefix(&messages);
    let mut expected = wire_canonical(&earlier);
    expected.extend_from_slice(&messages[1..3]);
    assert_eq!(prefix, expected);
    let snapshot = plan.exact_snapshot(&prefix);
    assert_eq!(
        exact_context_messages(&snapshot[0]).unwrap(),
        wire_canonical(&prefix)
    );
    assert_eq!(snapshot[1..], messages[3..]);
}
