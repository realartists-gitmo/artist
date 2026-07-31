use artist_session::{EventLogReader, EventLogWriter, ProviderContextHandle, spawn_writer};
use serde_json::json;

#[tokio::test]
async fn roundtrip_restart_and_latest_compaction_pruning() {
    let dir = tempfile::tempdir().unwrap();
    let writer = EventLogWriter::open(dir.path(), "s1").unwrap();
    let (recorder, task) = spawn_writer(writer, None);
    let handle = ProviderContextHandle::from_events(&[], recorder.clone());
    handle
        .commit(
            "conversation",
            "openai.responses",
            vec![
                json!({"type":"reasoning","encrypted_content":"secret"}),
                json!({"type":"compaction","encrypted_content":"old"}),
                json!({"type":"message","phase":"commentary"}),
                json!({"type":"compaction","encrypted_content":"new"}),
                json!({"type":"future","opaque":true}),
            ],
        )
        .await;
    assert_eq!(
        handle.items("conversation", "openai.responses").await,
        vec![
            json!({"type":"compaction","encrypted_content":"new"}),
            json!({"type":"future","opaque":true})
        ]
    );
    drop(handle);
    drop(recorder);
    task.close().await.unwrap();

    let events = EventLogReader::new(dir.path()).read_all().unwrap();
    let restarted = ProviderContextHandle::from_events(&events, artist_session::Recorder::noop());
    assert_eq!(
        restarted
            .items("conversation", "openai.responses")
            .await
            .len(),
        2
    );
    assert!(artist_session::render_markdown(&events).contains("secret") == false);
    assert!(artist_session::render_markdown(&events).contains("encrypted_content") == false);
}
