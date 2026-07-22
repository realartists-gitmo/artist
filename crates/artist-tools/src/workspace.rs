use anyhow::{Context, Result, bail};
use fff_search::{FFFMode, FilePicker, FilePickerOptions, SharedFilePicker, SharedFrecency};
use hashline_tools::{AgentIdentity, FileCoordinator, FileToolConfig};

mod search_scope;

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
                allow_outside_workspace: true,
                follow_symlinks: false,
            },
            state.join("hashlines.sqlite3"),
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
        let workspace = Workspace::open(root.path(), state.path()).unwrap();
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
