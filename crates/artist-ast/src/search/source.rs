//! The seam between *finding* candidates and *ranking* them.
//!
//! Upstream, `Index::search` did both: it owned the embedder, the BM25 table
//! and the on-disk corpus, and it also owned the fusion and re-ranking that
//! turn two candidate lists into an answer. Those halves have different
//! lifetimes. Candidate generation is a storage concern — whoever holds the
//! index decides how to produce likely chunks. Ranking is not: reciprocal-rank
//! fusion, file-coherence boosting, exact-definition boosting and path
//! penalties are judgements about *code*, and they are worth applying to
//! candidates from any store.
//!
//! Splitting them lets a host with its own code index reuse the ranking rather
//! than reimplement it, without a second index over the same working tree.
//! [`Index`](super::index::Index) remains the default implementation, so
//! nothing about the standalone behaviour changes.

use super::chunker::Chunk;
use super::fusion::{combine, resolve_alpha, rrf_scores};
use super::ranking::{apply_query_boost, boost_multi_chunk_files, rerank_topk};

/// One search hit — a chunk with its final score.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub chunk: Chunk,
    pub score: f32,
}

/// Options for `search`. `find-related` doesn't need any (just `top_k`).
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    pub top_k: usize,
    /// Override the auto-resolved alpha. `None` = auto-detect from query type.
    pub alpha: Option<f32>,
    /// If set, restrict to chunks whose `language` field is in this set.
    pub languages: Option<Vec<String>>,
    /// If set, restrict to chunks whose `file_path` starts with this POSIX
    /// prefix (relative to home). `""` or `None` = no filter.
    pub query_scope: Option<String>,
    /// `path:` filters — keep only chunks whose `file_path` (lowercased)
    /// contains ANY of these substrings. Empty = no filter.
    pub path_contains: Vec<String>,
    /// `name:` filters — keep only chunks whose file name/stem (lowercased)
    /// contains ANY of these substrings. Empty = no filter.
    pub name_contains: Vec<String>,
}

impl SearchOptions {
    pub fn with_top_k(top_k: usize) -> Self {
        Self {
            top_k,
            ..Default::default()
        }
    }
}

/// Two candidate lists over the same corpus, each `(chunk_id, raw_score)`
/// ordered best-first.
///
/// Scores are *not* required to be comparable between the two lists — fusion is
/// by reciprocal rank, so only the ordering within each list is used. A source
/// that has no lexical leg (or no dense leg) returns an empty vector for it and
/// fusion degrades to the other.
#[derive(Debug, Clone, Default)]
pub struct Candidates {
    pub semantic: Vec<(u32, f32)>,
    pub lexical: Vec<(u32, f32)>,
}

/// A store that can propose chunks for a query.
///
/// Implementors own filtering. The options carry language, scope, `path:` and
/// `name:` predicates, and it is the source's job to honour them — an
/// index-position mask is meaningless to a store that does not lay its corpus
/// out in a contiguous array.
pub trait CandidateSource {
    /// The corpus that returned ids index into.
    ///
    /// Ids are positions in this slice. A source that materialises results
    /// per-query may return a per-query corpus; the ranking layer only ever
    /// indexes it with ids the same call produced.
    fn chunks(&self) -> &[Chunk];

    /// Propose candidates for `query`, honouring the filters in `opts`.
    ///
    /// `limit` is the per-leg candidate budget, already widened beyond
    /// `opts.top_k` — return up to that many per leg and let ranking cut down.
    fn candidates(&self, query: &str, opts: &SearchOptions, limit: usize) -> Candidates;
}

/// Widening factor from `top_k` to the per-leg candidate budget. Fusion and the
/// boosts need more to work with than the caller ultimately wants, or a chunk
/// that only one leg ranked highly can never be recovered by the other.
pub const CANDIDATE_WIDENING: usize = 5;

/// Fuse, boost and re-rank candidates into a final answer.
///
/// This is the whole of the ranking pipeline and is deliberately free of any
/// reference to how the candidates were found.
pub fn rank(source: &dyn CandidateSource, query: &str, opts: &SearchOptions) -> Vec<SearchHit> {
    let chunks = source.chunks();
    if chunks.is_empty() || opts.top_k == 0 {
        return Vec::new();
    }

    let alpha = resolve_alpha(query, opts.alpha);
    let candidates = source.candidates(query, opts, opts.top_k * CANDIDATE_WIDENING);

    // Enforce the trait's contract here rather than trusting it downstream.
    // The ranking functions index `chunks` directly (e.g. `boost_multi_chunk_files`),
    // which was sound when ids could only come from `Index`'s own arrays. Now
    // that any store can supply them, an id past the end of the corpus would
    // panic deep inside ranking. Drop them at the boundary instead: a source
    // that miscounts should degrade, not take the process down.
    let in_range = |c: &(u32, f32)| (c.0 as usize) < chunks.len();
    let semantic: Vec<(u32, f32)> = candidates.semantic.into_iter().filter(in_range).collect();
    let lexical: Vec<(u32, f32)> = candidates.lexical.into_iter().filter(in_range).collect();

    // Reciprocal-rank fusion, then the alpha blend between the two legs.
    let sem_rrf = rrf_scores(&semantic);
    let lex_rrf = rrf_scores(&lexical);
    let mut scored = combine(&sem_rrf, &lex_rrf, alpha);

    // File coherence: a file matching in several chunks is likelier to be the
    // one being asked about than a file matching in one.
    boost_multi_chunk_files(&mut scored, chunks);
    // Exact-definition boosting and test/stub penalties.
    let scored = apply_query_boost(scored, query, chunks);

    rerank_topk(&scored, chunks, opts.top_k, /* penalise_paths */ true)
        .into_iter()
        .filter_map(|(id, score)| {
            chunks.get(id as usize).map(|chunk| SearchHit {
                chunk: chunk.clone(),
                score,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        chunks: Vec<Chunk>,
        candidates: Candidates,
        last_limit: std::cell::Cell<usize>,
    }

    impl CandidateSource for Fake {
        fn chunks(&self) -> &[Chunk] {
            &self.chunks
        }
        fn candidates(&self, _q: &str, _o: &SearchOptions, limit: usize) -> Candidates {
            self.last_limit.set(limit);
            self.candidates.clone()
        }
    }

    fn chunk(path: &str) -> Chunk {
        Chunk {
            file_path: path.to_string(),
            ..Default::default()
        }
    }

    fn fake(n: usize, candidates: Candidates) -> Fake {
        Fake {
            chunks: (0..n).map(|i| chunk(&format!("src/f{i}.rs"))).collect(),
            candidates,
            last_limit: std::cell::Cell::new(0),
        }
    }

    #[test]
    fn empty_corpus_returns_nothing() {
        let f = fake(0, Candidates::default());
        assert!(rank(&f, "q", &SearchOptions::with_top_k(5)).is_empty());
    }

    #[test]
    fn zero_top_k_returns_nothing() {
        let f = fake(3, Candidates::default());
        assert!(rank(&f, "q", &SearchOptions::with_top_k(0)).is_empty());
    }

    #[test]
    fn candidate_budget_is_widened_beyond_top_k() {
        let f = fake(10, Candidates::default());
        let _ = rank(&f, "q", &SearchOptions::with_top_k(4));
        assert_eq!(f.last_limit.get(), 4 * CANDIDATE_WIDENING);
    }

    /// A source with only one leg must still produce results — fusion should
    /// degrade to whichever leg is populated rather than cancelling out.
    #[test]
    fn a_single_leg_still_ranks() {
        let f = fake(
            3,
            Candidates {
                semantic: vec![(0, 0.9), (2, 0.4)],
                lexical: Vec::new(),
            },
        );
        let hits = rank(&f, "q", &SearchOptions::with_top_k(3));
        assert!(!hits.is_empty(), "single-leg candidates ranked to nothing");
    }

    /// Ids that fall outside the corpus are dropped rather than panicking; a
    /// third-party source getting this wrong should degrade, not crash.
    #[test]
    fn out_of_range_ids_are_dropped_not_panicked() {
        let f = fake(
            2,
            Candidates {
                semantic: vec![(0, 0.9), (99, 0.8)],
                lexical: Vec::new(),
            },
        );
        let hits = rank(&f, "q", &SearchOptions::with_top_k(5));
        assert!(hits.iter().all(|h| h.chunk.file_path.starts_with("src/f")));
    }
}
