//! Append-only storage for validated canonical session records.

use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use artist_core::{InitialContext, RECORD_VERSION, SessionId, SessionRecord, TranscriptEntry};
use async_trait::async_trait;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const FILE_FORMAT_VERSION: u32 = 2;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("session already exists: {0}")]
    Exists(SessionId),
    #[error("session not found: {0}")]
    NotFound(SessionId),
    #[error("append conflict: expected sequence {expected}, durable sequence is {actual}")]
    Conflict { expected: u64, actual: u64 },
    #[error("session record is invalid: {0}")]
    Invalid(#[from] artist_core::RecordError),
    #[error("stored session is corrupt: {0}")]
    Corrupt(String),
    #[error("storage I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("stored data is invalid: {0}")]
    Data(#[from] serde_json::Error),
    #[error("storage task failed: {0}")]
    Task(String),
}

#[async_trait]
pub trait SessionStore: Send + Sync + 'static {
    async fn create(&self, record: SessionRecord) -> Result<(), StoreError>;
    async fn append(
        &self,
        session_id: &SessionId,
        expected_sequence: u64,
        entries: &[TranscriptEntry],
    ) -> Result<u64, StoreError>;
    async fn load(&self, session_id: &SessionId) -> Result<SessionRecord, StoreError>;
}

#[derive(Clone, Default)]
pub struct MemoryStore(Arc<tokio::sync::RwLock<HashMap<SessionId, SessionRecord>>>);

#[async_trait]
impl SessionStore for MemoryStore {
    async fn create(&self, record: SessionRecord) -> Result<(), StoreError> {
        record.validate()?;
        let id = record.session_id().clone();
        let mut sessions = self.0.write().await;
        if sessions.contains_key(&id) {
            return Err(StoreError::Exists(id));
        }
        sessions.insert(id, record);
        Ok(())
    }

    async fn append(
        &self,
        session_id: &SessionId,
        expected_sequence: u64,
        entries: &[TranscriptEntry],
    ) -> Result<u64, StoreError> {
        let mut sessions = self.0.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or_else(|| StoreError::NotFound(session_id.clone()))?;
        if record.next_sequence() != expected_sequence {
            return Err(StoreError::Conflict {
                expected: expected_sequence,
                actual: record.next_sequence(),
            });
        }
        record.append_batch(entries)?;
        Ok(record.next_sequence())
    }

    async fn load(&self, session_id: &SessionId) -> Result<SessionRecord, StoreError> {
        self.0
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(session_id.clone()))
    }
}

#[derive(Clone)]
pub struct FileStore {
    directory: PathBuf,
    cache: Arc<Mutex<HashMap<SessionId, Cached>>>,
    disk_replays: Arc<AtomicU64>,
}

#[derive(Clone)]
struct Cached {
    file_len: u64,
    valid_len: u64,
    record: SessionRecord,
    last_hash: String,
    legacy: bool,
}

impl FileStore {
    pub async fn new(directory: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let directory = directory.into();
        tokio::fs::create_dir_all(&directory).await?;
        Ok(Self {
            directory,
            cache: Arc::default(),
            disk_replays: Arc::default(),
        })
    }

    fn path(directory: &Path, id: &SessionId) -> PathBuf {
        directory.join(format!("{}.jsonl", hex(id.as_str().as_bytes())))
    }

    fn lock_path(directory: &Path, id: &SessionId) -> PathBuf {
        directory.join(format!("{}.lock", hex(id.as_str().as_bytes())))
    }

    async fn blocking<T: Send + 'static>(
        operation: impl FnOnce() -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, StoreError> {
        tokio::task::spawn_blocking(operation)
            .await
            .map_err(|error| StoreError::Task(error.to_string()))?
    }
}

#[async_trait]
impl SessionStore for FileStore {
    async fn create(&self, record: SessionRecord) -> Result<(), StoreError> {
        record.validate()?;
        let directory = self.directory.clone();
        let cache = self.cache.clone();
        Self::blocking(move || {
            let id = record.session_id().clone();
            let _lock = lock(&directory, &id)?;
            let path = Self::path(&directory, &id);
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        StoreError::Exists(id.clone())
                    } else {
                        StoreError::Io(error)
                    }
                })?;
            let (bytes, last_hash) = encode_full(&record)?;
            file.write_all(&bytes)?;
            file.sync_data()?;
            sync_directory(&directory)?;
            let file_len = bytes.len() as u64;
            cache.lock().unwrap().insert(
                id,
                Cached {
                    file_len,
                    valid_len: file_len,
                    record,
                    last_hash,
                    legacy: false,
                },
            );
            Ok(())
        })
        .await
    }

    async fn append(
        &self,
        session_id: &SessionId,
        expected_sequence: u64,
        entries: &[TranscriptEntry],
    ) -> Result<u64, StoreError> {
        let directory = self.directory.clone();
        let cache = self.cache.clone();
        let disk_replays = self.disk_replays.clone();
        let id = session_id.clone();
        let entries = entries.to_vec();
        Self::blocking(move || {
            let _lock = lock(&directory, &id)?;
            let path = Self::path(&directory, &id);
            let metadata = std::fs::metadata(&path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    StoreError::NotFound(id.clone())
                } else {
                    StoreError::Io(error)
                }
            })?;
            let mut loaded = {
                let mut cache = cache.lock().unwrap();
                match cache.remove(&id) {
                    Some(cached) if cached.file_len == metadata.len() => cached,
                    _ => {
                        disk_replays.fetch_add(1, Ordering::Relaxed);
                        decode_path(&path, &id)?
                    }
                }
            };
            if loaded.record.next_sequence() != expected_sequence {
                let actual = loaded.record.next_sequence();
                cache.lock().unwrap().insert(id, loaded);
                return Err(StoreError::Conflict {
                    expected: expected_sequence,
                    actual,
                });
            }
            if let Err(error) = loaded.record.append_batch(&entries) {
                cache.lock().unwrap().insert(id, loaded);
                return Err(error.into());
            }
            if entries.is_empty() {
                let next = loaded.record.next_sequence();
                cache.lock().unwrap().insert(id, loaded);
                return Ok(next);
            }

            if loaded.legacy {
                let (bytes, last_hash) = encode_full(&loaded.record)?;
                replace_atomically(&directory, &path, &bytes)?;
                loaded.file_len = bytes.len() as u64;
                loaded.valid_len = loaded.file_len;
                loaded.last_hash = last_hash;
                loaded.legacy = false;
            } else {
                let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
                if loaded.valid_len != loaded.file_len {
                    file.set_len(loaded.valid_len)?;
                }
                file.seek(SeekFrom::Start(loaded.valid_len))?;
                let payload = BatchPayload {
                    expected_sequence,
                    entries: entries.clone(),
                    previous_hash: loaded.last_hash.clone(),
                };
                let hash = hash(&payload)?;
                let frame = Frame::Batch {
                    expected_sequence: payload.expected_sequence,
                    entries: payload.entries,
                    previous_hash: payload.previous_hash,
                    hash: hash.clone(),
                };
                let bytes = line(&frame)?;
                file.write_all(&bytes)?;
                file.sync_data()?;
                loaded.file_len = loaded.valid_len + bytes.len() as u64;
                loaded.valid_len = loaded.file_len;
                loaded.last_hash = hash;
            }
            let next = loaded.record.next_sequence();
            cache.lock().unwrap().insert(id, loaded);
            Ok(next)
        })
        .await
    }

    async fn load(&self, session_id: &SessionId) -> Result<SessionRecord, StoreError> {
        let directory = self.directory.clone();
        let cache = self.cache.clone();
        let disk_replays = self.disk_replays.clone();
        let id = session_id.clone();
        Self::blocking(move || {
            let _lock = lock(&directory, &id)?;
            let path = Self::path(&directory, &id);
            let metadata = std::fs::metadata(&path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    StoreError::NotFound(id.clone())
                } else {
                    StoreError::Io(error)
                }
            })?;
            let mut cache_guard = cache.lock().unwrap();
            if let Some(cached) = cache_guard.get(&id)
                && cached.file_len == metadata.len()
            {
                return Ok(cached.record.clone());
            }
            disk_replays.fetch_add(1, Ordering::Relaxed);
            let loaded = decode_path(&path, &id)?;
            let record = loaded.record.clone();
            cache_guard.insert(id, loaded);
            Ok(record)
        })
        .await
    }
}

fn lock(directory: &Path, id: &SessionId) -> Result<File, StoreError> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(FileStore::lock_path(directory, id))?;
    file.lock_exclusive()?;
    Ok(file)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
enum Frame {
    Header {
        format_version: u32,
        record_version: u32,
        session_id: SessionId,
        initial_context: InitialContext,
        hash: String,
    },
    Batch {
        expected_sequence: u64,
        entries: Vec<TranscriptEntry>,
        previous_hash: String,
        hash: String,
    },
}

#[derive(Serialize)]
struct HeaderPayload {
    format_version: u32,
    record_version: u32,
    session_id: SessionId,
    initial_context: InitialContext,
}

#[derive(Serialize)]
struct BatchPayload {
    expected_sequence: u64,
    entries: Vec<TranscriptEntry>,
    previous_hash: String,
}

fn encode_full(record: &SessionRecord) -> Result<(Vec<u8>, String), StoreError> {
    let header = HeaderPayload {
        format_version: FILE_FORMAT_VERSION,
        record_version: RECORD_VERSION,
        session_id: record.session_id().clone(),
        initial_context: record.initial_context().clone(),
    };
    let header_hash = hash(&header)?;
    let mut bytes = line(&Frame::Header {
        format_version: header.format_version,
        record_version: header.record_version,
        session_id: header.session_id,
        initial_context: header.initial_context,
        hash: header_hash.clone(),
    })?;
    if record.entries().is_empty() {
        return Ok((bytes, header_hash));
    }
    let payload = BatchPayload {
        expected_sequence: 0,
        entries: record.entries().to_vec(),
        previous_hash: header_hash,
    };
    let batch_hash = hash(&payload)?;
    bytes.extend(line(&Frame::Batch {
        expected_sequence: payload.expected_sequence,
        entries: payload.entries,
        previous_hash: payload.previous_hash,
        hash: batch_hash.clone(),
    })?);
    Ok((bytes, batch_hash))
}

fn decode_path(path: &Path, expected_id: &SessionId) -> Result<Cached, StoreError> {
    let mut file = OpenOptions::new().read(true).open(path)?;
    let file_len = file.metadata()?.len();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let valid_len = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    if valid_len == 0 {
        return Err(StoreError::Corrupt("missing complete header frame".into()));
    }
    let complete = &bytes[..valid_len];
    let first = complete
        .split(|byte| *byte == b'\n')
        .find(|line| !line.is_empty())
        .ok_or_else(|| StoreError::Corrupt("missing header frame".into()))?;
    let marker: serde_json::Value = serde_json::from_slice(first)?;
    if marker.get("frame").is_some() {
        decode_v2(complete, file_len, valid_len as u64, expected_id)
    } else if marker.get("record").is_some() {
        decode_v1(complete, file_len, valid_len as u64, expected_id)
    } else {
        Err(StoreError::Corrupt("unknown file framing".into()))
    }
}

fn decode_v2(
    bytes: &[u8],
    file_len: u64,
    valid_len: u64,
    expected_id: &SessionId,
) -> Result<Cached, StoreError> {
    let mut lines = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty());
    let Some(Frame::Header {
        format_version,
        record_version,
        session_id,
        initial_context,
        hash: stored_hash,
    }) = lines.next().map(serde_json::from_slice).transpose()?
    else {
        return Err(StoreError::Corrupt("first frame is not a header".into()));
    };
    if format_version != FILE_FORMAT_VERSION || record_version != RECORD_VERSION {
        return Err(StoreError::Corrupt(format!(
            "unsupported file/record version {format_version}/{record_version}"
        )));
    }
    if &session_id != expected_id {
        return Err(StoreError::Corrupt("session ID mismatch".into()));
    }
    let expected_hash = hash(&HeaderPayload {
        format_version,
        record_version,
        session_id: session_id.clone(),
        initial_context: initial_context.clone(),
    })?;
    if stored_hash != expected_hash {
        return Err(StoreError::Corrupt("header hash mismatch".into()));
    }
    let mut record = SessionRecord::new(session_id, initial_context);
    let mut last_hash = stored_hash;
    for bytes in lines {
        let Frame::Batch {
            expected_sequence,
            entries,
            previous_hash,
            hash: stored_hash,
        } = serde_json::from_slice(bytes)?
        else {
            return Err(StoreError::Corrupt("header appears after log start".into()));
        };
        if previous_hash != last_hash {
            return Err(StoreError::Corrupt("broken frame hash chain".into()));
        }
        let expected_hash = hash(&BatchPayload {
            expected_sequence,
            entries: entries.clone(),
            previous_hash: previous_hash.clone(),
        })?;
        if stored_hash != expected_hash {
            return Err(StoreError::Corrupt("batch hash mismatch".into()));
        }
        if record.next_sequence() != expected_sequence {
            return Err(StoreError::Corrupt(format!(
                "batch expected {expected_sequence}, preceding tail is {}",
                record.next_sequence()
            )));
        }
        record.append_batch(&entries)?;
        last_hash = stored_hash;
    }
    Ok(Cached {
        file_len,
        valid_len,
        record,
        last_hash,
        legacy: false,
    })
}

fn decode_v1(
    bytes: &[u8],
    file_len: u64,
    valid_len: u64,
    expected_id: &SessionId,
) -> Result<Cached, StoreError> {
    let mut lines = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty());
    let mut header: serde_json::Value = serde_json::from_slice(
        lines
            .next()
            .ok_or_else(|| StoreError::Corrupt("missing legacy header".into()))?,
    )?;
    let object = header
        .as_object_mut()
        .ok_or_else(|| StoreError::Corrupt("legacy header is not an object".into()))?;
    if object
        .remove("record")
        .and_then(|value| value.as_str().map(str::to_owned))
        .as_deref()
        != Some("header")
    {
        return Err(StoreError::Corrupt("invalid legacy header".into()));
    }
    let version = object
        .remove("version")
        .ok_or_else(|| StoreError::Corrupt("legacy header has no version".into()))?;
    let session_id = object
        .remove("session_id")
        .ok_or_else(|| StoreError::Corrupt("legacy header has no session ID".into()))?;
    if serde_json::from_value::<SessionId>(session_id.clone())? != *expected_id {
        return Err(StoreError::Corrupt("session ID mismatch".into()));
    }
    let initial_context = object
        .remove("initial_context")
        .ok_or_else(|| StoreError::Corrupt("legacy header has no context".into()))?;
    let mut entries = Vec::new();
    for bytes in lines {
        let mut entry: serde_json::Value = serde_json::from_slice(bytes)?;
        let object = entry
            .as_object_mut()
            .ok_or_else(|| StoreError::Corrupt("legacy entry is not an object".into()))?;
        if object
            .remove("record")
            .and_then(|value| value.as_str().map(str::to_owned))
            .as_deref()
            != Some("entry")
        {
            return Err(StoreError::Corrupt("invalid legacy entry frame".into()));
        }
        entries.push(entry);
    }
    let record: SessionRecord = serde_json::from_value(serde_json::json!({
        "version": version,
        "session_id": session_id,
        "initial_context": initial_context,
        "entries": entries,
    }))?;
    Ok(Cached {
        file_len,
        valid_len,
        record,
        last_hash: String::new(),
        legacy: true,
    })
}

fn replace_atomically(directory: &Path, path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_data()?;
    temporary
        .persist(path)
        .map_err(|error| StoreError::Io(error.error))?;
    sync_directory(directory)?;
    Ok(())
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<(), StoreError> {
    File::open(directory)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_: &Path) -> Result<(), StoreError> {
    Ok(())
}

fn line(value: &impl Serialize) -> Result<Vec<u8>, StoreError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn hash(value: &impl Serialize) -> Result<String, StoreError> {
    Ok(hex(&Sha256::digest(serde_json::to_vec(value)?)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_core::{MessageId, RunId, Source, TranscriptEntryKind};

    fn empty(id: &str) -> SessionRecord {
        SessionRecord::new(SessionId::from(id), InitialContext { fragments: vec![] })
    }

    fn input(record: &SessionRecord, id: &str) -> TranscriptEntry {
        record.entry(TranscriptEntryKind::Input {
            message_id: MessageId::from(id),
            source: Source::User,
            content: "hello".into(),
        })
    }

    async fn round_trip(store: impl SessionStore) {
        let id = SessionId::from("a/session");
        let initial = empty("a/session");
        store.create(initial.clone()).await.unwrap();
        let entries = initial.entries_for([
            TranscriptEntryKind::Input {
                message_id: MessageId::from("input"),
                source: Source::User,
                content: "hello".into(),
            },
            TranscriptEntryKind::RunStarted {
                run_id: RunId::from("run"),
                input_id: MessageId::from("input"),
            },
        ]);
        assert_eq!(store.append(&id, 0, &entries).await.unwrap(), 2);
        let mut expected = initial;
        expected.append_batch(&entries).unwrap();
        assert_eq!(store.load(&id).await.unwrap(), expected);
    }

    #[tokio::test]
    async fn memory_round_trip() {
        round_trip(MemoryStore::default()).await;
    }

    #[tokio::test]
    async fn file_round_trip() {
        let temporary = tempfile::tempdir().unwrap();
        round_trip(FileStore::new(temporary.path()).await.unwrap()).await;
    }

    #[tokio::test]
    async fn expected_sequence_rejects_competing_memory_and_file_writers() {
        let memory = MemoryStore::default();
        let record = empty("memory-race");
        memory.create(record.clone()).await.unwrap();
        let first = input(&record, "first");
        let competing = input(&record, "competing");
        memory
            .append(record.session_id(), 0, &[first])
            .await
            .unwrap();
        assert!(matches!(
            memory.append(record.session_id(), 0, &[competing]).await,
            Err(StoreError::Conflict {
                expected: 0,
                actual: 1
            })
        ));

        let temporary = tempfile::tempdir().unwrap();
        let one = FileStore::new(temporary.path()).await.unwrap();
        let two = FileStore::new(temporary.path()).await.unwrap();
        let record = empty("file-race");
        one.create(record.clone()).await.unwrap();
        let first = input(&record, "first");
        let competing = input(&record, "competing");
        let first_batch = [first];
        let competing_batch = [competing];
        let (left, right) = tokio::join!(
            one.append(record.session_id(), 0, &first_batch),
            two.append(record.session_id(), 0, &competing_batch),
        );
        assert!(matches!(
            (&left, &right),
            (
                Ok(1),
                Err(StoreError::Conflict {
                    expected: 0,
                    actual: 1
                })
            ) | (
                Err(StoreError::Conflict {
                    expected: 0,
                    actual: 1
                }),
                Ok(1)
            )
        ));
    }

    #[tokio::test]
    async fn torn_tail_is_ignored_then_truncated_by_the_next_append() {
        let temporary = tempfile::tempdir().unwrap();
        let store = FileStore::new(temporary.path()).await.unwrap();
        let record = empty("torn");
        store.create(record.clone()).await.unwrap();
        store
            .append(record.session_id(), 0, &[input(&record, "first")])
            .await
            .unwrap();
        let path = FileStore::path(temporary.path(), record.session_id());
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(br#"{"frame":"batch""#)
            .unwrap();
        let recovered = FileStore::new(temporary.path()).await.unwrap();
        let loaded = recovered.load(record.session_id()).await.unwrap();
        assert_eq!(loaded.next_sequence(), 1);
        recovered
            .append(record.session_id(), 1, &[input(&loaded, "second")])
            .await
            .unwrap();
        assert_eq!(
            recovered
                .load(record.session_id())
                .await
                .unwrap()
                .next_sequence(),
            2
        );
    }

    #[tokio::test]
    async fn completed_corrupt_frame_is_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let store = FileStore::new(temporary.path()).await.unwrap();
        let record = empty("corrupt");
        store.create(record.clone()).await.unwrap();
        let path = FileStore::path(temporary.path(), record.session_id());
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(b"{not-json}\n")
            .unwrap();
        let fresh = FileStore::new(temporary.path()).await.unwrap();
        assert!(fresh.load(record.session_id()).await.is_err());
    }

    #[tokio::test]
    async fn a_torn_batch_is_entirely_invisible() {
        let temporary = tempfile::tempdir().unwrap();
        let store = FileStore::new(temporary.path()).await.unwrap();
        let record = empty("atomic-batch");
        store.create(record.clone()).await.unwrap();
        let batch = record.entries_for([
            TranscriptEntryKind::Input {
                message_id: MessageId::from("input"),
                source: Source::User,
                content: "hello".into(),
            },
            TranscriptEntryKind::RunStarted {
                run_id: RunId::from("run"),
                input_id: MessageId::from("input"),
            },
        ]);
        store.append(record.session_id(), 0, &batch).await.unwrap();
        let path = FileStore::path(temporary.path(), record.session_id());
        let mut bytes = std::fs::read(&path).unwrap();
        let header_end = bytes.iter().position(|byte| *byte == b'\n').unwrap() + 1;
        bytes.truncate(header_end + (bytes.len() - header_end) / 2);
        std::fs::write(&path, bytes).unwrap();
        let recovered = FileStore::new(temporary.path()).await.unwrap();
        assert_eq!(
            recovered
                .load(record.session_id())
                .await
                .unwrap()
                .next_sequence(),
            0
        );
    }

    #[tokio::test]
    async fn hash_tampering_and_broken_chains_are_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let store = FileStore::new(temporary.path()).await.unwrap();
        let record = empty("hashes");
        store.create(record.clone()).await.unwrap();
        store
            .append(record.session_id(), 0, &[input(&record, "first")])
            .await
            .unwrap();
        let once = store.load(record.session_id()).await.unwrap();
        store
            .append(record.session_id(), 1, &[input(&once, "second")])
            .await
            .unwrap();
        let path = FileStore::path(temporary.path(), record.session_id());
        let original = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<_> = original.lines().map(str::to_owned).collect();
        let mut first_batch: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
        first_batch["entries"][0]["kind"]["content"] = "tampered".into();
        lines[1] = serde_json::to_string(&first_batch).unwrap();
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
        let fresh = FileStore::new(temporary.path()).await.unwrap();
        assert!(matches!(
            fresh.load(record.session_id()).await,
            Err(StoreError::Corrupt(message)) if message.contains("hash mismatch")
        ));

        let lines: Vec<_> = original.lines().collect();
        std::fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        let fresh = FileStore::new(temporary.path()).await.unwrap();
        assert!(matches!(
            fresh.load(record.session_id()).await,
            Err(StoreError::Corrupt(message)) if message.contains("hash chain")
        ));
    }

    #[tokio::test]
    async fn cached_hot_appends_do_not_replay_the_file() {
        let temporary = tempfile::tempdir().unwrap();
        let store = FileStore::new(temporary.path()).await.unwrap();
        let mut record = empty("hot-path");
        store.create(record.clone()).await.unwrap();
        for index in 0..1_000 {
            let entry = input(&record, &format!("input-{index}"));
            store
                .append(
                    record.session_id(),
                    record.next_sequence(),
                    std::slice::from_ref(&entry),
                )
                .await
                .unwrap();
            record.append(entry).unwrap();
        }
        assert_eq!(store.disk_replays.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn migrates_legacy_jsonl_on_append() {
        let temporary = tempfile::tempdir().unwrap();
        let id = SessionId::from("legacy-file");
        let path = FileStore::path(temporary.path(), &id);
        let lines = [
            serde_json::json!({"record":"header","version":1,"session_id":"legacy-file","initial_context":{"fragments":[]}}),
            serde_json::json!({"record":"entry","event_id":"legacy-file:event:0","sequence":0,"kind":{"entry":"input","message_id":"input","source":"user","content":"hello"}}),
        ];
        let bytes: Vec<u8> = lines
            .into_iter()
            .flat_map(|value| line(&value).unwrap())
            .collect();
        std::fs::write(&path, bytes).unwrap();
        let store = FileStore::new(temporary.path()).await.unwrap();
        let migrated = store.load(&id).await.unwrap();
        assert_eq!(migrated.version(), RECORD_VERSION);
        store
            .append(&id, 1, &[input(&migrated, "second")])
            .await
            .unwrap();
        let first: serde_json::Value = serde_json::from_slice(
            std::fs::read(path)
                .unwrap()
                .split(|byte| *byte == b'\n')
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(first["frame"], "header");
    }
}
