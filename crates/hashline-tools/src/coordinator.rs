//! Multi-agent file coordinator: path locks, whole-file BLAKE3 hashes,
//! per-agent `FileToolManager` instances, and SQLite anchor persistence.

use std::{
    collections::HashMap,
    fs::OpenOptions as StdOpenOptions,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use tokio::{
    fs,
    io::AsyncWriteExt,
    sync::{Mutex, OwnedMutexGuard},
};
use uuid::Uuid;

use crate::{
    AgentIdentity, EditRequest, EditResult, FileToolConfig, FileToolManager, HashlineError,
    HashlineErrorCode, ReadFileRequest, ReadFileResult, StateStore,
};

/// Conditions for whole-file write (separate from line-level edit anchors).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WriteCondition {
    /// Create only if the path does not exist.
    #[default]
    Absent,
    /// Replace only if the current whole-file BLAKE3 matches.
    ContentHash { hash: String },
    /// Unconditional create-or-replace.
    Any,
}

#[derive(Debug)]
pub struct CoordinatedReadResult {
    pub result: ReadFileResult,
    pub content_hash: String,
}

#[derive(Debug)]
pub struct CoordinatedEditResult {
    pub result: EditResult,
    pub content_hash: String,
}

/// Coordinates concurrent multi-agent file access.
///
/// - One in-memory [`FileToolManager`] per agent (lazy, restored from SQLite).
/// - Cross-process exclusive locks per normalized path under `lock_directory`.
/// - Persists issued mnemonic bindings after every successful read/write/edit.
#[derive(Clone)]
pub struct FileCoordinator {
    config: FileToolConfig,
    state: StateStore,
    managers: Arc<Mutex<HashMap<String, Arc<Mutex<FileToolManager>>>>>,
    path_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    lock_directory: Arc<PathBuf>,
}

struct PathTransaction {
    _local_guard: OwnedMutexGuard<()>,
    lock_file: std::fs::File,
}

impl Drop for PathTransaction {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.lock_file);
    }
}

impl FileCoordinator {
    pub fn new(config: FileToolConfig, state: StateStore, lock_directory: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&lock_directory).with_context(|| {
            format!(
                "failed to create lock directory {}",
                lock_directory.display()
            )
        })?;
        Ok(Self {
            config,
            state,
            managers: Arc::new(Mutex::new(HashMap::new())),
            path_locks: Arc::new(Mutex::new(HashMap::new())),
            lock_directory: Arc::new(lock_directory),
        })
    }

    /// Convenience constructor: open (or create) SQLite at `db_path` and use
    /// `lock_directory` for cross-process file locks.
    pub fn open(
        config: FileToolConfig,
        db_path: impl AsRef<Path>,
        lock_directory: impl AsRef<Path>,
    ) -> Result<Self> {
        let state = StateStore::open(db_path.as_ref()).map_err(anyhow::Error::msg)?;
        Self::new(config, state, lock_directory.as_ref().to_path_buf())
    }

    pub fn state(&self) -> &StateStore {
        &self.state
    }

    pub async fn read_file(
        &self,
        actor: &AgentIdentity,
        request: ReadFileRequest,
    ) -> Result<CoordinatedReadResult> {
        let _ = self.state.register_agent(actor).await;
        let manager = self.manager_for(actor).await?;
        let mut manager = manager.lock().await;
        let result = manager.read_file(request).await?;
        let normalized = manager.normalized_path(&result.path)?;
        let bytes = fs::read(&normalized)
            .await
            .with_context(|| format!("failed to read {normalized}"))?;
        let content_hash = content_hash(&bytes);
        let state = manager.export_issued_prefixes();
        drop(manager);
        self.state
            .replace_anchor_state(&actor.id, &state)
            .await
            .map_err(anyhow::Error::msg)?;
        Ok(CoordinatedReadResult {
            result,
            content_hash,
        })
    }

    pub async fn write_file(
        &self,
        actor: &AgentIdentity,
        path: String,
        content: String,
        condition: WriteCondition,
    ) -> Result<CoordinatedReadResult> {
        let _ = self.state.register_agent(actor).await;
        let manager = self.manager_for(actor).await?;
        let normalized = {
            let manager = manager.lock().await;
            manager.normalized_path(&path)?
        };
        let _transaction = self.lock_path(&normalized).await?;
        let destination = Path::new(&normalized);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        match condition {
            WriteCondition::Absent => match atomic_create(destination, content.as_bytes()).await {
                Ok(()) => {}
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists) =>
                {
                    return Err(anyhow::Error::new(HashlineError::new(
                        HashlineErrorCode::AlreadyExists,
                        format!("file already exists: {path}"),
                        false,
                    )));
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("failed to create {path}"));
                }
            },
            WriteCondition::ContentHash { hash } => {
                let current = fs::read(destination).await.with_context(|| {
                    format!("failed to read {path} for conditional replacement")
                })?;
                let actual = content_hash(&current);
                if actual != hash.to_ascii_lowercase() {
                    bail!(
                        "content hash mismatch for {path}: expected {hash}, current hash is {actual}"
                    );
                }
                atomic_replace(destination, content.as_bytes()).await?;
            }
            WriteCondition::Any => {
                atomic_replace(destination, content.as_bytes()).await?;
            }
        }

        let mut manager = manager.lock().await;
        manager.forget_path(&path)?;
        let result = manager
            .read_file(ReadFileRequest {
                path: path.clone(),
                start_line: 1,
                max_lines: None,
            })
            .await?;
        let state = manager.export_issued_prefixes();
        drop(manager);
        self.state
            .replace_anchor_state(&actor.id, &state)
            .await
            .map_err(anyhow::Error::msg)?;
        Ok(CoordinatedReadResult {
            result,
            content_hash: content_hash(content.as_bytes()),
        })
    }

    pub async fn delete_file(
        &self,
        actor: &AgentIdentity,
        path: String,
        expected_hash: String,
    ) -> Result<Option<String>> {
        let _ = self.state.register_agent(actor).await;
        let manager = self.manager_for(actor).await?;
        let normalized = {
            let manager = manager.lock().await;
            manager.normalized_path(&path)?
        };
        let _transaction = self.lock_path(&normalized).await?;
        let destination = Path::new(&normalized);
        let metadata = match fs::metadata(destination).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect {path} for conditional deletion"));
            }
        };
        if !metadata.is_file() {
            bail!("delete only supports files: {path}");
        }
        let current = fs::read(destination)
            .await
            .with_context(|| format!("failed to read {path} for conditional deletion"))?;
        let actual = content_hash(&current);
        if actual != expected_hash.to_ascii_lowercase() {
            bail!(
                "content hash mismatch for {path}: expected {expected_hash}, current hash is {actual}"
            );
        }
        fs::remove_file(destination)
            .await
            .with_context(|| format!("failed to delete {path}"))?;
        sync_parent(destination).await?;

        let mut manager = manager.lock().await;
        manager.forget_path(&path)?;
        let state = manager.export_issued_prefixes();
        drop(manager);
        if let Err(error) = self.state.replace_anchor_state(&actor.id, &state).await {
            eprintln!("failed to persist anchor cleanup after deleting {path}: {error}");
        }
        Ok(Some(actual))
    }

    pub async fn edit_file(
        &self,
        actor: &AgentIdentity,
        request: EditRequest,
    ) -> Result<CoordinatedEditResult> {
        let _ = self.state.register_agent(actor).await;
        let manager = self.manager_for(actor).await?;
        let normalized = {
            let manager = manager.lock().await;
            manager.normalized_path(&request.path)?
        };
        let _transaction = self.lock_path(&normalized).await?;
        let mut manager = manager.lock().await;
        let result = manager.edit_file(request).await?;
        let bytes = fs::read(&normalized)
            .await
            .with_context(|| format!("failed to read {normalized} after edit"))?;
        let state = manager.export_issued_prefixes();
        drop(manager);
        self.state
            .replace_anchor_state(&actor.id, &state)
            .await
            .map_err(anyhow::Error::msg)?;
        Ok(CoordinatedEditResult {
            result,
            content_hash: content_hash(&bytes),
        })
    }

    /// Dry-run an edit batch: locks the path, resolves anchors, returns the
    /// would-be result without writing. Does not persist anchor state.
    pub async fn preview_edit_file(
        &self,
        actor: &AgentIdentity,
        request: EditRequest,
    ) -> Result<CoordinatedEditResult> {
        let _ = self.state.register_agent(actor).await;
        let manager = self.manager_for(actor).await?;
        let normalized = {
            let manager = manager.lock().await;
            manager.normalized_path(&request.path)?
        };
        let _transaction = self.lock_path(&normalized).await?;
        let manager = manager.lock().await;
        let result = manager.preview_edit_file(request).await?;
        let bytes = fs::read(&normalized)
            .await
            .with_context(|| format!("failed to read {normalized} after edit preview"))?;
        Ok(CoordinatedEditResult {
            result,
            content_hash: content_hash(&bytes),
        })
    }

    async fn manager_for(&self, actor: &AgentIdentity) -> Result<Arc<Mutex<FileToolManager>>> {
        if let Some(manager) = self.managers.lock().await.get(&actor.id.0).cloned() {
            return Ok(manager);
        }
        let persisted = self
            .state
            .load_anchor_state(&actor.id)
            .await
            .map_err(anyhow::Error::msg)?;
        let mut manager = FileToolManager::with_config(self.config.clone());
        manager.import_issued_prefixes(persisted);
        let manager = Arc::new(Mutex::new(manager));
        let mut managers = self.managers.lock().await;
        Ok(managers
            .entry(actor.id.0.clone())
            .or_insert_with(|| manager.clone())
            .clone())
    }

    async fn lock_path(&self, normalized: &str) -> Result<PathTransaction> {
        let local_lock = {
            let mut locks = self.path_locks.lock().await;
            locks
                .entry(normalized.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let local_guard = local_lock.lock_owned().await;
        let lock_name = format!("{}.lock", blake3::hash(normalized.as_bytes()).to_hex());
        let lock_path = self.lock_directory.join(lock_name);
        let lock_file = tokio::task::spawn_blocking(move || -> Result<std::fs::File> {
            let file = StdOpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)
                .with_context(|| format!("failed to open path lock {}", lock_path.display()))?;
            file.lock_exclusive()
                .with_context(|| format!("failed to lock {}", lock_path.display()))?;
            Ok(file)
        })
        .await
        .context("path lock task failed")??;
        Ok(PathTransaction {
            _local_guard: local_guard,
            lock_file,
        })
    }
}

async fn atomic_create(path: &Path, bytes: &[u8]) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .context("destination file name is not UTF-8")?;
    let temporary = path.with_file_name(format!(".{file_name}.hashline-{}.tmp", Uuid::new_v4()));
    let result = async {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
        drop(file);
        fs::hard_link(&temporary, path).await?;
        let _ = fs::remove_file(&temporary).await;
        sync_parent(path).await?;
        Ok::<(), std::io::Error>(())
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temporary).await;
    }
    result.with_context(|| format!("failed to atomically create {}", path.display()))
}

async fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .context("destination file name is not UTF-8")?;
    let temporary = path.with_file_name(format!(".{file_name}.hashline-{}.tmp", Uuid::new_v4()));
    let result = async {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
        fs::rename(&temporary, path).await?;
        sync_parent(path).await?;
        Ok::<(), std::io::Error>(())
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temporary).await;
    }
    result.with_context(|| format!("failed to atomically replace {}", path.display()))
}

async fn sync_parent(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = parent.to_owned();
    tokio::task::spawn_blocking(move || std::fs::File::open(parent)?.sync_all())
        .await
        .map_err(std::io::Error::other)?
}

/// Whole-file BLAKE3 content hash as lowercase hex.
pub fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// How models should use anchors returned in `anchor | line` views.
pub const ANCHOR_USAGE: &str = "Use only the bare mnemonic token before ' | '. For the rendered line 'time | beta', pass anchor \"time\" (not \"time | beta\").";
