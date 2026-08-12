use anyhow::{Context, Result, bail};
use fff_search::{FFFMode, FilePicker, FilePickerOptions, SharedFilePicker, SharedFrecency};
use hashline_tools::{AgentIdentity, BatchWrite, FileCoordinator, FileToolConfig, ReadFileRequest};

mod search_scope;

use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// Where a project's anchor state lives, inside its state directory.
const ANCHOR_DB: &str = "hashlines.sqlite3";

/// Forget everything a finished conversation left in a project's anchor store.
///
/// Anchor state and write attribution are both keyed by the conversation, so
/// they die with it. Exact, rather than waiting for retention to notice: the
/// session is gone and nothing will ever load them again.
///
/// Quiet and best effort. The session's own files are already deleted by the
/// time this runs, so failing to tidy its rows must not turn a successful
/// delete into an error — and retention collects anything missed, including
/// the case where the project directory itself is gone and the state
/// directory can no longer be located.
pub async fn forget_conversation(state_dir: &Path, conversation_id: &str) -> usize {
    let database = state_dir.join(ANCHOR_DB);
    if !database.exists() {
        return 0;
    }
    match hashline_tools::StateStore::open(&database) {
        Ok(store) => store.forget_agent(conversation_id).await.unwrap_or(0),
        Err(_) => 0,
    }
}

#[derive(Clone)]
pub struct Workspace {
    root: Arc<PathBuf>,
    pub(crate) files: FileCoordinator,
    pub(crate) actor: AgentIdentity,
    pub(crate) index: SharedFilePicker,
    /// Paths already observed this session, so the first-touch note fires once.
    ///
    /// Shared across clones because `with_actor` hands subagents a new
    /// `Workspace` over the same project — two agents in one session should not
    /// each be told separately that a file has thirty importers.
    seen: Arc<std::sync::Mutex<std::collections::HashSet<PathBuf>>>,
    /// The architecture as it stood when this session first looked.
    ///
    /// Shared across clones for the same reason `seen` is: the project shape is
    /// a property of the session, and a subagent must not be told a dependency
    /// is new because it happens to hold a different handle. `None` until first
    /// use, and stays `None` for a project with nothing to describe.
    architecture: Arc<std::sync::Mutex<Option<crate::skeleton::Baseline>>>,
    /// Units this session has already been introduced to. Shared for the same
    /// reason `seen` is, and separate from it because a unit is entered once
    /// however many of its files get opened.
    entered: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

impl Workspace {
    /// Conditionally replace several existing files as one coordinated commit.
    /// Callers supply path/content/hash predicates already derived from one
    /// snapshot; the coordinator validates the full set before writing any.
    pub(crate) async fn write_files_atomic(
        &self,
        writes: Vec<BatchWrite>,
    ) -> Result<Vec<hashline_tools::CoordinatedReadResult>> {
        self.files.write_files_atomic(&self.actor, writes).await
    }

    /// Read a set of real files while holding every participating coordinator
    /// lock. The resulting snapshots belong to one Artist-coordinated view.
    pub(crate) async fn read_files_atomic(
        &self,
        paths: Vec<String>,
    ) -> Result<Vec<hashline_tools::CoordinatedReadResult>> {
        self.files.read_files_atomic(&self.actor, paths).await
    }

    /// Return the canonical Artist anchors for every logical line in one live
    /// project file. Semantic consumers such as LSP use this rather than
    /// manufacturing a parallel line-address scheme from byte offsets.
    pub async fn anchors_for(&self, input: &str) -> Result<Vec<(usize, String)>> {
        self.resolve_existing(input)?;
        let read = self
            .files
            .read_file(
                &self.actor,
                ReadFileRequest {
                    path: input.to_owned(),
                    start_line: 1,
                    max_lines: None,
                },
            )
            .await?;
        Ok(read
            .result
            .lines
            .iter()
            .enumerate()
            .map(|(index, line)| (index + 1, line.anchor.clone()))
            .collect())
    }

    /// A handle for asking what changed under this session.
    ///
    /// Handed out rather than exposing `files`/`actor`, so callers get the one
    /// question they need answered instead of the machinery behind it.
    pub fn drift_watch(&self) -> crate::drift::DriftWatch {
        crate::drift::DriftWatch::new(self.files.clone(), self.actor.clone())
    }

    /// Open a workspace for `actor`.
    ///
    /// The identity is required rather than defaulted because anchor state is
    /// stored per actor: everything that shares an id shares one row and
    /// overwrites it. A default would make that collision the easy path and an
    /// invisible one — callers that genuinely have no conversation yet should
    /// say so with a name of their own, and re-actor via [`Self::with_actor`]
    /// once they do.
    pub fn open(
        project_root: impl AsRef<Path>,
        state_dir: impl AsRef<Path>,
        actor: &str,
    ) -> Result<Self> {
        let root = std::fs::canonicalize(project_root).context("canonicalize project root")?;
        if !root.is_dir() {
            bail!("project root is not a directory")
        }
        let state = state_dir.as_ref();
        std::fs::create_dir_all(state)?;
        // Keep the dep/call graph out of the user's repository. Upstream writes
        // it to `<root>/.ast-bro/`, which would put a binary cache inside the
        // tree the agent is editing; artist already has a per-project state
        // directory for derived data, so point it there.
        artist_ast::graph_cache::cache::set_cache_base(Some(state.join("astgraph")));
        let files = FileCoordinator::open(
            FileToolConfig {
                workspace_root: Some(root.clone()),
                allow_outside_workspace: true,
                follow_symlinks: false,
            },
            state.join(ANCHOR_DB),
            state.join("locks"),
        )?;
        let picker = SharedFilePicker::default();
        let broad_root = is_broad_root(&root);
        FilePicker::new_with_shared_state(
            picker.clone(),
            SharedFrecency::default(),
            FilePickerOptions {
                base_path: root.to_string_lossy().into_owned(),
                mode: FFFMode::Ai,
                // FFF requires explicit opt-in before scanning a home directory or
                // file-system root. Avoid the expensive watcher and content index
                // there, but retain the path index used by the find tool.
                enable_content_indexing: !broad_root,
                watch: !broad_root,
                enable_fs_root_scanning: broad_root,
                enable_home_dir_scanning: broad_root,
                follow_symlinks: false,
                ..Default::default()
            },
        )?;
        Ok(Self {
            root: Arc::new(root),
            files,
            actor: AgentIdentity::from_id(actor).map_err(anyhow::Error::msg)?,
            index: picker,
            seen: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            architecture: Arc::new(std::sync::Mutex::new(None)),
            entered: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        })
    }

    pub fn with_actor(&self, id: &str) -> Result<Self> {
        let mut workspace = self.clone();
        workspace.actor = AgentIdentity::from_id(id).map_err(anyhow::Error::msg)?;
        Ok(workspace)
    }

    pub fn root(&self) -> &Path {
        self.root.as_ref()
    }

    pub(crate) async fn wait_for_index(&self) -> Result<()> {
        wait_for_picker(self.index.clone()).await
    }

    pub fn resolve_existing(&self, input: &str) -> Result<PathBuf> {
        let candidate = self.checked_join(input)?;
        let canonical = std::fs::canonicalize(&candidate)
            .with_context(|| format!("path does not exist: {input}"))?;
        if Path::new(input).is_absolute() {
            Ok(canonical)
        } else {
            self.ensure_inside(canonical, input)
        }
    }

    pub fn resolve_new(&self, input: &str) -> Result<PathBuf> {
        let candidate = self.checked_join(input)?;
        let mut parent = candidate.parent().context("path has no parent")?;
        while !parent.exists() {
            parent = parent.parent().context("path has no existing parent")?;
        }
        let canonical_parent = std::fs::canonicalize(parent)?;
        if !Path::new(input).is_absolute() {
            self.ensure_inside(canonical_parent, input)?;
        }
        Ok(candidate)
    }

    pub(crate) fn refresh_index(&self, path: &Path) {
        if !path.starts_with(self.root()) {
            return;
        }
        if let Ok(mut index) = self.index.write()
            && let Some(index) = index.as_mut()
        {
            index.handle_create_or_modify(path);
        }
        // Drop the memoised dep/call graph for this root. `get_or_init` already
        // re-validates against the working tree on every call, so this is not
        // what makes the graph correct — it makes the next query skip a
        // stat-walk it would otherwise perform only to reach the same
        // conclusion, and it closes the window where a write lands inside the
        // same mtime tick the cached records were stamped with.
        artist_ast::graph_cache::shared::forget(self.root());
    }

    /// True the first time `path` is observed this session.
    ///
    /// Records on the way out, so a caller that asks is committing to having
    /// shown whatever the answer gates. Cheap enough to call on every read.
    /// Architectural changes this edit caused, or empty when it caused none.
    ///
    /// The baseline is captured on first use rather than at open: a session
    /// that never edits anything should not pay to discover the shape of the
    /// project, and the first edit is early enough to be a true "before".
    pub(crate) fn architecture_delta(&self, file: &Path) -> Vec<String> {
        let Ok(mut held) = self.architecture.lock() else {
            return Vec::new();
        };
        if held.is_none() {
            *held = crate::skeleton::baseline(self.root());
        }
        held.as_mut()
            .map(|baseline| baseline.observe(self.root(), file))
            .unwrap_or_default()
    }

    /// An introduction to this file's unit, the first time the session goes
    /// there. `None` afterwards, and for a project with no architecture.
    /// [`Self::region_note`], reachable from the integration tests.
    ///
    /// The real method is crate-private because nothing outside the annotation
    /// hook should be firing it — calling it consumes the one introduction a
    /// unit gets.
    #[doc(hidden)]
    pub fn region_note_for_test(&self, file: &Path) -> Option<String> {
        self.region_note(file)
    }

    pub(crate) fn region_note(&self, file: &Path) -> Option<String> {
        let mut held = self.architecture.lock().ok()?;
        if held.is_none() {
            *held = crate::skeleton::baseline(self.root());
        }
        let baseline = held.as_ref()?;
        let unit = baseline.unit_of(file)?;
        if !self.entered.lock().ok()?.insert(unit.clone()) {
            return None;
        }
        baseline.region_note(file)
    }

    pub(crate) fn first_observation(&self, path: &Path) -> bool {
        self.seen
            .lock()
            .map(|mut seen| seen.insert(path.to_path_buf()))
            .unwrap_or(false)
    }

    pub fn display(&self, path: &Path) -> String {
        path.strip_prefix(self.root())
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn checked_join(&self, input: &str) -> Result<PathBuf> {
        if input.trim().is_empty() {
            bail!("path cannot be empty")
        }
        let path = Path::new(input);
        if path.is_absolute() {
            return Ok(path.to_owned());
        }
        if path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        {
            bail!("path traversal is not allowed: {input}")
        }
        Ok(self.root.join(path))
    }

    fn ensure_inside(&self, path: PathBuf, input: &str) -> Result<PathBuf> {
        if !path.starts_with(self.root()) {
            bail!("path escapes project root: {input}")
        }
        Ok(path)
    }
}

async fn wait_for_picker(picker: SharedFilePicker) -> Result<()> {
    let ready = tokio::task::spawn_blocking(move || picker.wait_for_scan(Duration::from_secs(30)))
        .await
        .context("join file indexing task")?;
    if !ready {
        bail!("timed out indexing files")
    }
    Ok(())
}

fn is_broad_root(root: &Path) -> bool {
    if root.parent().is_none() {
        return true;
    }

    dirs::home_dir()
        .and_then(|home| std::fs::canonicalize(home).ok())
        .is_some_and(|home| home == root)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identifies_file_system_and_home_roots() {
        let file_system_root = Path::new(std::path::MAIN_SEPARATOR_STR);
        assert!(is_broad_root(file_system_root));

        if let Some(home) = dirs::home_dir().and_then(|path| std::fs::canonicalize(path).ok()) {
            assert!(is_broad_root(&home));
        }
    }

    #[test]
    fn accepts_absolute_paths_but_rejects_relative_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(root.path(), state.path(), "test").unwrap();
        assert!(workspace.resolve_new("../escape").is_err());
        assert_eq!(
            workspace
                .resolve_new(outside.path().join("new.txt").to_str().unwrap())
                .unwrap(),
            outside.path().join("new.txt")
        );
        assert_eq!(
            workspace.resolve_new("src/lib.rs").unwrap(),
            root.path().join("src/lib.rs")
        );
    }
}
