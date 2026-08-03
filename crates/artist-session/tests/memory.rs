use artist_session::{
    AttachmentStore, ContentBlock, ConversationCompacted, EventLogReader, EventLogWriter,
    ReplayItem, SessionEvent, SessionMemory, TurnUser, replay_for_ui, spawn_writer,
};
use rig_core::completion::Message;
use rig_core::memory::ConversationMemory;

#[tokio::test]
async fn native_messages_round_trip_through_rig_memory() {
    let dir = tempfile::tempdir().unwrap();
    let writer = EventLogWriter::open(dir.path(), "s").unwrap();
    let (recorder, task) = spawn_writer(writer, None);
    let memory = SessionMemory::new(
        "s",
        dir.path(),
        recorder.clone(),
        AttachmentStore::new(dir.path().join("attachments")),
    );
    let messages = vec![Message::user("hello"), Message::assistant("hi")];

    memory.append("s", messages.clone()).await.unwrap();
    assert_eq!(memory.load("s").await.unwrap(), normalized(messages));

    drop(memory);
    drop(recorder);
    task.close().await.unwrap();
}

#[tokio::test]
async fn resumed_child_memory_reads_only_its_exact_lineage() {
    let dir = tempfile::tempdir().unwrap();
    let writer = EventLogWriter::open(dir.path(), "s").unwrap();
    let (recorder, task) = spawn_writer(writer, None);
    let attachments = AttachmentStore::new(dir.path().join("attachments"));
    let root = SessionMemory::new("s", dir.path(), recorder.clone(), attachments.clone());
    let child_recorder = recorder.child_lineage("delegate-child");
    let child = SessionMemory::for_lineage(
        "s:main/delegate-child",
        "main/delegate-child",
        dir.path(),
        child_recorder,
        attachments,
    );
    root.append("s", vec![Message::user("root only")])
        .await
        .unwrap();
    child
        .append("s:main/delegate-child", vec![Message::user("child only")])
        .await
        .unwrap();
    recorder.flush().await;

    assert_eq!(
        root.load("s").await.unwrap(),
        normalized(vec![Message::user("root only")])
    );
    assert_eq!(
        child.load("s:main/delegate-child").await.unwrap(),
        normalized(vec![Message::user("child only")])
    );

    drop(root);
    drop(child);
    drop(recorder);
    task.close().await.unwrap();
}

#[tokio::test]
async fn compaction_replaces_model_context_without_replacing_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let writer = EventLogWriter::open(dir.path(), "s").unwrap();
    let (recorder, task) = spawn_writer(writer, None);
    let memory = SessionMemory::new(
        "s",
        dir.path(),
        recorder.clone(),
        AttachmentStore::new(dir.path().join("attachments")),
    );
    memory
        .append(
            "s",
            vec![
                Message::user("old request"),
                Message::assistant("old answer"),
            ],
        )
        .await
        .unwrap();
    let snapshot = vec![Message::user("summary"), Message::assistant("old answer")];

    memory
        .compact(
            snapshot.clone(),
            ConversationCompacted {
                summary: "summary".into(),
                tokens_before: 42,
                kept_messages: 1,
                reason: "manual".into(),
                read_files: Vec::new(),
                modified_files: Vec::new(),
            },
        )
        .await
        .unwrap();

    memory
        .append(
            "s",
            vec![
                Message::user("next request"),
                Message::assistant("next answer"),
            ],
        )
        .await
        .unwrap();
    let mut expected = snapshot;
    expected.extend([
        Message::user("next request"),
        Message::assistant("next answer"),
    ]);
    assert_eq!(memory.load("s").await.unwrap(), normalized(expected));
    // `load` reads live context from RAM, but the event log is written by the
    // recorder task. Reading the file without flushing races that task, which
    // is why this test failed roughly one run in twenty.
    recorder.flush().await;
    let events = EventLogReader::new(dir.path()).read_all().unwrap();
    assert_eq!(
        replay_for_ui(&events),
        vec![
            ReplayItem::User("old request".into()),
            ReplayItem::Assistant("old answer".into()),
            ReplayItem::User("next request".into()),
            ReplayItem::Assistant("next answer".into()),
        ]
    );
    let SessionEvent::ConversationMessages(batch) = events[2].event() else {
        panic!("expected compacted conversation snapshot")
    };
    assert!(batch.reset);
    assert_eq!(batch.display_from, 2);

    drop(memory);
    drop(recorder);
    task.close().await.unwrap();
}

#[tokio::test]
async fn first_append_migrates_legacy_projection_into_reset_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let writer = EventLogWriter::open(dir.path(), "s").unwrap();
    let (recorder, task) = spawn_writer(writer, None);
    recorder.record(TurnUser {
        content: vec![ContentBlock::Text { text: "old".into() }],
        display: None,
        source: "prompt".into(),
    });
    recorder.flush().await;
    let memory = SessionMemory::new(
        "s",
        dir.path(),
        recorder.clone(),
        AttachmentStore::new(dir.path().join("attachments")),
    );

    memory
        .append("s", vec![Message::assistant("new")])
        .await
        .unwrap();
    assert_eq!(
        memory.load("s").await.unwrap(),
        normalized(vec![Message::user("old"), Message::assistant("new")])
    );
    // Appends update live context in RAM; durability is an explicit boundary.
    recorder.flush().await;
    let events = EventLogReader::new(dir.path()).read_all().unwrap();
    let SessionEvent::ConversationMessages(batch) = events.last().unwrap().event() else {
        panic!("expected native conversation snapshot")
    };
    assert!(batch.reset);
    assert_eq!(batch.display_from, 1);

    drop(memory);
    drop(recorder);
    task.close().await.unwrap();
}

fn normalized(messages: Vec<Message>) -> Vec<Message> {
    serde_json::from_value(serde_json::to_value(messages).unwrap()).unwrap()
}

/// A revise must be invisible to the transcript.
///
/// This is the regression test for the failure mode that would make decay
/// unusable: writing the reset with `display_from: 0` blanks the user's
/// scrollback, and since decay runs before every turn it would do so
/// repeatedly. Comparing the rendered replay with and without the revise is the
/// only way to catch it — the model-facing side looks fine either way.
#[tokio::test]
async fn revising_model_context_leaves_the_transcript_untouched() {
    let baseline = {
        let dir = tempfile::tempdir().unwrap();
        let writer = EventLogWriter::open(dir.path(), "s").unwrap();
        let (recorder, task) = spawn_writer(writer, None);
        let memory = SessionMemory::new(
            "s",
            dir.path(),
            recorder.clone(),
            AttachmentStore::new(dir.path().join("attachments")),
        );
        memory
            .append(
                "s",
                vec![Message::user("look"), Message::assistant("looked")],
            )
            .await
            .unwrap();
        recorder.flush().await;
        let events = EventLogReader::new(dir.path()).read_all().unwrap();
        let rendered = format!("{:?}", replay_for_ui(&events));
        drop(memory);
        drop(recorder);
        task.close().await.unwrap();
        rendered
    };

    let dir = tempfile::tempdir().unwrap();
    let writer = EventLogWriter::open(dir.path(), "s").unwrap();
    let (recorder, task) = spawn_writer(writer, None);
    let memory = SessionMemory::new(
        "s",
        dir.path(),
        recorder.clone(),
        AttachmentStore::new(dir.path().join("attachments")),
    );
    memory
        .append(
            "s",
            vec![Message::user("look"), Message::assistant("looked")],
        )
        .await
        .unwrap();
    // Same history, rewritten in place.
    memory
        .revise(vec![Message::user("look"), Message::assistant("looked")])
        .await
        .unwrap();
    recorder.flush().await;

    let events = EventLogReader::new(dir.path()).read_all().unwrap();
    assert_eq!(
        format!("{:?}", replay_for_ui(&events)),
        baseline,
        "a revise must not change what the user sees"
    );

    // And the write really did happen, hidden from display.
    let SessionEvent::ConversationMessages(batch) = events.last().unwrap().event() else {
        panic!("expected a conversation snapshot")
    };
    assert!(batch.reset);
    assert_eq!(batch.display_from, batch.messages.len());

    drop(memory);
    drop(recorder);
    task.close().await.unwrap();
}

/// A tool result carrying one text block and one inline PNG, as the `computer`
/// and `read` tools produce.
fn screenshot_turn(bytes: &[u8]) -> Vec<Message> {
    use base64::Engine as _;
    use rig_core::OneOrMany;
    use rig_core::completion::message::{
        Image, ImageMediaType, ToolResult, ToolResultContent, UserContent,
    };

    vec![Message::User {
        content: OneOrMany::one(UserContent::ToolResult(ToolResult {
            id: "fc_1".into(),
            call_id: None,
            content: OneOrMany::many([
                ToolResultContent::text("<observation surface=\"win:3\" epoch=\"1\">"),
                ToolResultContent::Image(Image {
                    data: rig_core::completion::message::DocumentSourceKind::Base64(
                        base64::engine::general_purpose::STANDARD.encode(bytes),
                    ),
                    media_type: Some(ImageMediaType::PNG),
                    detail: None,
                    additional_params: None,
                }),
            ])
            .unwrap(),
        })),
    }]
}

#[tokio::test]
async fn tool_result_images_leave_the_log_and_come_back_on_load() {
    let dir = tempfile::tempdir().unwrap();
    let writer = EventLogWriter::open(dir.path(), "s").unwrap();
    let (recorder, task) = spawn_writer(writer, None);
    let memory = SessionMemory::new(
        "s",
        dir.path(),
        recorder.clone(),
        AttachmentStore::new(dir.path().join("attachments")),
    );
    let bytes = b"pretend this is a 2 MiB screenshot";
    let turn = screenshot_turn(bytes);

    memory.append("s", turn.clone()).await.unwrap();
    recorder.flush().await;

    // The log holds a reference, not the pixels.
    let raw = std::fs::read_to_string(dir.path().join("events.jsonl")).unwrap();
    let encoded = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    };
    assert!(
        !raw.contains(&encoded),
        "inline base64 must never reach events.jsonl"
    );
    assert!(raw.contains("artist_attachment"));

    // The live projection still has the image inline for the next turn.
    assert_eq!(memory.load("s").await.unwrap(), normalized(turn.clone()));

    // And so does a cold reload, which is the resume path.
    let reloaded = SessionMemory::new(
        "s",
        dir.path(),
        recorder.clone(),
        AttachmentStore::new(dir.path().join("attachments")),
    );
    assert_eq!(reloaded.load("s").await.unwrap(), normalized(turn));

    drop(memory);
    drop(reloaded);
    drop(recorder);
    task.close().await.unwrap();
}
