//! Incremental code indexing.
//!
//! A full pass over a large repository costs on the order of two hours at the
//! embedder's measured throughput, so this is deliberately built as resumable
//! background work: chunks are compared by content hash, and only what actually
//! moved is re-embedded. Re-indexing an unchanged tree is close to free.

use crate::chunk::Chunker;
use crate::embed::Embedder;
use crate::store::MemoryStore;
use anyhow::Result;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Files larger than this are skipped: a generated bundle or vendored blob
/// costs a lot of embedding time and answers no useful question.
const MAX_FILE_BYTES: u64 = 512 * 1024;

/// Directories never worth indexing, regardless of ignore files.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
    ".artist",
];

#[derive(Debug, Default, PartialEq, Eq)]
pub struct IndexReport {
    pub files_scanned: usize,
    pub files_changed: usize,
    pub chunks_added: usize,
    pub chunks_removed: usize,
    pub files_skipped: usize,
}

pub struct Indexer {
    store: MemoryStore,
    embedder: Embedder,
    chunker: Chunker,
}

impl Indexer {
    pub fn new(store: MemoryStore, embedder: Embedder) -> Self {
        Self {
            store,
            embedder,
            chunker: Chunker::default(),
        }
    }

    /// Index one file, re-embedding only chunks whose content changed.
    pub async fn index_file(&self, root: &Path, path: &Path) -> Result<IndexReport> {
        let mut report = IndexReport {
            files_scanned: 1,
            ..Default::default()
        };

        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            // A file removed between scan and read is not an error; drop it.
            Err(_) => {
                self.store.remove_path(&rel(root, path)).await?;
                return Ok(report);
            }
        };
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            report.files_skipped = 1;
            return Ok(report);
        }
        let Ok(source) = std::fs::read_to_string(path) else {
            // Binary or non-UTF8: nothing to embed.
            report.files_skipped = 1;
            return Ok(report);
        };

        let relative = rel(root, path);
        let chunks = self.chunker.chunk_file(Path::new(&relative), &source);
        let existing = self.store.chunk_hashes(&relative).await?;

        let fresh: BTreeSet<&str> = chunks.iter().map(|c| c.content_hash.as_str()).collect();
        let known: BTreeSet<&str> = existing.iter().map(|(_, h)| h.as_str()).collect();

        // Anything whose hash vanished from the file is stale, including the
        // rows for chunks that merely shifted content.
        let stale: Vec<i64> = existing
            .iter()
            .filter(|(_, hash)| !fresh.contains(hash.as_str()))
            .map(|(id, _)| *id)
            .collect();
        let added: Vec<_> = chunks
            .into_iter()
            .filter(|c| !known.contains(c.content_hash.as_str()))
            .collect();

        if stale.is_empty() && added.is_empty() {
            return Ok(report);
        }
        report.files_changed = 1;

        if !stale.is_empty() {
            report.chunks_removed = stale.len();
            self.store.remove_chunks(&stale).await?;
        }
        if !added.is_empty() {
            let bodies: Vec<String> = added.iter().map(|c| c.body.clone()).collect();
            let vectors = self.embedder.embed_documents(bodies).await?;
            let paired: Vec<_> = added.into_iter().zip(vectors).collect();
            report.chunks_added = paired.len();
            self.store.put_chunks(&paired).await?;
        }
        Ok(report)
    }

    /// Walk `root` and index everything indexable beneath it.
    ///
    /// Intended to be spawned, not awaited on a user-facing path.
    pub async fn index_tree(&self, root: &Path) -> Result<IndexReport> {
        let mut total = IndexReport::default();
        for path in walk(root) {
            let report = self.index_file(root, &path).await?;
            total.files_scanned += report.files_scanned;
            total.files_changed += report.files_changed;
            total.chunks_added += report.chunks_added;
            total.chunks_removed += report.chunks_removed;
            total.files_skipped += report.files_skipped;
        }
        Ok(total)
    }
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Depth-first walk, skipping build output, VCS metadata and symlinks.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if file_type.is_dir() {
                if SKIP_DIRS.contains(&name.as_ref()) || name.starts_with('.') {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() && !name.starts_with('.') {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_skips_build_output_and_dotfiles() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("target/debug/blob"), "junk").unwrap();
        std::fs::write(root.join(".git/config"), "junk").unwrap();
        std::fs::write(root.join(".hidden"), "junk").unwrap();

        let found = walk(root);
        assert_eq!(found.len(), 1, "expected only src/main.rs, got {found:?}");
        assert!(found[0].ends_with("src/main.rs"));
    }
}
