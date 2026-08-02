//! Code retrieval, ranked by `artist-ast` over `artist-memory`'s index.
//!
//! These live here rather than in `artist-tools` because they need the memory
//! store and its embedder, which `artist-tools` has no business knowing about.
//!
//! The index is artist-memory's — one index over the working tree, not two.
//! What comes from `artist-ast` is the ranking: exact-definition boosting,
//! test and stub penalties, file-coherence weighting. See
//! `artist_memory::candidates` for why the fused hits enter as a single leg.

use crate::memory::MemoryHandle;
use artist_memory::candidates::MemoryCandidates;
use rig_core::tool::PortableTool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct CodeSearchError(String);

/// How many candidates to pull before ranking. Wider than `top_k` so the
/// boosts have something to reorder.
const FETCH_MULTIPLIER: usize = 4;

fn render(hits: &[artist_ast::search::source::SearchHit], root: &Path) -> String {
    if hits.is_empty() {
        return "no matches".into();
    }
    let mut out = String::new();
    for hit in hits {
        let path = Path::new(&hit.chunk.file_path);
        let shown = path.strip_prefix(root).unwrap_or(path);
        out.push_str(&format!(
            "{}:{}-{} ({:.3})\n",
            shown.display(),
            hit.chunk.start_line,
            hit.chunk.end_line,
            hit.score
        ));
        for line in hit.chunk.content.lines().take(3) {
            out.push_str(&format!("    {}\n", line.trim_end()));
        }
    }
    out
}

#[derive(Clone)]
pub struct CodeSearchTool {
    handle: MemoryHandle,
    root: std::path::PathBuf,
}

impl CodeSearchTool {
    pub fn new(handle: MemoryHandle, root: std::path::PathBuf) -> Self {
        Self { handle, root }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeSearchArgs {
    query: String,
    limit: Option<usize>,
}

impl PortableTool for CodeSearchTool {
    const NAME: &'static str = "code_search";
    type Error = CodeSearchError;
    type Args = CodeSearchArgs;
    type Output = String;

    fn description(&self) -> String {
        "Semantic + lexical search over indexed source. Ask in natural language — 'where do we \
         validate session tokens' — rather than guessing an identifier. Use grep when you know the \
         exact string, this when you know the intent."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 50}
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: CodeSearchArgs) -> Result<String, CodeSearchError> {
        let top_k = args.limit.unwrap_or(10).min(50);
        let vector = self.handle.query_vector(&args.query).await;
        let candidates = MemoryCandidates::fetch(
            self.handle.memory().store(),
            &args.query,
            &vector,
            top_k * FETCH_MULTIPLIER,
        )
        .await
        .map_err(|e| CodeSearchError(e.to_string()))?;

        if candidates.is_empty() {
            return Ok(
                "no matches. The code index may still be building — grep works meanwhile.".into(),
            );
        }
        Ok(render(&candidates.rank(&args.query, top_k), &self.root))
    }
}

#[derive(Clone)]
pub struct CodeRelatedTool {
    handle: MemoryHandle,
    root: std::path::PathBuf,
}

impl CodeRelatedTool {
    pub fn new(handle: MemoryHandle, root: std::path::PathBuf) -> Self {
        Self { handle, root }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRelatedArgs {
    path: String,
    line: u32,
    limit: Option<usize>,
}

/// Lines of context either side of the anchor line to use as the query.
const RELATED_CONTEXT: u32 = 12;

impl PortableTool for CodeRelatedTool {
    const NAME: &'static str = "code_related";
    type Error = CodeSearchError;
    type Args = CodeRelatedArgs;
    type Output = String;

    fn description(&self) -> String {
        "Find code similar to a given location — the other places that do the same kind of thing. \
         Useful for finding every site that needs the same change, or the existing pattern to \
         follow before writing something new."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "line": {"type": "integer", "minimum": 1},
                "limit": {"type": "integer", "minimum": 1, "maximum": 50}
            },
            "required": ["path", "line"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: CodeRelatedArgs) -> Result<String, CodeSearchError> {
        let top_k = args.limit.unwrap_or(10).min(50);
        let full = self.root.join(&args.path);
        let source = std::fs::read_to_string(&full)
            .map_err(|e| CodeSearchError(format!("{}: {e}", args.path)))?;

        // Use the neighbourhood of the anchor line as the query text. The store
        // has no "chunk at this location" lookup, and asking with the code
        // itself is what similarity means here anyway.
        let lines: Vec<&str> = source.lines().collect();
        let centre = (args.line as usize).saturating_sub(1);
        let lo = centre.saturating_sub(RELATED_CONTEXT as usize);
        let hi = (centre + RELATED_CONTEXT as usize).min(lines.len());
        if lo >= hi {
            return Err(CodeSearchError(format!(
                "line {} is past the end of {} ({} lines)",
                args.line,
                args.path,
                lines.len()
            )));
        }
        let query = lines[lo..hi].join("\n");

        let vector = self.handle.query_vector(&query).await;
        let candidates = MemoryCandidates::fetch(
            self.handle.memory().store(),
            &query,
            &vector,
            top_k * FETCH_MULTIPLIER,
        )
        .await
        .map_err(|e| CodeSearchError(e.to_string()))?;

        // Drop the source location itself — it is always its own best match and
        // the model already has it.
        let hits: Vec<_> = candidates
            .rank(&query, top_k + 1)
            .into_iter()
            .filter(|h| !h.chunk.file_path.ends_with(&args.path))
            .take(top_k)
            .collect();
        Ok(render(&hits, &self.root))
    }
}
