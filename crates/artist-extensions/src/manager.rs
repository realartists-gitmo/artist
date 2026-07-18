use crate::{Diagnostic, Event, EventBus, ExtensionContext, HostControl, Instance, Registry};
use anyhow::{Result, anyhow};
use rig_core::tool::{ToolDyn, ToolError};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Duration,
};

/// A live extension set. Instances survive for the lifetime of the manager,
/// allowing guests to retain state between tool, command, status, and event calls.
pub struct Manager {
    registry: Registry,
    instances: HashMap<String, Arc<Instance>>,
    statuses: Arc<RwLock<HashMap<String, String>>>,
    events: EventBus,
    context: Arc<RwLock<ExtensionContext>>,
    /// Abort handles for the per-status-item refresh loops, cancelled on drop
    /// so they don't leak their wasm instance across a Manager reload.
    status_tasks: Vec<tokio::task::AbortHandle>,
    /// Event-forwarding tasks (one per instance). Each holds an `Arc<Instance>`
    /// which keeps a bus sender alive, so `recv()` never returns `Closed` — the
    /// task can only be stopped by aborting it, or it leaks its wasm instance
    /// forever across every reload.
    event_tasks: Vec<tokio::task::AbortHandle>,
}

impl Drop for Manager {
    fn drop(&mut self) {
        for handle in self.status_tasks.iter().chain(&self.event_tasks) {
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
        let mut manager = Self {
            registry,
            instances,
            statuses: Default::default(),
            events,
            context,
            status_tasks: Vec::new(),
            event_tasks: Vec::new(),
        };
        manager.status_tasks = manager.start_status_refresh();
        manager.event_tasks = manager.start_event_forwarding();
        manager
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.registry.diagnostics
    }

    pub fn tools(&self) -> Vec<Box<dyn ToolDyn>> {
        self.registry
            .tools()
            .filter_map(|(manifest, declaration)| {
                let instance = self.instances.get(&manifest.id)?.clone();
                Some(Box::new(ExtensionTool {
                    instance,
                    name: declaration.name.clone(),
                    description: declaration.description.clone(),
                    parameters: declaration.parameters.clone(),
                }) as Box<dyn ToolDyn>)
            })
            .collect()
    }

    pub fn commands(&self) -> Vec<crate::CommandDeclaration> {
        self.registry
            .commands()
            .filter(|(m, _)| self.instances.contains_key(&m.id))
            .map(|(_, command)| command.clone())
            .collect()
    }

    pub async fn invoke_command(&self, name: &str, arguments: &str) -> Result<String> {
        let (manifest, _) = self
            .registry
            .commands()
            .find(|(_, command)| command.name == name)
            .ok_or_else(|| anyhow!("unknown extension command {name}"))?;
        self.instances
            .get(&manifest.id)
            .ok_or_else(|| anyhow!("extension unavailable"))?
            .invoke_command(name, arguments)
            .await
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
        self.registry
            .tools()
            .filter(|(m, _)| self.instances.contains_key(&m.id))
            .map(|(_, tool)| tool.name.clone())
            .collect()
    }
    pub fn status_declarations(&self) -> Vec<crate::StatusDeclaration> {
        self.registry
            .status_items()
            .filter(|(m, _)| self.instances.contains_key(&m.id))
            .map(|(_, status)| status.clone())
            .collect()
    }

    fn start_event_forwarding(&self) -> Vec<tokio::task::AbortHandle> {
        let instances = self.instances.values().cloned().collect::<Vec<_>>();
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
        for (manifest, declaration) in self.registry.status_items() {
            let Some(instance) = self.instances.get(&manifest.id).cloned() else {
                continue;
            };
            let cache = self.statuses.clone();
            let name = declaration.name.clone();
            let interval = declaration.refresh_ms.max(100);
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
}

struct ExtensionTool {
    instance: Arc<Instance>,
    name: String,
    description: String,
    parameters: serde_json::Value,
}
impl ToolDyn for ExtensionTool {
    fn name(&self) -> String {
        self.name.clone()
    }
    fn description(&self) -> String {
        self.description.clone()
    }
    fn parameters(&self) -> serde_json::Value {
        self.parameters.clone()
    }
    fn call<'a>(
        &'a self,
        args: String,
    ) -> rig_core::wasm_compat::WasmBoxedFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let arguments = serde_json::from_str(&args).map_err(ToolError::JsonError)?;
            self.instance
                .invoke_tool(&self.name, &arguments)
                .await
                .map_err(|error| {
                    ToolError::ToolCallError(Box::new(std::io::Error::other(error.to_string())))
                })
        })
    }
}
