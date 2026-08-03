use artist_session::{Envelope, SessionEvent};
use artist_ui_core::{FocusStack, FocusableObjectId, ObjectKind, ObjectState, WorkspaceProjection};
use serde_json::json;

fn event(seq: u64, lineage: &str, kind: &str, payload: serde_json::Value) -> Envelope {
    Envelope {
        v: 1,
        seq,
        ts: seq,
        session: "root-1".into(),
        run: None,
        lineage: lineage.into(),
        kind: kind.into(),
        payload,
    }
}

#[test]
fn old_events_decode_and_new_run_profile_is_optional() {
    let old = event(
        1,
        "main",
        "run.started",
        json!({"provider":"openai","model":"gpt"}),
    );
    let SessionEvent::RunStarted(started) = old.event() else {
        panic!("run did not decode")
    };
    assert_eq!(started.profile, None);
}

#[test]
fn ids_are_stable_and_lineage_builds_agent_ancestry() {
    let events = [
        event(
            1,
            "main",
            "run.started",
            json!({"provider":"x","model":"m","agent":"Ada"}),
        ),
        event(
            2,
            "main/delegate-child",
            "delegate.started",
            json!({"prompt":"inspect parser","read_only":true,"fork":false,"background":true}),
        ),
    ];
    let projection = WorkspaceProjection::from_envelopes(&events);
    let root = FocusableObjectId::new("root-1", "main", ObjectKind::Agent, "main");
    let child = FocusableObjectId::new(
        "root-1",
        "main/delegate-child",
        ObjectKind::Agent,
        "main/delegate-child",
    );
    assert_eq!(projection.get(&child).unwrap().parent.as_ref(), Some(&root));
    assert_eq!(projection.get(&root).unwrap().title, "Ada");
}

#[test]
fn active_and_five_recent_inactive_children_are_visible() {
    let mut events = vec![event(
        1,
        "main",
        "run.started",
        json!({"provider":"x","model":"m"}),
    )];
    for index in 0..7 {
        let task = format!("t{index}");
        events.push(event(
            2 + index * 2,
            "main",
            "task.started",
            json!({"task":task,"command":format!("job {index}")}),
        ));
        events.push(event(
            3 + index * 2,
            "main",
            "task.finished",
            json!({"task":task,"exit_code":0}),
        ));
    }
    events.push(event(
        20,
        "main",
        "task.started",
        json!({"task":"live","command":"watch","persistent":true}),
    ));
    let tree = WorkspaceProjection::from_envelopes(&events).visible_tree();
    assert_eq!(tree[0].children.len(), 6);
    assert_eq!(tree[0].hidden_children, 2);
    assert!(
        tree[0]
            .children
            .iter()
            .any(|node| node.object.durable_id == "live")
    );
}

#[test]
fn search_reveals_earlier_match_and_preserves_ancestry() {
    let events = [
        event(
            1,
            "main",
            "run.started",
            json!({"provider":"x","model":"m"}),
        ),
        event(
            2,
            "main/delegate-child",
            "delegate.started",
            json!({"prompt":"find the narwhal","read_only":true,"fork":false,"background":true}),
        ),
    ];
    let tree = WorkspaceProjection::from_envelopes(&events).search("narwhal");
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].children.len(), 1);
    assert_eq!(tree[0].children[0].object.lineage, "main/delegate-child");
}

#[test]
fn task_updates_coalesce_and_changes_associate_to_tool_call() {
    let events = [
        event(
            1,
            "main",
            "task.started",
            json!({"task":"bash-1","command":"cargo test","persistent":true}),
        ),
        event(
            2,
            "main",
            "task.updated",
            json!({"task":"bash-1","output":"one\n"}),
        ),
        event(
            3,
            "main",
            "task.updated",
            json!({"task":"bash-1","output":"two\n"}),
        ),
        event(
            4,
            "main",
            "task.finished",
            json!({"task":"bash-1","exit_code":1}),
        ),
        event(
            5,
            "main",
            "change.recorded",
            json!({"tool_call_id":"edit-9","path":"src/lib.rs","before_digest":"a","after_digest":"b","diff":"@@"}),
        ),
    ];
    let projection = WorkspaceProjection::from_envelopes(&events);
    let task = FocusableObjectId::new("root-1", "main", ObjectKind::Task, "bash-1");
    assert_eq!(
        projection.get(&task).unwrap().detail.as_deref(),
        Some("one\ntwo\n")
    );
    assert_eq!(projection.get(&task).unwrap().state, ObjectState::Failed);
    let changes = projection.changes_for_tool("edit-9");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].title, "src/lib.rs");
}

#[test]
fn focus_stack_preserves_return_context_and_jumps_to_ancestor() {
    let agent = FocusableObjectId::new("root-1", "main", ObjectKind::Agent, "main");
    let task = FocusableObjectId::new("root-1", "main", ObjectKind::Task, "bash-1");
    let mut stack = FocusStack::default();
    stack.push_context(agent.clone(), 12, Some("composer".into()));
    stack.push_context(task.clone(), 3, Some("task-input".into()));
    assert_eq!(stack.current(), Some(&task));
    assert!(stack.jump_to(&agent));
    assert_eq!(stack.current(), Some(&agent));
    let restored = stack.pop().unwrap();
    assert_eq!(restored.scroll_anchor, 12);
    assert_eq!(restored.keyboard_focus.as_deref(), Some("composer"));
}
