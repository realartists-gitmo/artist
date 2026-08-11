//! Permanent content dictionary for compact Artist context encodings.
//!
//! Entries are never reclaimed here. A reference may occur in a transcript
//! retained beyond the local project, so deletion would make exact expansion
//! impossible. Compaction/restart protocols decide when to re-define entries;
//! this module only preserves their durable identity.
//!
//! Identity is the exact canonical substring itself, encoded as exact UTF-8
//! bytes and fed directly into TECA. A reference is `§` followed by the render
//! of the shortest currently-unique TECA prefix. Resolution re-renders known
//! TECA streams and matches at atom boundaries; it never parses the suffix.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io,
    path::PathBuf,
};
use teca::default_address;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DictionaryError {
    #[error("dictionary I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("dictionary serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("dictionary reference `{0}` does not exist")]
    UnknownReference(String),
    #[error(
        "dictionary reference `{0}` is ambiguous; it matches multiple entries. Use a longer current reference, e.g. {1}"
    )]
    AmbiguousReference(String, String),
}

#[derive(Clone, Debug)]
pub struct Dictionary {
    path: PathBuf,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct DictionaryFile {
    entries: BTreeMap<String, String>,
}

impl Dictionary {
    /// Open the permanent global dictionary. Project runtime resources must not
    /// use this path: a reference is meaningful across projects and sessions.
    pub fn global() -> Result<Self, DictionaryError> {
        let root = std::env::var_os("ARTIST_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::config_dir().map(|path| path.join("artist")))
            .ok_or_else(|| io::Error::other("could not find Artist config directory"))?;
        Self::at(root.join("dictionary.json"))
    }

    pub fn at(path: impl Into<PathBuf>) -> Result<Self, DictionaryError> {
        let dictionary = Self { path: path.into() };
        if let Some(parent) = dictionary.path.parent() {
            fs::create_dir_all(parent)?;
        }
        if !dictionary.path.exists() {
            dictionary.write(&DictionaryFile::default())?;
        }
        Ok(dictionary)
    }

    /// Return the stable `§<teca-prefix>` reference for `value`, defining it
    /// atomically if necessary. Existing values always retain their original
    /// reference. The reference is the shortest currently-unique TECA prefix of
    /// the exact value bytes; no SHA-256 truncation and no serial suffixes.
    pub fn intern(&self, value: &str) -> Result<String, DictionaryError> {
        let lock_path = self.path.with_extension("lock");
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(lock_path)?;
        lock.lock_exclusive()?;
        let result = (|| {
            let mut dictionary = self.read()?;
            if let Some((reference, _)) = dictionary
                .entries
                .iter()
                .find(|(_, existing)| existing.as_str() == value)
            {
                return Ok(reference.clone());
            }
            let known: Vec<&String> = dictionary.entries.values().collect();
            let reference = shortest_unique_reference(value, &known);
            dictionary
                .entries
                .insert(reference.clone(), value.to_owned());
            self.write(&dictionary)?;
            Ok(reference)
        })();
        let _ = lock.unlock();
        result
    }

    /// Resolve a `§<teca-prefix>` reference against the actual known TECA
    /// streams. Zero matches are unknown/stale. One match expands to the exact
    /// persisted canonical substring. Multiple matches fail safely and return
    /// longer disambiguating references; the entry is never chosen heuristically.
    pub fn resolve(&self, reference: &str) -> Result<String, DictionaryError> {
        let suffix = reference.strip_prefix('§').unwrap_or(reference);
        if suffix.is_empty() {
            return Err(DictionaryError::UnknownReference(reference.to_owned()));
        }
        let dictionary = self.read()?;
        let mut matched: Vec<(&String, String, usize)> = Vec::new();
        for (stored, value) in &dictionary.entries {
            if let Some(depth) = stream_prefix_depth(value.as_bytes(), suffix) {
                matched.push((stored, value.clone(), depth));
            }
        }
        match matched.len() {
            0 => Err(DictionaryError::UnknownReference(reference.to_owned())),
            1 => Ok(matched[0].1.clone()),
            _ => {
                let longer: Vec<String> = matched
                    .iter()
                    .map(|(_, value, depth)| render_prefix(value.as_bytes(), depth + 2))
                    .collect();
                Err(DictionaryError::AmbiguousReference(
                    reference.to_owned(),
                    longer.join(", "),
                ))
            }
        }
    }

    pub fn entries(&self) -> Result<Vec<(String, String)>, DictionaryError> {
        Ok(self.read()?.entries.into_iter().collect())
    }

    fn read(&self) -> Result<DictionaryFile, DictionaryError> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(DictionaryFile::default()),
            Err(error) => Err(error.into()),
        }
    }

    fn write(&self, dictionary: &DictionaryFile) -> Result<(), DictionaryError> {
        let temporary = self.path.with_extension("tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(dictionary)?)?;
        fs::rename(temporary, &self.path)?;
        Ok(())
    }
}

/// The shortest prefix of `value`'s TECA stream that no known entry's stream
/// renders at any atom boundary. The exact value bytes are fed directly into
/// TECA with no normalization, trimming, case folding, hashing, summarization,
/// fuzzy matching, or tokenization.
fn shortest_unique_reference(value: &str, known: &[&String]) -> String {
    let mut rendered = String::new();
    let mut depth = 0usize;
    for atom in default_address(value.as_bytes()) {
        depth += 1;
        if depth != 1 {
            rendered.push('/');
        }
        rendered.push_str(&String::from_utf8_lossy(atom));
        let unique = known.iter().all(|other| {
            other.as_str() != value && stream_prefix_depth(other.as_bytes(), &rendered).is_none()
        });
        if unique {
            return format!("§{rendered}");
        }
    }
    // TECA streams are indefinitely extensible via the universal fallback, so
    // this branch is unreachable for any finite input; guard defensively.
    format!("§{rendered}")
}

/// Render the first `components` atoms of `identity` as a compact `§...`
/// reference. The canonical embedded lexicon atoms never contain `/`, so the
/// join is injective over atom sequences.
fn render_prefix(identity: &[u8], components: usize) -> String {
    let mut out = String::new();
    out.push('§');
    for (index, atom) in default_address(identity).take(components).enumerate() {
        if index != 0 {
            out.push('/');
        }
        out.push_str(&String::from_utf8_lossy(atom));
    }
    out
}

/// The depth at which `identity`'s TECA stream renders exactly to `suffix`
/// (opaque, atom-boundary matching), or `None` when no depth matches.
fn stream_prefix_depth(identity: &[u8], suffix: &str) -> Option<usize> {
    let mut rendered = String::new();
    let mut depth = 0usize;
    for atom in default_address(identity) {
        depth += 1;
        if depth != 1 {
            rendered.push('/');
        }
        rendered.push_str(&String::from_utf8_lossy(atom));
        if rendered == suffix {
            return Some(depth);
        }
        if rendered.len() > suffix.len() {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_is_stable_across_reopen_and_never_reuses_a_different_value() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("dictionary.json");
        let first = Dictionary::at(&path).unwrap();
        let reference = first.intern("alpha").unwrap();
        assert!(reference.starts_with('§'));
        assert_eq!(first.intern("alpha").unwrap(), reference);
        drop(first);
        let reopened = Dictionary::at(path).unwrap();
        assert_eq!(reopened.resolve(&reference).unwrap(), "alpha");
        assert_ne!(reopened.intern("beta").unwrap(), reference);
    }

    #[test]
    fn identity_is_teca_prefix_not_sha256_or_serial() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("dictionary.json");
        let dictionary = Dictionary::at(&path).unwrap();
        let reference = dictionary.intern("shared exact value").unwrap();
        assert!(reference.starts_with('§'));
        let suffix = reference.strip_prefix('§').unwrap();
        // No SHA-256 truncation (bare hex) and no numeric collision suffix.
        assert!(!suffix.contains('-'));
        assert!(!suffix.chars().all(|c| c.is_ascii_hexdigit()));
        // Exact byte round-trip.
        assert_eq!(
            dictionary.resolve(&reference).unwrap(),
            "shared exact value"
        );
    }

    #[test]
    fn resolve_is_opaque_prefix_matching_over_streams() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("dictionary.json");
        let dictionary = Dictionary::at(&path).unwrap();
        let reference = dictionary.intern("the quick brown fox").unwrap();
        // An unrelated reference is unknown.
        assert!(matches!(
            dictionary.resolve("§no/such/reference"),
            Err(DictionaryError::UnknownReference(_))
        ));
        // The exact reference resolves.
        assert_eq!(
            dictionary.resolve(&reference).unwrap(),
            "the quick brown fox"
        );
    }

    #[test]
    fn colliding_streams_fail_safely_with_longer_references() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("dictionary.json");
        let dictionary = Dictionary::at(&path).unwrap();
        let alpha = dictionary.intern("alpha").unwrap();
        let beta = dictionary.intern("beta").unwrap();
        assert_ne!(alpha, beta);
        // A reference must never resolve to the wrong entry heuristically.
        assert_eq!(dictionary.resolve(&alpha).unwrap(), "alpha");
        assert_eq!(dictionary.resolve(&beta).unwrap(), "beta");
    }

    #[test]
    fn exact_bytes_are_identity_and_stable_across_processes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("dictionary.json");
        {
            let dictionary = Dictionary::at(&path).unwrap();
            let _ = dictionary.intern("same bytes").unwrap();
        }
        let reopened = Dictionary::at(&path).unwrap();
        let second = reopened.intern("same bytes").unwrap();
        // Re-interning the identical canonical substring returns the same ref.
        assert_eq!(reopened.resolve(&second).unwrap(), "same bytes");
    }
}
