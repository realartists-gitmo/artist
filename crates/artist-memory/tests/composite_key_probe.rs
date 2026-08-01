//! Can an HNSW (and an FTS) index live on a relation with a *composite*
//! primary key?
//!
//! The proposition id is 128-bit and Cozo has no u128, so it is stored as two
//! `Int` columns. If the vector index cannot bind a two-column key, the whole
//! content-addressed identity has to be reshaped — better to find that out here
//! than halfway through rewriting the store.

use artist_memory::MemoryStore;
use artist_memory::schema::DIM;

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

#[tokio::test]
async fn hnsw_and_fts_work_on_a_two_column_key() {
    let dir = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(dir.path().join("probe.rocks"))
        .await
        .unwrap();

    store
        .script(
            r#"
            :create probe {
                hi: Int, lo: Int
                =>
                text: String,
                emb: <F32; 768>,
                live: Bool default true,
            }
            "#,
            Default::default(),
            true,
        )
        .await
        .expect("composite-key relation");

    store
        .script(
            r#"
            ::hnsw create probe:emb_idx {
                dim: 768, dtype: F32, fields: [emb], distance: Cosine,
                m: 16, ef_construction: 200, filter: live,
            }
            "#,
            Default::default(),
            true,
        )
        .await
        .expect("hnsw on a composite key");

    store
        .script(
            r#"
            ::fts create probe:text_fts {
                extractor: text, extract_filter: live,
                tokenizer: Simple,
                filters: [Lowercase, Stemmer('English'), Stopwords('en')]
            }
            "#,
            Default::default(),
            true,
        )
        .await
        .expect("fts on a composite key");

    // Two rows whose keys differ only in the low half — the case a single-column
    // key could not represent.
    for (lo, text) in [(1i64, "the provider is the codex proxy"), (2, "tabs not spaces")] {
        let emb: Vec<String> = vector(lo as u64).iter().map(|f| f.to_string()).collect();
        store
            .script(
                &format!(
                    "?[hi, lo, text, emb, live] <- [[7, {lo}, '{text}', \
                     vec([{}]), true]] :put probe {{hi, lo => text, emb, live}}",
                    emb.join(",")
                ), Default::default(), true)
            .await
            .expect("insert");
    }

    let qv: Vec<String> = vector(1).iter().map(|f| f.to_string()).collect();
    let rows = store
        .script(
            &format!(
                "?[hi, lo, text, dist] := ~probe:emb_idx{{hi, lo, text | \
                 query: vec([{}]), k: 2, ef: 50, bind_distance: dist}}",
                qv.join(",")
            ), Default::default(), false)
        .await
        .expect("hnsw search binding a two-column key");
    assert_eq!(rows.rows.len(), 2, "expected both rows back, got {rows:?}");

    let rows = store
        .script(
            "?[hi, lo, text] := ~probe:text_fts{hi, lo, text | query: 'codex', k: 2}", Default::default(), false)
        .await
        .expect("fts search binding a two-column key");
    assert_eq!(rows.rows.len(), 1, "expected the one match, got {rows:?}");
}
