//! What the read/write *policy* does, as opposed to what the storage contract
//! does (`store.rs`).
//!
//! Two behaviours here decide whether memory helps or hurts, and neither was
//! covered: what retrieval does with no embedder, and whether the admission
//! check can swallow a correction — because a contradiction is lexically
//! near-identical to what it contradicts.

use artist_memory::schema::DIM;
use artist_memory::{Admission, MemoryStore, NewFact, admit};

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
        source_session: "test-session".into(),
        source_seq: seed as i64,
        origin: "test".into(),
    }
}

async fn store(dir: &tempfile::TempDir) -> MemoryStore {
    MemoryStore::open(dir.path().join("memory.rocks"))
        .await
        .expect("open store")
}

/// `query_vector` yields an empty vector when no embedder is configured — a
/// cold start, a missing model file. Retrieval must fall back to the lexical
/// leg, because that leg needs no vector at all.
///
/// It did not: both legs were named in one fused script, so an empty `$qv`
/// failed the whole query on a dimension mismatch and `recall` swallowed the
/// error. The observable behaviour was no recall whatsoever.
#[tokio::test]
async fn retrieval_without_an_embedder_degrades_to_the_lexical_leg() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(&dir).await;
    s.put_facts(&[
        fact("Adam prefers tabs over spaces in Rust", 1),
        fact("the canvas server serves modules from memory", 2),
    ])
    .await
    .expect("put");

    let hits = s
        .search_facts("tabs", &[], 5)
        .await
        .expect("no embedder must not fail the query");
    assert!(
        hits.iter().any(|h| h.text.contains("tabs")),
        "the BM25 leg should still answer, got {hits:?}"
    );

    // And the vector leg is genuinely still wired when a vector is supplied.
    let hits = s
        .search_facts("tabs", &vector(1), 5)
        .await
        .expect("hybrid search");
    assert!(!hits.is_empty(), "hybrid retrieval returned nothing");
}

/// The prompt-conditioned channel passes the user's raw message as the query,
/// so retrieval has to survive whatever someone types.
///
/// Both halves of this were broken. The BM25 leg parses `query:` as an
/// expression in its own language, so `-`, `?`, `/`, `+`, `%` and `@` were
/// syntax errors that failed the *entire* fused query — seven of thirteen
/// realistic messages, silently, because `recall` discards the error. And the
/// leg is conjunctive, so even a well-formed sentence matched nothing unless
/// every word appeared in the fact.
#[tokio::test]
async fn retrieval_survives_whatever_the_user_types() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(&dir).await;
    s.put_facts(&[fact("the canvas server hot-reloads React apps", 1)])
        .await
        .expect("put");

    for query in [
        "hot-reloads",                       // hyphen
        "what does the canvas server do?",   // question mark
        "fix crates/artist-cli/src/main.rs", // slashes and dots
        "C++ interop",                       // plus
        "50% faster",                        // percent
        "email me@example.com",              // at
        "tabs AND spaces",                   // a literal operator word
        "",                                  // nothing at all
        "   ?!   ",                          // nothing searchable
    ] {
        s.search_facts(query, &[], 5)
            .await
            .unwrap_or_else(|e| panic!("query {query:?} must not fail retrieval: {e}"));
    }

    // Conjunctive matching made a whole sentence match nothing; a natural
    // question must reach the fact it is about.
    let hits = s
        .search_facts("what does the canvas server do?", &[], 5)
        .await
        .expect("search");
    assert_eq!(hits.len(), 1, "a question should reach the fact: {hits:?}");

    // Disjunction must not degenerate into matching everything.
    let hits = s
        .search_facts("zzzznotpresent yyyyalsomissing", &[], 5)
        .await
        .expect("search");
    assert!(hits.is_empty(), "nothing relevant should match: {hits:?}");
}

/// A correction is lexically near-identical to the belief it corrects, so the
/// LSH probe fires hardest exactly when the write matters most. The probe is
/// left alone — it is answering its own question correctly — and the *policy*
/// distinguishes restatement from revision.
#[tokio::test]
async fn a_correction_revises_rather_than_being_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(&dir).await;
    let stale = "always use tabs for indentation in this repo";
    let fix = "never use tabs for indentation in this repo";
    s.put_facts(&[fact(stale, 1)]).await.expect("put");

    // Deterministic: the same decision on every run, unlike the LSH probe this
    // replaces, which flagged this exact pair in 2 of 5 runs.
    for _ in 0..5 {
        let candidates = s
            .revision_candidates(fix, &vector(2), 5)
            .await
            .expect("candidates");
        assert_eq!(
            admit(fix, &candidates),
            Admission::Revises(artist_memory::identity::proposition_id(stale)),
            "a reversal must supersede, not be discarded"
        );
    }

    // A verbatim restatement still costs nothing.
    let candidates = s
        .revision_candidates(stale, &vector(1), 5)
        .await
        .expect("candidates");
    assert_eq!(
        admit(stale, &candidates),
        Admission::Restates(artist_memory::identity::proposition_id(stale))
    );

    // And an unrelated fact survives even though retrieval surfaces it, so the
    // discrimination above is the threshold's doing rather than empty recall.
    let candidates = s
        .revision_candidates("the canvas server hot-reloads React apps", &vector(3), 5)
        .await
        .expect("candidates");
    assert_eq!(
        admit("the canvas server hot-reloads React apps", &candidates),
        Admission::Insert
    );
}

/// A superseded fact leaves recall, and the surviving one carries the id and
/// timestamp the model needs to correct it in turn.
#[tokio::test]
async fn a_revision_leaves_the_old_belief_out_of_recall() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(&dir).await;
    s.put_facts(&[fact("always use tabs for indentation in this repo", 1)])
        .await
        .expect("put");
    let new = s
        .put_facts(&[fact("never use tabs for indentation in this repo", 2)])
        .await
        .expect("put")[0];
    s.supersede(
        artist_memory::identity::proposition_id("always use tabs for indentation in this repo"),
        new,
    )
    .await
    .expect("supersede");

    let hits = s
        .search_facts("tabs indentation", &[], 5)
        .await
        .expect("search");
    assert_eq!(
        hits.len(),
        1,
        "only the current belief should recall: {hits:?}"
    );
    assert_eq!(hits[0].id, new);
    assert!(hits[0].text.starts_with("never"));
    assert!(
        hits[0].created_at > 0.0,
        "a recalled fact must carry when it was written"
    );

    let rendered = artist_memory::render(&hits);
    assert!(
        rendered.contains(&format!("id=\"{}\"", artist_memory::identity::handle(new))),
        "{rendered}"
    );
}
