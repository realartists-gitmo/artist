//! Wasmtime component host for the Artist 0.6 plugin contracts.

mod packages;

pub use packages::{
    PACKAGE_FORMAT, PackageState, PackageStatus, PluginActivator, PluginPackageManifest,
    PluginPackages,
};

use std::{
    collections::HashMap,
    ops::Deref,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

use artist_core::{
    ContextFragment as CoreContextFragment, ContextRole as CoreContextRole, InitialContext,
    PluginCapability, PluginDescriptor, PluginId, ProfileManifest, ProfileSnapshot,
    SlashCommandAction as CoreSlashAction, SlashCommandDefinition as CoreSlashDefinition,
    SlashCommandResult as CoreSlashResult, ToolControl, ToolEffect as CoreToolEffect,
    validate_profile_name,
};
use artist_resource::{
    AnchoredDocument, AnchoredEditOperation, EnvironmentEntry, FilesystemProvider, GrepPage,
    IndexedGrepMatch, InvocationContext, ProfilesProvider, ResourceError, ResourceOperation,
    ResourceProvider, ResourceReply as CoreReply, ResourceRequest as CoreRequest,
    ResourceRoute as CoreRoute, ResourceRouter, ResourceUri, SearchEngine, SignalDefinition,
    TextReplacement, ToolDefinition as CoreTool, ToolError, ToolHandler,
    ToolOutput as CoreToolOutput, ToolRegistry, validate_schema,
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
    ContextFragment, ContextRole, HookDecision, HookEvent, Message, ModelConfig, ResourceRoute,
    ToolDefinition, ToolEffect,
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
    plugins: Arc<RwLock<Vec<LoadedPluginSlot>>>,
    registry: ToolRegistry,
    router: ResourceRouter,
    search: Arc<RwLock<Option<Arc<SearchEngine>>>>,
    working_directory: PathBuf,
    packages: Arc<PluginPackages>,
    slash_commands: Arc<RwLock<HashMap<String, RegisteredSlashCommand>>>,
    _activator: Arc<HostActivation>,
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
        let working_directory = std::env::current_dir()?;
        let profiles_root = working_directory.join(".artist/profiles");
        let plugins_root = working_directory.join("plugins");
        Self::new_inner(working_directory, profiles_root, plugins_root).await
    }

    pub async fn new_with_profiles(profiles_root: impl Into<PathBuf>) -> Result<Self, PluginError> {
        let working_directory = std::env::current_dir()?;
        let plugins_root = working_directory.join("plugins");
        Self::new_inner(working_directory, profiles_root.into(), plugins_root).await
    }

    pub async fn new_with_roots(
        profiles_root: impl Into<PathBuf>,
        plugins_root: impl Into<PathBuf>,
    ) -> Result<Self, PluginError> {
        Self::new_inner(
            std::env::current_dir()?,
            profiles_root.into(),
            plugins_root.into(),
        )
        .await
    }

    async fn new_inner(
        working_directory: PathBuf,
        profiles_root: PathBuf,
        plugins_root: PathBuf,
    ) -> Result<Self, PluginError> {
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
        let profiles = Arc::new(ProfilesProvider::new(profiles_root));
        let packages = Arc::new(PluginPackages::new(plugins_root));
        let linker = Arc::new(linker);
        let plugins = Arc::new(RwLock::new(Vec::new()));
        let slash_commands = Arc::new(RwLock::new(HashMap::new()));
        let activator = Arc::new(HostActivation {
            engine: engine.clone(),
            linker: linker.clone(),
            plugins: plugins.clone(),
            registry: registry.clone(),
            router: router.clone(),
            search: search.clone(),
            working_directory: working_directory.clone(),
            profiles: profiles.clone(),
            packages: packages.clone(),
            slash_commands: slash_commands.clone(),
            activation: Mutex::new(()),
        });
        let package_activator: Arc<dyn PluginActivator> = activator.clone();
        packages.set_activator(&package_activator);
        let host = Self {
            plugins,
            registry,
            router,
            search,
            working_directory,
            packages,
            slash_commands,
            _activator: activator,
        };
        host.load_active_packages().await?;
        Ok(host)
    }

    pub fn registry(&self) -> ToolRegistry {
        self.registry.clone()
    }
    pub fn router(&self) -> ResourceRouter {
        self.router.clone()
    }

    pub fn packages(&self) -> Arc<PluginPackages> {
        self.packages.clone()
    }

    /// Load the package's last successfully activated component. Package
    /// source and status remain available through `plugins:///`.
    pub async fn load_package(&self, package: &str) -> Result<PluginDescriptor, PluginError> {
        let manifest = self.packages.manifest(package)?;
        let component = self.packages.active_component(package)?;
        self._activator
            .activate(package, &manifest, &component)
            .await
            .map_err(|message| PluginError::Socket {
                plugin: manifest.id.clone(),
                socket: "package activation",
                message,
            })?;
        self.descriptor(&manifest.id)
            .await
            .ok_or_else(|| PluginError::Socket {
                plugin: manifest.id,
                socket: "package activation",
                message: "activated plugin is missing from the runtime".into(),
            })
    }

    pub async fn load_active_packages(&self) -> Result<Vec<PluginDescriptor>, PluginError> {
        let mut loaded = Vec::new();
        for package in self.packages.package_names()? {
            if self.packages.active_component(&package).is_ok() {
                loaded.push(self.load_package(&package).await?);
            }
        }
        Ok(loaded)
    }

    async fn descriptor(&self, id: &str) -> Option<PluginDescriptor> {
        let plugin = self
            .plugins
            .read()
            .expect("loaded plugin lock poisoned")
            .iter()
            .find(|slot| slot.id == id)
            .map(|slot| slot.plugin.clone())?;
        Some(plugin.lock().await.descriptor.clone())
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

    pub async fn descriptors(&self) -> Vec<PluginDescriptor> {
        let plugins = self
            .plugins
            .read()
            .expect("loaded plugin lock poisoned")
            .clone();
        let mut descriptors = Vec::with_capacity(plugins.len());
        for slot in plugins {
            descriptors.push(slot.plugin.lock().await.descriptor.clone());
        }
        descriptors
    }

    pub fn slash_commands(&self) -> Vec<CoreSlashDefinition> {
        let mut definitions = self
            .slash_commands
            .read()
            .expect("slash command lock poisoned")
            .values()
            .map(|registered| registered.definition.clone())
            .collect::<Vec<_>>();
        definitions.sort_by(|left, right| left.name.cmp(&right.name));
        definitions
    }

    pub async fn invoke_slash_command(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<CoreSlashResult, PluginError> {
        let registered = self
            .slash_commands
            .read()
            .expect("slash command lock poisoned")
            .get(name)
            .cloned()
            .ok_or_else(|| PluginError::Socket {
                plugin: "host".into(),
                socket: "slash invoke",
                message: format!("unknown slash command `{name}`"),
            })?;
        let mut plugin = registered.plugin.lock().await;
        let id = plugin.descriptor.id.to_string();
        plugin.store.data_mut().invocation = None;
        plugin.store.data_mut().pending_control = None;
        let result = {
            let LoadedPlugin {
                store, bindings, ..
            } = &mut *plugin;
            bindings
                .artist_plugin_slash_command_provider()
                .call_invoke(store, name, arguments)
                .await
        };
        let pending_control = plugin.store.data_mut().pending_control.take();
        let result = result?.map_err(|message| PluginError::Socket {
            plugin: id,
            socket: "slash invoke",
            message,
        })?;
        if pending_control.is_some() {
            return Err(PluginError::Socket {
                plugin: plugin.descriptor.id.to_string(),
                socket: "slash invoke",
                message:
                    "slash commands must return typed actions instead of model terminal control"
                        .into(),
            });
        }
        Ok(CoreSlashResult {
            output: result.output,
            actions: result.actions.into_iter().map(slash_action).collect(),
        })
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
                            role: from_core_context_role(f.role),
                        })
                        .collect(),
                )
                .await?
                .into_iter()
                .map(|f| CoreContextFragment {
                    source: f.source,
                    content: f.content,
                    role: context_role(f.role),
                })
                .collect(),
        })
    }

    pub async fn load_profile(&self, name: &str) -> Result<ProfileSnapshot, PluginError> {
        validate_profile_name(name)
            .map_err(|message| PluginError::Resource(ResourceError::Invalid(message)))?;
        let root = ResourceUri::resolve("profiles:///", Path::new("/"))
            .map_err(|error| PluginError::Resource(ResourceError::Invalid(error.to_string())))?;
        let catalog = match self
            .router
            .handle(CoreRequest::Children { uri: root })
            .await?
        {
            CoreReply::Children { children } => children
                .into_iter()
                .filter_map(|uri| {
                    uri.as_url()
                        .path_segments()
                        .and_then(|mut segments| segments.next_back())
                        .filter(|segment| !segment.is_empty())
                        .map(str::to_owned)
                })
                .collect::<Vec<_>>(),
            _ => {
                return Err(PluginError::Resource(ResourceError::Provider(
                    "profiles children provider returned a non-children reply".into(),
                )));
            }
        };
        let read = |file: &str| {
            ResourceUri::resolve(&format!("profiles:///{name}/{file}"), Path::new("/"))
                .map_err(|error| PluginError::Resource(ResourceError::Invalid(error.to_string())))
        };
        let instructions = match self
            .router
            .handle(CoreRequest::Read {
                uri: read("instructions.md")?,
                start_line: None,
                line_count: None,
            })
            .await?
        {
            CoreReply::Text { text } => text,
            _ => {
                return Err(PluginError::Resource(ResourceError::Provider(
                    "profile instructions provider returned a non-text reply".into(),
                )));
            }
        };
        let manifest = match self
            .router
            .handle(CoreRequest::Read {
                uri: read("profile.json")?,
                start_line: None,
                line_count: None,
            })
            .await?
        {
            CoreReply::Text { text } => {
                serde_json::from_str::<ProfileManifest>(&text).map_err(|error| {
                    PluginError::Resource(ResourceError::Invalid(error.to_string()))
                })?
            }
            _ => {
                return Err(PluginError::Resource(ResourceError::Provider(
                    "profile manifest provider returned a non-text reply".into(),
                )));
            }
        };
        let profile =
            ProfileSnapshot::from_manifest(name.to_owned(), instructions, manifest, catalog)
                .map_err(|message| PluginError::Resource(ResourceError::Invalid(message)))?;
        validate_schema(&profile.yield_schema)?;
        Ok(profile)
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
                effects: d.effects.into_iter().map(from_core_tool_effect).collect(),
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
        let plugins = self
            .plugins
            .read()
            .expect("loaded plugin lock poisoned")
            .clone();
        for slot in plugins {
            if slot
                .plugin
                .lock()
                .await
                .descriptor
                .capabilities
                .contains(&capability)
            {
                selected.push(slot.plugin);
            }
        }
        selected
    }
}

#[async_trait]
impl artist_kernel::ProfileSource for PluginHost {
    async fn load(&self, name: &str) -> Result<ProfileSnapshot, String> {
        self.load_profile(name)
            .await
            .map_err(|error| error.to_string())
    }
}

#[async_trait]
impl artist_kernel::SlashCommandSource for PluginHost {
    async fn invoke(&self, name: &str, arguments: &str) -> Result<CoreSlashResult, String> {
        self.invoke_slash_command(name, arguments)
            .await
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
struct RegisteredSlashCommand {
    owner: String,
    definition: CoreSlashDefinition,
    plugin: Arc<Mutex<LoadedPlugin>>,
}

struct LoadedPlugin {
    store: Store<HostState>,
    bindings: ArtistPlugin,
    descriptor: PluginDescriptor,
}

#[derive(Clone)]
struct LoadedPluginSlot {
    id: String,
    plugin: Arc<Mutex<LoadedPlugin>>,
}

struct HostActivation {
    engine: Engine,
    linker: Arc<Linker<HostState>>,
    plugins: Arc<RwLock<Vec<LoadedPluginSlot>>>,
    registry: ToolRegistry,
    router: ResourceRouter,
    search: Arc<RwLock<Option<Arc<SearchEngine>>>>,
    working_directory: PathBuf,
    profiles: Arc<ProfilesProvider>,
    packages: Arc<PluginPackages>,
    slash_commands: Arc<RwLock<HashMap<String, RegisteredSlashCommand>>>,
    activation: Mutex<()>,
}

#[async_trait]
impl PluginActivator for HostActivation {
    async fn activate(
        &self,
        _package: &str,
        manifest: &PluginPackageManifest,
        component_path: &Path,
    ) -> Result<(), String> {
        let _activation = self.activation.lock().await;
        let component = Component::from_file(&self.engine, component_path)
            .map_err(|error| error.to_string())?;
        let mut store = Store::new(
            &self.engine,
            HostState::new(
                self.registry.clone(),
                self.router.clone(),
                self.search.clone(),
                self.working_directory.clone(),
                self.profiles.clone(),
                self.packages.clone(),
            )
            .map_err(|error| error.to_string())?,
        );
        let bindings = ArtistPlugin::instantiate_async(&mut store, &component, &self.linker)
            .await
            .map_err(|error| error.to_string())?;
        let raw = bindings
            .artist_plugin_lifecycle()
            .call_descriptor(&mut store)
            .await
            .map_err(|error| error.to_string())?;
        let descriptor = PluginDescriptor {
            id: PluginId::new(raw.id),
            version: raw.version,
            capabilities: raw.capabilities.into_iter().map(capability).collect(),
        };
        if descriptor.id.as_str() != manifest.id {
            return Err(format!(
                "package declares `{}` but component declares `{}`",
                manifest.id, descriptor.id
            ));
        }
        if descriptor.capabilities.len() != 1 {
            return Err(format!(
                "plugin {} must advertise exactly one capability",
                descriptor.id
            ));
        }
        store.data_mut().plugin_id = Some(descriptor.id.to_string());

        let tool_definitions = if descriptor.capabilities == [PluginCapability::Tools] {
            bindings
                .artist_plugin_tool_provider()
                .call_definitions(&mut store)
                .await
                .map_err(|error| error.to_string())?
                .map_err(|message| format!("definitions failed: {message}"))?
        } else {
            Vec::new()
        };
        let routes = if descriptor.capabilities == [PluginCapability::Resources] {
            bindings
                .artist_plugin_resource_provider()
                .call_routes(&mut store)
                .await
                .map_err(|error| error.to_string())?
                .map_err(|message| format!("routes failed: {message}"))?
        } else {
            Vec::new()
        };
        let slash_definitions = if descriptor.capabilities == [PluginCapability::Commands] {
            bindings
                .artist_plugin_slash_command_provider()
                .call_definitions(&mut store)
                .await
                .map_err(|error| error.to_string())?
                .map_err(|message| format!("slash definitions failed: {message}"))?
        } else {
            Vec::new()
        };
        match descriptor.capabilities[0] {
            PluginCapability::Tools if tool_definitions.len() != 1 => {
                return Err("a tool component must define exactly one tool".into());
            }
            PluginCapability::Resources if routes.len() != 1 || routes[0].operations.len() != 1 => {
                return Err(
                    "a resource component must define exactly one route and one operation".into(),
                );
            }
            PluginCapability::Commands if slash_definitions.len() != 1 => {
                return Err("a slash-command component must define exactly one command".into());
            }
            _ => {}
        }

        let plugin = Arc::new(Mutex::new(LoadedPlugin {
            store,
            bindings,
            descriptor: descriptor.clone(),
        }));
        let owner = descriptor.id.to_string();
        let mut tools = Vec::<(CoreTool, Arc<dyn ToolHandler>)>::new();
        for definition in tool_definitions {
            let schema = serde_json::from_str(&definition.input_schema)
                .map_err(|error| format!("invalid schema for {}: {error}", definition.name))?;
            validate_schema(&schema).map_err(|error| error.to_string())?;
            if self
                .registry
                .owner(&definition.name)
                .is_some_and(|existing| existing != owner)
            {
                return Err(format!(
                    "tool `{}` is owned by another plugin",
                    definition.name
                ));
            }
            let name = definition.name.clone();
            tools.push((
                CoreTool {
                    name: name.clone(),
                    description: definition.description,
                    input_schema: schema,
                    effects: definition.effects.into_iter().map(tool_effect).collect(),
                },
                Arc::new(WasmTool {
                    plugin: plugin.clone(),
                    name,
                }),
            ));
        }
        let resource_provider: Arc<dyn ResourceProvider> = Arc::new(WasmResource {
            plugin: plugin.clone(),
        });
        let mut resource_routes = Vec::new();
        for route in routes {
            let core = core_route(route)?;
            let scratch = ResourceRouter::new();
            scratch
                .register(owner.clone(), core.clone(), resource_provider.clone())
                .await
                .map_err(|error| error.to_string())?;
            resource_routes.push((core, resource_provider.clone()));
        }
        {
            let commands = self
                .slash_commands
                .read()
                .expect("slash command lock poisoned");
            validate_slash_definitions(
                commands
                    .values()
                    .filter(|registered| registered.owner != owner)
                    .map(|registered| registered.definition.name.as_str()),
                &slash_definitions,
            )?;
        }

        self.registry
            .replace_owner(&owner, tools)
            .map_err(|error| error.to_string())?;
        self.router
            .replace_owner(&owner, resource_routes)
            .map_err(|error| error.to_string())?;
        {
            let mut commands = self
                .slash_commands
                .write()
                .expect("slash command lock poisoned");
            commands.retain(|_, registered| registered.owner != owner);
            for definition in slash_definitions {
                commands.insert(
                    definition.name.clone(),
                    RegisteredSlashCommand {
                        owner: owner.clone(),
                        definition: CoreSlashDefinition {
                            name: definition.name,
                            description: definition.description,
                        },
                        plugin: plugin.clone(),
                    },
                );
            }
        }
        let mut plugins = self.plugins.write().expect("loaded plugin lock poisoned");
        if let Some(slot) = plugins.iter_mut().find(|slot| slot.id == owner) {
            slot.plugin = plugin;
        } else {
            plugins.push(LoadedPluginSlot { id: owner, plugin });
        }
        Ok(())
    }
}

fn core_route(route: ResourceRoute) -> Result<CoreRoute, String> {
    Ok(CoreRoute::new(
        route.base_glob,
        route.projection_glob,
        route.operations.into_iter().map(resource_operation),
    )
    .with_signals(
        route
            .signals
            .into_iter()
            .map(|signal| {
                Ok(SignalDefinition {
                    name: signal.name,
                    description: signal.description,
                    payload_schema: serde_json::from_str(&signal.payload_schema)
                        .map_err(|error| format!("invalid signal payload schema: {error}"))?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
    ))
}
struct WasmTool {
    plugin: Arc<Mutex<LoadedPlugin>>,
    name: String,
}
#[async_trait]
impl ToolHandler for WasmTool {
    async fn call(
        &self,
        arguments: Value,
        context: InvocationContext,
    ) -> Result<CoreToolOutput, ToolError> {
        let mut plugin = self.plugin.lock().await;
        plugin.store.data_mut().invocation = Some(context);
        plugin.store.data_mut().pending_control = None;
        let result = {
            let LoadedPlugin {
                store, bindings, ..
            } = &mut *plugin;
            bindings
                .artist_plugin_tool_provider()
                .call_invoke(store, &self.name, &arguments.to_string())
                .await
        };
        let control = plugin.store.data_mut().pending_control.take();
        plugin.store.data_mut().invocation = None;
        let result = result
            .map_err(|e| ToolError::Failed(e.to_string()))?
            .map_err(ToolError::Failed)?;
        Ok(CoreToolOutput {
            value: serde_json::from_str(&result).unwrap_or(Value::String(result)),
            control,
        })
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
            .map_err(from_wit_error)?;
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
    profiles: Arc<ProfilesProvider>,
    packages: Arc<PluginPackages>,
    working_directory: PathBuf,
    invocation: Option<InvocationContext>,
    plugin_id: Option<String>,
    pending_control: Option<ToolControl>,
}
impl HostState {
    fn new(
        registry: ToolRegistry,
        router: ResourceRouter,
        search: Arc<RwLock<Option<Arc<SearchEngine>>>>,
        working_directory: PathBuf,
        profiles: Arc<ProfilesProvider>,
        packages: Arc<PluginPackages>,
    ) -> Result<Self, std::io::Error> {
        Ok(Self {
            table: ResourceTable::new(),
            wasi: WasiCtx::builder().build(),
            registry,
            router,
            search,
            filesystem: Arc::new(FilesystemProvider::new(&working_directory)),
            profiles,
            packages,
            working_directory,
            invocation: None,
            plugin_id: None,
            pending_control: None,
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

    fn authorize_resource(
        &self,
        uri: &ResourceUri,
        operation: ResourceOperation,
    ) -> Result<(), String> {
        let Some(invocation) = &self.invocation else {
            return Ok(());
        };
        let Some(profile) = invocation.profile.as_deref() else {
            return Ok(());
        };
        let Some(tool_name) = invocation.stack.last() else {
            return Ok(());
        };
        let definition = self
            .registry
            .definition(tool_name)
            .ok_or_else(|| format!("active tool `{tool_name}` is not registered"))?;
        if artist_resource::policy_allows_resource(
            profile,
            &definition,
            &operation.to_string(),
            &uri.to_string(),
        ) {
            Ok(())
        } else {
            Err(format!(
                "profile `{}` denies `{tool_name}` {operation} access to {uri}",
                profile.name
            ))
        }
    }

    fn authorize_request(&self, request: &CoreRequest) -> Result<(), String> {
        self.authorize_resource(request.uri(), request.operation())?;
        match request {
            CoreRequest::Move { to: Some(to), .. } => {
                self.authorize_resource(to, ResourceOperation::Move)
            }
            CoreRequest::Run { cwd: Some(cwd), .. } => {
                self.authorize_resource(cwd, ResourceOperation::Read)
            }
            _ => Ok(()),
        }
    }

    async fn read_complete(
        &mut self,
        uri: ResourceUri,
    ) -> Result<String, artist::plugin::types::ResourceError> {
        match self
            .router
            .handle(CoreRequest::Read {
                uri,
                start_line: None,
                line_count: None,
            })
            .await
            .map_err(core_to_wit_error)?
        {
            CoreReply::Text { text } => Ok(text),
            _ => Err(wit_provider("read provider returned a non-text reply")),
        }
    }

    async fn anchor_grep_page(&mut self, page: GrepPage, pattern: &str) -> Result<String, String> {
        let regex =
            regex::Regex::new(pattern).map_err(|error| format!("invalid regex: {error}"))?;
        let mut documents = HashMap::<ResourceUri, AnchoredDocument>::new();
        let mut matches = Vec::with_capacity(page.matches.len());

        for found in page.matches {
            self.authorize_resource(&found.uri, ResourceOperation::Read)?;
            if !documents.contains_key(&found.uri) {
                let text = match self
                    .router
                    .handle(CoreRequest::Read {
                        uri: found.uri.clone(),
                        start_line: None,
                        line_count: None,
                    })
                    .await
                    .map_err(|error| error.to_string())?
                {
                    CoreReply::Text { text } => text,
                    _ => return Err("read provider returned a non-text reply".into()),
                };
                let document = AnchoredDocument::new(&text).map_err(|error| error.to_string())?;
                documents.insert(found.uri.clone(), document);
            }
            let document = documents
                .get(&found.uri)
                .expect("grep document was inserted above");
            matches.push(anchor_grep_match(document, &regex, &found)?);
        }

        Ok(serde_json::json!({
            "matches": matches,
            "cursor": page.cursor,
        })
        .to_string())
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
                expected_sha256: request.expected_sha256,
                replacements: request
                    .replacements
                    .into_iter()
                    .map(|replacement| TextReplacement {
                        start_byte: replacement.start_byte,
                        end_byte: replacement.end_byte,
                        text: replacement.text,
                    })
                    .collect(),
            },
            W::Run(request) => CoreRequest::Run {
                target: self.uri(&request.target)?,
                input: request.input,
                cwd: request
                    .cwd
                    .as_deref()
                    .map(|uri| self.uri(uri))
                    .transpose()?,
                env: request
                    .env
                    .into_iter()
                    .map(|entry| EnvironmentEntry {
                        name: entry.name,
                        value: entry.value,
                    })
                    .collect(),
                timeout: request.timeout_ms.map(Duration::from_millis),
            },
            W::Signal(request) => CoreRequest::Signal {
                uri: self.uri(&request.uri)?,
                name: request.name,
                payload: request.payload,
            },
        })
    }
}

impl artist::plugin::host_tools::Host for HostState {
    async fn list_tools(&mut self) -> Result<Vec<artist::plugin::types::ToolDefinition>, String> {
        let definitions = self.invocation.as_ref().and_then(|invocation| {
            invocation
                .profile
                .as_deref()
                .map(|profile| self.registry.definitions_for(profile))
        });
        Ok(definitions
            .unwrap_or_else(|| self.registry.definitions())
            .into_iter()
            .map(|d| artist::plugin::types::ToolDefinition {
                name: d.name,
                description: d.description,
                input_schema: d.input_schema.to_string(),
                effects: d.effects.into_iter().map(from_core_tool_effect).collect(),
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
        let output = self
            .registry
            .call_output_with_context(&name, args, ctx)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(control) = output.control {
            if self.pending_control.is_some() {
                return Err("an invocation may request only one terminal control".into());
            }
            self.pending_control = Some(control);
        }
        Ok(output.value.to_string())
    }
}

impl artist::plugin::host_control::Host for HostState {
    async fn yield_(&mut self, payload: String) -> Result<(), String> {
        if self.pending_control.is_some() {
            return Err("an invocation may request only one terminal control".into());
        }
        let payload = serde_json::from_str(&payload)
            .map_err(|error| format!("yield payload is not valid JSON: {error}"))?;
        self.pending_control = Some(ToolControl::Yield { payload });
        Ok(())
    }

    async fn handoff(&mut self, profile: String, brief: String) -> Result<(), String> {
        if self.pending_control.is_some() {
            return Err("an invocation may request only one terminal control".into());
        }
        validate_profile_name(&profile)?;
        let active = self
            .invocation
            .as_ref()
            .and_then(|invocation| invocation.profile.as_deref())
            .ok_or("handoff requires an active profile")?;
        if !active.catalog.iter().any(|candidate| candidate == &profile) {
            return Err(format!("profile `{profile}` is not in the active catalog"));
        }
        self.pending_control = Some(ToolControl::Handoff { profile, brief });
        Ok(())
    }
}

impl artist::plugin::host_resources::Host for HostState {
    async fn handle(
        &mut self,
        request: artist::plugin::types::ResourceRequest,
    ) -> Result<artist::plugin::types::ResourceReply, artist::plugin::types::ResourceError> {
        let request = self.decode_wit_request(request).map_err(wit_invalid)?;
        self.authorize_request(&request).map_err(wit_invalid)?;
        self.router
            .handle(request)
            .await
            .map(core_to_wit_reply)
            .map_err(core_to_wit_error)
    }

    async fn read_anchored(
        &mut self,
        request: artist::plugin::types::ReadRequest,
    ) -> Result<artist::plugin::types::AnchoredReadReply, artist::plugin::types::ResourceError>
    {
        let uri = self.uri(&request.uri).map_err(wit_invalid)?;
        self.authorize_resource(&uri, ResourceOperation::Read)
            .map_err(wit_invalid)?;
        let text = self.read_complete(uri).await?;
        let document =
            AnchoredDocument::new(&text).map_err(|error| wit_invalid(error.to_string()))?;
        let reply = document.read(request.start_line, request.line_count);
        Ok(artist::plugin::types::AnchoredReadReply {
            revision: reply.revision,
            total_lines: reply.total_lines,
            lines: reply.lines.into_iter().map(wit_anchored_line).collect(),
        })
    }

    async fn edit_anchored(
        &mut self,
        request: artist::plugin::types::AnchoredEditRequest,
    ) -> Result<artist::plugin::types::AnchoredEditReply, artist::plugin::types::ResourceError>
    {
        use artist::plugin::types::AnchoredEditOperation as W;

        let uri = self.uri(&request.uri).map_err(wit_invalid)?;
        self.authorize_resource(&uri, ResourceOperation::Edit)
            .map_err(wit_invalid)?;
        let operations = request
            .operations
            .into_iter()
            .map(|operation| match operation {
                W::Replace(operation) => AnchoredEditOperation::Replace {
                    start: operation.start,
                    end: operation.end,
                    content: operation.content,
                },
                W::Delete(operation) => AnchoredEditOperation::Delete {
                    start: operation.start,
                    end: operation.end,
                },
                W::InsertBefore(operation) => AnchoredEditOperation::InsertBefore {
                    anchor: operation.anchor,
                    content: operation.content,
                },
                W::InsertAfter(operation) => AnchoredEditOperation::InsertAfter {
                    anchor: operation.anchor,
                    content: operation.content,
                },
            })
            .collect::<Vec<_>>();
        let before = self.read_complete(uri.clone()).await?;
        let document =
            AnchoredDocument::new(&before).map_err(|error| wit_invalid(error.to_string()))?;
        let plan = document
            .plan_edit(&operations)
            .map_err(|error| wit_invalid(error.to_string()))?;
        let reply = self
            .router
            .handle(CoreRequest::Edit {
                uri: uri.clone(),
                expected_sha256: plan.revision,
                replacements: plan.replacements,
            })
            .await
            .map_err(core_to_wit_error)?;
        let CoreReply::Edited { revision } = reply else {
            return Err(wit_provider("edit provider returned a non-edited reply"));
        };
        let after = self.read_complete(uri.clone()).await?;
        if artist_resource::sha256(after.as_bytes()) != revision {
            return Err(wit_provider(
                "edit provider revision does not match committed content",
            ));
        }
        let committed =
            AnchoredDocument::new(&after).map_err(|error| wit_provider(error.to_string()))?;
        let inserted = inserted_line_indexes(&before, &after);
        let committed_lines = committed.read(None, None).lines;
        let mut updated_lines = inserted
            .into_iter()
            .filter_map(|index| committed_lines.get(index).cloned())
            .map(wit_anchored_line)
            .collect::<Vec<_>>();
        let mut anchors_truncated = false;
        truncate_anchored_lines(
            &mut updated_lines,
            MODEL_OUTPUT_BUDGET / 8,
            &mut anchors_truncated,
        );
        let diff = similar::TextDiff::from_lines(&before, &after)
            .unified_diff()
            .header(&uri.to_string(), &uri.to_string())
            .to_string();
        let (diff, diff_truncated) = truncate_model_output(diff, MODEL_OUTPUT_BUDGET / 4);
        Ok(artist::plugin::types::AnchoredEditReply {
            revision,
            diff,
            updated_lines,
            diff_truncated,
            anchors_truncated,
        })
    }

    async fn metadata(
        &mut self,
        uri: String,
    ) -> Result<artist::plugin::types::ResourceMetadata, artist::plugin::types::ResourceError> {
        let uri = self.uri(&uri).map_err(wit_invalid)?;
        self.authorize_resource(&uri, ResourceOperation::Read)
            .map_err(wit_invalid)?;
        let metadata = self.router.metadata(&uri).map_err(core_to_wit_error)?;
        Ok(artist::plugin::types::ResourceMetadata {
            uri: metadata.uri.to_string(),
            operations: metadata
                .operations
                .into_iter()
                .map(to_wit_operation)
                .collect(),
            signals: metadata
                .signals
                .into_iter()
                .map(|signal| artist::plugin::types::SignalDefinition {
                    name: signal.name,
                    description: signal.description,
                    payload_schema: signal.payload_schema.to_string(),
                })
                .collect(),
        })
    }

    async fn find(
        &mut self,
        request: artist::plugin::types::FindRequest,
    ) -> Result<String, String> {
        let uri = self.uri(&request.uri)?;
        self.authorize_resource(&uri, ResourceOperation::Children)?;
        let search = self.search()?;
        search.synchronize(self.router.generation())?;
        let mut value = search.find(
            &uri,
            request.glob.as_deref(),
            optional_usize(request.max_depth, "max_depth")?,
            request.cursor.as_deref(),
            optional_usize(request.limit, "limit")?.unwrap_or(50),
        )?;
        if let Some(results) = value.get_mut("results").and_then(Value::as_array_mut) {
            results.retain(|result| {
                result
                    .as_str()
                    .and_then(|text| self.uri(text).ok())
                    .is_some_and(|uri| {
                        self.authorize_resource(&uri, ResourceOperation::Children)
                            .is_ok()
                    })
            });
        }
        Ok(value.to_string())
    }

    async fn grep(
        &mut self,
        request: artist::plugin::types::GrepRequest,
    ) -> Result<String, String> {
        let uri = self.uri(&request.uri)?;
        self.authorize_resource(&uri, ResourceOperation::Read)?;
        let search = self.search()?;
        search.synchronize(self.router.generation())?;
        let page = search.grep(
            &uri,
            &request.regex,
            request.include_glob.as_deref(),
            optional_usize(request.context, "context")?.unwrap_or(0),
            request.cursor.as_deref(),
            optional_usize(request.limit, "limit")?.unwrap_or(100),
        )?;
        self.anchor_grep_page(page, &request.regex).await
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

    async fn edit(
        &mut self,
        request: artist::plugin::types::EditRequest,
    ) -> Result<String, artist::plugin::types::ResourceError> {
        let uri = self.uri(&request.uri).map_err(wit_invalid)?;
        match self
            .filesystem
            .handle(CoreRequest::Edit {
                uri,
                expected_sha256: request.expected_sha256,
                replacements: request
                    .replacements
                    .into_iter()
                    .map(|replacement| TextReplacement {
                        start_byte: replacement.start_byte,
                        end_byte: replacement.end_byte,
                        text: replacement.text,
                    })
                    .collect(),
            })
            .await
            .map_err(core_to_wit_error)?
        {
            CoreReply::Edited { revision } => Ok(revision),
            _ => Err(wit_provider("filesystem returned a non-edited reply")),
        }
    }
}

impl artist::plugin::native_profiles::Host for HostState {
    async fn read(
        &mut self,
        uri: String,
        start_line: Option<u64>,
        line_count: Option<u64>,
    ) -> Result<String, String> {
        let uri = self.uri(&uri)?;
        match self
            .profiles
            .handle(CoreRequest::Read {
                uri,
                start_line,
                line_count,
            })
            .await
            .map_err(|error| error.to_string())?
        {
            CoreReply::Text { text } => Ok(text),
            _ => Err("profiles provider returned a non-text reply".into()),
        }
    }

    async fn children(&mut self, uri: String) -> Result<Vec<String>, String> {
        let uri = self.uri(&uri)?;
        match self
            .profiles
            .handle(CoreRequest::Children { uri })
            .await
            .map_err(|error| error.to_string())?
        {
            CoreReply::Children { children } => {
                Ok(children.into_iter().map(|uri| uri.to_string()).collect())
            }
            _ => Err("profiles provider returned a non-children reply".into()),
        }
    }
}

impl artist::plugin::native_plugins::Host for HostState {
    async fn read(
        &mut self,
        uri: String,
        start_line: Option<u64>,
        line_count: Option<u64>,
    ) -> Result<String, String> {
        let uri = self.uri(&uri)?;
        match self
            .packages
            .handle(CoreRequest::Read {
                uri,
                start_line,
                line_count,
            })
            .await
            .map_err(|error| error.to_string())?
        {
            CoreReply::Text { text } => Ok(text),
            _ => Err("plugins provider returned a non-text reply".into()),
        }
    }

    async fn children(&mut self, uri: String) -> Result<Vec<String>, String> {
        let uri = self.uri(&uri)?;
        match self
            .packages
            .handle(CoreRequest::Children { uri })
            .await
            .map_err(|error| error.to_string())?
        {
            CoreReply::Children { children } => {
                Ok(children.into_iter().map(|uri| uri.to_string()).collect())
            }
            _ => Err("plugins provider returned a non-children reply".into()),
        }
    }

    async fn write(&mut self, uri: String, text: String) -> Result<(), String> {
        let uri = self.uri(&uri)?;
        match self
            .packages
            .handle(CoreRequest::Write { uri, text })
            .await
            .map_err(|error| error.to_string())?
        {
            CoreReply::Written => Ok(()),
            _ => Err("plugins provider returned a non-written reply".into()),
        }
    }

    async fn move_(&mut self, from: String, to: Option<String>) -> Result<(), String> {
        let from = self.uri(&from)?;
        let to = to.map(|uri| self.uri(&uri)).transpose()?;
        match self
            .packages
            .handle(CoreRequest::Move { from, to })
            .await
            .map_err(|error| error.to_string())?
        {
            CoreReply::Moved => Ok(()),
            _ => Err("plugins provider returned a non-moved reply".into()),
        }
    }

    async fn edit(
        &mut self,
        request: artist::plugin::types::EditRequest,
    ) -> Result<String, artist::plugin::types::ResourceError> {
        let uri = self.uri(&request.uri).map_err(wit_invalid)?;
        match self
            .packages
            .handle(CoreRequest::Edit {
                uri,
                expected_sha256: request.expected_sha256,
                replacements: request
                    .replacements
                    .into_iter()
                    .map(|replacement| TextReplacement {
                        start_byte: replacement.start_byte,
                        end_byte: replacement.end_byte,
                        text: replacement.text,
                    })
                    .collect(),
            })
            .await
            .map_err(core_to_wit_error)?
        {
            CoreReply::Edited { revision } => Ok(revision),
            _ => Err(wit_provider("plugins provider returned a non-edited reply")),
        }
    }

    async fn signal(
        &mut self,
        uri: String,
        name: String,
        payload: Option<String>,
    ) -> Result<(), artist::plugin::types::ResourceError> {
        let uri = self.uri(&uri).map_err(wit_invalid)?;
        match self
            .packages
            .handle(CoreRequest::Signal { uri, name, payload })
            .await
            .map_err(core_to_wit_error)?
        {
            CoreReply::Signaled => Ok(()),
            _ => Err(wit_provider(
                "plugins provider returned a non-signaled reply",
            )),
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
        W::Commands => PluginCapability::Commands,
    }
}

fn slash_action(value: artist::plugin::types::SlashCommandAction) -> CoreSlashAction {
    use artist::plugin::types::SlashCommandAction as W;
    match value {
        W::Input(content) => CoreSlashAction::Input { content },
        W::Steer(content) => CoreSlashAction::Steer { content },
        W::Abort(reason) => CoreSlashAction::Abort { reason },
        W::ActivateProfile(activation) => CoreSlashAction::ActivateProfile {
            profile: activation.profile,
            brief: activation.brief,
        },
    }
}

fn validate_slash_definitions<'a>(
    existing: impl IntoIterator<Item = &'a str>,
    definitions: &[artist::plugin::types::SlashCommandDefinition],
) -> Result<(), String> {
    let mut names = existing
        .into_iter()
        .map(str::to_owned)
        .collect::<std::collections::HashSet<_>>();
    for definition in definitions {
        let name = &definition.name;
        if name.is_empty()
            || name.starts_with('/')
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(format!(
                "invalid slash-command name `{name}`; use ASCII letters, digits, `-`, or `_`, without a leading slash"
            ));
        }
        if !names.insert(name.clone()) {
            return Err(format!("duplicate global slash-command name `{name}`"));
        }
    }
    Ok(())
}

fn tool_effect(value: artist::plugin::types::ToolEffect) -> CoreToolEffect {
    use artist::plugin::types::ToolEffect as W;
    match value {
        W::Observe => CoreToolEffect::Observe,
        W::Mutate => CoreToolEffect::Mutate,
        W::Execute => CoreToolEffect::Execute,
        W::SessionControl => CoreToolEffect::SessionControl,
        W::Unknown => CoreToolEffect::Unknown,
    }
}

fn from_core_tool_effect(value: CoreToolEffect) -> artist::plugin::types::ToolEffect {
    use artist::plugin::types::ToolEffect as W;
    match value {
        CoreToolEffect::Observe => W::Observe,
        CoreToolEffect::Mutate => W::Mutate,
        CoreToolEffect::Execute => W::Execute,
        CoreToolEffect::SessionControl => W::SessionControl,
        CoreToolEffect::Unknown => W::Unknown,
    }
}

fn context_role(value: artist::plugin::types::ContextRole) -> CoreContextRole {
    use artist::plugin::types::ContextRole as W;
    match value {
        W::System => CoreContextRole::System,
        W::Agents => CoreContextRole::Agents,
        W::Profile => CoreContextRole::Profile,
        W::Identity => CoreContextRole::Identity,
        W::Other => CoreContextRole::Other,
    }
}

fn from_core_context_role(value: CoreContextRole) -> artist::plugin::types::ContextRole {
    use artist::plugin::types::ContextRole as W;
    match value {
        CoreContextRole::System => W::System,
        CoreContextRole::Agents => W::Agents,
        CoreContextRole::Profile => W::Profile,
        CoreContextRole::Identity => W::Identity,
        CoreContextRole::Other => W::Other,
    }
}
fn resource_operation(value: artist::plugin::types::ResourceOperation) -> ResourceOperation {
    use artist::plugin::types::ResourceOperation as W;
    match value {
        W::Read => ResourceOperation::Read,
        W::Children => ResourceOperation::Children,
        W::Write => ResourceOperation::Write,
        W::Edit => ResourceOperation::Edit,
        W::Move => ResourceOperation::Move,
        W::Run => ResourceOperation::Run,
        W::Signal => ResourceOperation::Signal,
        W::Poll => ResourceOperation::Poll,
    }
}

fn to_wit_operation(value: ResourceOperation) -> artist::plugin::types::ResourceOperation {
    use artist::plugin::types::ResourceOperation as W;
    match value {
        ResourceOperation::Read => W::Read,
        ResourceOperation::Children => W::Children,
        ResourceOperation::Write => W::Write,
        ResourceOperation::Edit => W::Edit,
        ResourceOperation::Move => W::Move,
        ResourceOperation::Run => W::Run,
        ResourceOperation::Signal => W::Signal,
        ResourceOperation::Poll => W::Poll,
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
        CoreRequest::Edit {
            uri,
            expected_sha256,
            replacements,
        } => w::ResourceRequest::Edit(w::EditRequest {
            uri: uri.to_string(),
            expected_sha256,
            replacements: replacements
                .into_iter()
                .map(|replacement| w::TextReplacement {
                    start_byte: replacement.start_byte,
                    end_byte: replacement.end_byte,
                    text: replacement.text,
                })
                .collect(),
        }),
        CoreRequest::Run {
            target,
            input,
            cwd,
            env,
            timeout,
        } => w::ResourceRequest::Run(w::RunRequest {
            target: target.to_string(),
            input,
            cwd: cwd.map(|uri| uri.to_string()),
            env: env
                .into_iter()
                .map(|entry| w::EnvironmentEntry {
                    name: entry.name,
                    value: entry.value,
                })
                .collect(),
            timeout_ms: timeout.map(|duration| duration.as_millis() as u64),
        }),
        CoreRequest::Signal { uri, name, payload } => {
            w::ResourceRequest::Signal(w::SignalRequest {
                uri: uri.to_string(),
                name,
                payload,
            })
        }
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
        R::Edited(reply) => CoreReply::Edited {
            revision: reply.revision,
        },
        R::Moved => CoreReply::Moved,
        R::Started(uri) => CoreReply::Started {
            uri: ResourceUri::resolve(&uri, Path::new("/"))
                .map_err(|error| ResourceError::Provider(error.to_string()))?,
        },
        R::Signaled => CoreReply::Signaled,
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
        CoreReply::Edited { revision } => {
            R::Edited(artist::plugin::types::EditedReply { revision })
        }
        CoreReply::Moved => R::Moved,
        CoreReply::Started { uri } => R::Started(uri.to_string()),
        CoreReply::Signaled => R::Signaled,
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
    use artist::plugin::types::{ConflictError, ResourceError as W, RouteError};
    match error {
        ResourceError::NotFound { uri, operation } => W::NotFound(RouteError {
            uri: uri.to_string(),
            operation: to_wit_operation(operation),
        }),
        ResourceError::Unsupported { uri, operation } => W::Unsupported(RouteError {
            uri: uri.to_string(),
            operation: to_wit_operation(operation),
        }),
        ResourceError::Conflict {
            uri,
            current_revision,
        } => W::Conflict(ConflictError {
            uri: uri.to_string(),
            current_revision,
        }),
        ResourceError::Invalid(message) => W::Invalid(message),
        ResourceError::Provider(message) => W::Provider(message),
    }
}

fn from_wit_error(error: artist::plugin::types::ResourceError) -> ResourceError {
    use artist::plugin::types::ResourceError as W;
    match error {
        W::NotFound(error) => ResourceError::NotFound {
            uri: match ResourceUri::resolve(&error.uri, Path::new("/")) {
                Ok(uri) => uri,
                Err(parse) => {
                    return ResourceError::Provider(format!(
                        "plugin returned an invalid error URI: {parse}"
                    ));
                }
            },
            operation: resource_operation(error.operation),
        },
        W::Unsupported(error) => ResourceError::Unsupported {
            uri: match ResourceUri::resolve(&error.uri, Path::new("/")) {
                Ok(uri) => uri,
                Err(parse) => {
                    return ResourceError::Provider(format!(
                        "plugin returned an invalid error URI: {parse}"
                    ));
                }
            },
            operation: resource_operation(error.operation),
        },
        W::Conflict(error) => ResourceError::Conflict {
            uri: match ResourceUri::resolve(&error.uri, Path::new("/")) {
                Ok(uri) => uri,
                Err(parse) => {
                    return ResourceError::Provider(format!(
                        "plugin returned an invalid error URI: {parse}"
                    ));
                }
            },
            current_revision: error.current_revision,
        },
        W::Invalid(message) => ResourceError::Invalid(message),
        W::Provider(message) => ResourceError::Provider(message),
    }
}

fn wit_invalid(message: impl Into<String>) -> artist::plugin::types::ResourceError {
    artist::plugin::types::ResourceError::Invalid(message.into())
}

fn wit_provider(message: impl Into<String>) -> artist::plugin::types::ResourceError {
    artist::plugin::types::ResourceError::Provider(message.into())
}

fn wit_anchored_line(line: artist_resource::AnchoredLine) -> artist::plugin::types::AnchoredLine {
    artist::plugin::types::AnchoredLine {
        line_number: line.line_number,
        anchor: line.anchor,
        text: line.text,
    }
}

fn anchor_grep_match(
    document: &AnchoredDocument,
    regex: &regex::Regex,
    found: &IndexedGrepMatch,
) -> Result<Value, String> {
    let matched = anchored_search_line(document, found.line_number, &found.text)?;
    let match_offset = document
        .byte_offset(found.line_number, found.column)
        .ok_or_else(|| stale_grep_result(&found.uri))?;
    let line_offset = document
        .byte_offset(found.line_number, 0)
        .expect("a resolved line always has a byte offset");
    if !regex
        .find_iter(&matched.text)
        .any(|candidate| line_offset + candidate.start() == match_offset)
    {
        return Err(stale_grep_result(&found.uri));
    }

    let before_count = u64::try_from(found.before.len())
        .map_err(|_| "grep context contains too many lines".to_owned())?;
    let before_start = found
        .line_number
        .checked_sub(before_count)
        .ok_or_else(|| stale_grep_result(&found.uri))?;
    let before = found
        .before
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let offset = u64::try_from(index)
                .map_err(|_| "grep context contains too many lines".to_owned())?;
            let line_number = before_start
                .checked_add(offset)
                .ok_or_else(|| stale_grep_result(&found.uri))?;
            let line = anchored_search_line(document, line_number, text)?;
            Ok(serde_json::json!({"anchor": line.anchor, "text": text}))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let after = found
        .after
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let offset = u64::try_from(index)
                .map_err(|_| "grep context contains too many lines".to_owned())?
                .checked_add(1)
                .ok_or_else(|| stale_grep_result(&found.uri))?;
            let line_number = found
                .line_number
                .checked_add(offset)
                .ok_or_else(|| stale_grep_result(&found.uri))?;
            let line = anchored_search_line(document, line_number, text)?;
            Ok(serde_json::json!({"anchor": line.anchor, "text": text}))
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(serde_json::json!({
        "uri": found.uri.to_string(),
        "anchor": matched.anchor,
        "column": found.column,
        "text": found.text,
        "before": before,
        "after": after,
    }))
}

fn anchored_search_line(
    document: &AnchoredDocument,
    line_number: u64,
    indexed_text: &str,
) -> Result<artist_resource::AnchoredLine, String> {
    let line = document.line(line_number).ok_or_else(|| {
        "grep result no longer exists in the routed resource; retry the search".to_owned()
    })?;
    // FFF truncates displayed lines to a UTF-8-safe 512-byte prefix.
    if !line.text.starts_with(indexed_text) {
        return Err("grep result changed before anchors were assigned; retry the search".into());
    }
    Ok(line)
}

fn stale_grep_result(uri: &ResourceUri) -> String {
    format!("grep result for {uri} changed before anchors were assigned; retry the search")
}

const MODEL_OUTPUT_BUDGET: usize = 64 * 1024;

fn truncate_model_output(mut output: String, budget: usize) -> (String, bool) {
    if output.len() <= budget {
        return (output, false);
    }
    let marker = "\n[output truncated]\n";
    let mut boundary = budget.saturating_sub(marker.len());
    while !output.is_char_boundary(boundary) {
        boundary -= 1;
    }
    output.truncate(boundary);
    output.push_str(marker);
    (output, true)
}

fn inserted_line_indexes(before: &str, after: &str) -> Vec<usize> {
    use similar::ChangeTag;
    similar::TextDiff::from_lines(before, after)
        .iter_all_changes()
        .filter_map(|change| {
            (change.tag() == ChangeTag::Insert)
                .then(|| change.new_index())
                .flatten()
        })
        .collect()
}

fn truncate_anchored_lines(
    lines: &mut Vec<artist::plugin::types::AnchoredLine>,
    budget: usize,
    truncated: &mut bool,
) {
    let mut bytes = 0;
    let keep = lines
        .iter()
        .take_while(|line| {
            bytes += line.anchor.len() + line.text.len() + 32;
            bytes <= budget
        })
        .count();
    if keep < lines.len() {
        lines.truncate(keep);
        *truncated = true;
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
        let profiles = tempfile::tempdir().unwrap();
        let plugins = tempfile::tempdir().unwrap();
        let host = PluginHost::new_with_roots(profiles.path(), plugins.path())
            .await
            .unwrap();
        assert!(host.tools().await.unwrap().is_empty());
        assert!(host.slash_commands().is_empty());
    }

    #[test]
    fn slash_command_names_are_valid_and_globally_unique() {
        use artist::plugin::types::SlashCommandDefinition;

        let definitions = vec![SlashCommandDefinition {
            name: "status".into(),
            description: "Show status".into(),
        }];
        validate_slash_definitions(std::iter::empty(), &definitions).unwrap();
        assert!(validate_slash_definitions(["status"], &definitions).is_err());
        assert!(
            validate_slash_definitions(
                std::iter::empty(),
                &[
                    definitions[0].clone(),
                    SlashCommandDefinition {
                        name: "status".into(),
                        description: "Duplicate".into(),
                    },
                ],
            )
            .is_err()
        );
        for invalid in ["", "/status", "has space", "é"] {
            assert!(
                validate_slash_definitions(
                    std::iter::empty(),
                    &[SlashCommandDefinition {
                        name: invalid.into(),
                        description: String::new(),
                    }],
                )
                .is_err()
            );
        }
    }

    #[test]
    fn expanded_resource_variants_cross_the_wit_boundary_losslessly() {
        use artist::plugin::types as w;

        let uri = ResourceUri::resolve("mem://work", Path::new("/")).unwrap();
        let request = to_wit_request(CoreRequest::Run {
            target: uri.clone(),
            input: "input".into(),
            cwd: Some(uri.clone()),
            env: vec![EnvironmentEntry {
                name: "KEY".into(),
                value: "value".into(),
            }],
            timeout: Some(Duration::from_millis(9)),
        });
        assert!(matches!(
            request,
            w::ResourceRequest::Run(w::RunRequest {
                target,
                input,
                cwd: Some(cwd),
                env,
                timeout_ms: Some(9),
            }) if target == uri.to_string()
                && input == "input"
                && cwd == uri.to_string()
                && env[0].name == "KEY"
                && env[0].value == "value"
        ));
        assert!(matches!(
            to_wit_request(CoreRequest::Signal {
                uri: uri.clone(),
                name: "pause".into(),
                payload: Some("why".into()),
            }),
            w::ResourceRequest::Signal(w::SignalRequest { name, payload: Some(payload), .. })
                if name == "pause" && payload == "why"
        ));
        assert!(matches!(
            to_wit_request(CoreRequest::Edit {
                uri: uri.clone(),
                expected_sha256: "a".repeat(64),
                replacements: vec![TextReplacement {
                    start_byte: 1,
                    end_byte: 2,
                    text: "x".into(),
                }],
            }),
            w::ResourceRequest::Edit(w::EditRequest { replacements, .. })
                if replacements[0].start_byte == 1
                    && replacements[0].end_byte == 2
                    && replacements[0].text == "x"
        ));
        assert_eq!(
            from_wit_reply(w::ResourceReply::Started(uri.to_string())).unwrap(),
            CoreReply::Started { uri: uri.clone() }
        );
        assert_eq!(
            from_wit_error(w::ResourceError::Conflict(w::ConflictError {
                uri: uri.to_string(),
                current_revision: "b".repeat(64),
            })),
            ResourceError::Conflict {
                uri,
                current_revision: "b".repeat(64),
            }
        );
    }

    #[test]
    fn model_output_truncation_preserves_utf8_and_marks_the_result() {
        let (output, truncated) = truncate_model_output("é".repeat(100), 31);
        assert!(truncated);
        assert!(output.is_char_boundary(output.len()));
        assert!(output.ends_with("[output truncated]\n"));
        assert!(output.len() <= 31);
    }

    #[test]
    fn grep_locations_render_as_anchors_without_line_numbers() {
        let document = AnchoredDocument::new("before\nneedle here\nafter\n").unwrap();
        let found = IndexedGrepMatch {
            uri: ResourceUri::resolve("file:///work/example.rs", Path::new("/")).unwrap(),
            line_number: 2,
            column: 0,
            text: "needle here".into(),
            before: vec!["before".into()],
            after: vec!["after".into()],
        };

        let rendered =
            anchor_grep_match(&document, &regex::Regex::new("^needle").unwrap(), &found).unwrap();
        assert!(rendered.get("line").is_none());
        assert!(rendered.get("line_number").is_none());
        assert!(rendered["anchor"].as_str().is_some());
        assert!(rendered["before"][0]["anchor"].as_str().is_some());
        assert!(rendered["after"][0]["anchor"].as_str().is_some());
    }

    #[test]
    fn grep_anchor_translation_rejects_a_changed_snapshot() {
        let document = AnchoredDocument::new("different text\n").unwrap();
        let found = IndexedGrepMatch {
            uri: ResourceUri::resolve("file:///work/example.rs", Path::new("/")).unwrap(),
            line_number: 1,
            column: 0,
            text: "indexed text".into(),
            before: Vec::new(),
            after: Vec::new(),
        };

        assert!(
            anchor_grep_match(&document, &regex::Regex::new("indexed").unwrap(), &found,).is_err()
        );
    }
}
