//! Integration tests against a real RocksDB-backed store.
//!
//! Embeddings here are deterministic synthetic vectors rather than model
//! output, so these stay hermetic and fast. What they exercise is the storage
//! contract: schema idempotency, the vector round-trip, hybrid retrieval, and
//! — the one that would silently rot — that superseding a fact actually
//! removes it from vector recall.

use artist_memory::schema::DIM;
use artist_memory::{MemoryStore, NewFact, Scope};

/// Deterministic unit-ish vector, distinct per seed.
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

fn fact(text: &str, seed: u64) -> NewFact {
    NewFact {
        subject: "artist".into(),
        predicate: "does".into(),
        object: text.into(),
        text: text.into(),
        embedding: vector(seed),
        source_session: "test-session".into(),
        source_seq: seed as i64,
        origin: "test".into(),
    }
}

async fn store(dir: &tempfile::TempDir) -> MemoryStore {
    MemoryStore::open(dir.path().join("memory.rocks"), Scope::Project)
        .await
        .expect("open store")
}

#[tokio::test]
async fn schema_applies_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.rocks");

    let first = MemoryStore::open(&path, Scope::Project).await.unwrap();
    // The constant, not a literal — a legitimate DDL bump should not
    // require editing this assertion.
    assert_eq!(
        first.schema_version().await.unwrap(),
        Some(artist_memory::schema::SCHEMA_VERSION)
    );
    drop(first);

    // Re-opening must not attempt to recreate relations or indexes.
    let second = MemoryStore::open(&path, Scope::Project).await.unwrap();
    assert_eq!(
        second.schema_version().await.unwrap(),
        Some(artist_memory::schema::SCHEMA_VERSION)
    );
    assert_eq!(second.fact_count().await.unwrap(), 0);
}

#[tokio::test]
async fn facts_round_trip_through_vector_and_text_search() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir).await;

    let facts = vec![
        fact("compaction keeps recent turns and never cuts at a tool result", 1),
        fact("session events are appended to events.jsonl on disk", 2),
        fact("the todo list survives a handoff verbatim", 3),
    ];
    let ids = store.put_facts(&facts).await.unwrap();
    assert_eq!(ids, vec![0, 1, 2]);
    assert_eq!(store.fact_count().await.unwrap(), 3);

    // The lexical leg alone should find this by wording.
    let hits = store
        .search_facts("events jsonl disk", &vector(99), 5)
        .await
        .unwrap();
    assert!(
        hits.iter().any(|h| h.text.contains("events.jsonl")),
        "expected the BM25 leg to surface the jsonl fact, got {hits:?}"
    );

    // The semantic leg alone should find this by vector identity.
    let hits = store.search_facts("nothing lexical here", &vector(3), 5).await.unwrap();
    assert_eq!(
        hits.first().map(|h| h.id),
        Some(2),
        "expected the exact-vector match to rank first, got {hits:?}"
    );
}

#[tokio::test]
async fn superseding_removes_a_fact_from_vector_recall() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir).await;

    store
        .put_facts(&[fact("the model is gpt-4", 1), fact("the model is opus", 2)])
        .await
        .unwrap();

    let before = store.search_facts("model", &vector(1), 10).await.unwrap();
    assert!(before.iter().any(|h| h.id == 0), "fact 0 should start visible");

    store.supersede(0, 1).await.unwrap();

    let after = store.search_facts("model", &vector(1), 10).await.unwrap();
    assert!(
        !after.iter().any(|h| h.id == 0),
        "superseded fact must leave recall; the HNSW create-time `filter: live` \
         is what enforces this, so a regression here means the index lost its filter"
    );
    assert_eq!(store.fact_count().await.unwrap(), 1);
}

/// Admission candidates come from ordinary retrieval, and a restatement must
/// be among them every time.
///
/// This replaces a probe over a dedicated MinHash-LSH index. That test's own
/// doc comment conceded the index "matches only about half the time" near its
/// threshold and deliberately stayed away from the boundary — while the
/// production admission path sat right on it. The index is gone; the decision
/// is now made exactly, over these candidates, in `admission.rs`.
#[tokio::test]
async fn revision_candidates_surface_a_restatement() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir).await;
    let original = "the compaction planner walks backward to retain roughly twenty thousand \
                    tokens and never cuts at a tool result";

    store.put_facts(&[fact(original, 1)]).await.unwrap();

    let restatement = format!("{original} boundary");
    let candidates = store
        .revision_candidates(&restatement, &vector(1), 5)
        .await
        .unwrap();
    assert!(
        candidates.iter().any(|(id, _)| *id == 0),
        "a near-identical restatement must be a candidate, got {candidates:?}"
    );

    // Retrieval is recall-oriented, so unrelated facts can appear here — that
    // is expected, and is exactly why the threshold lives in `admit` rather
    // than in the query.
    assert_eq!(
        artist_memory::admit(&restatement, &candidates),
        artist_memory::Admission::Revises(0)
    );
    assert_eq!(
        artist_memory::admit("the terminal renders with ratatui and syntect", &candidates),
        artist_memory::Admission::Insert
    );
}

#[tokio::test]
async fn export_and_import_round_trips_through_a_fresh_store() {
    let source_dir = tempfile::tempdir().unwrap();
    let source = store(&source_dir).await;
    source
        .put_facts(&[fact("alpha fact about compaction", 1), fact("beta fact about rules", 2)])
        .await
        .unwrap();
    let payload = source.export_json().await.unwrap();
    assert!(payload.contains("alpha fact"), "export should carry row data");

    let target_dir = tempfile::tempdir().unwrap();
    let target = store(&target_dir).await;
    target.import_json(payload).await.unwrap();

    assert_eq!(target.fact_count().await.unwrap(), 2);
    // import_json reindexes, so search must work on the restored store.
    let hits = target
        .search_facts("compaction", &vector(1), 5)
        .await
        .unwrap();
    assert!(
        hits.iter().any(|h| h.text.contains("alpha")),
        "restored store must be searchable, got {hits:?}"
    );
}
