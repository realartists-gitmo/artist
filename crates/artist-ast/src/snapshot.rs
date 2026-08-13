//! Immutable repository snapshots for host/resource integrations.

use std::{
    fs, io,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// A private, immutable-once-captured copy of the analyzable repository tree.
/// Existing path-based graph APIs can operate on `root()` without rereading
/// the mutable working tree.
#[derive(Debug)]
pub struct RepositorySnapshot {
    original_root: PathBuf,
    root: TempDir,
}

impl RepositorySnapshot {
    pub fn capture(root: &Path) -> io::Result<Self> {
        let original_root = root.canonicalize()?;
        let temp = tempfile::Builder::new()
            .prefix("artist-ast-snapshot-")
            .tempdir()?;
        for path in crate::walk_paths(&[original_root.clone()], None) {
            let relative = path.strip_prefix(&original_root).unwrap_or(&path);
            let destination = temp.path().join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&path, destination)?;
        }
        Ok(Self {
            original_root,
            root: temp,
        })
    }

    pub fn capture_for_file(root: &Path, file: &Path) -> io::Result<Self> {
        let analysis_root = if file.starts_with(root) {
            root.to_owned()
        } else {
            file.parent().unwrap_or(root).to_owned()
        };
        Self::capture(&analysis_root)
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    pub fn map_path(&self, path: &Path) -> PathBuf {
        self.root
            .path()
            .join(path.strip_prefix(&self.original_root).unwrap_or(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_isolated_from_later_working_tree_writes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        fs::write(&file, "fn old_name() {}\n").unwrap();
        let snapshot = RepositorySnapshot::capture(dir.path()).unwrap();
        fs::write(&file, "fn new_name() {}\n").unwrap();
        let parsed = crate::parse_file(snapshot.root().join("main.rs").as_path()).unwrap();
        assert_eq!(parsed.declarations[0].name, "old_name");
    }
}
