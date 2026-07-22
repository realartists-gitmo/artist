use super::{Workspace, is_broad_root, wait_for_picker};
use anyhow::{Context, Result};
use fff_search::{FFFMode, FilePicker, FilePickerOptions, SharedFilePicker, SharedFrecency};
use std::path::{Path, PathBuf};

/// Selects either the persistent project index or a transient index rooted at
/// an explicitly requested absolute directory outside the project.
pub(crate) struct SearchScope {
    pub(crate) index: SharedFilePicker,
    base: PathBuf,
    filter: Option<String>,
    absolute_output: bool,
}

impl Workspace {
    pub(crate) async fn search_scope(
        &self,
        input: Option<&str>,
        content_indexing: bool,
    ) -> Result<SearchScope> {
        let Some(input) = input else {
            self.wait_for_index().await?;
            return Ok(SearchScope::project(self, None));
        };
        let resolved = self.resolve_existing(input)?;
        if resolved.starts_with(self.root()) {
            self.wait_for_index().await?;
            let filter = self.display(&resolved);
            return Ok(SearchScope::project(
                self,
                (!filter.is_empty()).then_some(filter),
            ));
        }

        let (base, filter) = if resolved.is_dir() {
            (resolved, None)
        } else {
            let parent = resolved
                .parent()
                .context("absolute file path has no parent")?
                .to_owned();
            let name = resolved
                .file_name()
                .context("absolute file path has no name")?
                .to_string_lossy()
                .into_owned();
            (parent, Some(name))
        };
        let index = transient_index(&base, content_indexing)?;
        wait_for_picker(index.clone()).await?;
        Ok(SearchScope {
            index,
            base,
            filter,
            absolute_output: true,
        })
    }
}

impl SearchScope {
    fn project(workspace: &Workspace, filter: Option<String>) -> Self {
        Self {
            index: workspace.index.clone(),
            base: workspace.root().to_owned(),
            filter,
            absolute_output: false,
        }
    }

    pub(crate) fn matches(&self, relative: &str) -> bool {
        self.filter
            .as_ref()
            .is_none_or(|scope| relative == scope || relative.starts_with(&format!("{scope}/")))
    }

    pub(crate) fn display(&self, relative: &str) -> String {
        if self.absolute_output {
            normalized(&self.base.join(relative))
        } else {
            relative.to_owned()
        }
    }
}

fn transient_index(base: &Path, content_indexing: bool) -> Result<SharedFilePicker> {
    let index = SharedFilePicker::default();
    let broad_root = is_broad_root(base);
    FilePicker::new_with_shared_state(
        index.clone(),
        SharedFrecency::default(),
        FilePickerOptions {
            base_path: base.to_string_lossy().into_owned(),
            mode: FFFMode::Ai,
            enable_content_indexing: content_indexing && !broad_root,
            watch: false,
            enable_fs_root_scanning: broad_root,
            enable_home_dir_scanning: broad_root,
            follow_symlinks: false,
            ..Default::default()
        },
    )?;
    Ok(index)
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
