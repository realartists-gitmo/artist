use crate::{Diagnostic, Event, EventBus, ExtensionContext, HostControl, Instance, Registry};
use anyhow::{Result, anyhow};
use artist_tool_api::{
    ArtistDynamicTool, ArtistToolAnnotations, ArtistToolDefinition, ArtistToolOutput, ToolCategory,
    text_output_schema,
};
use rig_core::tool::ToolExecutionError;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

/// A live extension set. Instances survive for the lifetime of the manager,
/// allowing guests to retain state between tool, command, status, and event calls.
pub struct Manager {
    live: RwLock<LiveSet>,
    statuses: Arc<RwLock<HashMap<String, String>>>,
    events: EventBus,
    context: Arc<RwLock<ExtensionContext>>,
    control: Arc<dyn HostControl>,
    /// Abort handles for the per-status-item refresh loops, cancelled on drop
    /// so they don't leak their wasm instance across a Manager reload.
    tasks: Mutex<TaskSet>,
    /// Concurrent reload requests serialize before candidate discovery so the
    /// final active set and its refresh tasks are one coherent revision.
    reload_lock: tokio::sync::Mutex<()>,
}

struct LiveSet {
    registry: Registry,
    instances: HashMap<String, Arc<Instance>>,
}

#[derive(Default)]
struct TaskSet {
    status: Vec<tokio::task::AbortHandle>,
    /// Event-forwarding tasks (one per instance). Each holds an `Arc<Instance>`
    /// which keeps a bus sender alive, so `recv()` never returns `Closed` — the
    /// task can only be stopped by aborting it, or it leaks its wasm instance
    /// forever across every reload.
    event: Vec<tokio::task::AbortHandle>,
}

impl Drop for Manager {
    fn drop(&mut self) {
        let tasks = self.tasks.lock().unwrap_or_else(|error| error.into_inner());
        for handle in tasks.status.iter().chain(&tasks.event) {
            handle.abort();
        }
    }
}

impl Manager {
    pub async fn load(
        root: PathBuf,
        mut context: ExtensionContext,
        control: Arc<dyn HostControl>,
    ) -> Self {
        let mut registry = Registry::load(root);
        let events = EventBus::new(64);
        context.recent_events = events.recent();
        let context = Arc::new(RwLock::new(context));
        let instances = Self::activate(&mut registry, &context, &events, &control).await;
        let manager = Self {
            live: RwLock::new(LiveSet {
                registry,
                instances,
            }),
            statuses: Default::default(),
            events,
            context,
            control,
            tasks: Mutex::new(TaskSet::default()),
            reload_lock: tokio::sync::Mutex::new(()),
        };
        manager.restart_tasks();
        manager
    }

    /// Atomically activate a newly discovered extension set.
    ///
    /// A replacement is compiled and instantiated before the currently active
    /// set is disturbed.  If a replacement fails, its last working instance
    /// and declaration remain live and the failure is recorded as a registry
    /// diagnostic, so editing the broken revision remains possible.
    pub async fn reload(&self) {
        let _reload = self.reload_lock.lock().await;
        let (previous, old_instances) = {
            let live = self.live.read().expect("extension state poisoned");
            (live.registry.clone(), live.instances.clone())
        };
        let mut candidate = Registry::load(previous.root().to_owned());
        let mut instances =
            Self::activate(&mut candidate, &self.context, &self.events, &self.control).await;

        let mut failed_ids = candidate
            .diagnostics
            .iter()
            .filter_map(|diagnostic| {
                candidate.extensions.iter().find_map(|extension| {
                    (diagnostic.path == extension.wasm
                        && diagnostic.message.starts_with("activate "))
                    .then(|| extension.manifest.id.clone())
                })
            })
            .collect::<Vec<_>>();
        // Discovery failures (bad manifest or a missing module) have no
        // `DiscoveredExtension`, but they still describe a revision of an
        // existing extension directory. Retain that extension's old working
        // revision; a directory that simply vanished has no diagnostic and is
        // therefore an intentional removal.
        for diagnostic in &candidate.diagnostics {
            let Some(id) = diagnostic
                .path
                .parent()
                .and_then(|directory| directory.file_name())
                .and_then(|name| name.to_str())
            else {
                continue;
            };
            let was_active = old_instances.contains_key(id);
            let still_discovered = candidate
                .extensions
                .iter()
                .any(|extension| extension.manifest.id == id);
            if was_active && !still_discovered && !failed_ids.iter().any(|known| known == id) {
                failed_ids.push(id.to_owned());
            }
        }
        preserve_last_working_layouts(&mut candidate, &previous, &failed_ids);
        for id in failed_ids {
            let Some(previous) = old_instances.get(&id).cloned() else {
                continue;
            };
            if candidate
                .extensions
                .iter()
                .any(|extension| extension.manifest.id == id)
            {
                instances.insert(id, previous);
            }
        }

        // Only replace the live set after every candidate activation attempt
        // above has settled.  Tasks retain instance Arcs, so stop them before
        // releasing the old map.
        *self.live.write().expect("extension state poisoned") = LiveSet {
            registry: candidate,
            instances,
        };
        self.statuses
            .write()
            .expect("status cache poisoned")
            .clear();
        self.restart_tasks();
    }

    async fn activate(
        registry: &mut Registry,
        context: &Arc<RwLock<ExtensionContext>>,
        events: &EventBus,
        control: &Arc<dyn HostControl>,
    ) -> HashMap<String, Arc<Instance>> {
        let loads = registry.extensions.iter().cloned().map(|extension| {
            let context = context.clone();
            let events = events.clone();
            let control = control.clone();
            async move {
                let result = Instance::load(&extension, context, events, control).await;
                (extension, result)
            }
        });
        let mut instances = HashMap::new();
        for (extension, result) in futures::future::join_all(loads).await {
            match result {
                Ok(instance) => {
                    instances.insert(extension.manifest.id.clone(), Arc::new(instance));
                }
                Err(error) => registry.diagnostics.push(Diagnostic {
                    path: extension.wasm,
                    message: format!("activate {}: {error:#}", extension.manifest.id),
                }),
            }
        }
        instances
    }

    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.live
            .read()
            .expect("extension state poisoned")
            .registry
            .diagnostics
            .clone()
    }

    /// A coherent, immutable view of the live declarations.  Consumers that
    /// render `tools://` use this rather than rediscovering files themselves,
    /// so a failed replacement continues to expose the active revision.
    pub fn registry_snapshot(&self) -> Registry {
        self.live
            .read()
            .expect("extension state poisoned")
            .registry
            .clone()
    }

    /// Invoke one active extension declaration from the currently live set.
    /// The instance is cloned while holding the read lock and called only
    /// afterwards, so an in-flight invocation never blocks a reload.
    pub async fn invoke_tool(
        &self,
        extension_id: &str,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<ArtistToolOutput> {
        let instance = {
            let live = self.live.read().expect("extension state poisoned");
            let declared = live.registry.tools().any(|(manifest, declaration)| {
                manifest.id == extension_id && declaration.name == name
            });
            if !declared {
                return Err(anyhow!(
                    "unknown extension tool tools://{extension_id}/tool/{name}"
                ));
            }
            live.instances
                .get(extension_id)
                .cloned()
                .ok_or_else(|| anyhow!("extension {extension_id} is unavailable"))?
        };
        let output: rig_core::tool::ToolOutput = instance
            .invoke_tool(name, arguments)
            .await
            .map(Into::into)
            .map_err(|error| anyhow!(error.to_string()))?;
        ArtistToolOutput::from_tool_output(output).map_err(|error| anyhow!(error.to_string()))
    }

    /// IDs of extensions that activated successfully, in stable display order.
    pub fn extension_ids(&self) -> Vec<String> {
        let live = self.live.read().expect("extension state poisoned");
        let mut ids = live.instances.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        ids
    }

    /// Exact active extension material at one tool-surface boundary. The
    /// manifest is retained verbatim and the component digest distinguishes a
    /// hot-reloaded implementation from an identically named extension.
    pub fn provenance(&self) -> Vec<serde_json::Value> {
        let live = self.live.read().expect("extension state poisoned");
        let mut entries = live
            .registry
            .extensions
            .iter()
            .filter(|extension| live.instances.contains_key(&extension.manifest.id))
            .map(|extension| {
                let wasm = std::fs::read(&extension.wasm).ok();
                serde_json::json!({
                    "id": extension.manifest.id,
                    "manifest": extension.manifest,
                    "wasmSha256": wasm.as_ref().map(|bytes| hex_digest(bytes)),
                    "wasmPath": extension.wasm.file_name().and_then(|name| name.to_str()),
                })
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
        entries
    }

    pub fn tools(&self) -> Vec<ArtistDynamicTool> {
        let live = self.live.read().expect("extension state poisoned");
        live.registry
            .tools()
            .filter_map(|(manifest, declaration)| {
                let instance = live.instances.get(&manifest.id)?.clone();
                let name = declaration.name.clone();
                let invoked_name = name.clone();
                let definition = ArtistToolDefinition {
                    title: artist_tool_api::humanize(&name),
                    name: name.clone(),
                    description: declaration.description.clone(),
                    input_schema: declaration.parameters.clone(),
                    output_schema: text_output_schema(
                        &name,
                        "Result returned by the extension tool.",
                    ),
                    category: ToolCategory::External,
                    annotations: ArtistToolAnnotations {
                        read_only: false,
                        destructive: true,
                        idempotent: false,
                        open_world: true,
                    },
                };
                Some(ArtistDynamicTool::new(definition, move |arguments| {
                    let instance = instance.clone();
                    let name = invoked_name.clone();
                    Box::pin(async move {
                        let output: rig_core::tool::ToolOutput = instance
                            .invoke_tool(&name, &arguments)
                            .await
                            .map(Into::into)
                            .map_err(|error| ToolExecutionError::other(error.to_string()))?;
                        ArtistToolOutput::from_tool_output(output)
                    })
                }))
            })
            .collect()
    }

    /// Live extension tools keyed by their canonical Artist resource path.
    /// The path, rather than a provider tool name, is the stable local dispatch
    /// identity: `run(tools://<extension>/tool/<name>, args)` remains local even
    /// when provider schemas are refreshed later.
    pub fn path_tools(&self) -> Vec<(String, ArtistDynamicTool)> {
        let tools = self.tools();
        let mut paths = self
            .live
            .read()
            .expect("extension state poisoned")
            .registry
            .tools()
            .filter(|(manifest, declaration)| {
                self.live
                    .read()
                    .expect("extension state poisoned")
                    .instances
                    .contains_key(&manifest.id)
                    && !declaration.name.is_empty()
                    && !declaration.name.contains('/')
            })
            .filter_map(|(manifest, declaration)| {
                tools
                    .iter()
                    .find(|tool| tool.name() == declaration.name)
                    .cloned()
                    .map(|tool| {
                        (
                            format!("tools://{}/tool/{}", manifest.id, declaration.name),
                            tool,
                        )
                    })
            })
            .collect::<Vec<_>>();
        paths.sort_by(|left, right| left.0.cmp(&right.0));
        paths
    }

    /// UI metadata for tools backed by a live extension instance.
    pub fn tool_icons(&self) -> HashMap<String, String> {
        let live = self.live.read().expect("extension state poisoned");
        live.registry
            .tools()
            .filter(|(manifest, _)| live.instances.contains_key(&manifest.id))
            .filter_map(|(_, tool)| {
                tool.icon
                    .as_ref()
                    .map(|icon| (tool.name.clone(), icon.clone()))
            })
            .collect()
    }

    pub fn commands(&self) -> Vec<crate::CommandDeclaration> {
        let live = self.live.read().expect("extension state poisoned");
        live.registry
            .commands()
            .filter(|(m, _)| live.instances.contains_key(&m.id))
            .map(|(_, command)| command.clone())
            .collect()
    }

    pub async fn invoke_command(&self, name: &str, arguments: &str) -> Result<String> {
        let instance = {
            let live = self.live.read().expect("extension state poisoned");
            let (manifest, _) = live
                .registry
                .commands()
                .find(|(_, command)| command.name == name)
                .ok_or_else(|| anyhow!("unknown extension command {name}"))?;
            live.instances
                .get(&manifest.id)
                .cloned()
                .ok_or_else(|| anyhow!("extension unavailable"))?
        };
        instance.invoke_command(name, arguments).await
    }

    pub fn status_items(&self) -> Vec<(String, String)> {
        self.statuses
            .read()
            .expect("status cache poisoned")
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    pub fn publish(&self, event: Event) -> Result<()> {
        self.events.publish(&event)?;
        Ok(())
    }
    pub fn recent_events(&self) -> Vec<String> {
        self.events.recent()
    }
    pub fn update_context(&self, update: impl FnOnce(&mut ExtensionContext)) {
        update(&mut self.context.write().expect("extension context poisoned"));
    }
    pub fn tool_names(&self) -> Vec<String> {
        let live = self.live.read().expect("extension state poisoned");
        live.registry
            .tools()
            .filter(|(m, _)| live.instances.contains_key(&m.id))
            .map(|(_, tool)| tool.name.clone())
            .collect()
    }
    pub fn status_declarations(&self) -> Vec<crate::StatusDeclaration> {
        let live = self.live.read().expect("extension state poisoned");
        live.registry
            .status_items()
            .filter(|(m, _)| live.instances.contains_key(&m.id))
            .map(|(_, status)| status.clone())
            .collect()
    }

    fn start_event_forwarding(&self) -> Vec<tokio::task::AbortHandle> {
        let instances = self
            .live
            .read()
            .expect("extension state poisoned")
            .instances
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut handles = Vec::new();
        for instance in instances {
            let mut receiver = self.events.subscribe();
            let task = tokio::spawn(async move {
                loop {
                    match receiver.recv().await {
                        Ok(json) => {
                            let _ = instance.event(&json).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            handles.push(task.abort_handle());
        }
        handles
    }

    fn start_status_refresh(&self) -> Vec<tokio::task::AbortHandle> {
        let mut handles = Vec::new();
        let entries = {
            let live = self.live.read().expect("extension state poisoned");
            live.registry
                .status_items()
                .filter_map(|(manifest, declaration)| {
                    live.instances.get(&manifest.id).cloned().map(|instance| {
                        (
                            instance,
                            declaration.name.clone(),
                            declaration.refresh_ms.max(100),
                        )
                    })
                })
                .collect::<Vec<_>>()
        };
        for (instance, name, interval) in entries {
            let cache = self.statuses.clone();
            let task = tokio::spawn(async move {
                loop {
                    if let Ok(value) = instance.status(&name).await {
                        cache
                            .write()
                            .expect("status cache poisoned")
                            .insert(name.clone(), value);
                    }
                    tokio::time::sleep(Duration::from_millis(interval)).await;
                }
            });
            handles.push(task.abort_handle());
        }
        handles
    }

    fn restart_tasks(&self) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|error| error.into_inner());
        for handle in tasks.status.iter().chain(&tasks.event) {
            handle.abort();
        }
        tasks.status = self.start_status_refresh();
        tasks.event = self.start_event_forwarding();
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Keep the old declaration/module pairing visible when the new pairing did
/// not activate.  This lives outside [`Manager::reload`] so the no-Wasm portion
/// of the atomic fallback contract can be tested without a live guest.
fn preserve_last_working_layouts(
    candidate: &mut Registry,
    previous: &Registry,
    failed_ids: &[String],
) {
    for id in failed_ids {
        let Some(previous_layout) = previous
            .extensions
            .iter()
            .find(|extension| extension.manifest.id == *id)
            .cloned()
        else {
            continue;
        };
        if let Some(slot) = candidate
            .extensions
            .iter_mut()
            .find(|extension| extension.manifest.id == *id)
        {
            *slot = previous_layout;
        } else {
            candidate.extensions.push(previous_layout);
        }
    }
    candidate
        .extensions
        .sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiscoveredExtension, Manifest, NoopControl};

    fn extension(id: &str, wasm: &str) -> DiscoveredExtension {
        DiscoveredExtension {
            manifest: Manifest {
                id: id.into(),
                name: format!("{id} extension"),
                description: String::new(),
                enabled: true,
                tools: Vec::new(),
                statusbar: Vec::new(),
                commands: Vec::new(),
            },
            wasm: PathBuf::from(wasm),
        }
    }

    #[test]
    fn failed_replacement_keeps_last_working_layout() {
        let previous = Registry {
            root: PathBuf::from("extensions"),
            extensions: vec![extension("demo", "extensions/demo/working.wasm")],
            diagnostics: Vec::new(),
        };
        let mut candidate = Registry {
            root: PathBuf::from("extensions"),
            extensions: vec![extension("demo", "extensions/demo/broken.wasm")],
            diagnostics: Vec::new(),
        };

        preserve_last_working_layouts(&mut candidate, &previous, &["demo".into()]);

        assert_eq!(
            candidate.extensions[0].wasm,
            PathBuf::from("extensions/demo/working.wasm")
        );
    }

    #[test]
    fn discovery_failure_reinserts_last_working_layout() {
        let previous = Registry {
            root: PathBuf::from("extensions"),
            extensions: vec![extension("demo", "extensions/demo/working.wasm")],
            diagnostics: Vec::new(),
        };
        let mut candidate = Registry {
            root: PathBuf::from("extensions"),
            extensions: Vec::new(),
            diagnostics: Vec::new(),
        };

        preserve_last_working_layouts(&mut candidate, &previous, &["demo".into()]);

        assert_eq!(candidate.extensions.len(), 1);
        assert_eq!(
            candidate.extensions[0].wasm,
            PathBuf::from("extensions/demo/working.wasm")
        );
    }

    #[tokio::test]
    async fn an_arc_manager_can_reload_without_replacing_its_host_handle() {
        let root = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            Manager::load(
                root.path().to_owned(),
                ExtensionContext::default(),
                Arc::new(NoopControl),
            )
            .await,
        );
        let host_handle = Arc::clone(&manager);

        manager.reload().await;

        assert!(host_handle.extension_ids().is_empty());
        assert!(host_handle.diagnostics().is_empty());
    }
}
