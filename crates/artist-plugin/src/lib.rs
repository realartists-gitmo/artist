//! Wasmtime component host for the Artist 0.4 plugin contracts.

use std::{
    ops::Deref,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

use artist_core::{
    ContextFragment as CoreContextFragment, InitialContext, PluginCapability, PluginDescriptor,
    PluginId,
};
use artist_resource::{
    FilesystemProvider, InvocationContext, ResourceError, ResourceOperation, ResourceProvider,
    ResourceReply as CoreReply, ResourceRequest as CoreRequest, ResourceRoute as CoreRoute,
    ResourceRouter, ResourceUri, SearchEngine, ToolDefinition as CoreTool, ToolError, ToolHandler,
    ToolRegistry,
};
use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::Mutex;
use wasmtime::{
    Engine, Store,
    component::{Component, HasSelf, Linker, ResourceTable},
};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

wasmtime::component::bindgen!({
    path: "../../wit",
    world: "artist-plugin",
    imports: { default: async },
    exports: { default: async },
});

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
    search: Arc<RwLock<Option<Arc<SearchEngine>>>>,
    working_directory: PathBuf,
}

#[cfg(target_os = "linux")]
pub struct PluginFabric {
    fabric: artist_resource::ResourceFabric,
    slot: Arc<RwLock<Option<Arc<SearchEngine>>>>,
    mounted_search: Arc<SearchEngine>,
}

#[cfg(target_os = "linux")]
impl Deref for PluginFabric {
    type Target = artist_resource::ResourceFabric;

    fn deref(&self) -> &Self::Target {
        &self.fabric
    }
}

#[cfg(target_os = "linux")]
impl Drop for PluginFabric {
    fn drop(&mut self) {
        let mut slot = self.slot.write().expect("search service lock poisoned");
        if slot
            .as_ref()
            .is_some_and(|search| Arc::ptr_eq(search, &self.mounted_search))
        {
            *slot = None;
        }
    }
}

impl PluginHost {
    pub async fn new() -> Result<Self, PluginError> {
        let registry = ToolRegistry::new();
        let router = ResourceRouter::new();
        let search = Arc::new(RwLock::new(None));
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.async_support(true);
        let engine = Engine::new(&config)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        ArtistPlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        let working_directory = std::env::current_dir()?;
        Ok(Self {
            engine,
            linker,
            plugins: Vec::new(),
            registry,
            router,
            search,
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
    pub async fn mount_fabric(&self) -> Result<PluginFabric, PluginError> {
        let fabric = artist_resource::ResourceFabric::mount(
            self.router.clone(),
            self.working_directory.clone(),
            tokio::runtime::Handle::current(),
        )
        .await
        .map_err(PluginError::Tool)?;
        let mounted_search = fabric.search().clone();
        *self.search.write().expect("search service lock poisoned") = Some(mounted_search.clone());
        Ok(PluginFabric {
            fabric,
            slot: self.search.clone(),
            mounted_search,
        })
    }

    pub async fn load(&mut self, path: impl AsRef<Path>) -> Result<PluginDescriptor, PluginError> {
        let component = Component::from_file(&self.engine, path)?;
        let mut store = Store::new(
            &self.engine,
            HostState::new(
                self.registry.clone(),
                self.router.clone(),
                self.search.clone(),
                self.working_directory.clone(),
            )?,
        );
        let bindings =
            ArtistPlugin::instantiate_async(&mut store, &component, &self.linker).await?;
        let raw = bindings
            .artist_plugin_lifecycle()
            .call_descriptor(&mut store)
            .await?;
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
                .call_definitions(&mut store)
                .await?
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
                .call_routes(&mut store)
                .await?
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
            self.registry.register_owned(
                core,
                Arc::new(WasmTool {
                    plugin: plugin.clone(),
                    name: definition.name,
                }),
                descriptor.id.to_string(),
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
            self.router
                .register(descriptor.id.to_string(), core, resource_provider.clone())
                .await?;
        }
        self.plugins.push(plugin);
        Ok(descriptor)
    }

    pub async fn descriptors(&self) -> Vec<PluginDescriptor> {
        let mut descriptors = Vec::with_capacity(self.plugins.len());
        for plugin in &self.plugins {
            descriptors.push(plugin.lock().await.descriptor.clone());
        }
        descriptors
    }

    pub async fn compose_initial_context(
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
                )
                .await?
                .into_iter()
                .map(|f| CoreContextFragment {
                    source: f.source,
                    content: f.content,
                })
                .collect(),
        })
    }
    pub async fn compose_prompt(
        &mut self,
        mut fragments: Vec<ContextFragment>,
    ) -> Result<Vec<ContextFragment>, PluginError> {
        for plugin in self.with(PluginCapability::Prompt).await {
            let mut p = plugin.lock().await;
            let id = p.descriptor.id.to_string();
            let LoadedPlugin {
                store, bindings, ..
            } = &mut *p;
            fragments = bindings
                .artist_plugin_lifecycle()
                .call_compose_prompt(store, &fragments)
                .await?
                .map_err(|message| PluginError::Socket {
                    plugin: id,
                    socket: "compose-prompt",
                    message,
                })?;
        }
        Ok(fragments)
    }
    pub async fn tools(&self) -> Result<Vec<ToolDefinition>, PluginError> {
        Ok(self
            .registry
            .definitions()
            .into_iter()
            .map(|d| ToolDefinition {
                name: d.name,
                description: d.description,
                input_schema: d.input_schema.to_string(),
            })
            .collect())
    }
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<Option<String>, PluginError> {
        let arguments =
            serde_json::from_str(arguments).map_err(|e| ToolError::Arguments(e.to_string()))?;
        match self.registry.call(name, arguments).await {
            Ok(value) => Ok(Some(value.to_string())),
            Err(ToolError::Unknown(_)) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub async fn handle_resource(&self, request: CoreRequest) -> Result<CoreReply, PluginError> {
        self.router
            .handle(request)
            .await
            .map_err(PluginError::Resource)
    }
    pub async fn transform_context(
        &mut self,
        mut messages: Vec<Message>,
    ) -> Result<Vec<Message>, PluginError> {
        for plugin in self.with(PluginCapability::Context).await {
            let mut p = plugin.lock().await;
            let id = p.descriptor.id.to_string();
            let LoadedPlugin {
                store, bindings, ..
            } = &mut *p;
            messages = bindings
                .artist_plugin_lifecycle()
                .call_transform_context(store, &messages)
                .await?
                .map_err(|message| PluginError::Socket {
                    plugin: id,
                    socket: "transform-context",
                    message,
                })?;
        }
        Ok(messages)
    }
    pub async fn observe_hook(
        &mut self,
        event: &HookEvent,
    ) -> Result<Vec<HookDecision>, PluginError> {
        let mut decisions = Vec::new();
        for plugin in self.with(PluginCapability::Hooks).await {
            let decision = {
                let mut p = plugin.lock().await;
                let id = p.descriptor.id.to_string();
                let LoadedPlugin {
                    store, bindings, ..
                } = &mut *p;
                bindings
                    .artist_plugin_lifecycle()
                    .call_observe_hook(store, event)
                    .await?
                    .map_err(|message| PluginError::Socket {
                        plugin: id,
                        socket: "observe-hook",
                        message,
                    })?
            };
            decisions.push(decision);
        }
        Ok(decisions)
    }
    pub async fn configure_model(
        &mut self,
        mut config: ModelConfig,
    ) -> Result<ModelConfig, PluginError> {
        for plugin in self.with(PluginCapability::Model).await {
            let mut p = plugin.lock().await;
            let id = p.descriptor.id.to_string();
            let LoadedPlugin {
                store, bindings, ..
            } = &mut *p;
            config = bindings
                .artist_plugin_lifecycle()
                .call_configure_model(store, &config)
                .await?
                .map_err(|message| PluginError::Socket {
                    plugin: id,
                    socket: "configure-model",
                    message,
                })?;
        }
        Ok(config)
    }
    pub async fn observe_event(&mut self, event: &str) -> Result<(), PluginError> {
        for plugin in self.with(PluginCapability::Events).await {
            let mut p = plugin.lock().await;
            let id = p.descriptor.id.to_string();
            let LoadedPlugin {
                store, bindings, ..
            } = &mut *p;
            bindings
                .artist_plugin_lifecycle()
                .call_observe_event(store, event)
                .await?
                .map_err(|message| PluginError::Socket {
                    plugin: id,
                    socket: "observe-event",
                    message,
                })?;
        }
        Ok(())
    }
    async fn with(&self, capability: PluginCapability) -> Vec<Arc<Mutex<LoadedPlugin>>> {
        let mut selected = Vec::new();
        for plugin in &self.plugins {
            if plugin
                .lock()
                .await
                .descriptor
                .capabilities
                .contains(&capability)
            {
                selected.push(plugin.clone());
            }
        }
        selected
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
        let mut plugin = self.plugin.lock().await;
        plugin.store.data_mut().invocation = Some(context);
        let result = {
            let LoadedPlugin {
                store, bindings, ..
            } = &mut *plugin;
            bindings
                .artist_plugin_tool_provider()
                .call_invoke(store, &self.name, &arguments.to_string())
                .await
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
        let mut plugin = self.plugin.lock().await;
        let request = to_wit_request(request);
        let LoadedPlugin {
            store, bindings, ..
        } = &mut *plugin;
        let reply = bindings
            .artist_plugin_resource_provider()
            .call_handle(store, &request)
            .await
            .map_err(|e| ResourceError::Provider(e.to_string()))?
            .map_err(|e| ResourceError::Provider(format!("{}: {}", e.kind, e.message)))?;
        from_wit_reply(reply)
    }
}

struct HostState {
    table: ResourceTable,
    wasi: WasiCtx,
    registry: ToolRegistry,
    router: ResourceRouter,
    search: Arc<RwLock<Option<Arc<SearchEngine>>>>,
    filesystem: Arc<FilesystemProvider>,
    working_directory: PathBuf,
    invocation: Option<InvocationContext>,
    plugin_id: Option<String>,
}
impl HostState {
    fn new(
        registry: ToolRegistry,
        router: ResourceRouter,
        search: Arc<RwLock<Option<Arc<SearchEngine>>>>,
        working_directory: PathBuf,
    ) -> Result<Self, std::io::Error> {
        Ok(Self {
            table: ResourceTable::new(),
            wasi: WasiCtx::builder().build(),
            registry,
            router,
            search,
            filesystem: Arc::new(FilesystemProvider::new(&working_directory)),
            working_directory,
            invocation: None,
            plugin_id: None,
        })
    }
    fn uri(&self, text: &str) -> Result<ResourceUri, String> {
        ResourceUri::resolve(text, &self.working_directory).map_err(|e| e.to_string())
    }

    fn search(&self) -> Result<Arc<SearchEngine>, String> {
        self.search
            .read()
            .expect("search service lock poisoned")
            .clone()
            .ok_or_else(|| "search index is not mounted".into())
    }

    fn decode_wit_request(
        &self,
        request: artist::plugin::types::ResourceRequest,
    ) -> Result<CoreRequest, String> {
        use artist::plugin::types::ResourceRequest as W;
        Ok(match request {
            W::Read(request) => CoreRequest::Read {
                uri: self.uri(&request.uri)?,
                start_line: request.start_line,
                line_count: request.line_count,
            },
            W::Children(uri) => CoreRequest::Children {
                uri: self.uri(&uri)?,
            },
            W::Write(request) => CoreRequest::Write {
                uri: self.uri(&request.uri)?,
                text: request.text,
            },
            W::Move(request) => CoreRequest::Move {
                from: self.uri(&request.source)?,
                to: request.to.as_deref().map(|uri| self.uri(uri)).transpose()?,
            },
            W::Poll(request) => CoreRequest::Poll {
                uri: self.uri(&request.uri)?,
                pattern: request.match_,
                timeout: request.timeout_ms.map(Duration::from_millis),
            },
            W::Edit(request) => CoreRequest::Edit {
                uri: self.uri(&request.uri)?,
                instructions: request.instructions,
            },
        })
    }
}

impl artist::plugin::host_tools::Host for HostState {
    async fn list_tools(&mut self) -> Result<Vec<artist::plugin::types::ToolDefinition>, String> {
        Ok(self
            .registry
            .definitions()
            .into_iter()
            .map(|d| artist::plugin::types::ToolDefinition {
                name: d.name,
                description: d.description,
                input_schema: d.input_schema.to_string(),
            })
            .collect())
    }
    async fn call_tool(&mut self, name: String, arguments: String) -> Result<String, String> {
        let args = serde_json::from_str(&arguments).map_err(|e| e.to_string())?;
        let ctx = self
            .invocation
            .clone()
            .unwrap_or_else(InvocationContext::root);
        if self.plugin_id.as_deref() == self.registry.owner(&name).as_deref() {
            let mut cycle = ctx.stack.clone();
            cycle.push(name.clone());
            return Err(ToolError::Recursive {
                cycle: cycle.join(" -> "),
            }
            .to_string());
        }
        self.registry
            .call_with_context(&name, args, ctx)
            .await
            .map(|v| v.to_string())
            .map_err(|e| e.to_string())
    }
}

impl artist::plugin::host_resources::Host for HostState {
    async fn handle(
        &mut self,
        request: artist::plugin::types::ResourceRequest,
    ) -> Result<artist::plugin::types::ResourceReply, artist::plugin::types::ResourceError> {
        let request = self
            .decode_wit_request(request)
            .map_err(|message| wit_resource_error("invalid", message))?;
        self.router
            .handle(request)
            .await
            .map(core_to_wit_reply)
            .map_err(core_to_wit_error)
    }

    async fn find(
        &mut self,
        request: artist::plugin::types::FindRequest,
    ) -> Result<String, String> {
        let search = self.search()?;
        search.synchronize(self.router.generation())?;
        search
            .find(
                &self.uri(&request.uri)?,
                request.glob.as_deref(),
                optional_usize(request.max_depth, "max_depth")?,
                request.cursor.as_deref(),
                optional_usize(request.limit, "limit")?.unwrap_or(50),
            )
            .map(|value| value.to_string())
    }

    async fn grep(
        &mut self,
        request: artist::plugin::types::GrepRequest,
    ) -> Result<String, String> {
        let search = self.search()?;
        search.synchronize(self.router.generation())?;
        search
            .grep(
                &self.uri(&request.uri)?,
                &request.regex,
                request.include_glob.as_deref(),
                optional_usize(request.context, "context")?.unwrap_or(0),
                request.cursor.as_deref(),
                optional_usize(request.limit, "limit")?.unwrap_or(100),
            )
            .map(|value| value.to_string())
    }
}

impl artist::plugin::native_filesystem::Host for HostState {
    async fn read(
        &mut self,
        uri: String,
        start_line: Option<u64>,
        line_count: Option<u64>,
    ) -> Result<String, String> {
        let uri = self.uri(&uri)?;
        match self
            .filesystem
            .handle(CoreRequest::Read {
                uri,
                start_line,
                line_count,
            })
            .await
            .map_err(|error| error.to_string())?
        {
            CoreReply::Text { text } => Ok(text),
            _ => Err("filesystem provider returned a non-text reply".into()),
        }
    }
    async fn children(&mut self, uri: String) -> Result<Vec<String>, String> {
        let uri = self.uri(&uri)?;
        match self
            .filesystem
            .handle(CoreRequest::Children { uri })
            .await
            .map_err(|error| error.to_string())?
        {
            CoreReply::Children { children } => {
                Ok(children.into_iter().map(|uri| uri.to_string()).collect())
            }
            _ => Err("filesystem provider returned a non-children reply".into()),
        }
    }
    async fn write(&mut self, uri: String, text: String) -> Result<(), String> {
        let uri = self.uri(&uri)?;
        match self
            .filesystem
            .handle(CoreRequest::Write { uri, text })
            .await
            .map_err(|error| error.to_string())?
        {
            CoreReply::Written => Ok(()),
            _ => Err("filesystem provider returned a non-written reply".into()),
        }
    }
    async fn move_(&mut self, from: String, to: Option<String>) -> Result<(), String> {
        let from = self.uri(&from)?;
        let to = to.map(|uri| self.uri(&uri)).transpose()?;
        match self
            .filesystem
            .handle(CoreRequest::Move { from, to })
            .await
            .map_err(|error| error.to_string())?
        {
            CoreReply::Moved => Ok(()),
            _ => Err("filesystem provider returned a non-moved reply".into()),
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

fn core_to_wit_reply(reply: CoreReply) -> artist::plugin::types::ResourceReply {
    use artist::plugin::types::{PollOutcome as W, ResourceReply as R};
    match reply {
        CoreReply::Text { text } => R::Text(text),
        CoreReply::Children { children } => {
            R::Children(children.into_iter().map(|uri| uri.to_string()).collect())
        }
        CoreReply::Written => R::Written,
        CoreReply::Moved => R::Moved,
        CoreReply::Poll { text, outcome } => R::Poll(artist::plugin::types::PollReply {
            text,
            outcome: match outcome {
                artist_resource::PollOutcome::Matched => W::Matched,
                artist_resource::PollOutcome::Closed => W::Closed,
                artist_resource::PollOutcome::TimedOut => W::TimedOut,
            },
        }),
    }
}

fn core_to_wit_error(error: ResourceError) -> artist::plugin::types::ResourceError {
    let kind = match &error {
        ResourceError::NotFound { .. } => "not-found",
        ResourceError::Unsupported { .. } => "unsupported",
        ResourceError::Invalid(_) => "invalid",
        ResourceError::Provider(_) => "provider",
    };
    wit_resource_error(kind, error.to_string())
}

fn wit_resource_error(
    kind: &str,
    message: impl Into<String>,
) -> artist::plugin::types::ResourceError {
    artist::plugin::types::ResourceError {
        kind: kind.into(),
        message: message.into(),
    }
}

fn optional_usize(value: Option<u64>, field: &str) -> Result<Option<usize>, String> {
    value
        .map(|value| {
            usize::try_from(value).map_err(|_| format!("{field} is too large for this host"))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn host_initializes_without_native_model_tools_or_a_private_runtime() {
        let host = PluginHost::new().await.unwrap();
        assert!(host.tools().await.unwrap().is_empty());
    }
}
