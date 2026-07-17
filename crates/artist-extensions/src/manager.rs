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
}

impl Manager {
    pub async fn load(
        root: PathBuf,
        mut context: ExtensionContext,
        control: Arc<dyn HostControl>,
    ) -> Self {
        let registry = Registry::load(root);
        let events = EventBus::new(64);
        context.recent_events = events.recent();
        let mut instances = HashMap::new();
        for extension in &registry.extensions {
            if let Ok(instance) = Instance::load(extension, &context, control.clone()).await {
                instances.insert(extension.manifest.id.clone(), Arc::new(instance));
            }
        }
        let manager = Self {
            registry,
            instances,
            statuses: Default::default(),
            events,
        };
        manager.start_status_refresh();
        manager.start_event_forwarding();
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

    fn start_event_forwarding(&self) {
        let mut receiver = self.events.subscribe();
        let instances = self.instances.values().cloned().collect::<Vec<_>>();
        tokio::spawn(async move {
            while let Ok(json) = receiver.recv().await {
                for instance in &instances {
                    let _ = instance.event(&json).await;
                }
            }
        });
    }

    fn start_status_refresh(&self) {
        for (manifest, declaration) in self.registry.status_items() {
            let Some(instance) = self.instances.get(&manifest.id).cloned() else {
                continue;
            };
            let cache = self.statuses.clone();
            let name = declaration.name.clone();
            let interval = declaration.refresh_ms.max(100);
            tokio::spawn(async move {
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
        }
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
