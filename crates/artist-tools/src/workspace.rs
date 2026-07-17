use anyhow::{Context, Result, bail};
use fff_search::{FFFMode, FilePicker, FilePickerOptions, SharedFilePicker, SharedFrecency};
use hashline_tools::{AgentIdentity, FileCoordinator, FileToolConfig};
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Clone)]
pub struct Workspace {
    root: Arc<PathBuf>,
    pub(crate) files: FileCoordinator,
    pub(crate) actor: AgentIdentity,
    pub(crate) index: SharedFilePicker,
}

impl Workspace {
    pub fn open(project_root: impl AsRef<Path>, state_dir: impl AsRef<Path>) -> Result<Self> {
        let root = std::fs::canonicalize(project_root).context("canonicalize project root")?;
        if !root.is_dir() {
            bail!("project root is not a directory")
        }
        let state = state_dir.as_ref();
        std::fs::create_dir_all(state)?;
        let files = FileCoordinator::open(
            FileToolConfig {
                workspace_root: Some(root.clone()),
                allow_outside_workspace: false,
                follow_symlinks: false,
            },
            state.join("hashlines.sqlite3"),
            state.join("locks"),
        )?;
        let picker = SharedFilePicker::default();
        FilePicker::new_with_shared_state(
            picker.clone(),
            SharedFrecency::default(),
            FilePickerOptions {
                base_path: root.to_string_lossy().into_owned(),
                mode: FFFMode::Ai,
                enable_content_indexing: true,
                watch: true,
                follow_symlinks: false,
                ..Default::default()
            },
        )?;
        Ok(Self {
            root: Arc::new(root),
            files,
            actor: AgentIdentity::from_id("artist").map_err(anyhow::Error::msg)?,
            index: picker,
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
        let picker = self.index.clone();
        let ready =
            tokio::task::spawn_blocking(move || picker.wait_for_scan(Duration::from_secs(30)))
                .await
                .context("join project indexing task")?;
        if !ready {
            bail!("timed out indexing project files")
        }
        Ok(())
    }

    pub fn resolve_existing(&self, input: &str) -> Result<PathBuf> {
        let candidate = self.checked_join(input)?;
        let canonical = std::fs::canonicalize(&candidate)
            .with_context(|| format!("path does not exist: {input}"))?;
        self.ensure_inside(canonical, input)
    }

    pub fn resolve_new(&self, input: &str) -> Result<PathBuf> {
        let candidate = self.checked_join(input)?;
        let mut parent = candidate.parent().context("path has no parent")?;
        while !parent.exists() {
            parent = parent.parent().context("path has no existing parent")?;
        }
        let canonical_parent = std::fs::canonicalize(parent)?;
        self.ensure_inside(canonical_parent, input)?;
        Ok(candidate)
    }

    pub(crate) fn refresh_index(&self, path: &Path) {
        if let Ok(mut index) = self.index.write()
            && let Some(index) = index.as_mut()
        {
            index.handle_create_or_modify(path);
        }
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
            bail!("absolute paths are not allowed: {input}")
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_escape_and_absolute_paths() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(root.path(), state.path()).unwrap();
        assert!(workspace.resolve_new("../escape").is_err());
        assert!(workspace.resolve_new("/tmp/escape").is_err());
        assert_eq!(
            workspace.resolve_new("src/lib.rs").unwrap(),
            root.path().join("src/lib.rs")
        );
    }
}
