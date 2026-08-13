//! Hybrid code search (BM25 + dense embeddings) over a per-repo persistent index.
//!
//! Public entry points:
//! - `Index::open(path)` / `Index::build(path)` — open or build the on-disk index
//! - `index.search(query, opts)` — hybrid retrieval + ranked top-k
//! - `index.find_related(file, line, opts)` — semantic similarity from a chunk
//!
//! Retrieval is gated behind the `search` feature. Two submodules are the
//! exception and are always compiled:
//!
//! - `cache`, because `graph_cache` reuses its file-delta helpers
//!   (`compute_delta`, `hash_file`, `Delta`, `FileRecord`) — generic
//!   content-hash change detection, nothing to do with retrieval;
//! - `chunker`, because `cache` needs `is_indexable`. It is self-contained
//!   (no `crate::` references at all) and splitting source into AST-aligned
//!   chunks is useful well beyond this module.

pub mod cache;
pub mod chunker;

// The ranking half: reciprocal-rank fusion, file-coherence and
// exact-definition boosting, path penalties, and the `CandidateSource` seam.
// Deliberately separable from `search` — it needs only `Chunk` and `regex`, so
// a host with its own code index can implement the trait and reuse all of this
// without compiling an embedder, an HTTP stack or a model loader.
#[cfg(feature = "ranking")]
pub mod fusion;
#[cfg(feature = "ranking")]
pub mod query;
#[cfg(feature = "ranking")]
pub mod ranking;
#[cfg(feature = "ranking")]
pub mod source;
#[cfg(feature = "ranking")]
pub mod tokens;

#[cfg(feature = "search")]
pub mod bm25;
#[cfg(feature = "search")]
pub mod cli;
#[cfg(feature = "search")]
pub mod download;
#[cfg(feature = "search")]
pub mod embed;
#[cfg(feature = "search")]
pub mod format;
#[cfg(feature = "search")]
pub mod index;
#[cfg(feature = "search")]
pub mod shared;
