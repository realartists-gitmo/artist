//! Permanent content dictionary for compact Artist context encodings.
//!
//! Entries are never reclaimed here. A reference may occur in a transcript
//! retained beyond the local project, so deletion would make exact expansion
//! impossible. Compaction/restart protocols decide when to re-define entries;
//! this module only preserves their durable identity.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io,
    path::PathBuf,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DictionaryError {
    #[error("dictionary I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("dictionary serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("dictionary reference `{0}` does not exist")]
    UnknownReference(String),
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

    /// Return the stable `§<opaque-prefix>` reference for `value`, defining it
    /// atomically if necessary. Prefix collisions receive deterministic serial
    /// suffixes and existing values always retain their original reference.
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
            let prefix = opaque_prefix(value);
            let mut serial = 1_u64;
            let reference = loop {
                let candidate = if serial == 1 {
                    format!("§{prefix}")
                } else {
                    format!("§{prefix}-{serial}")
                };
                if !dictionary.entries.contains_key(&candidate) {
                    break candidate;
                }
                serial = serial.saturating_add(1);
            };
            dictionary
                .entries
                .insert(reference.clone(), value.to_owned());
            self.write(&dictionary)?;
            Ok(reference)
        })();
        let _ = lock.unlock();
        result
    }

    pub fn resolve(&self, reference: &str) -> Result<String, DictionaryError> {
        self.read()?
            .entries
            .get(reference)
            .cloned()
            .ok_or_else(|| DictionaryError::UnknownReference(reference.to_owned()))
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

fn opaque_prefix(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .take(10)
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
}
