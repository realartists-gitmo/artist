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
