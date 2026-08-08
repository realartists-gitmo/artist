//! Durable HTTP MCP identities.
//!
//! HTTP transport sessions are not artist identities.  Identity is established only by
//! the explicit transport-owned `identity` call and is resumed by the bare artist name.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{Error, Names, Registration, Result, permits::write_atomic};

static SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpIdentity {
    /// Public stable artist name. This is the only model-facing identity.
    pub name: String,
    /// Internal actor/session key. Never expose this to the model.
    pub actor: String,
    pub profile: String,
    pub project: String,
    pub lease_session: String,
    pub created_at: u64,
    pub last_seen: u64,
}

#[derive(Clone, Debug)]
pub struct HttpIdentities {
    dir: PathBuf,
    names: Names,
}

impl HttpIdentities {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            names: crate::names(),
        }
    }

    /// Use a scoped roster rather than the machine-wide one. For tests that must
    /// not mutate the user's real roster, and for sandboxed harnesses.
    pub fn with_names(mut self, names: Names) -> Self {
        self.names = names;
        self
    }

    pub fn create(&self, profile: &str, project: &str) -> Result<HttpIdentity> {
        validate_profile(profile)?;
        fs::create_dir_all(&self.dir)?;
        let _guard = Lock::take(&self.dir)?;
        let token = unique_token();
        let actor = format!("a-{token}");
        let lease_session = format!("http-{token}");
        let claimed = self.names.claim(&Registration {
            session: lease_session.clone(),
            actor: actor.clone(),
            project: Some(project.to_owned()),
            profile: Some(profile.to_owned()),
            parent: None,
        })?;
        let now = now();
        let identity = HttpIdentity {
            name: claimed.name,
            actor,
            profile: profile.to_owned(),
            project: project.to_owned(),
            lease_session,
            created_at: now,
            last_seen: now,
        };
        if let Err(error) = self.write(&identity) {
            let _ = self.names.release(&identity.lease_session);
            return Err(error);
        }
        Ok(identity)
    }

    /// Resume exactly the named identity. Missing/expired identities never mint a new one.
    pub fn resume(&self, name: &str) -> Result<Option<HttpIdentity>> {
        validate_name(name)?;
        fs::create_dir_all(&self.dir)?;
        let _guard = Lock::take(&self.dir)?;
        let path = self.path(name);
        let Some(mut identity) = read(&path)? else {
            return Ok(None);
        };
        identity.last_seen = now();
        write_atomic(&path, &serde_json::to_vec(&identity)?)?;
        Ok(Some(identity))
    }

    pub fn get(&self, name: &str) -> Result<Option<HttpIdentity>> {
        validate_name(name)?;
        let _guard = Lock::take(&self.dir).ok();
        read(&self.path(name))
    }

    /// Remove identities older than `keep_since`, releasing their machine-wide name lease.
    pub fn prune(&self, keep_since: u64) -> Result<usize> {
        fs::create_dir_all(&self.dir)?;
        let _guard = Lock::take(&self.dir)?;
        let mut removed = 0;
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() || entry.file_name() == ".lock" {
                continue;
            }
            let Some(identity) = read(&entry.path())? else {
                continue;
            };
            if identity.last_seen >= keep_since {
                continue;
            }
            fs::remove_file(entry.path())?;
            self.names.release(&identity.lease_session)?;
            removed += 1;
        }
        Ok(removed)
    }

    fn write(&self, identity: &HttpIdentity) -> Result<()> {
        write_atomic(&self.path(&identity.name), &serde_json::to_vec(identity)?)?;
        Ok(())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{}.json", encode(name)))
    }
}

fn validate_profile(profile: &str) -> Result<()> {
    if profile.is_empty() || profile.len() > 128 || profile.chars().any(char::is_control) {
        return Err(Error::Corrupt("invalid HTTP MCP profile".into()));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(Error::Corrupt("invalid HTTP MCP artist name".into()));
    }
    Ok(())
}

fn read(path: &Path) -> Result<Option<HttpIdentity>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| Error::Corrupt(e.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn unique_token() -> String {
    let n = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}{:x}{:x}", std::process::id(), nanos, n)
}

fn encode(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len() * 2);
    for byte in value.bytes() {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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
    fn resume_never_mints() {
        let root = tempfile::tempdir().unwrap();
        let ids = HttpIdentities::new(root.path().join("http"));
        assert!(ids.resume("Monet").unwrap().is_none());
    }
}
