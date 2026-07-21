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
    let snapshot = vec![Message::user("summary"), Message::assistant("recent")];

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

    assert_eq!(memory.load("s").await.unwrap(), normalized(snapshot));
    let events = EventLogReader::new(dir.path()).read_all().unwrap();
    assert_eq!(
        replay_for_ui(&events),
        vec![
            ReplayItem::User("old request".into()),
            ReplayItem::Assistant("old answer".into()),
        ]
    );
    let SessionEvent::ConversationMessages(batch) = events.last().unwrap().event() else {
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
