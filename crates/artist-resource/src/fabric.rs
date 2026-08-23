use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use tokio::runtime::Handle;

use crate::{
    ResourceRouter, ResourceUri, SearchEngine, ToolError, fuse::FuseMount, uri_to_mount_path,
};

/// Owns the ephemeral Unix projection and its one FFF index.
pub struct ResourceFabric {
    mount: FuseMount,
    search: Arc<SearchEngine>,
    refresh_task: tokio::task::JoinHandle<()>,
    working_directory: PathBuf,
}

impl ResourceFabric {
    /// Routes registered before construction are visible in the initial crawl.
    /// Later registrations and topology-changing operations trigger a new crawl.
    pub async fn mount(
        router: ResourceRouter,
        working_directory: impl Into<PathBuf>,
        runtime: Handle,
    ) -> Result<Self, ToolError> {
        let working_directory = working_directory.into();
        let mut generations = router.subscribe_generation();
        let mount = FuseMount::mount(router.clone(), runtime)
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        let search = Arc::new(SearchEngine::new(mount.root()).map_err(ToolError::Failed)?);
        search
            .synchronize(router.generation())
            .map_err(ToolError::Failed)?;
        let refresh_search = search.clone();
        let refresh_task = tokio::spawn(async move {
            while generations.changed().await.is_ok() {
                let generation = *generations.borrow_and_update();
                let search = refresh_search.clone();
                let _ = tokio::task::spawn_blocking(move || search.synchronize(generation)).await;
            }
        });
        Ok(Self {
            mount,
            search,
            refresh_task,
            working_directory,
        })
    }

    pub fn root(&self) -> &Path {
        self.mount.root()
    }
    pub fn search(&self) -> &Arc<SearchEngine> {
        &self.search
    }
    pub fn projected_working_directory(&self) -> Result<PathBuf, String> {
        let uri = ResourceUri::resolve(&self.working_directory.to_string_lossy(), Path::new("/"))
            .map_err(|e| e.to_string())?;
        uri_to_mount_path(self.root(), &uri)
    }

    /// Launch shells and REPLs in the projected file subtree with the whole
    /// logical mount discoverable through `ARTIST_ROOT`.
    pub fn configure_command(&self, command: &mut Command) -> Result<(), String> {
        command
            .current_dir(self.projected_working_directory()?)
            .env("ARTIST_ROOT", self.root());
        Ok(())
    }
}

impl Drop for ResourceFabric {
    fn drop(&mut self) {
        self.refresh_task.abort();
    }
}
