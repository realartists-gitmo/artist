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

/// How to choose chunk boundaries.
///
/// Switchable because changing boundaries rewrites every `content_hash` and so
/// invalidates the whole index — a full re-embed, which `index.rs` documents as
/// hours on a large repository. That is not a change to make on the assumption
/// it is better; both strategies stay available so they can be compared on a
/// fixed subtree first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Strategy {
    /// `text-splitter`'s generic tree descent: split at the highest structural
    /// boundary that fits. Knows the syntax tree but not what a declaration is,
    /// so a chunk boundary can still land inside a function body.
    #[default]
    Grammar,
    /// One chunk per declaration, from `artist-ast`'s language adapters, each
    /// carrying its own signature. Declarations larger than `max_chars` fall
    /// back to grammar splitting within their own span, so an oversized
    /// function still gets indexed rather than being dropped or truncated.
    Declaration,
}

pub struct Chunker {
    max_chars: usize,
    strategy: Strategy,
}

impl Default for Chunker {
    fn default() -> Self {
        Self {
            max_chars: MAX_CHUNK_CHARS,
            strategy: Strategy::default(),
        }
    }
}

impl Chunker {
    pub fn new(max_chars: usize) -> Self {
        Self {
            max_chars,
            strategy: Strategy::default(),
        }
    }

    pub fn with_strategy(max_chars: usize, strategy: Strategy) -> Self {
        Self {
            max_chars,
            strategy,
        }
    }

    pub fn chunk_file(&self, path: &Path, source: &str) -> Vec<Chunk> {
        let display = path.display().to_string();
        match self.strategy {
            Strategy::Grammar => {
                chunk_source(source, &display, Lang::from_path(path), self.max_chars)
            }
            Strategy::Declaration => chunk_by_declaration(source, path, self.max_chars),
        }
    }
}

/// One chunk per declaration, falling back to grammar splitting for anything
/// the adapters cannot parse or that is too large to stand alone.
pub fn chunk_by_declaration(source: &str, path: &Path, max_chars: usize) -> Vec<Chunk> {
    let display = path.display().to_string();
    let lang = Lang::from_path(path);
    if source.trim().is_empty() {
        return Vec::new();
    }
    let Some(parsed) = artist_ast::parse_file(path) else {
        return chunk_source(source, &display, lang, max_chars);
    };

    let lines: Vec<&str> = source.lines().collect();
    let mut flat = Vec::new();
    flatten_declarations(&parsed.declarations, &mut flat);
    if flat.is_empty() {
        return chunk_source(source, &display, lang, max_chars);
    }

    let mut chunks = Vec::new();
    for decl in flat {
        let start = decl.start_line.saturating_sub(1);
        let end = decl.end_line.min(lines.len());
        if start >= end {
            continue;
        }
        let body = lines[start..end].join("\n");
        if body.trim().is_empty() {
            continue;
        }

        if body.chars().count() > max_chars {
            // Too large to stand alone. Split within its own span so the pieces
            // still belong to this declaration, then shift the resulting line
            // numbers back into the file's coordinate space.
            let line_offset = decl.start_line.saturating_sub(1) as u32;
            for mut piece in chunk_source(&body, &display, lang, max_chars) {
                piece.start_line += line_offset;
                piece.end_line += line_offset;
                chunks.push(piece);
            }
            continue;
        }

        chunks.push(Chunk {
            path: display.clone(),
            lang: lang.as_str().to_owned(),
            start_line: decl.start_line as u32,
            end_line: decl.end_line as u32,
            content_hash: hash(body.trim()),
            body,
        });
    }

    if chunks.is_empty() {
        return chunk_source(source, &display, lang, max_chars);
    }
    chunks
}

fn flatten_declarations<'a>(
    decls: &'a [artist_ast::core::Declaration],
    out: &mut Vec<&'a artist_ast::core::Declaration>,
) {
    for d in decls {
        // A container with children is represented by its children rather than
        // itself: indexing an entire `impl` block as one chunk buries every
        // method inside it.
        if d.children.is_empty() {
            out.push(d);
        } else {
            flatten_declarations(&d.children, out);
        }
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
        assert!(
            chunks.len() > 1,
            "expected the heuristic to split: {chunks:?}"
        );
    }

    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(chunk_source("   \n\n", "e.rs", Lang::Rust, 1500).is_empty());
    }

    // --- declaration-aligned strategy -------------------------------------

    /// Write a fixture to a real path: the adapters dispatch on file extension
    /// and read from disk, so this strategy cannot be exercised in memory.
    fn fixture(name: &str, body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(name);
        std::fs::write(&path, body).expect("write fixture");
        (dir, path)
    }

    const TWO_FNS: &str = "\
fn alpha(x: u32) -> u32 {
    let doubled = x * 2;
    doubled + 1
}

fn beta(y: u32) -> u32 {
    let halved = y / 2;
    halved - 1
}
";

    /// The point of the strategy: a chunk is a whole declaration, signature
    /// included, rather than a window that can begin mid-body.
    #[test]
    fn declaration_chunks_are_whole_declarations() {
        let (_dir, path) = fixture("a.rs", TWO_FNS);
        let chunks = chunk_by_declaration(TWO_FNS, &path, 1500);
        assert_eq!(chunks.len(), 2, "expected one chunk per function");
        assert!(chunks[0].body.contains("fn alpha"));
        assert!(chunks[0].body.contains("doubled + 1"));
        assert!(!chunks[0].body.contains("fn beta"), "chunks bled together");
        assert!(chunks[1].body.contains("fn beta"));
    }

    #[test]
    fn declaration_chunks_carry_true_line_ranges() {
        let (_dir, path) = fixture("a.rs", TWO_FNS);
        let chunks = chunk_by_declaration(TWO_FNS, &path, 1500);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[1].start_line, 6, "second fn starts on line 6");
    }

    /// A declaration past the size cap must still be indexed — split within its
    /// own span, with line numbers shifted back into the file's space, not
    /// dropped and not truncated.
    #[test]
    fn oversized_declaration_splits_but_keeps_file_line_numbers() {
        let big_body: String = (0..80).map(|i| format!("    let v{i} = {i};\n")).collect();
        let source = format!("fn huge() {{\n{big_body}}}\n");
        let (_dir, path) = fixture("a.rs", &source);
        let chunks = chunk_by_declaration(&source, &path, 200);
        assert!(chunks.len() > 1, "oversized declaration was not split");
        assert!(
            chunks.iter().all(|c| c.start_line >= 1),
            "line numbers were not shifted into file space"
        );
        let last = chunks.last().expect("chunks");
        assert!(
            last.end_line <= source.lines().count() as u32,
            "shifted line numbers ran past the end of the file"
        );
    }

    /// No adapter for the language means falling back rather than losing the
    /// file — the failure mode the old default was justified by.
    #[test]
    fn unparseable_file_falls_back_to_grammar_chunking() {
        let body = "some prose\n\nmore prose\n";
        let (_dir, path) = fixture("a.unknownext", body);
        let chunks = chunk_by_declaration(body, &path, 1500);
        assert!(!chunks.is_empty(), "fallback produced nothing");
    }

    #[test]
    fn empty_source_yields_no_declaration_chunks() {
        let (_dir, path) = fixture("a.rs", "  \n\n");
        assert!(chunk_by_declaration("  \n\n", &path, 1500).is_empty());
    }

    /// Changing boundaries changes every `content_hash`, which is exactly why
    /// the strategy is switchable rather than simply replaced.
    #[test]
    fn the_two_strategies_really_do_differ() {
        let (_dir, path) = fixture("a.rs", TWO_FNS);
        let grammar = Chunker::with_strategy(1500, Strategy::Grammar).chunk_file(&path, TWO_FNS);
        let decls = Chunker::with_strategy(1500, Strategy::Declaration).chunk_file(&path, TWO_FNS);
        let grammar_hashes: Vec<_> = grammar.iter().map(|c| &c.content_hash).collect();
        let decl_hashes: Vec<_> = decls.iter().map(|c| &c.content_hash).collect();
        assert_ne!(
            grammar_hashes, decl_hashes,
            "strategies produced identical hashes; the A/B would be meaningless"
        );
    }
}
