//! The global/project split.
//!
//! Two databases rather than one relation with a scope column, because a
//! query-time HNSW `filter:` is only a post-filter over the `ef` candidate
//! pool — partitioning by filter would silently return fewer than `k` rows.
//! These tests pin the behaviour that split is supposed to buy.

use artist_memory::schema::DIM;
use artist_memory::{Memory, NewFact, Scope};

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
        subject: String::new(),
        predicate: String::new(),
        object: text.into(),
        text: text.into(),
        embedding: vector(seed),
        source_session: "s".into(),
        source_seq: 0,
        origin: "test".into(),
    }
}

async fn memory(dir: &tempfile::TempDir) -> Memory {
    Memory::open(&dir.path().join("config"), &dir.path().join("state"))
        .await
        .expect("open memory")
}

#[tokio::test]
async fn search_merges_both_scopes_and_labels_them() {
    let dir = tempfile::tempdir().unwrap();
    let memory = memory(&dir).await;

    memory
        .global()
        .put_facts(&[fact("prefers the harness edit tools over sed", 1)])
        .await
        .unwrap();
    memory
        .project()
        .put_facts(&[fact("this repository stages work on the Gortnite branch", 2)])
        .await
        .unwrap();

    let hits = memory.search("harness edit tools", &vector(1), 10).await.unwrap();
    assert!(
        hits.iter().any(|h| h.scope == Scope::Global),
        "a global fact should be reachable, got {hits:?}"
    );

    let hits = memory.search("Gortnite branch", &vector(2), 10).await.unwrap();
    assert!(
        hits.iter().any(|h| h.scope == Scope::Project),
        "a project fact should be reachable, got {hits:?}"
    );
}

#[tokio::test]
async fn the_stores_are_genuinely_independent() {
    let dir = tempfile::tempdir().unwrap();
    let memory = memory(&dir).await;

    memory.global().put_facts(&[fact("global only", 1)]).await.unwrap();

    assert_eq!(memory.global().fact_count().await.unwrap(), 1);
    assert_eq!(
        memory.project().fact_count().await.unwrap(),
        0,
        "a global write must not land in the project store"
    );

    // Ids are allocated per store, so both start at zero without colliding.
    let ids = memory
        .project()
        .put_facts(&[fact("project only", 2)])
        .await
        .unwrap();
    assert_eq!(ids, vec![0]);
}

#[tokio::test]
async fn recall_is_capped_at_the_requested_limit() {
    let dir = tempfile::tempdir().unwrap();
    let memory = memory(&dir).await;

    let facts: Vec<NewFact> = (0..6)
        .map(|i| fact(&format!("compaction detail number {i}"), i + 10))
        .collect();
    memory.project().put_facts(&facts).await.unwrap();
    memory
        .global()
        .put_facts(&[fact("compaction detail from global", 99)])
        .await
        .unwrap();

    // Each store is asked for `k`, so the merge must re-truncate or a caller
    // asking for 3 could receive 6.
    let hits = memory.search("compaction detail", &vector(10), 3).await.unwrap();
    assert!(hits.len() <= 3, "expected at most 3 hits, got {}", hits.len());
}
