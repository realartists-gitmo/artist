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
    AgentIdentity, Drift, DriftCandidate, DriftKind, EditRequest, EditResult, FileToolConfig,
    FileToolManager, HashlineError, HashlineErrorCode, ReadFileRequest, ReadFileResult, StateStore,
};

/// How long a conversation's anchor state outlives its last use.
///
/// Resuming a month-old session should still find the anchors it was given.
/// Past that the state is almost certainly dead, and losing it costs a re-read
/// rather than anything worse.
pub const ANCHOR_RETENTION_DAYS: u64 = 30;

/// How long a write stays attributable.
///
/// Far shorter than the anchor state it accompanies, because attribution is
/// only actionable while the change is recent — "another session edited this
/// last month" is not a coordination signal, it is trivia.
pub const WRITER_RETENTION_DAYS: u64 = 7;

/// The most conversations whose anchor state is kept, whatever their age.
///
/// A backstop for the case the age rule cannot reach: thousands of short-lived
/// sessions inside the retention window. Set high enough that reaching it is
/// itself the signal something is wrong.
pub const MAX_RETAINED_AGENTS: usize = 2_000;

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
    /// Whether the one-off tidy of pre-conversation anchor state has run.
    ///
    /// Once per coordinator rather than per session: the rows it removes are
    /// inert, so repeating the sweep would be work for nothing.
    tidied: Arc<Mutex<bool>>,
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
            tidied: Arc::new(Mutex::new(false)),
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
        let state = manager.export_anchor_state();
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
        let state = manager.export_anchor_state();
        drop(manager);
        let hash = content_hash(content.as_bytes());
        self.note_writer(&normalized, actor, &hash).await;
        self.state
            .replace_anchor_state(&actor.id, &state)
            .await
            .map_err(anyhow::Error::msg)?;
        Ok(CoordinatedReadResult {
            result,
            content_hash: hash,
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
        let state = manager.export_anchor_state();
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
        let state = manager.export_anchor_state();
        drop(manager);
        let hash = content_hash(&bytes);
        self.note_writer(&normalized, actor, &hash).await;
        self.state
            .replace_anchor_state(&actor.id, &state)
            .await
            .map_err(anyhow::Error::msg)?;
        Ok(CoordinatedEditResult {
            result,
            content_hash: hash,
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

    /// Files this actor has been shown that no longer match disk.
    ///
    /// Runs after every tool call, so the shape matters more than it looks.
    /// Three phases, and the middle one is the reason:
    ///
    /// 1. take the candidate list under the manager lock — no I/O,
    /// 2. stat them with the lock *released*, because this is the phase that
    ///    runs every time and touches every tracked file,
    /// 3. re-take the lock only for paths that actually moved, where reading
    ///    and reconciling is affordable because it is rare.
    ///
    /// Collapsing this into one locked pass would put a stat storm inside the
    /// mutex that every edit contends for, on a hot path, to answer "nothing
    /// changed" nearly every time.
    pub async fn drifted(&self, actor: &AgentIdentity) -> Result<Vec<Drift>> {
        let manager = self.manager_for(actor).await?;

        let candidates = { manager.lock().await.drift_candidates() };
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        // Phase two, unlocked. `spawn_blocking` because this is real filesystem
        // I/O on an async executor, and the tracked set grows with the session.
        let moved = tokio::task::spawn_blocking(move || {
            candidates
                .into_iter()
                .filter(DriftCandidate::may_have_moved)
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        if moved.is_empty() {
            return Ok(Vec::new());
        }

        let mut drifts = { manager.lock().await.describe_drift(&moved) };

        // Attribution comes from the store rather than from memory, so a write
        // by another *process* in this worktree is attributable too — which is
        // the melting-pot case, and the one an in-process map could never see.
        // Only for paths that actually drifted, so the query stays off the hot
        // path even though the write side does not.
        let changed: Vec<String> = drifts.iter().map(|drift| drift.path.clone()).collect();
        let writers = self
            .state
            .writers_for(&changed)
            .await
            .map_err(anyhow::Error::msg)?;
        for drift in &mut drifts {
            let DriftKind::Modified { after_text, .. } = &drift.kind else {
                // Nothing to check a hash against, so nothing to claim.
                continue;
            };
            let Some((writer, hash)) = writers.get(&drift.path) else {
                continue;
            };
            // Only claim it if the file still holds exactly what that agent
            // wrote. Without this the last agent to touch a path is blamed for
            // every later change to it, including the user's own edits.
            if writer != &actor.id.0 && *hash == content_hash(after_text.as_bytes()) {
                drift.writer = Some(writer.clone());
            }
        }
        Ok(drifts)
    }

    /// Record who wrote a path and what they left there, for drift attribution.
    ///
    /// Best effort: attribution is a nicety on top of a drift report that is
    /// itself a nicety on top of anchors that already fail safe. Losing a row
    /// costs a change reported as unattributed — never a wrong edit — so this
    /// must not fail a write that has already landed on disk.
    async fn note_writer(&self, path: &str, actor: &AgentIdentity, hash: &str) {
        let _ = self.state.record_writer(path, &actor.id, hash).await;
    }

    /// Retire state nobody needs any more.
    ///
    /// Runs once, lazily, on the first manager a coordinator hands out — rather
    /// than at construction, which would put database writes in the path of
    /// every `Workspace::open` including those that never touch a file.
    ///
    /// Every rule here is safe to lose. Retiring anchor state costs a re-read;
    /// retiring an attribution costs a change reported as unattributed. Neither
    /// can cost a wrong edit, which is what makes the thresholds a matter of
    /// taste rather than an argument about correctness.
    async fn tidy_once(&self) {
        let mut tidied = self.tidied.lock().await;
        if *tidied {
            return;
        }
        *tidied = true;
        // Identities that cannot be a conversation: their rows are unreachable.
        let _ = self.state.forget_unowned_anchor_state().await;
        // Conversations nobody has touched in a month. Long enough that
        // resuming one still finds its anchors, short enough to bound growth.
        let _ = self.state.retire_idle_agents(ANCHOR_RETENTION_DAYS).await;
        // A smoke alarm for the above.
        let _ = self.state.cap_agents(MAX_RETAINED_AGENTS).await;
        // Attribution ages out far faster: nobody needs telling that another
        // session touched a file last month.
        let _ = self.state.forget_stale_writers(WRITER_RETENTION_DAYS).await;
    }

    async fn manager_for(&self, actor: &AgentIdentity) -> Result<Arc<Mutex<FileToolManager>>> {
        if let Some(manager) = self.managers.lock().await.get(&actor.id.0).cloned() {
            return Ok(manager);
        }
        self.tidy_once().await;
        let persisted = self
            .state
            .load_anchor_state(&actor.id)
            .await
            .map_err(anyhow::Error::msg)?;
        let mut manager = FileToolManager::with_config(self.config.clone());
        manager.import_anchor_state(persisted);
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

/// How models should use anchors returned in `anchor: line` views.
pub const ANCHOR_USAGE: &str = "Use only the bare mnemonic token before ': '. For the rendered line 'time: beta', pass anchor \"time\" (not \"time: beta\").";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReadFileRequest;

    /// One coordinator over one store and lock directory, which is what a
    /// parent and its subagents share: `FileCoordinator` is `Clone` over `Arc`
    /// fields, so cloning it into a child keeps one set of maps.
    fn coordinator(name: &str) -> (FileCoordinator, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "artist-coord-{}-{name}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        let config = FileToolConfig {
            workspace_root: Some(root.clone()),
            ..FileToolConfig::default()
        };
        let files = FileCoordinator::open(config, root.join("anchors.sqlite"), root.join("locks"))
            .expect("coordinator");
        (files, root)
    }

    fn agent(id: &str) -> AgentIdentity {
        AgentIdentity::from_id(id).expect("identity")
    }

    async fn read(files: &FileCoordinator, actor: &AgentIdentity, path: &str) {
        files
            .read_file(
                actor,
                ReadFileRequest {
                    path: path.to_owned(),
                    start_line: 1,
                    max_lines: None,
                },
            )
            .await
            .expect("read");
    }

    /// The melting-pot case: two sessions, one worktree. What one writes, the
    /// other has to hear about — and hear *who*, because another agent working
    /// the same file is a coordination signal rather than just a stale view.
    #[tokio::test]
    async fn another_agents_write_is_attributed_to_it() {
        let (files, root) = coordinator("attribution");
        let file = root.join("shared.txt");
        std::fs::write(&file, "one\ntwo\n").expect("seed");
        let path = file.to_string_lossy().into_owned();

        let alpha = agent("alpha");
        let beta = agent("beta");
        read(&files, &alpha, &path).await;

        files
            .write_file(
                &beta,
                path.clone(),
                "one\nCHANGED BY BETA\n".to_owned(),
                WriteCondition::Any,
            )
            .await
            .expect("beta writes");

        let drifts = files.drifted(&alpha).await.expect("alpha checks");
        assert_eq!(drifts.len(), 1, "{drifts:?}");
        assert_eq!(
            drifts[0].writer.as_deref(),
            Some("beta"),
            "the other agent's write was not attributed: {drifts:?}"
        );
    }

    /// The same write must be silent for the agent that made it. Anchor state
    /// is per-actor, so this is the check that the two halves — per-actor views
    /// and a shared writer map — do not report an agent to itself.
    #[tokio::test]
    async fn an_agent_is_not_told_about_its_own_write() {
        let (files, root) = coordinator("self");
        let file = root.join("mine.txt");
        std::fs::write(&file, "one\ntwo\n").expect("seed");
        let path = file.to_string_lossy().into_owned();

        let beta = agent("beta");
        read(&files, &beta, &path).await;
        files
            .write_file(
                &beta,
                path.clone(),
                "one\nCHANGED BY BETA\n".to_owned(),
                WriteCondition::Any,
            )
            .await
            .expect("beta writes");

        let drifts = files.drifted(&beta).await.expect("beta checks");
        assert!(
            drifts.is_empty(),
            "an agent was told about its own write: {drifts:?}"
        );
    }

    /// A change from outside every agent — the user's editor, a formatter, a
    /// checkout — is reported unattributed rather than blamed on whoever wrote
    /// last. Guessing here would be worse than silence: it would send the model
    /// coordinating with an agent that did nothing.
    #[tokio::test]
    async fn a_change_from_outside_any_agent_is_unattributed() {
        let (files, root) = coordinator("outside");
        let file = root.join("shared.txt");
        std::fs::write(&file, "one\ntwo\n").expect("seed");
        let path = file.to_string_lossy().into_owned();

        let alpha = agent("alpha");
        read(&files, &alpha, &path).await;
        std::fs::write(&file, "one\nEDITED IN VIM\n").expect("outside write");

        let drifts = files.drifted(&alpha).await.expect("alpha checks");
        assert_eq!(drifts.len(), 1, "{drifts:?}");
        assert!(
            drifts[0].writer.is_none(),
            "an outside change was blamed on an agent: {drifts:?}"
        );
    }

    /// An agent that wrote a file once must not be blamed for every later
    /// change to it. Recording who wrote is not enough — the claim has to be
    /// checked against what is actually there, or the user's own edit gets
    /// attributed to whichever agent last touched the path, and the model goes
    /// off coordinating with an agent that did nothing.
    #[tokio::test]
    async fn a_later_outside_edit_is_not_blamed_on_the_last_agent_writer() {
        let (files, root) = coordinator("stale-attribution");
        let file = root.join("shared.txt");
        std::fs::write(&file, "one\ntwo\n").expect("seed");
        let path = file.to_string_lossy().into_owned();

        let alpha = agent("alpha");
        let beta = agent("beta");
        read(&files, &alpha, &path).await;

        files
            .write_file(
                &beta,
                path.clone(),
                "one\nWRITTEN BY BETA\n".to_owned(),
                WriteCondition::Any,
            )
            .await
            .expect("beta writes");
        // ...and then somebody outside every agent edits it again.
        std::fs::write(&file, "one\nTHEN EDITED IN VIM\n").expect("outside write");

        let drifts = files.drifted(&alpha).await.expect("alpha checks");
        assert_eq!(drifts.len(), 1, "{drifts:?}");
        assert!(
            drifts[0].writer.is_none(),
            "a later outside edit was blamed on beta: {drifts:?}"
        );
    }

    /// The melting-pot case that an in-memory map could never serve: the write
    /// came from a *different process*, so nothing in this one saw it happen.
    /// Two coordinators over one store stand in for two artist processes
    /// sharing a worktree.
    #[tokio::test]
    async fn a_write_from_another_process_is_still_attributed() {
        let (first, root) = coordinator("cross-process");
        let file = root.join("shared.txt");
        std::fs::write(&file, "one\ntwo\n").expect("seed");
        let path = file.to_string_lossy().into_owned();

        let alpha = agent("alpha");
        read(&first, &alpha, &path).await;

        // A second artist, with its own coordinator and its own in-memory
        // state, over the same database and lock directory.
        let config = FileToolConfig {
            workspace_root: Some(root.clone()),
            ..FileToolConfig::default()
        };
        let second = FileCoordinator::open(config, root.join("anchors.sqlite"), root.join("locks"))
            .expect("second coordinator");
        second
            .write_file(
                &agent("beta"),
                path.clone(),
                "one\nWRITTEN BY ANOTHER PROCESS\n".to_owned(),
                WriteCondition::Any,
            )
            .await
            .expect("beta writes");

        let drifts = first.drifted(&alpha).await.expect("alpha checks");
        assert_eq!(drifts.len(), 1, "{drifts:?}");
        assert_eq!(
            drifts[0].writer.as_deref(),
            Some("beta"),
            "a cross-process write was not attributed: {drifts:?}"
        );
    }

    /// Anchor state written before conversation scoping belongs to nobody —
    /// nothing loads it, and anchors reissue on read. Tidying it is safe, and
    /// leaving it would keep a row around that looks like a session.
    #[tokio::test]
    async fn anchor_state_from_before_conversation_scoping_is_forgotten() {
        let (files, root) = coordinator("tidy");
        let file = root.join("a.txt");
        std::fs::write(&file, "one\n").expect("seed");
        let path = file.to_string_lossy().into_owned();

        // Stand in for a session from before the change.
        read(&files, &agent("artist"), &path).await;
        let state = files.state.clone();
        assert!(
            !state
                .load_anchor_state(&agent("artist").id)
                .await
                .expect("load")
                .is_empty(),
            "the fixture did not write any state to tidy"
        );

        // A fresh coordinator over the same store tidies on its first manager.
        let config = FileToolConfig {
            workspace_root: Some(root.clone()),
            ..FileToolConfig::default()
        };
        let next = FileCoordinator::open(config, root.join("anchors.sqlite"), root.join("locks"))
            .expect("next coordinator");
        read(&next, &agent("session-01"), &path).await;

        assert!(
            state
                .load_anchor_state(&agent("artist").id)
                .await
                .expect("load")
                .is_empty(),
            "unowned anchor state survived"
        );
        // The real session's own state is untouched.
        assert!(
            !state
                .load_anchor_state(&agent("session-01").id)
                .await
                .expect("load")
                .is_empty(),
            "tidying took a real session's state with it"
        );
    }

    /// Deleting a session has to actually delete it. Retention would collect
    /// these eventually; leaving them until then means "delete" quietly meant
    /// "delete most of".
    #[tokio::test]
    async fn forgetting_a_conversation_takes_its_anchors_and_attributions() {
        let (files, root) = coordinator("forget-agent");
        let file = root.join("a.txt");
        std::fs::write(&file, "one\n").expect("seed");
        let path = file.to_string_lossy().into_owned();

        let doomed = agent("session-doomed");
        let keeper = agent("session-keeper");
        read(&files, &keeper, &path).await;
        files
            .write_file(
                &doomed,
                path.clone(),
                "one\ntwo\n".to_owned(),
                WriteCondition::Any,
            )
            .await
            .expect("write");

        let state = files.state.clone();
        assert!(
            !state
                .writers_for(&[path.clone()])
                .await
                .expect("writers")
                .is_empty(),
            "the fixture recorded no attribution to forget"
        );

        state.forget_agent("session-doomed").await.expect("forget");

        assert!(
            state
                .load_anchor_state(&doomed.id)
                .await
                .expect("load")
                .is_empty(),
            "anchor state outlived its conversation"
        );
        assert!(
            state
                .writers_for(&[path.clone()])
                .await
                .expect("writers")
                .is_empty(),
            "attribution outlived its conversation"
        );
        // Path-keyed rows made it tempting to collect only by agent; the other
        // session must be untouched either way.
        assert!(
            !state
                .load_anchor_state(&keeper.id)
                .await
                .expect("load")
                .is_empty(),
            "forgetting one conversation took another's state"
        );
    }

    /// Retention is by agent, not by row. A half-retired session leaves some
    /// anchors resolving and some not, and the model cannot tell which it
    /// holds — a clean sweep gives it an honest "re-read" instead.
    #[tokio::test]
    async fn retiring_an_idle_conversation_takes_all_of_it() {
        let (files, root) = coordinator("retire");
        let file = root.join("a.txt");
        std::fs::write(&file, "one\ntwo\nthree\n").expect("seed");
        let path = file.to_string_lossy().into_owned();

        let idle = agent("session-idle");
        read(&files, &idle, &path).await;
        let state = files.state.clone();

        // Nothing is idle yet, so a sweep must leave it alone.
        state.retire_idle_agents(30).await.expect("retire");
        assert!(
            !state
                .load_anchor_state(&idle.id)
                .await
                .expect("load")
                .is_empty(),
            "a live conversation was retired"
        );

        // Zero days makes everything already-idle, which is the same query the
        // real threshold runs.
        let removed = state.retire_idle_agents(0).await.expect("retire");
        assert!(removed > 0, "nothing was retired");
        assert!(
            state
                .load_anchor_state(&idle.id)
                .await
                .expect("load")
                .is_empty(),
            "an idle conversation kept its anchors"
        );
    }

    /// Each agent keeps its own view, so one reading a file must not make the
    /// other's drift disappear — the shared writer map is shared, the views are
    /// not.
    #[tokio::test]
    async fn one_agents_read_does_not_clear_anothers_drift() {
        let (files, root) = coordinator("independent");
        let file = root.join("shared.txt");
        std::fs::write(&file, "one\ntwo\n").expect("seed");
        let path = file.to_string_lossy().into_owned();

        let alpha = agent("alpha");
        let beta = agent("beta");
        read(&files, &alpha, &path).await;

        std::fs::write(&file, "one\nMOVED ON\n").expect("outside write");
        // Beta reads the *new* content, so beta sees no drift...
        read(&files, &beta, &path).await;
        assert!(files.drifted(&beta).await.expect("beta").is_empty());

        // ...but alpha still holds the old view and must still be told.
        let drifts = files.drifted(&alpha).await.expect("alpha");
        assert_eq!(
            drifts.len(),
            1,
            "another agent's read swallowed alpha's drift: {drifts:?}"
        );
    }
}
