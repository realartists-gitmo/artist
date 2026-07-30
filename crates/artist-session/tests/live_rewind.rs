use artist_session::{
    AttachmentStore, EventLogReader, EventLogWriter, HistoryRewind, ProviderContextHandle,
    SessionEvent, SessionMemory, spawn_writer,
};
use rig_core::{completion::Message, memory::ConversationMemory};
use serde_json::json;

#[tokio::test]
async fn live_rewind_restores_shared_memory_and_provider_context() {
    let dir = tempfile::tempdir().unwrap();
    let writer = EventLogWriter::open(dir.path(), "s1").unwrap();
    let (recorder, task) = spawn_writer(writer, None);
    let memory = SessionMemory::new(
        "s1",
        dir.path(),
        recorder.clone(),
        AttachmentStore::new(dir.path().join("attachments")),
    );
    let memory_clone = memory.clone();
    let context = ProviderContextHandle::from_events(&[], recorder.clone());
    let context_clone = context.clone();

    memory
        .append("s1", vec![Message::user("kept")])
        .await
        .unwrap();
    context
        .commit("s1", "openai.responses", vec![json!({"opaque":"kept"})])
        .await;
    recorder.flush().await;
    let events = EventLogReader::new(dir.path()).read_all().unwrap();
    let keep_seq = events
        .iter()
        .rposition(|event| matches!(event.event(), SessionEvent::ProviderContext(_)))
        .map(|index| events[index].seq)
        .unwrap();

    memory
        .append("s1", vec![Message::user("divergent")])
        .await
        .unwrap();
    context
        .commit("s1", "openai.responses", vec![json!({"opaque":"future"})])
        .await;
    recorder.record(HistoryRewind {
        to_seq: keep_seq,
        reason: "test".into(),
        by: "test".into(),
    });
    recorder.flush().await;

    let events = EventLogReader::new(dir.path()).read_all().unwrap();
    memory.reload_from_events(&events).unwrap();
    context.restore_from_events(&events).await;

    let loaded = memory_clone.load("s1").await.unwrap();
    let serialized = serde_json::to_string(&loaded).unwrap();
    assert!(serialized.contains("kept"));
    assert!(!serialized.contains("divergent"));
    assert_eq!(
        context_clone.items("s1", "openai.responses").await,
        vec![json!({"opaque":"kept"})]
    );

    drop(memory_clone);
    drop(memory);
    drop(context_clone);
    drop(context);
    drop(recorder);
    task.close().await.unwrap();
}
