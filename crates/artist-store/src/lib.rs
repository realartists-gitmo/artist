//! Append-only storage for canonical session records.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use artist_core::{InitialContext, SessionId, SessionRecord, TranscriptEntry};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
    sync::RwLock,
};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("session already exists: {0}")]
    Exists(SessionId),
    #[error("session not found: {0}")]
    NotFound(SessionId),
    #[error("session record is invalid: {0}")]
    Invalid(#[from] artist_core::RecordError),
    #[error("storage I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("stored data is invalid: {0}")]
    Data(#[from] serde_json::Error),
}

#[async_trait]
pub trait SessionStore: Send + Sync + 'static {
    async fn create(&self, record: SessionRecord) -> Result<(), StoreError>;
    async fn append(
        &self,
        session_id: &SessionId,
        entries: &[TranscriptEntry],
    ) -> Result<(), StoreError>;
    async fn load(&self, session_id: &SessionId) -> Result<SessionRecord, StoreError>;
}

#[derive(Clone, Default)]
pub struct MemoryStore(Arc<RwLock<HashMap<SessionId, SessionRecord>>>);

#[async_trait]
impl SessionStore for MemoryStore {
    async fn create(&self, record: SessionRecord) -> Result<(), StoreError> {
        record.validate()?;
        let id = record.session_id.clone();
        if self.0.write().await.insert(id.clone(), record).is_some() {
            return Err(StoreError::Exists(id));
        }
        Ok(())
    }

    async fn append(
        &self,
        session_id: &SessionId,
        entries: &[TranscriptEntry],
    ) -> Result<(), StoreError> {
        let mut sessions = self.0.write().await;
        let record = sessions
            .get(session_id)
            .ok_or_else(|| StoreError::NotFound(session_id.clone()))?;
        let mut candidate = record.clone();
        for entry in entries {
            candidate.append(entry.clone())?;
        }
        sessions.insert(session_id.clone(), candidate);
        Ok(())
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
}

impl FileStore {
    pub async fn new(directory: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let directory = directory.into();
        fs::create_dir_all(&directory).await?;
        Ok(Self { directory })
    }

    fn path(&self, id: &SessionId) -> PathBuf {
        self.directory.join(format!("{}.jsonl", hex(id.as_str())))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum StoredLine {
    Header {
        version: u32,
        session_id: SessionId,
        initial_context: InitialContext,
    },
    Entry(TranscriptEntry),
}

#[async_trait]
impl SessionStore for FileStore {
    async fn create(&self, record: SessionRecord) -> Result<(), StoreError> {
        record.validate()?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(self.path(&record.session_id))
            .await
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    StoreError::Exists(record.session_id.clone())
                } else {
                    StoreError::Io(error)
                }
            })?;
        write_line(
            &mut file,
            &StoredLine::Header {
                version: record.version,
                session_id: record.session_id.clone(),
                initial_context: record.initial_context.clone(),
            },
        )
        .await?;
        for entry in record.entries {
            write_line(&mut file, &StoredLine::Entry(entry)).await?;
        }
        file.sync_data().await?;
        Ok(())
    }

    async fn append(
        &self,
        session_id: &SessionId,
        entries: &[TranscriptEntry],
    ) -> Result<(), StoreError> {
        let current = self.load(session_id).await?;
        let mut candidate = current;
        for entry in entries {
            candidate.append(entry.clone())?;
        }

        let mut file = OpenOptions::new()
            .append(true)
            .open(self.path(session_id))
            .await?;
        for entry in entries {
            write_line(&mut file, &StoredLine::Entry(entry.clone())).await?;
        }
        file.sync_data().await?;
        Ok(())
    }

    async fn load(&self, session_id: &SessionId) -> Result<SessionRecord, StoreError> {
        let data = fs::read_to_string(self.path(session_id))
            .await
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    StoreError::NotFound(session_id.clone())
                } else {
                    StoreError::Io(error)
                }
            })?;
        let mut lines = data.lines();
        let Some(first) = lines.next() else {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "empty session record",
            )));
        };
        let StoredLine::Header {
            version,
            session_id: stored_id,
            initial_context,
        } = serde_json::from_str(first)?
        else {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "missing session header",
            )));
        };
        if &stored_id != session_id {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "session ID mismatch",
            )));
        }
        let mut record = SessionRecord {
            version,
            session_id: stored_id,
            initial_context,
            entries: vec![],
        };
        for line in lines {
            let StoredLine::Entry(entry) = serde_json::from_str(line)? else {
                return Err(StoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "duplicate session header",
                )));
            };
            record.entries.push(entry);
        }
        record.validate()?;
        Ok(record)
    }
}

async fn write_line(file: &mut tokio::fs::File, value: &StoredLine) -> Result<(), StoreError> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    file.write_all(&line).await?;
    Ok(())
}

fn hex(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_core::{MessageId, RECORD_VERSION, TranscriptEntryKind};

    async fn round_trip(store: impl SessionStore) {
        let id = SessionId::from("a/session");
        let mut record = SessionRecord::new(id.clone(), InitialContext { fragments: vec![] });
        let entry = record.entry(TranscriptEntryKind::Input {
            message_id: MessageId::from("message"),
            source: artist_core::Source::User,
            content: "hello".into(),
        });
        store.create(record.clone()).await.unwrap();
        store
            .append(&id, std::slice::from_ref(&entry))
            .await
            .unwrap();
        record.append(entry).unwrap();
        assert_eq!(store.load(&id).await.unwrap(), record);
    }

    #[tokio::test]
    async fn memory_round_trip() {
        round_trip(MemoryStore::default()).await;
    }

    #[tokio::test]
    async fn file_round_trip() {
        let root = std::env::temp_dir().join(format!("artist-store-test-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).await.unwrap();
        }
        round_trip(FileStore::new(&root).await.unwrap()).await;
        fs::remove_dir_all(root).await.unwrap();
    }

    #[test]
    fn version_is_current() {
        assert_eq!(RECORD_VERSION, 1);
    }
}
