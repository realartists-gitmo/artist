//! Wasmtime component host for the Artist 0.3 plugin contracts.

use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use artist_core::{
    ContextFragment as CoreContextFragment, InitialContext, PluginCapability, PluginDescriptor,
    PluginId,
};
use artist_resource::{
    InvocationContext, ResourceError, ResourceOperation, ResourceProvider,
    ResourceReply as CoreReply, ResourceRequest as CoreRequest, ResourceRoute as CoreRoute,
    ResourceRouter, ResourceUri, ToolDefinition as CoreTool, ToolError, ToolHandler, ToolRegistry,
    UniversalTools,
};
use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio::runtime::Runtime;
use wasmtime::{
    Engine, Store,
    component::{Component, HasSelf, Linker, ResourceTable},
};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

wasmtime::component::bindgen!({ path: "../../wit", world: "artist-plugin" });

pub use artist::plugin::types::{
    ContextFragment, HookDecision, HookEvent, Message, ModelConfig, ResourceRoute, ToolDefinition,
};

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("component runtime failed: {0}")]
    Runtime(#[from] wasmtime::Error),
    #[error("plugin host I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("plugin {plugin} failed in {socket}: {message}")]
    Socket {
        plugin: String,
        socket: &'static str,
        message: String,
    },
    #[error("tool registry failed: {0}")]
    Tool(#[from] ToolError),
    #[error("resource registry failed: {0}")]
    Resource(#[from] ResourceError),
}

pub struct PluginHost {
    engine: Engine,
    linker: Linker<HostState>,
    plugins: Vec<Arc<Mutex<LoadedPlugin>>>,
    registry: ToolRegistry,
    router: ResourceRouter,
    runtime: Arc<Runtime>,
    working_directory: PathBuf,
}

impl PluginHost {
    pub fn new() -> Result<Self, PluginError> {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?,
        );
        let registry = ToolRegistry::new();
        let router = ResourceRouter::new();
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        ArtistPlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        let working_directory = std::env::current_dir()?;
        block_on(
            &runtime,
            UniversalTools::new(router.clone(), working_directory.clone()).register(&registry),
        )?;
        Ok(Self {
            engine,
            linker,
            plugins: Vec::new(),
            registry,
            router,
            runtime,
            working_directory,
        })
    }

    pub fn registry(&self) -> ToolRegistry {
        self.registry.clone()
    }
    pub fn router(&self) -> ResourceRouter {
        self.router.clone()
    }

    #[cfg(target_os = "linux")]
    pub fn mount_fabric(&self) -> Result<artist_resource::ResourceFabric, PluginError> {
        block_on(
            &self.runtime,
            artist_resource::ResourceFabric::mount_shared(
                self.router.clone(),
                self.registry.clone(),
                self.working_directory.clone(),
                self.runtime.handle().clone(),
            ),
        )
        .map_err(PluginError::Tool)
    }

    pub fn load(&mut self, path: impl AsRef<Path>) -> Result<PluginDescriptor, PluginError> {
        let component = Component::from_file(&self.engine, path)?;
        let mut store = Store::new(
            &self.engine,
            HostState::new(
                self.registry.clone(),
                self.runtime.clone(),
                self.working_directory.clone(),
            )?,
        );
        let bindings = ArtistPlugin::instantiate(&mut store, &component, &self.linker)?;
        let raw = bindings
            .artist_plugin_lifecycle()
            .call_descriptor(&mut store)?;
        let descriptor = PluginDescriptor {
            id: PluginId::new(raw.id),
            version: raw.version,
            capabilities: raw.capabilities.into_iter().map(capability).collect(),
        };
        store.data_mut().plugin_id = Some(descriptor.id.to_string());
        // The WIT world keeps every socket statically typed, while the
        // descriptor decides which sockets a component actually participates
        // in. Never probe an unadvertised socket: narrow components are free
        // to make their mandatory ABI stubs fail loudly.
        let tool_definitions = if descriptor.capabilities.contains(&PluginCapability::Tools) {
            bindings
                .artist_plugin_tool_provider()
                .call_definitions(&mut store)?
                .map_err(|message| socket(&descriptor, "definitions", message))?
        } else {
            Vec::new()
        };
        let routes = if descriptor
            .capabilities
            .contains(&PluginCapability::Resources)
        {
            bindings
                .artist_plugin_resource_provider()
                .call_routes(&mut store)?
                .map_err(|message| socket(&descriptor, "routes", message))?
        } else {
            Vec::new()
        };
        let plugin = Arc::new(Mutex::new(LoadedPlugin {
            store,
            bindings,
            descriptor: descriptor.clone(),
        }));
        for definition in tool_definitions {
            let schema = serde_json::from_str(&definition.input_schema).map_err(|e| {
                PluginError::Tool(ToolError::Failed(format!(
                    "invalid schema for {}: {e}",
                    definition.name
                )))
            })?;
            let core = CoreTool {
                name: definition.name.clone(),
                description: definition.description,
                input_schema: schema,
            };
            block_on(
                &self.runtime,
                self.registry.register_owned(
                    core,
                    Arc::new(WasmTool {
                        plugin: plugin.clone(),
                        name: definition.name,
                    }),
                    descriptor.id.to_string(),
                ),
            )?;
        }
        let resource_provider: Arc<dyn ResourceProvider> = Arc::new(WasmResource {
            plugin: plugin.clone(),
        });
        for route in routes {
            let core = CoreRoute::new(
                route.base_glob,
                route.projection_glob,
                route.operations.into_iter().map(resource_operation),
            );
            block_on(
                &self.runtime,
                self.router
                    .register(descriptor.id.to_string(), core, resource_provider.clone()),
            )?;
        }
        self.plugins.push(plugin);
        Ok(descriptor)
    }

    pub fn descriptors(&self) -> impl Iterator<Item = PluginDescriptor> + '_ {
        self.plugins
            .iter()
            .map(|p| p.lock().unwrap().descriptor.clone())
    }

    pub fn compose_initial_context(
        &mut self,
        context: InitialContext,
    ) -> Result<InitialContext, PluginError> {
        Ok(InitialContext {
            fragments: self
                .compose_prompt(
                    context
                        .fragments
                        .into_iter()
                        .map(|f| ContextFragment {
                            source: f.source,
                            content: f.content,
                        })
                        .collect(),
                )?
                .into_iter()
                .map(|f| CoreContextFragment {
                    source: f.source,
                    content: f.content,
                })
                .collect(),
        })
    }
    pub fn compose_prompt(
        &mut self,
        mut fragments: Vec<ContextFragment>,
    ) -> Result<Vec<ContextFragment>, PluginError> {
        for plugin in self.with(PluginCapability::Prompt) {
            let mut p = plugin.lock().unwrap();
            let id = p.descriptor.id.to_string();
            let LoadedPlugin {
                store, bindings, ..
            } = &mut *p;
            fragments = bindings
                .artist_plugin_lifecycle()
                .call_compose_prompt(store, &fragments)?
                .map_err(|message| PluginError::Socket {
                    plugin: id,
                    socket: "compose-prompt",
                    message,
                })?;
        }
        Ok(fragments)
    }
    pub fn tools(&self) -> Result<Vec<ToolDefinition>, PluginError> {
        Ok(block_on(&self.runtime, self.registry.definitions())
            .into_iter()
            .map(|d| ToolDefinition {
                name: d.name,
                description: d.description,
                input_schema: d.input_schema.to_string(),
            })
            .collect())
    }
    pub fn call_tool(&self, name: &str, arguments: &str) -> Result<Option<String>, PluginError> {
        let arguments =
            serde_json::from_str(arguments).map_err(|e| ToolError::Arguments(e.to_string()))?;
        match block_on(&self.runtime, self.registry.call(name, arguments)) {
            Ok(value) => Ok(Some(value.to_string())),
            Err(ToolError::Unknown(_)) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn handle_resource(&self, request: CoreRequest) -> Result<CoreReply, PluginError> {
        block_on(&self.runtime, self.router.handle(request)).map_err(PluginError::Resource)
    }
    pub fn transform_context(
        &mut self,
        mut messages: Vec<Message>,
    ) -> Result<Vec<Message>, PluginError> {
        for plugin in self.with(PluginCapability::Context) {
            let mut p = plugin.lock().unwrap();
            let id = p.descriptor.id.to_string();
            let LoadedPlugin {
                store, bindings, ..
            } = &mut *p;
            messages = bindings
                .artist_plugin_lifecycle()
                .call_transform_context(store, &messages)?
                .map_err(|message| PluginError::Socket {
                    plugin: id,
                    socket: "transform-context",
                    message,
                })?;
        }
        Ok(messages)
    }
    pub fn observe_hook(&mut self, event: &HookEvent) -> Result<Vec<HookDecision>, PluginError> {
        self.with(PluginCapability::Hooks)
            .into_iter()
            .map(|plugin| {
                let mut p = plugin.lock().unwrap();
                let id = p.descriptor.id.to_string();
                let LoadedPlugin {
                    store, bindings, ..
                } = &mut *p;
                bindings
                    .artist_plugin_lifecycle()
                    .call_observe_hook(store, event)?
                    .map_err(|message| PluginError::Socket {
                        plugin: id,
                        socket: "observe-hook",
                        message,
                    })
            })
            .collect()
    }
    pub fn configure_model(&mut self, mut config: ModelConfig) -> Result<ModelConfig, PluginError> {
        for plugin in self.with(PluginCapability::Model) {
            let mut p = plugin.lock().unwrap();
            let id = p.descriptor.id.to_string();
            let LoadedPlugin {
                store, bindings, ..
            } = &mut *p;
            config = bindings
                .artist_plugin_lifecycle()
                .call_configure_model(store, &config)?
                .map_err(|message| PluginError::Socket {
                    plugin: id,
                    socket: "configure-model",
                    message,
                })?;
        }
        Ok(config)
    }
    pub fn observe_event(&mut self, event: &str) -> Result<(), PluginError> {
        for plugin in self.with(PluginCapability::Events) {
            let mut p = plugin.lock().unwrap();
            let id = p.descriptor.id.to_string();
            let LoadedPlugin {
                store, bindings, ..
            } = &mut *p;
            bindings
                .artist_plugin_lifecycle()
                .call_observe_event(store, event)?
                .map_err(|message| PluginError::Socket {
                    plugin: id,
                    socket: "observe-event",
                    message,
                })?;
        }
        Ok(())
    }
    fn with(&self, capability: PluginCapability) -> Vec<Arc<Mutex<LoadedPlugin>>> {
        self.plugins
            .iter()
            .filter(|p| {
                p.lock()
                    .unwrap()
                    .descriptor
                    .capabilities
                    .contains(&capability)
            })
            .cloned()
            .collect()
    }
}

struct LoadedPlugin {
    store: Store<HostState>,
    bindings: ArtistPlugin,
    descriptor: PluginDescriptor,
}
fn socket(descriptor: &PluginDescriptor, socket: &'static str, message: String) -> PluginError {
    PluginError::Socket {
        plugin: descriptor.id.to_string(),
        socket,
        message,
    }
}

struct WasmTool {
    plugin: Arc<Mutex<LoadedPlugin>>,
    name: String,
}
#[async_trait]
impl ToolHandler for WasmTool {
    async fn call(&self, arguments: Value, context: InvocationContext) -> Result<Value, ToolError> {
        let mut plugin = self
            .plugin
            .lock()
            .map_err(|_| ToolError::Failed("plugin lock poisoned".into()))?;
        plugin.store.data_mut().invocation = Some(context);
        let result = {
            let LoadedPlugin {
                store, bindings, ..
            } = &mut *plugin;
            bindings.artist_plugin_tool_provider().call_invoke(
                store,
                &self.name,
                &arguments.to_string(),
            )
        };
        plugin.store.data_mut().invocation = None;
        let result = result
            .map_err(|e| ToolError::Failed(e.to_string()))?
            .map_err(ToolError::Failed)?;
        Ok(serde_json::from_str(&result).unwrap_or(Value::String(result)))
    }
}

struct WasmResource {
    plugin: Arc<Mutex<LoadedPlugin>>,
}
#[async_trait]
impl ResourceProvider for WasmResource {
    async fn handle(&self, request: CoreRequest) -> Result<CoreReply, ResourceError> {
        let mut plugin = self
            .plugin
            .lock()
            .map_err(|_| ResourceError::Provider("plugin lock poisoned".into()))?;
        let request = to_wit_request(request);
        let LoadedPlugin {
            store, bindings, ..
        } = &mut *plugin;
        let reply = bindings
            .artist_plugin_resource_provider()
            .call_handle(store, &request)
            .map_err(|e| ResourceError::Provider(e.to_string()))?
            .map_err(|e| ResourceError::Provider(format!("{}: {}", e.kind, e.message)))?;
        from_wit_reply(reply)
    }
}

struct HostState {
    table: ResourceTable,
    wasi: WasiCtx,
    registry: ToolRegistry,
    runtime: Arc<Runtime>,
    working_directory: PathBuf,
    invocation: Option<InvocationContext>,
    plugin_id: Option<String>,
}
impl HostState {
    fn new(
        registry: ToolRegistry,
        runtime: Arc<Runtime>,
        working_directory: PathBuf,
    ) -> Result<Self, std::io::Error> {
        Ok(Self {
            table: ResourceTable::new(),
            wasi: WasiCtx::builder().build(),
            registry,
            runtime,
            working_directory,
            invocation: None,
            plugin_id: None,
        })
    }
    fn uri(&self, text: &str) -> Result<ResourceUri, String> {
        ResourceUri::resolve(text, &self.working_directory).map_err(|e| e.to_string())
    }
}

impl artist::plugin::host_tools::Host for HostState {
    fn list_tools(&mut self) -> Result<Vec<artist::plugin::types::ToolDefinition>, String> {
        let registry = self.registry.clone();
        Ok(block_on(&self.runtime, registry.definitions())
            .into_iter()
            .map(|d| artist::plugin::types::ToolDefinition {
                name: d.name,
                description: d.description,
                input_schema: d.input_schema.to_string(),
            })
            .collect())
    }
    fn call_tool(&mut self, name: String, arguments: String) -> Result<String, String> {
        let args = serde_json::from_str(&arguments).map_err(|e| e.to_string())?;
        let ctx = self
            .invocation
            .clone()
            .unwrap_or_else(InvocationContext::root);
        let registry = self.registry.clone();
        if self.plugin_id.as_deref() == block_on(&self.runtime, registry.owner(&name)).as_deref() {
            let mut cycle = ctx.stack.clone();
            cycle.push(name.clone());
            return Err(ToolError::Recursive {
                cycle: cycle.join(" -> "),
            }
            .to_string());
        }
        block_on(&self.runtime, registry.call_with_context(&name, args, ctx))
            .map(|v| v.to_string())
            .map_err(|e| e.to_string())
    }
}

fn block_on<F: Future>(runtime: &Runtime, future: F) -> F::Output {
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| runtime.block_on(future))
    } else {
        runtime.block_on(future)
    }
}

impl artist::plugin::native_filesystem::Host for HostState {
    fn read(
        &mut self,
        uri: String,
        start_line: Option<u64>,
        line_count: Option<u64>,
    ) -> Result<String, String> {
        let uri = self.uri(&uri)?;
        let path = uri.file_path().ok_or("not a file URI")?;
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let start = start_line.unwrap_or(1).saturating_sub(1) as usize;
        Ok(text
            .split_inclusive('\n')
            .skip(start)
            .take(line_count.unwrap_or(u64::MAX) as usize)
            .collect())
    }
    fn children(&mut self, uri: String) -> Result<Vec<String>, String> {
        let path = self.uri(&uri)?.file_path().ok_or("not a file URI")?;
        let mut out = std::fs::read_dir(path)
            .map_err(|e| e.to_string())?
            .map(|e| {
                e.map_err(|e| e.to_string()).and_then(|e| {
                    ResourceUri::resolve(&e.path().to_string_lossy(), Path::new("/"))
                        .map(|u| u.to_string())
                        .map_err(|e| e.to_string())
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        out.sort();
        Ok(out)
    }
    fn write(&mut self, uri: String, text: String) -> Result<(), String> {
        let path = self.uri(&uri)?.file_path().ok_or("not a file URI")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, text).map_err(|e| e.to_string())
    }
    fn move_(&mut self, from: String, to: Option<String>) -> Result<(), String> {
        let from = self.uri(&from)?.file_path().ok_or("not a file URI")?;
        if let Some(to) = to {
            let to = self.uri(&to)?.file_path().ok_or("not a file URI")?;
            std::fs::rename(from, to).map_err(|e| e.to_string())
        } else if from.is_dir() {
            std::fs::remove_dir_all(from).map_err(|e| e.to_string())
        } else {
            std::fs::remove_file(from).map_err(|e| e.to_string())
        }
    }
}
impl artist::plugin::types::Host for HostState {}
impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

fn capability(value: artist::plugin::types::Capability) -> PluginCapability {
    use artist::plugin::types::Capability as W;
    match value {
        W::Prompt => PluginCapability::Prompt,
        W::Tools => PluginCapability::Tools,
        W::Resources => PluginCapability::Resources,
        W::Context => PluginCapability::Context,
        W::Hooks => PluginCapability::Hooks,
        W::Model => PluginCapability::Model,
        W::Events => PluginCapability::Events,
    }
}
fn resource_operation(value: artist::plugin::types::ResourceOperation) -> ResourceOperation {
    use artist::plugin::types::ResourceOperation as W;
    match value {
        W::Read => ResourceOperation::Read,
        W::Children => ResourceOperation::Children,
        W::Write => ResourceOperation::Write,
        W::Move => ResourceOperation::Move,
        W::Poll => ResourceOperation::Poll,
        W::Edit => ResourceOperation::Edit,
    }
}

fn to_wit_request(request: CoreRequest) -> artist::plugin::types::ResourceRequest {
    use artist::plugin::types as w;
    match request {
        CoreRequest::Read {
            uri,
            start_line,
            line_count,
        } => w::ResourceRequest::Read(w::ReadRequest {
            uri: uri.to_string(),
            start_line,
            line_count,
        }),
        CoreRequest::Children { uri } => w::ResourceRequest::Children(uri.to_string()),
        CoreRequest::Write { uri, text } => w::ResourceRequest::Write(w::WriteRequest {
            uri: uri.to_string(),
            text,
        }),
        CoreRequest::Move { from, to } => w::ResourceRequest::Move(w::MoveRequest {
            source: from.to_string(),
            to: to.map(|u| u.to_string()),
        }),
        CoreRequest::Poll {
            uri,
            pattern,
            timeout,
        } => w::ResourceRequest::Poll(w::PollRequest {
            uri: uri.to_string(),
            match_: pattern,
            timeout_ms: timeout.map(|d| d.as_millis() as u64),
        }),
        CoreRequest::Edit { uri, instructions } => w::ResourceRequest::Edit(w::EditRequest {
            uri: uri.to_string(),
            instructions,
        }),
    }
}
fn from_wit_reply(reply: artist::plugin::types::ResourceReply) -> Result<CoreReply, ResourceError> {
    use artist::plugin::types::{PollOutcome as W, ResourceReply as R};
    Ok(match reply {
        R::Text(text) => CoreReply::Text { text },
        R::Children(children) => CoreReply::Children {
            children: children
                .into_iter()
                .map(|u| {
                    ResourceUri::resolve(&u, Path::new("/"))
                        .map_err(|e| ResourceError::Provider(e.to_string()))
                })
                .collect::<Result<_, _>>()?,
        },
        R::Written => CoreReply::Written,
        R::Moved => CoreReply::Moved,
        R::Poll(p) => CoreReply::Poll {
            text: p.text,
            outcome: match p.outcome {
                W::Matched => artist_resource::PollOutcome::Matched,
                W::Closed => artist_resource::PollOutcome::Closed,
                W::TimedOut => artist_resource::PollOutcome::TimedOut,
            },
        },
    })
}
