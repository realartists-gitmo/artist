use artist_session::ToolOutcomeRecord;
use futures::StreamExt;
use rig_agent::agent::{AgentBuilder, MultiTurnStreamItem};
use rig_agent::streaming::StreamingChat;
use rig_core::completion::Message;
use rig_core::test_utils::{MockCompletionModel, MockStreamEvent};

use crate::{PromptEvent, delegate::child_conversation_messages};

#[test]
fn child_conversation_excludes_forked_parent_history() {
    let seed_history = vec![
        Message::user("parent prompt"),
        Message::assistant("parent answer"),
    ];
    let run_messages = vec![
        Message::user("delegated prompt"),
        Message::assistant("delegated answer"),
    ];

    let batch =
        child_conversation_messages(&seed_history, seed_history.len(), run_messages.clone());

    assert!(batch.reset);
    assert_eq!(batch.display_from, 0);
    assert_eq!(batch.messages, run_messages);
}

#[test]
fn child_conversation_keeps_ttsr_accepted_retry_prefix_once() {
    let seed_history = vec![
        Message::user("parent prompt"),
        Message::assistant("parent answer"),
        Message::user("delegated prompt"),
        Message::assistant("accepted tool turn"),
    ];
    let run_messages = vec![
        Message::user("<system-reminder>retry</system-reminder>"),
        Message::assistant("final answer"),
    ];

    let batch = child_conversation_messages(&seed_history, 2, run_messages);

    assert_eq!(
        batch.messages,
        vec![
            Message::user("delegated prompt"),
            Message::assistant("accepted tool turn"),
            Message::user("<system-reminder>retry</system-reminder>"),
            Message::assistant("final answer"),
        ]
    );
}

#[test]
fn nested_prompt_event_preserves_child_identity_and_full_tool_result() {
    let event = PromptEvent::SubagentEvent {
        id: "a-quiet-river".into(),
        event: Box::new(PromptEvent::ToolResult {
            id: "call-1".into(),
            content: "complete tool output".into(),
            outcome: Some(ToolOutcomeRecord::Success),
            duration_ms: Some(42),
            images: vec![crate::ToolImage {
                attachment: "ab12cd".into(),
                media_type: Some("png".into()),
                bytes: 1024,
            }],
        }),
    };

    let value = serde_json::to_value(event).unwrap();

    assert_eq!(value["type"], "subagent_event");
    assert_eq!(value["data"]["id"], "a-quiet-river");
    assert_eq!(value["data"]["event"]["type"], "tool_result");
    assert_eq!(value["data"]["event"]["data"]["id"], "call-1");
    assert_eq!(
        value["data"]["event"]["data"]["content"],
        "complete tool output"
    );
    assert_eq!(
        value["data"]["event"]["data"]["outcome"]["status"],
        "success"
    );
    assert_eq!(value["data"]["event"]["data"]["duration_ms"], 42);
    assert_eq!(
        value["data"]["event"]["data"]["images"][0]["attachment"],
        "ab12cd"
    );
    assert_eq!(value["data"]["event"]["data"]["images"][0]["bytes"], 1024);
}

#[tokio::test]
async fn rig_final_response_messages_exclude_supplied_parent_history() {
    let model = MockCompletionModel::from_stream_turns([vec![
        MockStreamEvent::text("delegated answer"),
        MockStreamEvent::final_response_with_default_usage(),
    ]]);
    let agent = AgentBuilder::new(model).default_max_turns(2).build();
    let mut stream = agent
        .stream_chat(
            Message::user("delegated prompt"),
            vec![
                Message::user("parent prompt"),
                Message::assistant("parent answer"),
            ],
        )
        .await;
    let mut final_messages = None;
    while let Some(item) = stream.next().await {
        if let Ok(MultiTurnStreamItem::FinalResponse(response)) = item {
            final_messages = response.messages().map(ToOwned::to_owned);
        }
    }

    assert_eq!(
        final_messages.unwrap(),
        vec![
            Message::user("delegated prompt"),
            Message::assistant("delegated answer"),
        ]
    );
}
