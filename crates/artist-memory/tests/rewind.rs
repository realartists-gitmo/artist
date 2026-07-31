//! Memory must be rewind-aware.
//!
//! Facts are recorded as `memory.written` events and reconciled over
//! `visible_events`, so rewinding past a write has to make the fact disappear
//! from recall — the same property `TodoStore` has, and for the same reason:
//! the harness owns the state, the log is the truth, and rewind masks events
//! rather than deleting them.

use artist_memory::schema::DIM;
use artist_memory::{MemoryStore, NewFact, Scope};
use artist_session::{EventLogReader, EventLogWriter, MAIN_LINEAGE, MemoryWritten, SessionEvent};

fn append(writer: &mut EventLogWriter, event: MemoryWritten) -> u64 {
    writer
        .append(None, MAIN_LINEAGE, &SessionEvent::MemoryWritten(event))
        .unwrap()
}

fn vector(seed: u64) -> Vec<f32> {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    let raw: Vec<f32> = (0..DIM)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((x >> 33) as f32 / u32::MAX as f32) - 0.5
        })
        .collect();
    let norm = raw.iter().map(|v| v * v).sum::<f32>().sqrt();
    raw.into_iter().map(|v| v / norm).collect()
}

fn new_fact(text: &str, seed: u64) -> NewFact {
    NewFact {
        subject: "artist".into(),
        predicate: "does".into(),
        object: text.into(),
        text: text.into(),
        embedding: vector(seed),
        source_session: "s".into(),
        source_seq: seed as i64,
        origin: "test".into(),
    }
}

fn written(fact_id: i64, text: &str) -> MemoryWritten {
    MemoryWritten {
        fact_id,
        scope: "project".into(),
        subject: "artist".into(),
        predicate: "does".into(),
        object: text.into(),
        text: text.into(),
        origin: "test".into(),
        superseded: None,
    }
}

#[tokio::test]
async fn rewinding_past_a_write_removes_the_fact() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path().join("session");
    std::fs::create_dir_all(&session_dir).unwrap();

    let store = MemoryStore::open(dir.path().join("memory.rocks"), Scope::Project)
        .await
        .unwrap();

    let ids = store
        .put_facts(&[new_fact("prefers tabs over spaces", 1), new_fact("uses fish shell", 2)])
        .await
        .unwrap();
    assert_eq!(ids, vec![0, 1]);

    let mut writer = EventLogWriter::open(&session_dir, "s").unwrap();
    let first_seq = append(&mut writer, written(0, "prefers tabs over spaces"));
    append(&mut writer, written(1, "uses fish shell"));

    // Nothing masked yet: both facts are accounted for.
    let events = EventLogReader::new(&session_dir).read_all().unwrap();
    let report = store.reconcile(&events).await.unwrap();
    assert!(report.is_clean(), "expected a clean reconcile, got {report:?}");
    assert_eq!(store.fact_count().await.unwrap(), 2);

    // Masks every event after `first_seq`, i.e. the second write.
    writer.append_rewind(first_seq, "test", "test").unwrap();
    let events = EventLogReader::new(&session_dir).read_all().unwrap();
    let report = store.reconcile(&events).await.unwrap();

    assert_eq!(
        report.removed,
        vec![1],
        "the masked fact should have been dropped, got {report:?}"
    );
    assert_eq!(store.fact_count().await.unwrap(), 1);

    let hits = store.search_facts("fish shell", &vector(2), 5).await.unwrap();
    assert!(
        !hits.iter().any(|h| h.id == 1),
        "a rewound fact must not be recallable, got {hits:?}"
    );
}

#[tokio::test]
async fn a_lost_store_reports_what_the_log_still_knows() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path().join("session");
    std::fs::create_dir_all(&session_dir).unwrap();

    let mut writer = EventLogWriter::open(&session_dir, "s").unwrap();
    append(&mut writer, written(0, "the provider is the codex proxy"));
    let events = EventLogReader::new(&session_dir).read_all().unwrap();

    // A fresh store stands in for one lost to an unsynced WAL or a format break.
    let store = MemoryStore::open(dir.path().join("memory.rocks"), Scope::Project)
        .await
        .unwrap();
    let report = store.reconcile(&events).await.unwrap();

    assert!(report.removed.is_empty());
    assert_eq!(
        report.missing.len(),
        1,
        "the log should surface the fact the store lost, got {report:?}"
    );
    assert_eq!(report.missing[0].fact_id, 0);
    assert_eq!(report.missing[0].text, "the provider is the codex proxy");
}

#[tokio::test]
async fn other_scopes_are_ignored_during_reconcile() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path().join("session");
    std::fs::create_dir_all(&session_dir).unwrap();

    let store = MemoryStore::open(dir.path().join("memory.rocks"), Scope::Project)
        .await
        .unwrap();
    store.put_facts(&[new_fact("project scoped", 1)]).await.unwrap();

    let mut writer = EventLogWriter::open(&session_dir, "s").unwrap();
    append(&mut writer, written(0, "project scoped"));
    // A global-scope write must not make the project store drop its own facts.
    let mut global = written(7, "global scoped");
    global.scope = "global".into();
    append(&mut writer, global);

    let events = EventLogReader::new(&session_dir).read_all().unwrap();
    let report = store.reconcile(&events).await.unwrap();
    assert!(
        report.is_clean(),
        "a global write must not disturb the project store, got {report:?}"
    );
    assert_eq!(store.fact_count().await.unwrap(), 1);
}
