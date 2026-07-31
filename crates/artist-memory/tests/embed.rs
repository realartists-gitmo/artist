//! End-to-end embedder checks against the real model.
//!
//! Ignored by default: the model is ~550 MB and is not in the repo. Run with
//!
//! ```text
//! ARTIST_TEST_MODEL_DIR=/path/to/model cargo test -p artist-memory --test embed -- --ignored
//! ```
//!
//! where the directory holds `model.onnx` and `tokenizer.json`.

use artist_memory::embed::Embedder;
use artist_memory::schema::DIM;

fn model_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("ARTIST_TEST_MODEL_DIR").map(std::path::PathBuf::from)
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[tokio::test]
#[ignore = "needs a local embedding model; see the module docs"]
async fn embeddings_are_normalized_and_discriminative() {
    let Some(dir) = model_dir() else {
        panic!("set ARTIST_TEST_MODEL_DIR to run this test");
    };
    let embedder = Embedder::load(&dir, DIM).await.expect("load model");

    let docs = vec![
        "fn compact(messages: &[Message]) -> Option<CompactionPlan> { walk_backward(messages) }"
            .to_string(),
        "fn append_event(log: &mut EventLogWriter, event: SessionEvent) -> Result<u64> { log.append(event) }"
            .to_string(),
        "The quick brown fox jumps over the lazy dog.".to_string(),
    ];
    let vectors = embedder.embed_documents(docs).await.expect("embed");

    assert_eq!(vectors.len(), 3);
    for vector in &vectors {
        assert_eq!(vector.len(), DIM, "width must match the schema column");
        // The graph CLS-pools but does not normalize; we do, and cosine
        // similarity in the store depends on it.
        let norm = cosine(vector, vector).sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-4,
            "expected unit length, got {norm}"
        );
    }

    // Two Rust functions should sit closer together than either does to prose.
    let code_pair = cosine(&vectors[0], &vectors[1]);
    let code_vs_prose = cosine(&vectors[0], &vectors[2]);
    assert!(
        code_pair > code_vs_prose,
        "code/code {code_pair:.4} should exceed code/prose {code_vs_prose:.4}"
    );
}

#[tokio::test]
#[ignore = "needs a local embedding model; see the module docs"]
async fn a_query_retrieves_the_function_it_describes() {
    let Some(dir) = model_dir() else {
        panic!("set ARTIST_TEST_MODEL_DIR to run this test");
    };
    let embedder = Embedder::load(&dir, DIM).await.expect("load model");

    let docs: Vec<String> = vec![
        "fn render_markdown(events: &[Envelope]) -> String { /* transcript projection */ }".into(),
        "fn should_compact(projected: u64, window: u64, settings: &CompactionConfig) -> bool { projected > window - settings.reserve_tokens }".into(),
        "fn compile_glob(pattern: Option<&str>) -> Result<Option<GlobMatcher>, ToolError> { /* ... */ }".into(),
    ];
    let corpus = embedder.embed_documents(docs).await.expect("embed docs");

    // Asked by behaviour, not by name — the whole point of a semantic index.
    let query = embedder
        .embed_query("how does the harness decide when to shrink the conversation")
        .await
        .expect("embed query");

    let scores: Vec<f32> = corpus.iter().map(|d| cosine(&query, d)).collect();
    let best = scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(i, _)| i)
        .unwrap();
    assert_eq!(
        best, 1,
        "expected should_compact to win; scores were {scores:?}"
    );
}

#[tokio::test]
#[ignore = "needs a local embedding model; see the module docs"]
async fn the_query_prefix_actually_changes_the_vector() {
    // CodeRankEmbed requires an instruction prefix on queries only. Getting
    // this wrong is silent: retrieval simply gets worse. This asserts the two
    // paths genuinely differ.
    let Some(dir) = model_dir() else {
        panic!("set ARTIST_TEST_MODEL_DIR to run this test");
    };
    let embedder = Embedder::load(&dir, DIM).await.expect("load model");

    let text = "where are session events written to disk";
    let as_query = embedder.embed_query(text).await.expect("query");
    let as_document = embedder
        .embed_documents(vec![text.to_string()])
        .await
        .expect("document")
        .pop()
        .unwrap();

    let similarity = cosine(&as_query, &as_document);
    assert!(
        similarity < 0.999,
        "query and document encodings should differ, got {similarity:.5}"
    );
}
