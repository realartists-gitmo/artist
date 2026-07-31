//! Presenting this store as a candidate source for `artist-ast`'s ranking.
//!
//! Both sides index the working tree, and running two indexes over one tree
//! means paying twice and reconciling two notions of freshness. This store
//! wins: it already has incremental content-hash invalidation, AST-aligned
//! chunk boundaries, source locations, and hybrid retrieval (HNSW + BM25 fused
//! by reciprocal rank in `search_chunks`).
//!
//! What it does *not* have is the code-specific ranking `artist-ast` carries:
//! exact-definition boosting, test and stub penalties, file-coherence
//! weighting. Implementing [`CandidateSource`] borrows exactly that, and
//! nothing else — no second index, no second embedder, and no additional
//! dependencies, since the `ranking` feature needs only `Chunk` and `regex`.
//!
//! ## On double fusion
//!
//! `search_chunks` returns hits its Cozo script has *already* fused across the
//! semantic and full-text legs. Splitting them back apart to refill both legs
//! here would fuse twice and distort the result, so the fused list is handed
//! over as the single dense leg and the lexical leg is left empty. Fusion over
//! one leg is a rank-preserving transform, so ordering survives intact and the
//! boosts — the part actually worth borrowing — apply on top.

use crate::store::MemoryStore;
use anyhow::Result;
use artist_ast::search::chunker::Chunk;
use artist_ast::search::source::{CandidateSource, Candidates, SearchHit, SearchOptions, rank};

/// A materialised set of candidates for one query.
///
/// The trait is synchronous but this store is not, so retrieval happens up
/// front in [`fetch`](Self::fetch) and the trait methods then serve what was
/// already fetched.
pub struct MemoryCandidates {
    chunks: Vec<Chunk>,
    fused: Vec<(u32, f32)>,
}

impl MemoryCandidates {
    /// Retrieve and materialise candidates for `query`.
    ///
    /// `query_vec` is the caller's embedding of `query` — this crate's
    /// `Embedder` produces it, and threading it in keeps this type free of
    /// any opinion about which embedder is in use.
    pub async fn fetch(
        store: &MemoryStore,
        query: &str,
        query_vec: &[f32],
        k: usize,
    ) -> Result<Self> {
        let hits = store.search_chunks(query, query_vec, k).await?;
        let mut chunks = Vec::with_capacity(hits.len());
        let mut fused = Vec::with_capacity(hits.len());
        for (i, hit) in hits.into_iter().enumerate() {
            fused.push((i as u32, hit.score as f32));
            chunks.push(Chunk {
                content: hit.body,
                file_path: hit.path,
                start_line: hit.start_line,
                end_line: hit.end_line,
                // `CodeHit` carries line spans but not byte offsets. Nothing in
                // the ranking path reads these — the boosts and penalties work
                // off `file_path` and `content` — so zero is honest rather
                // than a guess that could be mistaken for a real offset.
                start_byte: 0,
                end_byte: 0,
                language: hit.lang,
            });
        }
        Ok(Self { chunks, fused })
    }

    /// Rank the fetched candidates with `artist-ast`'s code-aware pipeline.
    pub fn rank(&self, query: &str, top_k: usize) -> Vec<SearchHit> {
        rank(self, query, &SearchOptions::with_top_k(top_k))
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

impl CandidateSource for MemoryCandidates {
    fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }

    /// Filtering already happened in the store's query, and the candidate list
    /// is fixed once fetched, so `opts` and `limit` carry no further work here.
    fn candidates(&self, _query: &str, _opts: &SearchOptions, _limit: usize) -> Candidates {
        Candidates {
            semantic: self.fused.clone(),
            lexical: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates_from(paths: &[(&str, f32)]) -> MemoryCandidates {
        MemoryCandidates {
            chunks: paths
                .iter()
                .map(|(p, _)| Chunk {
                    file_path: (*p).to_string(),
                    content: "fn thing() {}".into(),
                    language: "rust".into(),
                    ..Default::default()
                })
                .collect(),
            fused: paths
                .iter()
                .enumerate()
                .map(|(i, (_, s))| (i as u32, *s))
                .collect(),
        }
    }

    #[test]
    fn empty_store_ranks_to_nothing() {
        let c = candidates_from(&[]);
        assert!(c.is_empty());
        assert!(c.rank("thing", 5).is_empty());
    }

    #[test]
    fn fused_hits_survive_ranking() {
        let c = candidates_from(&[("src/a.rs", 0.9), ("src/b.rs", 0.5)]);
        let hits = c.rank("thing", 5);
        assert_eq!(hits.len(), 2, "fused candidates were dropped by ranking");
    }

    /// The point of the whole exercise: `artist-ast`'s path penalties should
    /// demote a test file relative to a source file the store ranked *above*
    /// it. If this stops holding, the borrow is buying nothing.
    #[test]
    fn ast_path_penalties_reorder_the_store_ranking() {
        // The store put the test file first.
        let c = candidates_from(&[("tests/thing_test.rs", 0.95), ("src/thing.rs", 0.90)]);
        let hits = c.rank("thing", 5);
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].chunk.file_path, "src/thing.rs",
            "expected the test-file penalty to demote it below the source file"
        );
    }

    #[test]
    fn top_k_is_respected() {
        let c = candidates_from(&[
            ("src/a.rs", 0.9),
            ("src/b.rs", 0.8),
            ("src/c.rs", 0.7),
            ("src/d.rs", 0.6),
        ]);
        assert_eq!(c.rank("thing", 2).len(), 2);
    }
}
