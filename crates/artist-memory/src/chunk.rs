//! AST-aware source chunking.
//!
//! `text-splitter`'s `CodeSplitter` descends the syntax tree, splitting at the
//! highest structural boundary that fits and only going deeper when a node is
//! too large. Grammars are compiled C and cost real binary size, so they sit
//! behind cargo features; anything without one falls back to a blank-line and
//! brace-depth heuristic rather than becoming unindexable.

use crate::types::Chunk;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Target chunk size in characters. Chosen so a chunk stays well inside the
/// embedding model's window after tokenization.
pub const MAX_CHUNK_CHARS: usize = 1500;

/// Languages we can parse. The `Other` arm still indexes, heuristically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Markdown,
    Python,
    Go,
    TypeScript,
    Other,
}

impl Lang {
    pub fn as_str(&self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Markdown => "markdown",
            Lang::Python => "python",
            Lang::Go => "go",
            Lang::TypeScript => "typescript",
            Lang::Other => "text",
        }
    }

    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|e| e.to_str()) {
            Some("rs") => Lang::Rust,
            Some("md" | "markdown") => Lang::Markdown,
            Some("py" | "pyi") => Lang::Python,
            Some("go") => Lang::Go,
            Some("ts" | "tsx" | "js" | "jsx" | "mts" | "cts") => Lang::TypeScript,
            _ => Lang::Other,
        }
    }
}

pub struct Chunker {
    max_chars: usize,
}

impl Default for Chunker {
    fn default() -> Self {
        Self {
            max_chars: MAX_CHUNK_CHARS,
        }
    }
}

impl Chunker {
    pub fn new(max_chars: usize) -> Self {
        Self { max_chars }
    }

    pub fn chunk_file(&self, path: &Path, source: &str) -> Vec<Chunk> {
        let display = path.display().to_string();
        chunk_source(source, &display, Lang::from_path(path), self.max_chars)
    }
}

/// Split `source` into chunks carrying 1-indexed inclusive line ranges.
pub fn chunk_source(source: &str, path: &str, lang: Lang, max_chars: usize) -> Vec<Chunk> {
    if source.trim().is_empty() {
        return Vec::new();
    }
    let spans = match ts_language(lang) {
        Some(language) => split_with_grammar(source, language, max_chars)
            .unwrap_or_else(|| heuristic_spans(source, max_chars)),
        None => heuristic_spans(source, max_chars),
    };

    let line_index = LineIndex::new(source);
    spans
        .into_iter()
        .filter_map(|(offset, text)| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return None;
            }
            Some(Chunk {
                path: path.to_owned(),
                lang: lang.as_str().to_owned(),
                start_line: line_index.line_of(offset),
                end_line: line_index.line_of(offset + text.len().saturating_sub(1)),
                content_hash: hash(trimmed),
                body: text,
            })
        })
        .collect()
}

fn split_with_grammar(
    source: &str,
    language: tree_sitter::Language,
    max_chars: usize,
) -> Option<Vec<(usize, String)>> {
    let splitter = text_splitter::CodeSplitter::new(language, max_chars).ok()?;
    Some(
        splitter
            .chunk_indices(source)
            .map(|(offset, text)| (offset, text.to_owned()))
            .collect(),
    )
}

fn ts_language(lang: Lang) -> Option<tree_sitter::Language> {
    match lang {
        #[cfg(feature = "lang-rust")]
        Lang::Rust => Some(tree_sitter_rust::LANGUAGE.into()),
        #[cfg(feature = "lang-markdown")]
        Lang::Markdown => Some(tree_sitter_md::LANGUAGE.into()),
        #[cfg(feature = "lang-python")]
        Lang::Python => Some(tree_sitter_python::LANGUAGE.into()),
        #[cfg(feature = "lang-go")]
        Lang::Go => Some(tree_sitter_go::LANGUAGE.into()),
        #[cfg(feature = "lang-typescript")]
        Lang::TypeScript => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        _ => None,
    }
}

/// Parser-free fallback: break at blank lines that return to column zero, then
/// pack greedily. Keeps every file indexable regardless of grammar coverage.
fn heuristic_spans(source: &str, max_chars: usize) -> Vec<(usize, String)> {
    let mut spans = Vec::new();
    let mut start = 0usize;
    let mut current = String::new();
    let mut offset = 0usize;

    for line in source.split_inclusive('\n') {
        let boundary = current.len() + line.len() > max_chars
            && !current.trim().is_empty()
            && line
                .chars()
                .next()
                .is_none_or(|c| !c.is_whitespace() || line.trim().is_empty());
        if boundary {
            spans.push((start, std::mem::take(&mut current)));
            start = offset;
        }
        if current.is_empty() {
            start = offset;
        }
        current.push_str(line);
        offset += line.len();
    }
    if !current.trim().is_empty() {
        spans.push((start, current));
    }
    spans
}

/// First 8 bytes of SHA-256, hex encoded. Enough to detect that a chunk's
/// content changed; collisions only cost a redundant re-embed.
fn hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Byte offset -> 1-indexed line, via binary search over line starts.
struct LineIndex {
    starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let mut starts = vec![0usize];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        Self {
            starts,
            len: source.len(),
        }
    }

    fn line_of(&self, offset: usize) -> u32 {
        let offset = offset.min(self.len);
        match self.starts.binary_search(&offset) {
            Ok(i) => (i + 1) as u32,
            Err(i) => i as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "fn a() {\n    1\n}\n\nfn b() {\n    2\n}\n";

    #[test]
    fn line_index_maps_offsets_to_lines() {
        let index = LineIndex::new(SAMPLE);
        assert_eq!(index.line_of(0), 1);
        assert_eq!(index.line_of(9), 2);
        assert_eq!(index.line_of(SAMPLE.len() - 1), 7);
    }

    #[test]
    fn chunks_cover_all_non_whitespace_content() {
        let chunks = chunk_source(SAMPLE, "a.rs", Lang::Rust, 1500);
        assert!(!chunks.is_empty());
        let joined: String = chunks.iter().map(|c| c.body.as_str()).collect();
        let strip = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        assert_eq!(strip(&joined), strip(SAMPLE));
    }

    #[test]
    fn line_ranges_are_ordered_and_within_the_file() {
        let lines = SAMPLE.lines().count() as u32;
        for chunk in chunk_source(SAMPLE, "a.rs", Lang::Rust, 1500) {
            assert!(chunk.start_line >= 1, "{chunk:?}");
            assert!(chunk.start_line <= chunk.end_line, "{chunk:?}");
            assert!(chunk.end_line <= lines + 1, "{chunk:?}");
        }
    }

    #[test]
    fn unknown_languages_still_produce_chunks() {
        let text = "alpha\n\nbeta\n\ngamma\n";
        let chunks = chunk_source(text, "x.unknown", Lang::Other, 8);
        assert!(chunks.len() > 1, "expected the heuristic to split: {chunks:?}");
    }

    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(chunk_source("   \n\n", "e.rs", Lang::Rust, 1500).is_empty());
    }
}
