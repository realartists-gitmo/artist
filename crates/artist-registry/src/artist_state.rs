//! Durable per-artist state for semantics described as "this artist session".
//!
//! These values intentionally live beside the universal registry instead of in
//! process memory. Stateless HTTP requests and daemon restarts therefore observe
//! the same seen/loaded/first-use facts.

use std::{
    fs,
    path::{Path, PathBuf},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, permits::write_atomic};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistState {
    #[serde(default)]
    pub seen_skill_names: Vec<String>,
    #[serde(default)]
    pub loaded_skill_names: Vec<String>,
    #[serde(default)]
    pub seen_canvas_entries: Vec<String>,
    #[serde(default)]
    pub computer_skill_delivered: bool,
}

#[derive(Clone, Debug)]
pub struct ArtistStates {
    dir: PathBuf,
}

impl ArtistStates {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn get(&self, artist: &str) -> Result<ArtistState> {
        validate_artist(artist)?;
        let _lock = Lock::take(&self.dir)?;
        read(&self.path(artist))
    }

    pub fn update<R>(&self, artist: &str, f: impl FnOnce(&mut ArtistState) -> R) -> Result<R> {
        validate_artist(artist)?;
        let _lock = Lock::take(&self.dir)?;
        let path = self.path(artist);
        let mut state = read(&path)?;
        let result = f(&mut state);
        fs::create_dir_all(&self.dir)?;
        write_atomic(&path, &serde_json::to_vec(&state)?)?;
        Ok(result)
    }

    fn path(&self, artist: &str) -> PathBuf {
        self.dir.join(format!("{}.json", encode(artist)))
    }
}

fn validate_artist(artist: &str) -> Result<()> {
    if artist.is_empty() || artist.len() > 128 || artist.chars().any(char::is_control) {
        return Err(Error::Corrupt("invalid artist identity".into()));
    }
    Ok(())
}

fn read(path: &Path) -> Result<ArtistState> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| Error::Corrupt(e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ArtistState::default()),
        Err(e) => Err(e.into()),
    }
}

fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

struct Lock(fs::File);
impl Lock {
    fn take(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(dir.join(".lock"))?;
        file.lock_exclusive()?;
        Ok(Self(file))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_is_shared_between_registry_handles() {
        let root = tempfile::tempdir().unwrap();
        let a = ArtistStates::new(root.path().join("artists"));
        let b = ArtistStates::new(root.path().join("artists"));
        a.update("Goethe", |s| s.seen_skill_names.push("rust".into()))
            .unwrap();
        assert_eq!(b.get("Goethe").unwrap().seen_skill_names, vec!["rust"]);
    }
}
