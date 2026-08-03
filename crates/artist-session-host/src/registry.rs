use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRecord {
    pub version: u16,
    pub session: String,
    pub pid: u32,
    pub socket: PathBuf,
    pub token: String,
    pub started_at_ms: u64,
}

pub struct HostRegistry {
    root: PathBuf,
}

impl HostRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)
            .with_context(|| format!("create host runtime {}", root.display()))?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        Ok(Self { root })
    }
    pub fn platform_default() -> Result<Self> {
        let root = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("artist-{}", unsafe { libc::geteuid() }))
            })
            .join("artist/hosts");
        Self::new(root)
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn acquire(&self, session: &str) -> Result<HostLease> {
        validate_session(session)?;
        if let Some(record) = self.resolve(session)? {
            bail!("session {} is already owned by pid {}", session, record.pid);
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        let record = HostRecord {
            version: crate::PROTOCOL_VERSION,
            session: session.into(),
            pid: std::process::id(),
            socket: self.root.join(format!("{session}.sock")),
            token,
            started_at_ms: now_ms(),
        };
        let path = self.record_path(session);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("claim session host {}", path.display()))?;
        serde_json::to_writer(&mut file, &record)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(HostLease {
            record,
            record_path: path,
        })
    }
    pub fn resolve(&self, session: &str) -> Result<Option<HostRecord>> {
        validate_session(session)?;
        let path = self.record_path(session);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let record: HostRecord = serde_json::from_slice(&bytes).context("decode host record")?;
        if !process_alive(record.pid) {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_file(&record.socket);
            return Ok(None);
        }
        Ok(Some(record))
    }
    fn record_path(&self, session: &str) -> PathBuf {
        self.root.join(format!("{session}.json"))
    }
}

pub struct HostLease {
    record: HostRecord,
    record_path: PathBuf,
}
impl HostLease {
    pub fn record(&self) -> &HostRecord {
        &self.record
    }
}
impl Drop for HostLease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.record_path);
        let _ = fs::remove_file(&self.record.socket);
    }
}

fn process_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
fn validate_session(session: &str) -> Result<()> {
    if session.is_empty()
        || !session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("invalid session id {session:?}");
    }
    Ok(())
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
