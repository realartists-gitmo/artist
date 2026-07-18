//! Content-addressed blob store for image payloads, one directory per
//! session. Blobs are keyed by their SHA-256 hex digest so identical images
//! are stored once and event records stay small.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct AttachmentStore {
    dir: PathBuf,
}

impl AttachmentStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Store `bytes`, returning the content address. Idempotent.
    pub fn put(&self, bytes: &[u8]) -> Result<String> {
        let id = hex_digest(bytes);
        let path = self.dir.join(&id);
        if path.exists() {
            return Ok(id);
        }
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("create attachments dir {}", self.dir.display()))?;
        let mut tmp = tempfile_in(&self.dir)?;
        tmp.write_all(bytes).context("write attachment")?;
        let (file, tmp_path) = tmp.into_parts();
        file.sync_data().context("sync attachment")?;
        drop(file);
        fs::rename(&tmp_path, &path).context("publish attachment")?;
        Ok(id)
    }

    pub fn get(&self, id: &str) -> Result<Vec<u8>> {
        anyhow::ensure!(
            !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid attachment id {id:?}"
        );
        fs::read(self.dir.join(id)).with_context(|| format!("read attachment {id}"))
    }

    /// Attachment ids referenced by nothing in `referenced` are removed.
    pub fn prune(&self, referenced: &std::collections::HashSet<String>) -> Result<usize> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Ok(0);
        };
        let mut removed = 0;
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !referenced.contains(&name) {
                fs::remove_file(entry.path())
                    .with_context(|| format!("prune attachment {name}"))?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

struct TempFile {
    file: Option<fs::File>,
    path: PathBuf,
}

impl TempFile {
    fn into_parts(mut self) -> (fs::File, PathBuf) {
        (self.file.take().expect("file present"), self.path.clone())
    }
}

impl Write for TempFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.as_mut().expect("file present").write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.as_mut().expect("file present").flush()
    }
}

fn tempfile_in(dir: &Path) -> Result<TempFile> {
    // The temp name must be unique per staging write, not just per process:
    // the same store is shared across concurrent background delegates, so a
    // pid-only name lets two simultaneous `put()`s truncate each other's file
    // and publish torn bytes under a content-addressed name. A process-local
    // atomic counter makes every concurrent writer's staging path distinct.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(format!(".tmp-{}-{}", std::process::id(), seq));
    let file =
        fs::File::create(&path).with_context(|| format!("create temp file {}", path.display()))?;
    Ok(TempFile {
        file: Some(file),
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_round_trip_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(dir.path().join("attachments"));
        let id = store.put(b"png bytes").unwrap();
        let id2 = store.put(b"png bytes").unwrap();
        assert_eq!(id, id2);
        assert_eq!(store.get(&id).unwrap(), b"png bytes");
    }

    #[test]
    fn get_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(dir.path());
        assert!(store.get("../secrets").is_err());
        assert!(store.get("").is_err());
    }
}
