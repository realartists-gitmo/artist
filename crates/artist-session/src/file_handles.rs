//! Remote handles for content the provider has already been given.
//!
//! A stateless completion API resends the whole conversation on every request,
//! so an image inlined as base64 is re-uploaded on every turn for the rest of
//! the session — a computer-use run with twenty screenshots pays for all twenty
//! again each time the model speaks, inflated a further third by base64.
//!
//! Where a provider offers a files endpoint, the bytes can go up once and be
//! referenced by id thereafter. That is worth having for the bandwidth alone,
//! but the stronger reason is the same one behind [`crate::AttachmentStore`]'s
//! content addressing and behind the frozen prompt prefix: **content that is not
//! in the request cannot vary between requests.** Bytes replaced by a handle
//! stop being a way to accidentally invalidate a prompt cache, because they are
//! no longer part of the cached prefix at all.
//!
//! This ledger is the digest → remote id map that makes that substitution
//! possible across turns and across resumes. It deliberately stores no expiry:
//! provider retention is not something we can track accurately, so the contract
//! is that a caller which finds a handle rejected calls [`HandleLedger::forget`]
//! and falls back to inlining. A missing handle always degrades to today's
//! behaviour, never to a failed turn.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// Maps content digests to the ids a provider knows them by.
///
/// Keyed by namespace as well as digest because a handle is only meaningful to
/// the account that uploaded it: the same screenshot sent to two providers, or
/// to two accounts on one provider, has two unrelated ids.
#[derive(Clone, Debug)]
pub struct HandleLedger {
    dir: PathBuf,
}

impl HandleLedger {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The id this namespace knows `digest` by, if it has been uploaded.
    pub fn get(&self, namespace: &str, digest: &str) -> Option<String> {
        let raw = fs::read_to_string(self.path(namespace, digest)?).ok()?;
        let id = raw.trim();
        (!id.is_empty()).then(|| id.to_owned())
    }

    /// Record that `namespace` holds `digest` as `remote_id`.
    ///
    /// Written through a staging file for the same reason attachments are:
    /// concurrent delegates share one store, and two simultaneous writers to
    /// the same path can otherwise publish a torn id.
    pub fn put(&self, namespace: &str, digest: &str, remote_id: &str) -> Result<()> {
        let path = self
            .path(namespace, digest)
            .context("invalid namespace or digest")?;
        let parent = path.parent().expect("path has a parent");
        fs::create_dir_all(parent)
            .with_context(|| format!("create handle dir {}", parent.display()))?;

        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let staging = parent.join(format!(".tmp-{}-{}", std::process::id(), seq));
        let mut file = fs::File::create(&staging)
            .with_context(|| format!("create staging file {}", staging.display()))?;
        file.write_all(remote_id.as_bytes())
            .context("write handle")?;
        file.sync_data().context("sync handle")?;
        drop(file);
        fs::rename(&staging, &path).context("publish handle")?;
        Ok(())
    }

    /// Drop a handle the provider no longer honours.
    ///
    /// The expected trigger is a rejected request naming an expired or deleted
    /// file. Forgetting is enough: the next turn finds nothing, inlines the
    /// bytes, and re-uploads.
    pub fn forget(&self, namespace: &str, digest: &str) {
        if let Some(path) = self.path(namespace, digest) {
            let _ = fs::remove_file(path);
        }
    }

    /// `dir/<namespace>/<digest>`, or `None` when either component is unusable.
    ///
    /// The digest must look like one. Handles are consulted with ids that came
    /// from tool output, and a caller passing `..` must not be able to steer a
    /// read or a write out of the ledger directory.
    fn path(&self, namespace: &str, digest: &str) -> Option<PathBuf> {
        if digest.is_empty()
            || digest.len() > 128
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return None;
        }
        Some(self.dir.join(slug(namespace)).join(digest))
    }
}

/// A filesystem-safe, collision-free name for a provider namespace.
///
/// Readable where it can be — a namespace is usually something like
/// `openai:acct_1234` and being able to see that in a directory listing is
/// worth keeping — with a digest suffix so two namespaces that sanitise to the
/// same string still get separate directories.
fn slug(namespace: &str) -> String {
    let readable: String = namespace
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    let digest = Sha256::digest(namespace.as_bytes());
    format!(
        "{readable}-{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> (tempfile::TempDir, HandleLedger) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = HandleLedger::new(dir.path().join("handles"));
        (dir, ledger)
    }

    const DIGEST: &str = "abc123def456";

    #[test]
    fn a_recorded_handle_comes_back() {
        let (_dir, ledger) = ledger();
        ledger.put("openai:acct", DIGEST, "file-xyz").unwrap();
        assert_eq!(
            ledger.get("openai:acct", DIGEST).as_deref(),
            Some("file-xyz")
        );
    }

    #[test]
    fn an_unknown_digest_has_no_handle() {
        let (_dir, ledger) = ledger();
        assert!(ledger.get("openai:acct", DIGEST).is_none());
    }

    /// A handle is only meaningful to the account that uploaded it. Serving one
    /// account's file id to another would reference a file it cannot read.
    #[test]
    fn handles_do_not_leak_across_namespaces() {
        let (_dir, ledger) = ledger();
        ledger.put("openai:acct-one", DIGEST, "file-one").unwrap();
        assert!(ledger.get("openai:acct-two", DIGEST).is_none());
        assert!(ledger.get("anthropic:acct-one", DIGEST).is_none());
    }

    /// Namespaces that sanitise to the same readable string must not collide:
    /// `a/b` and `a:b` both become `a_b` before the digest suffix.
    #[test]
    fn similar_namespaces_stay_separate() {
        let (_dir, ledger) = ledger();
        ledger.put("acct/one", DIGEST, "slash").unwrap();
        ledger.put("acct:one", DIGEST, "colon").unwrap();
        assert_eq!(ledger.get("acct/one", DIGEST).as_deref(), Some("slash"));
        assert_eq!(ledger.get("acct:one", DIGEST).as_deref(), Some("colon"));
    }

    /// The recovery path: a rejected handle is forgotten, and the next lookup
    /// misses so the caller re-inlines and re-uploads.
    #[test]
    fn a_forgotten_handle_is_gone() {
        let (_dir, ledger) = ledger();
        ledger.put("openai:acct", DIGEST, "file-xyz").unwrap();
        ledger.forget("openai:acct", DIGEST);
        assert!(ledger.get("openai:acct", DIGEST).is_none());
    }

    /// Forgetting something never recorded is the common case after a provider
    /// error, and must not be an error itself.
    #[test]
    fn forgetting_an_unknown_handle_is_harmless() {
        let (_dir, ledger) = ledger();
        ledger.forget("openai:acct", DIGEST);
    }

    /// Digests reach this from tool output. A traversal attempt must not read
    /// or write outside the ledger.
    #[test]
    fn a_digest_that_is_not_a_digest_is_refused() {
        let (_dir, ledger) = ledger();
        for bad in ["../../etc/passwd", "", "not-hex", "abc/def"] {
            assert!(ledger.get("ns", bad).is_none(), "{bad} was accepted");
            assert!(ledger.put("ns", bad, "id").is_err(), "{bad} was written");
        }
    }

    /// Overwriting must land whole — a torn id would reference nothing.
    #[test]
    fn rewriting_a_handle_replaces_it_atomically() {
        let (_dir, ledger) = ledger();
        ledger.put("ns", DIGEST, "first").unwrap();
        ledger
            .put("ns", DIGEST, "second-and-rather-longer")
            .unwrap();
        assert_eq!(
            ledger.get("ns", DIGEST).as_deref(),
            Some("second-and-rather-longer")
        );
    }
}
