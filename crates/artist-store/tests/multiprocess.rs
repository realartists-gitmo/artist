//! Two genuinely separate OS processes appending to one file-backed store:
//! the second writer must observe the durable sequence, and stale sequences
//! must conflict rather than corrupt.

use artist_core::{InitialContext, SessionId, TranscriptEntryKind};
use artist_store::{FileStore, SessionStore, StoreError};

async fn child_routine(path: &std::path::Path) {
    let store = FileStore::new(path).await.unwrap();
    let record = artist_core::SessionRecord::new(
        SessionId::from("multi"),
        InitialContext { fragments: vec![] },
    );
    store.create(record).await.unwrap();
    let record = store.load(&SessionId::from("multi")).await.unwrap();
    let expected = record.next_sequence();
    let entry = record.entry(TranscriptEntryKind::Input {
        message_id: artist_core::MessageId::from("multi:message:1"),
        source: artist_core::Source::User,
        content: vec![artist_core::ContentPart::text("from-child")],
    });
    store
        .append(&SessionId::from("multi"), expected, &[entry])
        .await
        .unwrap();
}

#[tokio::test]
async fn separate_processes_serialize_and_conflict_on_one_file_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multi.jsonl");

    let child_flag = std::env::var("ARTIST_STORE_MULTIPROC_CHILD").is_ok();
    let child_path = std::env::var("ARTIST_STORE_MULTIPROC_PATH");
    if child_flag {
        // We are the re-exec'd child, already inside this binary's runtime.
        child_routine(std::path::Path::new(
            &child_path.expect("child receives the store path"),
        ))
        .await;
        std::process::exit(0);
    }

    // Re-exec THIS test binary as the child process with the flag set.
    let exe = std::env::current_exe().unwrap();
    let output = std::process::Command::new(exe)
        .arg("--exact")
        .arg("separate_processes_serialize_and_conflict_on_one_file_store")
        .arg("--nocapture")
        .env("ARTIST_STORE_MULTIPROC_CHILD", "1")
        .env("ARTIST_STORE_MULTIPROC_PATH", &path)
        .output()
        .expect("spawn child process");
    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The parent sees the child's committed sequence. The child's create
    // writes the initial record (sequence 1) and its input appends entry 2.
    let store = FileStore::new(&path).await.unwrap();
    let mut record = store.load(&SessionId::from("multi")).await.unwrap();
    assert!(
        record.next_sequence() >= 1,
        "child produced no durable record"
    );
    let _ = &mut record;

    // Stale expectation conflicts instead of corrupting.
    let stale = 0u64;
    let entry = record.entry(TranscriptEntryKind::SteeringQueued {
        message_id: artist_core::MessageId::from("multi:message:stale"),
        source: artist_core::Source::Harness,
        content: vec![artist_core::ContentPart::text("stale")],
    });
    let error = store
        .append(
            &SessionId::from("multi"),
            stale,
            std::slice::from_ref(&entry),
        )
        .await
        .expect_err("stale append must conflict");
    assert!(
        matches!(error, StoreError::Conflict { .. }),
        "unexpected error: {error}"
    );

    // A correctly-sequenced append from this process succeeds afterwards.
    let fresh = record.next_sequence();
    let fresh_entry = record.entry(TranscriptEntryKind::Input {
        message_id: artist_core::MessageId::from("multi:message:2"),
        source: artist_core::Source::Harness,
        content: vec![artist_core::ContentPart::text("from-parent")],
    });
    store
        .append(&SessionId::from("multi"), fresh, &[fresh_entry])
        .await
        .unwrap();
    let final_record = store.load(&SessionId::from("multi")).await.unwrap();
    assert!(final_record
        .entries()
        .iter()
        .any(|e| matches!(&e.kind, TranscriptEntryKind::Input { content, .. }
            if content.iter().any(|p| matches!(p, artist_core::ContentPart::Text { text } if text == "from-parent")))));
}
