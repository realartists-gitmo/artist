//! Durable, open-ended session records shared by every Artist process on a project.
//!
//! The registry owns only universal lifecycle/addressing state. A session kind keeps
//! whatever additional state it needs in `snapshot` and registers model-facing
//! behaviour in the harness layer. `kind` is deliberately a string: adding a future
//! session kind must not require adding an enum arm here.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Result, permits::write_atomic, process::Owner};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Completed,
    Failed,
    Cancelled,
    Abandoned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionLifecycle {
    Live,
    Stopped { status: SessionStatus },
}

impl SessionLifecycle {
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }
}

/// One universally addressable session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub kind: String,
    pub owner: Owner,
    /// Owning model-facing artist identity. This is a bare artist name, never an
    /// internal actor id.
    pub artist: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_artist: Option<String>,
    pub created_at: u64,
    pub last_seen: u64,
    pub lifecycle: SessionLifecycle,
    #[serde(default)]
    pub cancel_requested: bool,
    /// Durable start of a graceful-stop interval. Retaining it across owner
    /// restart prevents cancellation from receiving a fresh grace period.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_requested_at: Option<u64>,
    /// Kind-authored durable current snapshot. The universal registry never
    /// interprets its shape.
    #[serde(default)]
    pub snapshot: Value,
    /// Monotonic active-observation request/completion counters. Kinds whose
    /// poll boundary is an explicit observation (canvas today, debugger later)
    /// use these without changing the universal poll schema.
    #[serde(default)]
    pub poll_requested: u64,
    #[serde(default)]
    pub poll_completed: u64,
    /// Internal roster lease key for identities whose public session id is an artist name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_lease: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelDisposition {
    LocalOwner,
    ForeignOwner(Owner),
    AlreadyStopped,
    Abandoned,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionInput {
    pub sequence: u64,
    pub input: Value,
}

#[derive(Clone, Debug)]
pub struct Sessions {
    dir: PathBuf,
}

impl Sessions {
    pub(crate) fn new(dir: PathBuf) -> Self {
        crate::process::install_wake_handler();
        Self { dir }
    }

    /// Create an exact model-visible id. The id remains reserved for as long as
    /// its record exists, including while stopped.
    pub fn create_exact(
        &self,
        id: &str,
        kind: &str,
        artist: &str,
        parent_artist: Option<&str>,
        snapshot: Value,
    ) -> Result<SessionRecord> {
        validate_id(id)?;
        validate_kind(kind)?;
        let _lock = Lock::take(&self.dir)?;
        if self.path(id).exists() {
            return Err(Error::Corrupt(format!(
                "session id `{id}` is still addressable"
            )));
        }
        let now = now();
        let record = SessionRecord {
            id: id.to_owned(),
            kind: kind.to_owned(),
            owner: Owner::current(),
            artist: artist.to_owned(),
            parent_artist: parent_artist.map(str::to_owned),
            created_at: now,
            last_seen: now,
            lifecycle: SessionLifecycle::Live,
            cancel_requested: false,
            cancel_requested_at: None,
            snapshot,
            poll_requested: 0,
            poll_completed: 0,
            name_lease: None,
        };
        self.write(&record)?;
        Ok(record)
    }

    /// Allocate a content-derived `<kind>:<slug>` id and create its record under
    /// the same registry lock, so two processes cannot win the same suffix.
    pub fn create_content(
        &self,
        kind: &str,
        content: &str,
        artist: &str,
        parent_artist: Option<&str>,
        snapshot: Value,
    ) -> Result<SessionRecord> {
        self.create_content_as(kind, kind, content, artist, parent_artist, snapshot)
    }

    /// Allocate a content-derived id prefix independently of the retained
    /// session kind. Native processes remain terminal-observable (`bash` kind)
    /// while receiving a distinct `process:<slug>` resource identity.
    pub fn create_content_as(
        &self,
        id_kind: &str,
        kind: &str,
        content: &str,
        artist: &str,
        parent_artist: Option<&str>,
        snapshot: Value,
    ) -> Result<SessionRecord> {
        validate_kind(id_kind)?;
        validate_kind(kind)?;
        let base = content_slug(content);
        let _lock = Lock::take(&self.dir)?;
        let mut ordinal = 1u64;
        loop {
            let suffix = if ordinal == 1 {
                String::new()
            } else {
                format!("-{ordinal}")
            };
            let id = format!("{id_kind}:{base}{suffix}");
            if !self.path(&id).exists() {
                let now = now();
                let record = SessionRecord {
                    id,
                    kind: kind.to_owned(),
                    owner: Owner::current(),
                    artist: artist.to_owned(),
                    parent_artist: parent_artist.map(str::to_owned),
                    created_at: now,
                    last_seen: now,
                    lifecycle: SessionLifecycle::Live,
                    cancel_requested: false,
                    cancel_requested_at: None,
                    snapshot,
                    poll_requested: 0,
                    poll_completed: 0,
                    name_lease: None,
                };
                self.write(&record)?;
                return Ok(record);
            }
            ordinal = ordinal.saturating_add(1);
        }
    }

    /// Reactivate a durable identity such as a canvas. A live owner is never
    /// stolen. A stopped record keeps its original `created_at` and id.
    pub fn reactivate(
        &self,
        id: &str,
        kind: &str,
        artist: &str,
        parent_artist: Option<&str>,
        snapshot: Value,
    ) -> Result<Reactivate> {
        validate_id(id)?;
        validate_kind(kind)?;
        let _lock = Lock::take(&self.dir)?;
        let path = self.path(id);
        let Some(mut record) = read_record(&path)? else {
            let now = now();
            let record = SessionRecord {
                id: id.to_owned(),
                kind: kind.to_owned(),
                owner: Owner::current(),
                artist: artist.to_owned(),
                parent_artist: parent_artist.map(str::to_owned),
                created_at: now,
                last_seen: now,
                lifecycle: SessionLifecycle::Live,
                cancel_requested: false,
                cancel_requested_at: None,
                snapshot,
                poll_requested: 0,
                poll_completed: 0,
                name_lease: None,
            };
            self.write(&record)?;
            return Ok(Reactivate::Opened(record));
        };
        if record.kind != kind {
            return Err(Error::Corrupt(format!(
                "session `{id}` is kind `{}`, not `{kind}`",
                record.kind
            )));
        }
        if record.lifecycle.is_live() && record.owner.is_alive() {
            record.last_seen = now();
            self.write(&record)?;
            return Ok(Reactivate::AlreadyLive(record));
        }
        if record.lifecycle.is_live() {
            record.lifecycle = SessionLifecycle::Stopped {
                status: SessionStatus::Abandoned,
            };
            record.last_seen = now();
            self.write(&record)?;
        }
        record.owner = Owner::current();
        record.artist = artist.to_owned();
        record.parent_artist = parent_artist.map(str::to_owned);
        record.last_seen = now();
        record.lifecycle = SessionLifecycle::Live;
        record.cancel_requested = false;
        record.cancel_requested_at = None;
        record.snapshot = snapshot;
        record.poll_requested = record.poll_completed;
        self.write(&record)?;
        Ok(Reactivate::Opened(record))
    }

    /// Read one record. The first reader to discover a dead live owner writes the
    /// `Abandoned` tombstone before returning it.
    pub fn get(&self, id: &str) -> Result<Option<SessionRecord>> {
        validate_id(id)?;
        let _lock = Lock::take(&self.dir)?;
        let Some(mut record) = read_record(&self.path(id))? else {
            return Ok(None);
        };
        let mut changed = false;
        if record.lifecycle.is_live() && !record.owner.is_alive() {
            record.lifecycle = SessionLifecycle::Stopped {
                status: SessionStatus::Abandoned,
            };
            changed = true;
        }
        record.last_seen = now();
        if changed {
            self.write(&record)?;
        } else {
            // `last_seen` is itself durable registry state.
            self.write(&record)?;
        }
        Ok(Some(record))
    }

    pub fn list(&self) -> Result<Vec<SessionRecord>> {
        fs::create_dir_all(&self.dir)?;
        let _lock = Lock::take(&self.dir)?;
        let mut records = Vec::new();
        for entry in fs::read_dir(&self.dir)?.filter_map(|entry| entry.ok()) {
            if !entry.file_type().ok().is_some_and(|ty| ty.is_file()) {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name == ".lock" || !name.ends_with(".json") {
                continue;
            }
            let Some(mut record) = read_record(&entry.path())? else {
                continue;
            };
            if record.lifecycle.is_live() {
                if !record.owner.is_alive() {
                    record.lifecycle = SessionLifecycle::Stopped {
                        status: SessionStatus::Abandoned,
                    };
                    self.write(&record)?;
                }
            }
            records.push(record);
        }
        // Listing is discovery, not an open: preserve the last explicit
        // interaction as the recency signal without mutating it here.
        records.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then(a.id.cmp(&b.id)));
        Ok(records)
    }

    pub fn set_snapshot(&self, id: &str, snapshot: Value) -> Result<SessionRecord> {
        self.update(id, |record| {
            record.snapshot = snapshot;
            Ok(())
        })
    }

    /// Atomically inspect and mutate one retained record under the registry lock.
    /// Kind implementations use this for first-writer-wins state such as ask answers.
    pub fn mutate(
        &self,
        id: &str,
        mutate: impl FnOnce(&mut SessionRecord) -> Result<()>,
    ) -> Result<SessionRecord> {
        self.update(id, mutate)
    }

    /// Request one active kind-authored observation and return its sequence.
    pub fn request_poll(&self, id: &str) -> Result<u64> {
        let record = self.update(id, |record| {
            if record.lifecycle.is_live() {
                record.poll_requested = record.poll_requested.saturating_add(1);
            }
            Ok(())
        })?;
        Ok(record.poll_requested)
    }

    pub fn complete_poll(&self, id: &str, sequence: u64, snapshot: Value) -> Result<SessionRecord> {
        self.update(id, |record| {
            record.snapshot = snapshot;
            record.poll_completed = record.poll_completed.max(sequence);
            Ok(())
        })
    }

    /// Associate an internal artist-name lease with this retained session. The
    /// lease is released only when registry retention prunes the record.
    pub fn set_name_lease(&self, id: &str, lease_key: String) -> Result<SessionRecord> {
        self.update(id, |record| {
            record.name_lease = Some(lease_key);
            Ok(())
        })
    }

    pub fn finish(
        &self,
        id: &str,
        status: SessionStatus,
        snapshot: Option<Value>,
    ) -> Result<SessionRecord> {
        self.update(id, |record| {
            if let Some(snapshot) = snapshot {
                record.snapshot = snapshot;
            }
            record.lifecycle = SessionLifecycle::Stopped { status };
            record.cancel_requested = false;
            record.cancel_requested_at = None;
            Ok(())
        })
    }

    /// Permanently remove a stopped resource and its queued input. Live
    /// resources are deliberately rejected: callers must first request a
    /// graceful stop and observe settlement, rather than making an owner race
    /// a disappearing durable record. A deleted artist session also releases
    /// its global roster reservation.
    pub fn delete_stopped(&self, id: &str) -> Result<SessionRecord> {
        validate_id(id)?;
        let _lock = Lock::take(&self.dir)?;
        let path = self.path(id);
        let Some(record) = read_record(&path)? else {
            return Err(Error::Corrupt(format!("unknown session `{id}`")));
        };
        if record.lifecycle.is_live() {
            return Err(Error::Corrupt(format!(
                "session `{id}` is live; stop it and poll until it settles before deleting"
            )));
        }
        if let Some(lease) = &record.name_lease {
            crate::names().release(lease)?;
        }
        fs::remove_file(path)?;
        let input = self.input_dir(id);
        if input.exists() {
            fs::remove_dir_all(input)?;
        }
        Ok(record)
    }

    pub fn cancel_requested(&self, id: &str) -> Result<bool> {
        Ok(self
            .get(id)?
            .is_some_and(|record| record.lifecycle.is_live() && record.cancel_requested))
    }

    /// Record a cancellation request. This method never claims that a foreign
    /// session has already become cancelled.
    pub fn request_cancel(&self, id: &str) -> Result<(CancelDisposition, SessionRecord)> {
        validate_id(id)?;
        let _lock = Lock::take(&self.dir)?;
        let path = self.path(id);
        let Some(mut record) = read_record(&path)? else {
            return Err(Error::Corrupt(format!("unknown session `{id}`")));
        };
        if !record.lifecycle.is_live() {
            return Ok((CancelDisposition::AlreadyStopped, record));
        }
        if !record.owner.is_alive() {
            record.lifecycle = SessionLifecycle::Stopped {
                status: SessionStatus::Abandoned,
            };
            record.cancel_requested = false;
            record.cancel_requested_at = None;
            record.last_seen = now();
            self.write(&record)?;
            return Ok((CancelDisposition::Abandoned, record));
        }
        if !record.cancel_requested {
            record.cancel_requested = true;
            let requested_at = now();
            record.cancel_requested_at = Some(requested_at);
            record.last_seen = requested_at;
            self.write(&record)?;
        }
        let disposition = if record.owner == Owner::current() {
            CancelDisposition::LocalOwner
        } else {
            CancelDisposition::ForeignOwner(record.owner)
        };
        Ok((disposition, record))
    }

    pub fn enqueue_input(&self, id: &str, input: Value) -> Result<SessionInput> {
        validate_id(id)?;
        let _lock = Lock::take(&self.dir)?;
        let Some(record) = read_record(&self.path(id))? else {
            return Err(Error::Corrupt(format!("unknown session `{id}`")));
        };
        if !record.lifecycle.is_live() {
            return Err(Error::Corrupt(format!("session `{id}` is stopped")));
        }
        let input_dir = self.input_dir(id);
        fs::create_dir_all(&input_dir)?;
        let next = fs::read_dir(&input_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()?
                    .strip_suffix(".json")?
                    .parse::<u64>()
                    .ok()
            })
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let item = SessionInput {
            sequence: next,
            input,
        };
        write_atomic(
            &input_dir.join(format!("{next:020}.json")),
            &serde_json::to_vec(&item)?,
        )?;
        Ok(item)
    }

    /// Atomically claim all currently queued inputs for the owner. Claimed files
    /// are removed only after they have been decoded, so a corrupt item cannot
    /// silently shift later input ordering.
    pub fn drain_inputs(&self, id: &str) -> Result<Vec<SessionInput>> {
        validate_id(id)?;
        let _lock = Lock::take(&self.dir)?;
        let dir = self.input_dir(id);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut paths = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect::<Vec<_>>();
        paths.sort();
        let mut values = Vec::with_capacity(paths.len());
        for path in &paths {
            let bytes = fs::read(path)?;
            values.push(serde_json::from_slice::<SessionInput>(&bytes)?);
        }
        for path in paths {
            let _ = fs::remove_file(path);
        }
        Ok(values)
    }

    pub fn prune(&self, keep_since: u64) -> Result<usize> {
        fs::create_dir_all(&self.dir)?;
        let _lock = Lock::take(&self.dir)?;
        let mut removed = 0;
        let entries = fs::read_dir(&self.dir)?;
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name == ".lock" || !name.ends_with(".json") {
                continue;
            }
            let Some(record) = read_record(&entry.path())? else {
                continue;
            };
            if record.lifecycle.is_live() || record.last_seen >= keep_since {
                continue;
            }
            if let Some(lease) = &record.name_lease {
                let _ = crate::names().release(lease);
            }
            let _ = fs::remove_file(entry.path());
            let _ = fs::remove_dir_all(self.input_dir(&record.id));
            removed += 1;
        }
        Ok(removed)
    }

    fn update(
        &self,
        id: &str,
        update: impl FnOnce(&mut SessionRecord) -> Result<()>,
    ) -> Result<SessionRecord> {
        validate_id(id)?;
        let _lock = Lock::take(&self.dir)?;
        let Some(mut record) = read_record(&self.path(id))? else {
            return Err(Error::Corrupt(format!("unknown session `{id}`")));
        };
        update(&mut record)?;
        record.last_seen = now();
        self.write(&record)?;
        Ok(record)
    }

    fn write(&self, record: &SessionRecord) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        write_atomic(&self.path(&record.id), &serde_json::to_vec(record)?)
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{}.json", encode(id)))
    }

    fn input_dir(&self, id: &str) -> PathBuf {
        self.dir.join("input").join(encode(id))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reactivate {
    Opened(SessionRecord),
    AlreadyLive(SessionRecord),
}

/// Slugify the complete content, then hard-cut the slugified base to 24
/// characters. The collision suffix is added later and is not part of this cap.
pub fn content_slug(content: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for ch in content.chars() {
        if ch.is_ascii_alphanumeric() {
            if dash && !slug.is_empty() {
                slug.push('-');
            }
            dash = false;
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.is_empty() {
            dash = true;
        }
    }
    let mut slug: String = slug.chars().take(24).collect();
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "session".to_owned()
    } else {
        slug
    }
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err(Error::Corrupt("invalid session id".into()));
    }
    Ok(())
}

fn validate_kind(kind: &str) -> Result<()> {
    if kind.is_empty()
        || kind.len() > 64
        || !kind
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err(Error::Corrupt(format!("invalid session kind `{kind}`")));
    }
    Ok(())
}

fn read_record(path: &Path) -> Result<Option<SessionRecord>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| Error::Corrupt(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn encode(id: &str) -> String {
    let mut out = String::with_capacity(id.len() * 2);
    for byte in id.as_bytes() {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
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

    fn sessions(root: &Path) -> Sessions {
        Sessions::new(root.join("sessions"))
    }

    #[test]
    fn content_ids_cut_the_slug_at_24_and_keep_suffixes_outside_the_cap() {
        let root = tempfile::tempdir().unwrap();
        let sessions = sessions(root.path());
        let one = sessions
            .create_content(
                "bash",
                "cargo test workspace and report",
                "Goethe",
                None,
                Value::Null,
            )
            .unwrap();
        let two = sessions
            .create_content(
                "bash",
                "cargo test workspace and report",
                "Goethe",
                None,
                Value::Null,
            )
            .unwrap();
        assert_eq!(one.id, "bash:cargo-test-workspace-and");
        assert_eq!(two.id, "bash:cargo-test-workspace-and-2");
    }

    #[test]
    fn stopped_ids_remain_reserved_until_pruned() {
        let root = tempfile::tempdir().unwrap();
        let sessions = sessions(root.path());
        let first = sessions
            .create_content("ask", "Ship now?", "Goethe", None, Value::Null)
            .unwrap();
        sessions
            .finish(&first.id, SessionStatus::Completed, None)
            .unwrap();
        let second = sessions
            .create_content("ask", "Ship now?", "Goethe", None, Value::Null)
            .unwrap();
        assert_eq!(second.id, "ask:ship-now-2");
    }

    #[test]
    fn list_does_not_refresh_stopped_records_last_seen() {
        let root = tempfile::tempdir().unwrap();
        let sessions = sessions(root.path());
        let record = sessions
            .create_content("ask", "old question", "Goethe", None, Value::Null)
            .unwrap();
        let stopped = sessions
            .finish(&record.id, SessionStatus::Completed, None)
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let listed = sessions.list().unwrap();
        let listed = listed.iter().find(|item| item.id == record.id).unwrap();
        assert_eq!(listed.last_seen, stopped.last_seen);
        assert_eq!(sessions.prune(stopped.last_seen + 1).unwrap(), 1);
    }

    #[test]
    fn prune_does_not_refresh_a_stopped_records_last_seen() {
        let root = tempfile::tempdir().unwrap();
        let sessions = sessions(root.path());
        let record = sessions
            .create_content("bash", "old command", "Goethe", None, Value::Null)
            .unwrap();
        let stopped = sessions
            .finish(&record.id, SessionStatus::Completed, None)
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert_eq!(sessions.prune(stopped.last_seen + 1).unwrap(), 1);
        assert!(!sessions.path(&record.id).exists());
    }

    #[test]
    fn canvas_identity_reactivates_in_place_but_does_not_steal_a_live_owner() {
        let root = tempfile::tempdir().unwrap();
        let sessions = sessions(root.path());
        let opened = sessions
            .reactivate("canvas:dash", "canvas", "Goethe", None, Value::Null)
            .unwrap();
        assert!(matches!(opened, Reactivate::Opened(_)));
        let live = sessions
            .reactivate("canvas:dash", "canvas", "Monet", None, Value::Null)
            .unwrap();
        assert!(matches!(live, Reactivate::AlreadyLive(_)));
        sessions
            .finish("canvas:dash", SessionStatus::Cancelled, None)
            .unwrap();
        let reopened = sessions
            .reactivate("canvas:dash", "canvas", "Monet", None, Value::Null)
            .unwrap();
        let Reactivate::Opened(record) = reopened else {
            panic!("not reopened")
        };
        assert_eq!(record.id, "canvas:dash");
        assert_eq!(record.artist, "Monet");
    }

    #[test]
    fn inputs_are_durable_and_ordered() {
        let root = tempfile::tempdir().unwrap();
        let sessions = sessions(root.path());
        sessions
            .create_exact("Goethe", "subagent", "Monet", None, Value::Null)
            .unwrap();
        sessions
            .enqueue_input("Goethe", serde_json::json!("one"))
            .unwrap();
        sessions
            .enqueue_input("Goethe", serde_json::json!({"two": 2}))
            .unwrap();
        let drained = sessions.drain_inputs("Goethe").unwrap();
        assert_eq!(drained[0].input, serde_json::json!("one"));
        assert_eq!(drained[1].input, serde_json::json!({"two": 2}));
        assert!(sessions.drain_inputs("Goethe").unwrap().is_empty());
    }
}
