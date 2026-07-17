mod discovery;
mod events;
mod manager;
mod manifest;
mod runtime;

pub use discovery::{Diagnostic, DiscoveredExtension, default_root, discover};
pub use events::{Event, EventBus};
pub use manager::Manager;
pub use manifest::{CommandDeclaration, Manifest, StatusDeclaration, ToolDeclaration};
pub use runtime::Instance;

use serde::{Deserialize, Serialize};
use std::{future::Future, path::PathBuf, pin::Pin};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExtensionContext {
    pub project: PathBuf,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub agent_state: serde_json::Value,
    pub recent_events: Vec<String>,
}

pub type ControlFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Trusted extensions can affect the active agent through these callbacks.
pub trait HostControl: Send + Sync {
    fn steer(&self, message: String) -> ControlFuture<'_>;
    fn prompt_after(&self, message: String) -> ControlFuture<'_>;
    fn stop(&self) -> ControlFuture<'_>;
}

#[derive(Default)]
pub struct NoopControl;
impl HostControl for NoopControl {
    fn steer(&self, _: String) -> ControlFuture<'_> {
        Box::pin(async {})
    }
    fn prompt_after(&self, _: String) -> ControlFuture<'_> {
        Box::pin(async {})
    }
    fn stop(&self) -> ControlFuture<'_> {
        Box::pin(async {})
    }
}

/// Runtime registry supporting atomic rediscovery/reload and diagnostics.
pub struct Registry {
    root: PathBuf,
    pub extensions: Vec<DiscoveredExtension>,
    pub diagnostics: Vec<Diagnostic>,
}
impl Registry {
    pub fn load(root: PathBuf) -> Self {
        let (extensions, diagnostics) = discover(&root);
        Self {
            root,
            extensions,
            diagnostics,
        }
    }
    pub fn reload(&mut self) {
        let (extensions, diagnostics) = discover(&self.root);
        self.extensions = extensions;
        self.diagnostics = diagnostics;
    }
    pub fn tools(&self) -> impl Iterator<Item = (&Manifest, &ToolDeclaration)> {
        self.extensions
            .iter()
            .flat_map(|e| e.manifest.tools.iter().map(move |t| (&e.manifest, t)))
    }
    pub fn commands(&self) -> impl Iterator<Item = (&Manifest, &CommandDeclaration)> {
        self.extensions
            .iter()
            .flat_map(|e| e.manifest.commands.iter().map(move |c| (&e.manifest, c)))
    }
    pub fn status_items(&self) -> impl Iterator<Item = (&Manifest, &StatusDeclaration)> {
        self.extensions
            .iter()
            .flat_map(|e| e.manifest.statusbar.iter().map(move |s| (&e.manifest, s)))
    }
}
