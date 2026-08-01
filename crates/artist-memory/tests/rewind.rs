//! Memory must be rewind-aware.
//!
//! Facts are recorded as `memory.written` events and reconciled over
//! `visible_events`, so rewinding past a write has to make the fact disappear
//! from recall — the same property `TodoStore` has, and for the same reason:
//! the harness owns the state, the log is the truth, and rewind masks events
//! rather than deleting them.

use artist_memory::schema::DIM;
use artist_memory::{MemoryStore, NewFact};
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

fn written(text: &str) -> MemoryWritten {
    MemoryWritten {
        fact_id: format!("{:032x}", artist_memory::identity::proposition_id(text).0),
        subject: "artist".into(),
        predicate: "does".into(),
        object: text.into(),
        text: text.into(),
        origin: "test".into(),
        superseded: None,
        embedding: Vec::new(),
        embedder: String::new(),
    }
}

/// The same event, but carrying a vector from a named embedding space.
fn written_with_vector(text: &str, seed: u64, embedder: &str) -> MemoryWritten {
    let mut w = written(text);
    w.embedding = vector(seed);
    w.embedder = embedder.into();
    w
}

/// A peer's log should reinsert without a forward pass — that is the whole
/// reason the vector is carried.
#[tokio::test]
async fn a_logged_vector_is_reused_instead_of_re_embedded() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path().join("session");
    std::fs::create_dir_all(&session_dir).unwrap();

    let mut writer = EventLogWriter::open(&session_dir, "s").unwrap();
    append(&mut writer, written_with_vector("rten runs on the CPU", 4, "gemma-768"));
    let events = EventLogReader::new(&session_dir).read_all().unwrap();

    let store = MemoryStore::open(dir.path().join("memory.rocks"))
        .await
        .unwrap();
    let report = store.reconcile(&events, "gemma-768").await.unwrap();

    assert_eq!(report.restored, 1, "the logged vector should have been reused");
    assert!(report.missing.is_empty(), "nothing should need re-embedding");
    assert_eq!(store.fact_count().await.unwrap(), 1);
}

/// A vector from a *different* model is the dangerous case: same width, other
/// geometry. It must be refused rather than indexed.
#[tokio::test]
async fn a_vector_from_another_embedder_is_not_reused() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path().join("session");
    std::fs::create_dir_all(&session_dir).unwrap();

    let mut writer = EventLogWriter::open(&session_dir, "s").unwrap();
    append(&mut writer, written_with_vector("rten runs on the CPU", 4, "coderank-768"));
    let events = EventLogReader::new(&session_dir).read_all().unwrap();

    let store = MemoryStore::open(dir.path().join("memory.rocks"))
        .await
        .unwrap();
    let report = store.reconcile(&events, "gemma-768").await.unwrap();

    assert_eq!(report.restored, 0, "a foreign vector must not be indexed");
    assert_eq!(report.missing.len(), 1, "it must be reported for re-embedding");
    assert_eq!(store.fact_count().await.unwrap(), 0);
}

#[tokio::test]
async fn rewinding_past_a_write_removes_the_fact() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path().join("session");
    std::fs::create_dir_all(&session_dir).unwrap();

    let store = MemoryStore::open(dir.path().join("memory.rocks"))
        .await
        .unwrap();

    let ids = store
        .put_facts(&[new_fact("prefers tabs over spaces", 1), new_fact("uses fish shell", 2)])
        .await
        .unwrap();
    assert_eq!(ids.len(), 2);

    let mut writer = EventLogWriter::open(&session_dir, "s").unwrap();
    let first_seq = append(&mut writer, written("prefers tabs over spaces"));
    append(&mut writer, written("uses fish shell"));

    // Nothing masked yet: both facts are accounted for.
    let events = EventLogReader::new(&session_dir).read_all().unwrap();
    let report = store.reconcile(&events, "test-embedder").await.unwrap();
    assert!(report.is_clean(), "expected a clean reconcile, got {report:?}");
    assert_eq!(store.fact_count().await.unwrap(), 2);

    // Masks every event after `first_seq`, i.e. the second write.
    writer.append_rewind(first_seq, "test", "test").unwrap();
    let events = EventLogReader::new(&session_dir).read_all().unwrap();
    let report = store.reconcile(&events, "test-embedder").await.unwrap();

    assert_eq!(
        report.removed,
        vec![artist_memory::identity::proposition_id("uses fish shell")],
        "the masked fact should have been dropped, got {report:?}"
    );
    assert_eq!(store.fact_count().await.unwrap(), 1);

    let hits = store.search_facts("fish shell", &vector(2), 5).await.unwrap();
    assert!(
        !hits
            .iter()
            .any(|h| h.id == artist_memory::identity::proposition_id("uses fish shell")),
        "a rewound fact must not be recallable, got {hits:?}"
    );
}

#[tokio::test]
async fn a_lost_store_reports_what_the_log_still_knows() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path().join("session");
    std::fs::create_dir_all(&session_dir).unwrap();

    let mut writer = EventLogWriter::open(&session_dir, "s").unwrap();
    append(&mut writer, written("the provider is the codex proxy"));
    let events = EventLogReader::new(&session_dir).read_all().unwrap();

    // A fresh store stands in for one lost to an unsynced WAL or a format break.
    let store = MemoryStore::open(dir.path().join("memory.rocks"))
        .await
        .unwrap();
    let report = store.reconcile(&events, "test-embedder").await.unwrap();

    assert!(report.removed.is_empty());
    assert_eq!(
        report.missing.len(),
        1,
        "the log should surface the fact the store lost, got {report:?}"
    );
    assert_eq!(
        report.missing[0].fact_id,
        format!(
            "{:032x}",
            artist_memory::identity::proposition_id("the provider is the codex proxy").0
        )
    );
    assert_eq!(report.missing[0].text, "the provider is the codex proxy");
}
