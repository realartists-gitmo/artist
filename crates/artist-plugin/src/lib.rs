//! Wasmtime component host for the Artist 0.8 plugin contracts.

mod packages;

pub use packages::{
    PACKAGE_FORMAT, PackageState, PackageStatus, PluginActivator, PluginBuild,
    PluginPackageManifest, PluginPackages,
};

use std::{
    collections::HashMap,
    ops::Deref,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

use artist_core::{
    ContentPart as CoreContentPart, ContextFragment as CoreContextFragment,
    ContextRole as CoreContextRole, InitialContext, InvocationScope, ModelRoute, PluginCapability,
    PluginDescriptor, PluginId, ProfileManifest, ProfileSnapshot,
    SlashCommandAction as CoreSlashAction, SlashCommandDefinition as CoreSlashDefinition,
    SlashCommandResult as CoreSlashResult, ToolControl, ToolEffect as CoreToolEffect,
    validate_profile_name,
};
use artist_resource::{
    AnchoredDocument, AnchoredEditOperation, EnvironmentEntry, FilesystemProvider, GrepPage,
    IndexedGrepMatch, InvocationContext, NextActionHint as CoreNextAction, ProfilesProvider,
    ResourceError, ResourceOperation, ResourceProvider, ResourceReply as CoreReply,
    ResourceRequest as CoreRequest, ResourceRoute as CoreRoute, ResourceRouter, ResourceUri,
    SearchEngine, SignalDefinition, TextReplacement, ToolAnnotations as CoreToolAnnotations,
    ToolDefinition as CoreTool, ToolError, ToolFailure as CoreToolFailure, ToolHandler,
    ToolOutput as CoreToolOutput, ToolRegistry, validate_schema,
};
use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio::{
    io::AsyncWriteExt,
    sync::{Mutex, mpsc},
};
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
    ContentPart, ContextFragment, ContextRole, HookDecision, HookEvent, ModelConfig, ModelContext,
    ModelHistoryItem, ResourceRoute, ToolAnnotations, ToolDefinition, ToolEffect,
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

pub struct PluginSessionCreate {
    pub request_id: String,
    pub session_id: artist_core::SessionId,
    pub profile: String,
    pub content: Vec<CoreContentPart>,
    pub attached: bool,
    pub recovery: String,
    pub relationship: String,
    pub creator_plugin_id: PluginId,
    pub parent_scope: Option<InvocationScope>,
}

#[async_trait]
pub trait PluginSessionService: Send + Sync + 'static {
    async fn create(&self, request: PluginSessionCreate) -> Result<artist_core::SessionId, String>;
    async fn send(
        &self,
        session_id: &artist_core::SessionId,
        content: Vec<CoreContentPart>,
    ) -> Result<(), String>;
    async fn steer(
        &self,
        session_id: &artist_core::SessionId,
        content: Vec<CoreContentPart>,
    ) -> Result<(), String>;
    async fn snapshot(&self, session_id: &artist_core::SessionId) -> Result<Value, String>;
    async fn events(
        &self,
        session_id: &artist_core::SessionId,
        cursor: u64,
        limit: u32,
    ) -> Result<(Vec<Value>, u64), String>;
    async fn stop(&self, session_id: &artist_core::SessionId, reason: String)
    -> Result<(), String>;
    async fn await_terminal(&self, session_id: &artist_core::SessionId) -> Result<Value, String>;
}

pub struct PluginHost {
    plugins: Arc<RwLock<Vec<LoadedPluginSlot>>>,
    registry: ToolRegistry,
    router: ResourceRouter,
    search: Arc<RwLock<Option<Arc<SearchEngine>>>>,
    working_directory: PathBuf,
    packages: Arc<PluginPackages>,
    slash_commands: Arc<RwLock<HashMap<String, RegisteredSlashCommand>>>,
    event_outbox: Arc<Mutex<HashMap<String, Vec<artist_core::PluginFact>>>>,
    fact_sinks: Arc<std::sync::Mutex<HashMap<String, artist_kernel::FactSink>>>,
    observer_events: mpsc::UnboundedSender<String>,
    observer_deliveries: Arc<RwLock<HashMap<String, u64>>>,
    session_service: Arc<RwLock<Option<Arc<dyn PluginSessionService>>>>,
    _provider_state: ProviderState,
    _activator: Arc<HostActivation>,
}

pub struct PluginFabric {
    fabric: artist_resource::ResourceFabric,
    slot: Arc<RwLock<Option<Arc<SearchEngine>>>>,
    mounted_search: Arc<SearchEngine>,
}

impl Deref for PluginFabric {
    type Target = artist_resource::ResourceFabric;

    fn deref(&self) -> &Self::Target {
        &self.fabric
    }
}

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
        // Durable scoped storage is mounted as an ordinary resource route so
        // plugins persist domain state through resource semantics instead of
        // a generic key/value import or ephemeral provider-state.
        let durable_store = artist_resource::storage_provider::StorageProvider::open(
            working_directory.join(".artist/durable-store"),
        )
        .map_err(|error| {
            PluginError::Resource(artist_resource::ResourceError::Provider(format!(
                "durable store failed to open: {error}"
            )))
        })?;
        router
            .register(
                "artist.host.storage",
                artist_resource::ResourceRoute::new(
                    "store:///**",
                    None::<String>,
                    [
                        artist_resource::ResourceOperation::Read,
                        artist_resource::ResourceOperation::Children,
                        artist_resource::ResourceOperation::Write,
                        artist_resource::ResourceOperation::Move,
                    ],
                ),
                Arc::new(durable_store),
            )
            .await
            .map_err(PluginError::Resource)?;
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
        let event_schemas = Arc::new(RwLock::new(HashMap::new()));
        let event_outbox = Arc::new(Mutex::new(HashMap::new()));
        let fact_sinks = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (runtime_events, runtime_receiver) = mpsc::unbounded_channel();
        let (observer_events, observer_receiver) = mpsc::unbounded_channel();
        let observer_deliveries = Arc::new(RwLock::new(HashMap::new()));
        tokio::spawn(observe_event_loop(
            plugins.clone(),
            observer_receiver,
            runtime_receiver,
            observer_deliveries.clone(),
            working_directory.join(".artist/plugin-observer-deadletters.jsonl"),
        ));
        let session_service = Arc::new(RwLock::new(None));
        let provider_state = ProviderState::default();
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
            event_schemas: event_schemas.clone(),
            event_outbox: event_outbox.clone(),
            fact_sinks: fact_sinks.clone(),
            runtime_events: runtime_events.clone(),
            session_service: session_service.clone(),
            provider_state: provider_state.clone(),
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
            event_outbox,
            fact_sinks,
            observer_events,
            observer_deliveries,
            session_service,
            _provider_state: provider_state,
            _activator: activator,
        };
        host.load_active_packages().await?;
        Ok(host)
    }

    pub fn observer_deliveries(&self, plugin_id: &str) -> u64 {
        self.observer_deliveries
            .read()
            .expect("observer delivery lock poisoned")
            .get(plugin_id)
            .copied()
            .unwrap_or_default()
    }

    pub fn set_session_service(&self, service: Arc<dyn PluginSessionService>) {
        *self
            .session_service
            .write()
            .expect("session service lock poisoned") = Some(service);
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

    pub async fn mount_fabric(
        &self,
        view: artist_resource::ResourceView,
    ) -> Result<PluginFabric, PluginError> {
        let fabric = artist_resource::ResourceFabric::mount(
            self.router.clone(),
            self.working_directory.clone(),
            view,
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
        descriptors.sort_by(|left, right| {
            lifecycle_order(
                left.priority,
                left.id.as_str(),
                right.priority,
                right.id.as_str(),
            )
        });
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
        &self,
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
    /// Compose as a deterministic pipeline ordered by `(priority, plugin id)`.
    /// Each provider receives the complete output of its predecessor.
    pub async fn compose_prompt(
        &self,
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
            .map(core_tool_to_wit)
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
    /// Transform context sequentially in lifecycle order; later transforms see
    /// the complete result of earlier transforms.
    pub async fn transform_context(
        &self,
        context: ModelContext,
    ) -> Result<ModelContext, PluginError> {
        self.transform_context_scoped(context, None).await
    }

    async fn transform_context_scoped(
        &self,
        mut context: ModelContext,
        scope: Option<&InvocationScope>,
    ) -> Result<ModelContext, PluginError> {
        for plugin in self.with(PluginCapability::Context).await {
            let mut p = plugin.lock().await;
            let id = p.descriptor.id.to_string();
            p.store.data_mut().invocation = scope.cloned().map(invocation_for_scope);
            let result = {
                let LoadedPlugin {
                    store, bindings, ..
                } = &mut *p;
                bindings
                    .artist_plugin_lifecycle()
                    .call_transform_context(store, &context)
                    .await
            };
            p.store.data_mut().invocation = None;
            context = result?.map_err(|message| PluginError::Socket {
                plugin: id,
                socket: "transform-context",
                message,
            })?;
        }
        Ok(context)
    }
    /// Observe hooks in lifecycle order. Rewrites are retained in that order;
    /// the first Stop is terminal and lower-precedence hooks are not invoked.
    pub async fn observe_hook(&self, event: &HookEvent) -> Result<Vec<HookDecision>, PluginError> {
        let mut decisions = Vec::new();
        for plugin in self.with(PluginCapability::Hooks).await {
            let decision = {
                let mut p = plugin.lock().await;
                let id = p.descriptor.id.to_string();
                p.store.data_mut().invocation =
                    Some(invocation_for_scope(wit_scope_to_core(event.scope.clone())));
                let LoadedPlugin {
                    store, bindings, ..
                } = &mut *p;
                let result = bindings
                    .artist_plugin_lifecycle()
                    .call_observe_hook(&mut *store, event)
                    .await?;
                store.data_mut().invocation = None;
                result.map_err(|message| PluginError::Socket {
                    plugin: id,
                    socket: "observe-hook",
                    message,
                })?
            };
            let terminal = hook_is_terminal(&decision);
            decisions.push(decision);
            if terminal {
                break;
            }
        }
        Ok(decisions)
    }
    /// Configure the model as a deterministic pipeline in lifecycle order.
    pub async fn configure_model(&self, config: ModelConfig) -> Result<ModelConfig, PluginError> {
        self.configure_model_scoped(config, None).await
    }

    async fn configure_model_scoped(
        &self,
        mut config: ModelConfig,
        scope: Option<&InvocationScope>,
    ) -> Result<ModelConfig, PluginError> {
        for plugin in self.with(PluginCapability::ModelConfig).await {
            let mut p = plugin.lock().await;
            let id = p.descriptor.id.to_string();
            p.store.data_mut().invocation = scope.cloned().map(invocation_for_scope);
            let result = {
                let LoadedPlugin {
                    store, bindings, ..
                } = &mut *p;
                bindings
                    .artist_plugin_lifecycle()
                    .call_configure_model(store, &config)
                    .await
            };
            p.store.data_mut().invocation = None;
            config = result?.map_err(|message| PluginError::Socket {
                plugin: id,
                socket: "configure-model",
                message,
            })?;
        }
        Ok(config)
    }
    /// Deliver events in lifecycle order, failing fast on the first observer
    /// error so lower-precedence observers never see a partially failed event.
    pub async fn observe_event(&self, event: &str) -> Result<(), PluginError> {
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
        let mut plugins = plugins;
        plugins.sort_by(|left, right| {
            lifecycle_order(left.priority, &left.id, right.priority, &right.id)
        });
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

fn lifecycle_order(
    left_priority: i32,
    left_id: &str,
    right_priority: i32,
    right_id: &str,
) -> std::cmp::Ordering {
    left_priority
        .cmp(&right_priority)
        .then_with(|| left_id.cmp(right_id))
}

fn hook_is_terminal(decision: &HookDecision) -> bool {
    matches!(decision, HookDecision::Stop(_))
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

#[async_trait]
impl artist_kernel::ModelProviderSource for PluginHost {
    async fn resolve(
        &self,
        route: &ModelRoute,
        _session_id: &artist_core::SessionId,
        _profile_epoch: Option<u64>,
    ) -> Result<Arc<dyn artist_kernel::StreamingModel>, String> {
        for plugin in self.with(PluginCapability::ModelProvider).await {
            let descriptors = {
                let mut plugin = plugin.lock().await;
                let LoadedPlugin {
                    store, bindings, ..
                } = &mut *plugin;
                bindings
                    .artist_plugin_model_provider()
                    .call_descriptors(store)
                    .await
                    .map_err(|error| error.to_string())??
            };
            if descriptors.iter().any(|descriptor| {
                descriptor.provider_id == route.provider
                    && descriptor.model_patterns.iter().any(|pattern| {
                        pattern == "*"
                            || pattern == &route.model
                            || pattern
                                .strip_suffix('*')
                                .is_some_and(|prefix| route.model.starts_with(prefix))
                    })
            }) {
                return Ok(Arc::new(WasmProviderModel {
                    plugin,
                    route: route.clone(),
                }));
            }
        }
        Err(format!(
            "no activated WASM provider supplies {}/{}",
            route.provider, route.model
        ))
    }
}

struct WasmProviderModel {
    plugin: Arc<Mutex<LoadedPlugin>>,
    route: ModelRoute,
}

impl artist_kernel::StreamingModel for WasmProviderModel {
    fn stream(
        &self,
        request: artist_kernel::ModelRequest,
        _steering: artist_kernel::Steering,
    ) -> artist_kernel::ModelStream {
        let plugin = self.plugin.clone();
        let route = self.route.clone();
        Box::pin(async_stream::stream! {
            let encoded = serde_json::json!({
                "session_id": request.session_id,
                "run_id": request.run_id,
                "context": request.context,
                "prompt": request.prompt,
                "history": request.history,
                "profile_epoch": request.profile_epoch,
                "route": route,
            })
            .to_string();
            let handle = {
                let mut loaded = plugin.lock().await;
                let LoadedPlugin { store, bindings, .. } = &mut *loaded;
                match bindings
                    .artist_plugin_model_provider()
                    .call_start(store, &encoded)
                    .await
                {
                    Ok(Ok(handle)) => handle,
                    Ok(Err(failure)) => {
                        yield Err(model_failure(failure));
                        return;
                    }
                    Err(error) => {
                        yield Err(artist_kernel::ModelError::new(error.to_string()));
                        return;
                    }
                }
            };
            let mut cancellation = WasmProviderCancellation {
                plugin: plugin.clone(),
                handle: Some(handle.clone()),
            };
            loop {
                let item = {
                    let mut loaded = plugin.lock().await;
                    let LoadedPlugin { store, bindings, .. } = &mut *loaded;
                    bindings
                        .artist_plugin_model_provider()
                        .call_poll(store, &handle)
                        .await
                };
                match item {
                    Ok(Ok(exports::artist::plugin::model_provider::StreamItem::Event(event))) => {
                        match serde_json::from_str::<artist_kernel::ModelEvent>(&event) {
                            Ok(event) => yield Ok(event),
                            Err(error) => {
                                yield Err(artist_kernel::ModelError::new(format!(
                                    "WASM provider emitted an invalid model event: {error}"
                                )));
                                return;
                            }
                        }
                    }
                    Ok(Ok(exports::artist::plugin::model_provider::StreamItem::Finished)) => {
                        cancellation.handle = None;
                        return;
                    }
                    Ok(Ok(exports::artist::plugin::model_provider::StreamItem::Failed(failure)))
                    | Ok(Err(failure)) => {
                        cancellation.handle = None;
                        yield Err(model_failure(failure));
                        return;
                    }
                    Err(error) => {
                        yield Err(artist_kernel::ModelError::new(error.to_string()));
                        return;
                    }
                }
            }
        })
    }
}

struct WasmProviderCancellation {
    plugin: Arc<Mutex<LoadedPlugin>>,
    handle: Option<String>,
}

impl Drop for WasmProviderCancellation {
    fn drop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        let plugin = self.plugin.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let mut loaded = plugin.lock().await;
                let LoadedPlugin {
                    store, bindings, ..
                } = &mut *loaded;
                let _ = bindings
                    .artist_plugin_model_provider()
                    .call_cancel(store, &handle)
                    .await;
            });
        }
    }
}

fn model_failure(failure: artist::plugin::types::ToolFailure) -> artist_kernel::ModelError {
    artist_kernel::ModelError(artist_core::ModelFailure {
        message: failure.message,
        class: artist_core::FailureClass::Provider,
        retriable: failure.retriable,
        provider_code: Some(failure.code),
        http_status: None,
        provider_request_id: None,
    })
}

#[async_trait]
impl artist_kernel::ExecutionExtensions for PluginHost {
    async fn compose_initial_context(
        &self,
        context: InitialContext,
    ) -> Result<InitialContext, artist_kernel::ExecutionExtensionError> {
        PluginHost::compose_initial_context(self, context)
            .await
            .map_err(|error| {
                artist_kernel::ExecutionExtensionError::new("compose-prompt", error.to_string())
            })
    }

    async fn prepare_model_request(
        &self,
        request: &mut artist_kernel::ModelRequest,
    ) -> Result<(), artist_kernel::ExecutionExtensionError> {
        let scope = InvocationScope {
            session_id: request.session_id.clone(),
            run_id: Some(request.run_id.clone()),
            call_id: None,
            correlation_id: artist_core::CorrelationId::new(format!("{}:context", request.run_id)),
            parent_correlation_id: None,
        };
        let transformed = self
            .transform_context_scoped(
                ModelContext {
                    context: request.context.clone(),
                    prompt: request
                        .prompt
                        .iter()
                        .cloned()
                        .map(core_content_to_wit)
                        .collect::<Result<_, _>>()
                        .map_err(|message| {
                            artist_kernel::ExecutionExtensionError::new(
                                "transform-context",
                                message,
                            )
                        })?,
                    history: request
                        .history
                        .iter()
                        .map(|item| ModelHistoryItem {
                            sequence: item.sequence,
                            message: serde_json::to_string(&item.message)
                                .expect("model history is serializable"),
                        })
                        .collect(),
                },
                Some(&scope),
            )
            .await
            .map_err(|error| {
                artist_kernel::ExecutionExtensionError::new("transform-context", error.to_string())
            })?;
        if transformed.prompt.is_empty() {
            return Err(artist_kernel::ExecutionExtensionError::new(
                "transform-context",
                "plugin removed the current prompt",
            ));
        }
        request.context = transformed.context;
        request.prompt = transformed
            .prompt
            .into_iter()
            .map(wit_content_to_core)
            .collect::<Result<_, _>>()
            .map_err(|message| {
                artist_kernel::ExecutionExtensionError::new("transform-context", message)
            })?;
        request.history = transformed
            .history
            .into_iter()
            .map(|item| {
                Ok(artist_kernel::ModelHistoryItem {
                    sequence: item.sequence,
                    message: serde_json::from_str(&item.message).map_err(|error| {
                        artist_kernel::ExecutionExtensionError::new(
                            "transform-context",
                            format!("plugin returned invalid typed history: {error}"),
                        )
                    })?,
                })
            })
            .collect::<Result<_, artist_kernel::ExecutionExtensionError>>()?;
        Ok(())
    }

    async fn configure_model(
        &self,
        _scope: &InvocationScope,
        route: ModelRoute,
    ) -> Result<ModelRoute, artist_kernel::ExecutionExtensionError> {
        let configured = self
            .configure_model_scoped(
                ModelConfig {
                    provider: route.provider,
                    model: route.model,
                    parameters: route.parameters.to_string(),
                },
                Some(_scope),
            )
            .await
            .map_err(|error| {
                artist_kernel::ExecutionExtensionError::new("configure-model", error.to_string())
            })?;
        let parameters = serde_json::from_str(&configured.parameters).map_err(|error| {
            artist_kernel::ExecutionExtensionError::new(
                "configure-model",
                format!("plugin returned invalid model parameters: {error}"),
            )
        })?;
        Ok(ModelRoute {
            provider: configured.provider,
            account: route.account,
            api_variant: route.api_variant,
            model: configured.model,
            reasoning: route.reasoning,
            parameters,
        })
    }

    async fn compact_context(
        &self,
        scope: &InvocationScope,
        history: &[artist_kernel::ModelHistoryItem],
    ) -> Result<Option<artist_core::ProjectionArtifact>, artist_kernel::ExecutionExtensionError>
    {
        let context = ModelContext {
            context: String::new(),
            prompt: Vec::new(),
            history: history
                .iter()
                .map(|item| ModelHistoryItem {
                    sequence: item.sequence,
                    message: serde_json::to_string(&item.message)
                        .expect("model history is serializable"),
                })
                .collect(),
        };
        for plugin in self.with(PluginCapability::Context).await {
            let mut plugin = plugin.lock().await;
            let id = plugin.descriptor.id.to_string();
            plugin.store.data_mut().invocation = Some(invocation_for_scope(scope.clone()));
            let result = {
                let LoadedPlugin {
                    store, bindings, ..
                } = &mut *plugin;
                bindings
                    .artist_plugin_lifecycle()
                    .call_compact_context(store, &context)
                    .await
            };
            plugin.store.data_mut().invocation = None;
            let result = result
                .map_err(|error| {
                    artist_kernel::ExecutionExtensionError::new(
                        "compact-context",
                        error.to_string(),
                    )
                })?
                .map_err(|message| {
                    artist_kernel::ExecutionExtensionError::new(
                        "compact-context",
                        format!("{id}: {message}"),
                    )
                })?;
            if let Some(artifact) = result {
                let content = artifact
                    .content
                    .into_iter()
                    .map(wit_content_to_core)
                    .collect::<Result<_, _>>()
                    .map_err(|message| {
                        artist_kernel::ExecutionExtensionError::new("compact-context", message)
                    })?;
                let derivations = serde_json::from_str(&artifact.derivations).map_err(|error| {
                    artist_kernel::ExecutionExtensionError::new(
                        "compact-context",
                        format!("invalid derivation metadata: {error}"),
                    )
                })?;
                return Ok(Some(artist_core::ProjectionArtifact {
                    content,
                    derivations,
                    projection_digest: artifact.projection_digest,
                    earliest_changed_sequence: artifact.earliest_changed_sequence,
                }));
            }
        }
        Ok(None)
    }

    async fn hook(
        &self,
        event: artist_kernel::LifecycleHookEvent,
    ) -> Result<Vec<artist_kernel::LifecycleHookDecision>, artist_kernel::ExecutionExtensionError>
    {
        let decisions = self
            .observe_hook(&HookEvent {
                phase: hook_phase_to_wit(event.phase),
                scope: invocation_scope_to_wit(&event.scope),
                payload: serde_json::to_string(&event.payload)
                    .expect("hook payload is serializable"),
            })
            .await
            .map_err(|error| {
                artist_kernel::ExecutionExtensionError::new("observe-hook", error.to_string())
            })?;
        Ok(decisions
            .into_iter()
            .map(|decision| match decision {
                HookDecision::Proceed => artist_kernel::LifecycleHookDecision::Proceed,
                HookDecision::Stop(reason) => artist_kernel::LifecycleHookDecision::Stop { reason },
                HookDecision::Rewrite(value) => artist_kernel::LifecycleHookDecision::Rewrite {
                    value: serde_json::from_str(&value).unwrap_or(serde_json::Value::String(value)),
                },
            })
            .collect())
    }

    async fn drain_plugin_facts(
        &self,
        session_id: &artist_core::SessionId,
    ) -> Vec<artist_core::PluginFact> {
        self.event_outbox
            .lock()
            .await
            .remove(session_id.as_str())
            .unwrap_or_default()
    }

    fn observe_committed(&self, event: artist_core::StreamEvent) {
        let encoded = serde_json::to_string(&event).expect("stream events are serializable");
        let _ = self.observer_events.send(encoded);
    }

    fn bind_fact_sink(&self, session_id: &artist_core::SessionId, sink: artist_kernel::FactSink) {
        self.fact_sinks
            .lock()
            .expect("fact sink lock poisoned")
            .insert(session_id.to_string(), sink);
    }

    fn unbind_fact_sink(&self, session_id: &artist_core::SessionId) {
        self.fact_sinks
            .lock()
            .expect("fact sink lock poisoned")
            .remove(session_id.as_str());
    }
}

async fn observe_event_loop(
    plugins: Arc<RwLock<Vec<LoadedPluginSlot>>>,
    mut committed: mpsc::UnboundedReceiver<String>,
    mut runtime: mpsc::UnboundedReceiver<String>,
    deliveries: Arc<RwLock<HashMap<String, u64>>>,
    deadletters: PathBuf,
) {
    loop {
        let event = tokio::select! {
            committed = committed.recv() => match committed {
                Some(event) => event,
                None => match runtime.recv().await {
                    Some(event) => event,
                    None => break,
                },
            },
            runtime = runtime.recv() => match runtime {
                Some(event) => event,
                None => match committed.recv().await {
                    Some(event) => event,
                    None => break,
                },
            },
        };
        deliver_observed_event(&plugins, &event, &deliveries, &deadletters).await;
    }
}

async fn deliver_observed_event(
    plugins: &Arc<RwLock<Vec<LoadedPluginSlot>>>,
    event: &str,
    deliveries: &Arc<RwLock<HashMap<String, u64>>>,
    deadletters: &PathBuf,
) {
    let mut slots = plugins.read().expect("loaded plugin lock poisoned").clone();
    slots
        .sort_by(|left, right| lifecycle_order(left.priority, &left.id, right.priority, &right.id));
    for slot in slots {
        let advertised = slot
            .plugin
            .lock()
            .await
            .descriptor
            .capabilities
            .contains(&PluginCapability::Events);
        if !advertised {
            continue;
        }
        let mut last_error = None;
        for attempt in 0..3 {
            let result = {
                let mut plugin = slot.plugin.lock().await;
                // Observation deliveries must not recurse: a guest that emits
                // while observing would re-enter the canonical path and loop.
                plugin.store.data_mut().observing = true;
                let id = plugin.descriptor.id.to_string();
                let LoadedPlugin {
                    store, bindings, ..
                } = &mut *plugin;
                let result = bindings
                    .artist_plugin_lifecycle()
                    .call_observe_event(&mut *store, event)
                    .await;
                store.data_mut().observing = false;
                result.map_err(PluginError::Runtime).and_then(|result| {
                    result.map_err(|message| PluginError::Socket {
                        plugin: id,
                        socket: "observe-event",
                        message,
                    })
                })
            };
            match result {
                Ok(()) => {
                    *deliveries
                        .write()
                        .expect("observer delivery lock poisoned")
                        .entry(slot.id.clone())
                        .or_default() += 1;
                    last_error = None;
                    break;
                }
                Err(error) => {
                    last_error = Some(error.to_string());
                    tokio::time::sleep(Duration::from_millis(10 * (attempt + 1))).await;
                }
            }
        }
        let Some(error) = last_error else {
            continue;
        };
        if let Some(parent) = deadletters.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let record = serde_json::json!({
            "plugin_id": slot.id,
            "event": serde_json::from_str::<Value>(event)
                .unwrap_or_else(|_| Value::String(event.to_string())),
            "error": error,
        });
        if let Ok(mut file) = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(deadletters)
            .await
        {
            let _ = file.write_all(format!("{record}\n").as_bytes()).await;
            let _ = file.flush().await;
        }
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
    priority: i32,
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
    event_schemas: Arc<RwLock<HashMap<(String, String), artist_core::PluginEventSchema>>>,
    event_outbox: Arc<Mutex<HashMap<String, Vec<artist_core::PluginFact>>>>,
    fact_sinks: Arc<std::sync::Mutex<HashMap<String, artist_kernel::FactSink>>>,
    runtime_events: mpsc::UnboundedSender<String>,
    session_service: Arc<RwLock<Option<Arc<dyn PluginSessionService>>>>,
    provider_state: ProviderState,
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
            HostState::new(HostServices {
                registry: self.registry.clone(),
                router: self.router.clone(),
                search: self.search.clone(),
                working_directory: self.working_directory.clone(),
                profiles: self.profiles.clone(),
                packages: self.packages.clone(),
                provider_state: self.provider_state.clone(),
                event_schemas: self.event_schemas.clone(),
                event_outbox: self.event_outbox.clone(),
                fact_sinks: self.fact_sinks.clone(),
                runtime_events: self.runtime_events.clone(),
                session_service: self.session_service.clone(),
            })
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
            priority: raw.priority,
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
        let model_descriptors = if descriptor.capabilities == [PluginCapability::ModelProvider] {
            bindings
                .artist_plugin_model_provider()
                .call_descriptors(&mut store)
                .await
                .map_err(|error| error.to_string())?
                .map_err(|message| format!("model-provider descriptors failed: {message}"))?
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
            PluginCapability::Resources if routes.is_empty() => {
                return Err("a resource component must define at least one route".into());
            }
            PluginCapability::Commands if slash_definitions.len() != 1 => {
                return Err("a slash-command component must define exactly one command".into());
            }
            PluginCapability::ModelProvider if model_descriptors.is_empty() => {
                return Err("a model-provider component must define at least one provider".into());
            }
            _ => {}
        }
        let mut provider_ids = std::collections::BTreeSet::new();
        for provider in &model_descriptors {
            if provider.provider_id.trim().is_empty()
                || provider.revision.trim().is_empty()
                || provider.model_patterns.is_empty()
                || !provider_ids.insert(provider.provider_id.clone())
            {
                return Err(
                    "model-provider descriptors require unique IDs, revisions, and model patterns"
                        .into(),
                );
            }
            let schema: Value = serde_json::from_str(&provider.parameter_schema)
                .map_err(|error| format!("invalid provider parameter schema: {error}"))?;
            validate_schema(&schema).map_err(|error| error.to_string())?;
        }

        let plugin = Arc::new(Mutex::new(LoadedPlugin {
            store,
            bindings,
            descriptor: descriptor.clone(),
        }));
        let owner = descriptor.id.to_string();
        let mut tools = Vec::<(CoreTool, Arc<dyn ToolHandler>)>::new();
        for definition in tool_definitions {
            let input_schema = serde_json::from_str(&definition.input_schema).map_err(|error| {
                format!("invalid input schema for {}: {error}", definition.name)
            })?;
            let output_schema =
                serde_json::from_str(&definition.output_schema).map_err(|error| {
                    format!("invalid output schema for {}: {error}", definition.name)
                })?;
            validate_schema(&input_schema).map_err(|error| error.to_string())?;
            validate_schema(&output_schema).map_err(|error| error.to_string())?;
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
                    category: definition.category,
                    input_schema,
                    output_schema,
                    effects: definition.effects.into_iter().map(tool_effect).collect(),
                    annotations: CoreToolAnnotations {
                        read_only: definition.annotations.read_only,
                        destructive: definition.annotations.destructive,
                        idempotent: definition.annotations.idempotent,
                        open_world: definition.annotations.open_world,
                    },
                },
                Arc::new(WasmTool {
                    plugin: plugin.clone(),
                    name,
                }),
            ));
        }
        // Resource calls use independent component stores so an awaited call
        // cannot serialize unrelated calls into the same logical provider.
        // All provider-visible host state remains shared through the cloned
        // registries/providers in HostState.
        let resource_provider: Arc<dyn ResourceProvider> = Arc::new(WasmResource {
            engine: self.engine.clone(),
            linker: self.linker.clone(),
            component: component.clone(),
            registry: self.registry.clone(),
            router: self.router.clone(),
            search: self.search.clone(),
            working_directory: self.working_directory.clone(),
            profiles: self.profiles.clone(),
            packages: self.packages.clone(),
            provider_state: self.provider_state.clone(),
            event_schemas: self.event_schemas.clone(),
            event_outbox: self.event_outbox.clone(),
            fact_sinks: self.fact_sinks.clone(),
            runtime_events: self.runtime_events.clone(),
            session_service: self.session_service.clone(),
            plugin_id: owner.clone(),
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
            slot.priority = descriptor.priority;
            slot.plugin = plugin;
        } else {
            plugins.push(LoadedPluginSlot {
                id: owner,
                priority: descriptor.priority,
                plugin,
            });
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
            .map_err(|error| ToolError::failed("component-runtime", error.to_string()))?
            .map_err(|failure| ToolError::Failed(Box::new(wit_failure_to_core(failure))))?;
        Ok(CoreToolOutput {
            value: serde_json::from_str(&result.value).map_err(|error| {
                ToolError::Output(format!("plugin returned invalid JSON value: {error}"))
            })?,
            content: result
                .content
                .into_iter()
                .map(wit_content_to_core)
                .collect::<Result<_, _>>()
                .map_err(|error| ToolError::Output(format!("invalid rich tool output: {error}")))?,
            control,
            next_actions: result
                .next_actions
                .into_iter()
                .map(wit_next_action_to_core)
                .collect::<Result<_, _>>()
                .map_err(ToolError::Output)?,
        })
    }
}

struct WasmResource {
    engine: Engine,
    linker: Arc<Linker<HostState>>,
    component: Component,
    registry: ToolRegistry,
    router: ResourceRouter,
    search: Arc<RwLock<Option<Arc<SearchEngine>>>>,
    working_directory: PathBuf,
    profiles: Arc<ProfilesProvider>,
    packages: Arc<PluginPackages>,
    provider_state: ProviderState,
    event_schemas: Arc<RwLock<HashMap<(String, String), artist_core::PluginEventSchema>>>,
    event_outbox: Arc<Mutex<HashMap<String, Vec<artist_core::PluginFact>>>>,
    fact_sinks: Arc<std::sync::Mutex<HashMap<String, artist_kernel::FactSink>>>,
    runtime_events: mpsc::UnboundedSender<String>,
    session_service: Arc<RwLock<Option<Arc<dyn PluginSessionService>>>>,
    plugin_id: String,
}

#[derive(Clone, Default)]
struct ProviderState {
    namespaces: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
}

impl ProviderState {
    fn validate(key: &str, value: Option<&str>) -> Result<(), String> {
        if key.is_empty() || key.len() > 4096 {
            return Err("provider state keys must contain 1..=4096 UTF-8 bytes".into());
        }
        if value.is_some_and(|value| value.len() > 1024 * 1024) {
            return Err("provider state values must not exceed 1 MiB".into());
        }
        Ok(())
    }

    fn get(&self, owner: &str, key: &str) -> Result<Option<String>, String> {
        Self::validate(key, None)?;
        Ok(self
            .namespaces
            .read()
            .expect("provider state lock poisoned")
            .get(owner)
            .and_then(|namespace| namespace.get(key))
            .cloned())
    }

    fn set(&self, owner: &str, key: String, value: String) -> Result<(), String> {
        Self::validate(&key, Some(&value))?;
        self.namespaces
            .write()
            .expect("provider state lock poisoned")
            .entry(owner.to_owned())
            .or_default()
            .insert(key, value);
        Ok(())
    }

    fn delete(&self, owner: &str, key: &str) -> Result<(), String> {
        Self::validate(key, None)?;
        let mut namespaces = self
            .namespaces
            .write()
            .expect("provider state lock poisoned");
        if let Some(namespace) = namespaces.get_mut(owner) {
            namespace.remove(key);
            if namespace.is_empty() {
                namespaces.remove(owner);
            }
        }
        Ok(())
    }

    fn compare_and_swap(
        &self,
        owner: &str,
        key: String,
        expected: Option<String>,
        value: Option<String>,
    ) -> Result<bool, String> {
        Self::validate(&key, expected.as_deref())?;
        Self::validate(&key, value.as_deref())?;
        let mut namespaces = self
            .namespaces
            .write()
            .expect("provider state lock poisoned");
        let namespace = namespaces.entry(owner.to_owned()).or_default();
        if namespace.get(&key) != expected.as_ref() {
            return Ok(false);
        }
        match value {
            Some(value) => {
                namespace.insert(key, value);
            }
            None => {
                namespace.remove(&key);
            }
        }
        if namespace.is_empty() {
            namespaces.remove(owner);
        }
        Ok(true)
    }
}

#[async_trait]
impl ResourceProvider for WasmResource {
    async fn handle(&self, request: CoreRequest) -> Result<CoreReply, ResourceError> {
        let mut state = HostState::new(HostServices {
            registry: self.registry.clone(),
            router: self.router.clone(),
            search: self.search.clone(),
            working_directory: self.working_directory.clone(),
            profiles: self.profiles.clone(),
            packages: self.packages.clone(),
            provider_state: self.provider_state.clone(),
            event_schemas: self.event_schemas.clone(),
            event_outbox: self.event_outbox.clone(),
            fact_sinks: self.fact_sinks.clone(),
            runtime_events: self.runtime_events.clone(),
            session_service: self.session_service.clone(),
        })
        .map_err(|error| ResourceError::Provider(error.to_string()))?;
        state.plugin_id = Some(self.plugin_id.clone());
        let mut store = Store::new(&self.engine, state);
        let bindings = ArtistPlugin::instantiate_async(&mut store, &self.component, &self.linker)
            .await
            .map_err(|error| ResourceError::Provider(error.to_string()))?;
        let request = to_wit_request(request);
        let reply = bindings
            .artist_plugin_resource_provider()
            .call_handle(&mut store, &request)
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
    provider_state: ProviderState,
    event_schemas: Arc<RwLock<HashMap<(String, String), artist_core::PluginEventSchema>>>,
    event_outbox: Arc<Mutex<HashMap<String, Vec<artist_core::PluginFact>>>>,
    fact_sinks: Arc<std::sync::Mutex<HashMap<String, artist_kernel::FactSink>>>,
    runtime_events: mpsc::UnboundedSender<String>,
    session_service: Arc<RwLock<Option<Arc<dyn PluginSessionService>>>>,
    working_directory: PathBuf,
    invocation: Option<InvocationContext>,
    plugin_id: Option<String>,
    pending_control: Option<ToolControl>,
    observing: bool,
}

/// Per-instantiation host services shared by every WASM store.
struct HostServices {
    registry: ToolRegistry,
    router: ResourceRouter,
    search: Arc<RwLock<Option<Arc<SearchEngine>>>>,
    working_directory: PathBuf,
    profiles: Arc<ProfilesProvider>,
    packages: Arc<PluginPackages>,
    provider_state: ProviderState,
    event_schemas: Arc<RwLock<HashMap<(String, String), artist_core::PluginEventSchema>>>,
    event_outbox: Arc<Mutex<HashMap<String, Vec<artist_core::PluginFact>>>>,
    fact_sinks: Arc<std::sync::Mutex<HashMap<String, artist_kernel::FactSink>>>,
    runtime_events: mpsc::UnboundedSender<String>,
    session_service: Arc<RwLock<Option<Arc<dyn PluginSessionService>>>>,
}

impl HostState {
    fn new(services: HostServices) -> Result<Self, std::io::Error> {
        let HostServices {
            registry,
            router,
            search,
            working_directory,
            profiles,
            packages,
            provider_state,
            event_schemas,
            event_outbox,
            fact_sinks,
            runtime_events,
            session_service,
        } = services;
        Ok(Self {
            table: ResourceTable::new(),
            wasi: WasiCtx::builder().build(),
            registry,
            router,
            search,
            filesystem: Arc::new(FilesystemProvider::new(&working_directory)),
            profiles,
            packages,
            provider_state,
            event_schemas,
            event_outbox,
            fact_sinks,
            runtime_events,
            session_service,
            working_directory,
            invocation: None,
            plugin_id: None,
            pending_control: None,
            observing: false,
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
                cursor: request.cursor,
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
            .map(core_tool_to_wit)
            .collect())
    }

    async fn call_tool(
        &mut self,
        name: String,
        arguments: String,
    ) -> Result<artist::plugin::types::ToolSuccess, artist::plugin::types::ToolFailure> {
        let args = serde_json::from_str(&arguments)
            .map_err(|error| wit_failure("invalid-arguments", error.to_string(), false))?;
        let ctx = self
            .invocation
            .clone()
            .unwrap_or_else(InvocationContext::root);
        if self.plugin_id.as_deref() == self.registry.owner(&name).as_deref() {
            let mut cycle = ctx.stack.clone();
            cycle.push(name.clone());
            return Err(wit_failure(
                "recursive-tool-invocation",
                ToolError::Recursive {
                    cycle: cycle.join(" -> "),
                }
                .to_string(),
                false,
            ));
        }
        let output = self
            .registry
            .call_output_with_context(&name, args, ctx)
            .await
            .map_err(core_failure_to_wit)?;
        if let Some(control) = output.control.clone() {
            if self.pending_control.is_some() {
                return Err(wit_failure(
                    "multiple-terminal-controls",
                    "an invocation may request only one terminal control",
                    false,
                ));
            }
            self.pending_control = Some(control);
        }
        core_output_to_wit(output)
    }
}

impl artist::plugin::host_events::Host for HostState {
    async fn register_schema(
        &mut self,
        schema: artist::plugin::host_events::EventSchema,
    ) -> Result<String, String> {
        let plugin_id = PluginId::new(self.plugin_id.clone().ok_or_else(|| {
            "event schema registration requires an activated plugin identity".to_owned()
        })?);
        let mut core = artist_core::PluginEventSchema {
            schema_id: artist_core::EventSchemaId::new(schema.schema_id.clone()),
            plugin_id,
            event_type: schema.event_type,
            version: schema.version,
            payload_schema: serde_json::from_str(&schema.payload_schema)
                .map_err(|error| format!("invalid event payload schema: {error}"))?,
            presentation_schema: serde_json::from_str(&schema.presentation_schema)
                .map_err(|error| format!("invalid event presentation schema: {error}"))?,
            schema_digest: String::new(),
            presentation: serde_json::from_str(&schema.presentation)
                .map_err(|error| format!("invalid event presentation: {error}"))?,
        };
        core.schema_digest = core.canonical_digest()?;
        core.validate()?;
        let key = (core.plugin_id.to_string(), schema.schema_id);
        let mut schemas = self
            .event_schemas
            .write()
            .expect("event schema lock poisoned");
        if let Some(existing) = schemas.get(&key) {
            if existing != &core {
                return Err("event schema ID changed for an activated plugin".into());
            }
        } else {
            schemas.insert(key, core.clone());
        }
        Ok(core.schema_digest)
    }

    async fn emit(
        &mut self,
        event: artist::plugin::host_events::EventRequest,
    ) -> Result<(), String> {
        let plugin_id =
            PluginId::new(self.plugin_id.clone().ok_or_else(|| {
                "event emission requires an activated plugin identity".to_owned()
            })?);
        let schema = self
            .event_schemas
            .read()
            .expect("event schema lock poisoned")
            .get(&(plugin_id.to_string(), event.schema_id.clone()))
            .cloned()
            .ok_or_else(|| "event emission refers to an unregistered schema".to_owned())?;
        let requested_scope = wit_scope_to_core(event.scope);
        let scope = self
            .invocation
            .as_ref()
            .and_then(|invocation| invocation.scope.clone())
            .unwrap_or(requested_scope);
        let core = artist_core::PluginEvent {
            plugin_id,
            schema_id: artist_core::EventSchemaId::new(event.schema_id),
            event_type: event.event_type,
            schema_version: event.schema_version,
            schema_digest: event.schema_digest,
            scope,
            payload: serde_json::from_str(&event.payload)
                .map_err(|error| format!("invalid event payload: {error}"))?,
            presentation: serde_json::from_str(&event.presentation)
                .map_err(|error| format!("invalid event presentation: {error}"))?,
        };
        core.validate_against(&schema)?;
        if self.observing {
            return Err(
                "plugins may not emit events while observing events; delivery loops are forbidden"
                    .into(),
            );
        }
        if !event.durable {
            // Runtime-only diagnostics never enter the canonical transcript;
            // they take the separate observation path only.
            let encoded = serde_json::to_string(&serde_json::json!({
                "runtime_plugin_event": core,
            }))
            .map_err(|error| format!("runtime event is not serializable: {error}"))?;
            let _ = self.runtime_events.send(encoded);
            return Ok(());
        }
        // Durable emission: success is returned only after the owning session
        // actor durably appends the facts and publishes their stream events.
        // The bounded channel applies backpressure to fast emitters.
        let sink = self
            .fact_sinks
            .lock()
            .expect("fact sink lock poisoned")
            .get(core.scope.session_id.as_str())
            .cloned();
        if let Some(sink) = sink {
            let (receipt_tx, receipt_rx) = tokio::sync::oneshot::channel();
            let envelope = artist_kernel::FactEnvelope {
                session_id: core.scope.session_id.clone(),
                facts: vec![
                    artist_core::PluginFact::SchemaRegistered { schema },
                    artist_core::PluginFact::Event { event: core },
                ],
                receipt: receipt_tx,
            };
            sink.send(envelope)
                .await
                .map_err(|_| "the session fact channel is closed".to_owned())?;
            return match receipt_rx.await {
                Ok(result) => result,
                Err(_) => Err("the session did not acknowledge the emission".into()),
            };
        }
        // Legacy fallback for hosts without a bound session actor: facts are
        // drained and appended at run boundaries.
        let mut outbox = self.event_outbox.lock().await;
        let facts = outbox.entry(core.scope.session_id.to_string()).or_default();
        facts.push(artist_core::PluginFact::SchemaRegistered { schema });
        facts.push(artist_core::PluginFact::Event { event: core });
        Ok(())
    }
}

impl artist::plugin::host_progress::Host for HostState {
    async fn emit(
        &mut self,
        progress: artist::plugin::host_progress::Progress,
    ) -> Result<(), String> {
        if progress
            .fraction
            .is_some_and(|fraction| !fraction.is_finite() || !(0.0..=1.0).contains(&fraction))
        {
            return Err("tool progress fraction must be finite and between zero and one".into());
        }
        let invocation = self
            .invocation
            .as_ref()
            .ok_or_else(|| "tool progress requires an active invocation".to_owned())?;
        let sink = invocation
            .progress
            .as_ref()
            .ok_or_else(|| "tool progress sink is unavailable".to_owned())?;
        let scope = invocation
            .scope
            .clone()
            .unwrap_or_else(|| wit_scope_to_core(progress.scope));
        sink.emit(artist_core::ToolProgress {
            scope,
            sequence: progress.sequence,
            fraction: progress.fraction,
            message: progress.message,
            detail: serde_json::from_str(&progress.detail)
                .map_err(|error| format!("invalid progress detail: {error}"))?,
        });
        Ok(())
    }
}

impl artist::plugin::host_sessions::Host for HostState {
    async fn create(
        &mut self,
        request: artist::plugin::host_sessions::CreateRequest,
    ) -> Result<String, String> {
        let service = self
            .session_service
            .read()
            .expect("session service lock poisoned")
            .clone()
            .ok_or_else(|| "host session service is unavailable".to_owned())?;
        let plugin_id =
            PluginId::new(self.plugin_id.clone().ok_or_else(|| {
                "session creation requires an activated plugin identity".to_owned()
            })?);
        let content = request
            .content
            .into_iter()
            .map(wit_content_to_core)
            .collect::<Result<_, _>>()?;
        let attached = matches!(
            request.attachment,
            artist::plugin::host_sessions::Attachment::Attached
        );
        let recovery = match request.recovery {
            artist::plugin::host_sessions::RecoveryPolicy::RemainInterrupted => {
                "remain-interrupted"
            }
            artist::plugin::host_sessions::RecoveryPolicy::ResumeQueuedWork => "resume-queued-work",
            artist::plugin::host_sessions::RecoveryPolicy::PluginResolved => "plugin-resolved",
        };
        service
            .create(PluginSessionCreate {
                request_id: request.request_id,
                session_id: artist_core::SessionId::new(request.session_id),
                profile: request.profile,
                content,
                attached,
                recovery: recovery.into(),
                relationship: request.relationship,
                creator_plugin_id: plugin_id,
                parent_scope: self
                    .invocation
                    .as_ref()
                    .and_then(|invocation| invocation.scope.clone()),
            })
            .await
            .map(|id| id.to_string())
    }

    async fn send(
        &mut self,
        session_id: String,
        content: Vec<artist::plugin::types::ContentPart>,
    ) -> Result<(), String> {
        let service = self.session_service()?;
        service
            .send(
                &artist_core::SessionId::new(session_id),
                content
                    .into_iter()
                    .map(wit_content_to_core)
                    .collect::<Result<_, _>>()?,
            )
            .await
    }

    async fn steer(
        &mut self,
        session_id: String,
        content: Vec<artist::plugin::types::ContentPart>,
    ) -> Result<(), String> {
        let service = self.session_service()?;
        service
            .steer(
                &artist_core::SessionId::new(session_id),
                content
                    .into_iter()
                    .map(wit_content_to_core)
                    .collect::<Result<_, _>>()?,
            )
            .await
    }

    async fn snapshot(&mut self, session_id: String) -> Result<String, String> {
        self.session_service()?
            .snapshot(&artist_core::SessionId::new(session_id))
            .await
            .map(|snapshot| snapshot.to_string())
    }

    async fn events(
        &mut self,
        session_id: String,
        cursor: u64,
        limit: u32,
    ) -> Result<artist::plugin::host_sessions::EventPage, String> {
        if limit == 0 || limit > 1_000 {
            return Err("session event page limit must be in 1..=1000".into());
        }
        let (events, next_cursor) = self
            .session_service()?
            .events(&artist_core::SessionId::new(session_id), cursor, limit)
            .await?;
        Ok(artist::plugin::host_sessions::EventPage {
            events: events.into_iter().map(|event| event.to_string()).collect(),
            next_cursor,
        })
    }

    async fn stop(&mut self, session_id: String, reason: String) -> Result<(), String> {
        self.session_service()?
            .stop(&artist_core::SessionId::new(session_id), reason)
            .await
    }

    async fn await_terminal(&mut self, session_id: String) -> Result<String, String> {
        self.session_service()?
            .await_terminal(&artist_core::SessionId::new(session_id))
            .await
            .map(|outcome| outcome.to_string())
    }
}

impl HostState {
    fn session_service(&self) -> Result<Arc<dyn PluginSessionService>, String> {
        self.session_service
            .read()
            .expect("session service lock poisoned")
            .clone()
            .ok_or_else(|| "host session service is unavailable".into())
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
impl artist::plugin::provider_state::Host for HostState {
    async fn get(&mut self, key: String) -> Result<Option<String>, String> {
        let owner = self
            .plugin_id
            .as_deref()
            .ok_or_else(|| "provider state requires an activated plugin identity".to_owned())?;
        self.provider_state.get(owner, &key)
    }

    async fn set(&mut self, key: String, value: String) -> Result<(), String> {
        let owner = self
            .plugin_id
            .as_deref()
            .ok_or_else(|| "provider state requires an activated plugin identity".to_owned())?;
        self.provider_state.set(owner, key, value)
    }

    async fn delete(&mut self, key: String) -> Result<(), String> {
        let owner = self
            .plugin_id
            .as_deref()
            .ok_or_else(|| "provider state requires an activated plugin identity".to_owned())?;
        self.provider_state.delete(owner, &key)
    }

    async fn compare_and_swap(
        &mut self,
        key: String,
        expected: Option<String>,
        value: Option<String>,
    ) -> Result<bool, String> {
        let owner = self
            .plugin_id
            .as_deref()
            .ok_or_else(|| "provider state requires an activated plugin identity".to_owned())?;
        self.provider_state
            .compare_and_swap(owner, key, expected, value)
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
        W::ModelConfig => PluginCapability::ModelConfig,
        W::ModelProvider => PluginCapability::ModelProvider,
        W::Events => PluginCapability::Events,
        W::Commands => PluginCapability::Commands,
    }
}

fn slash_action(value: artist::plugin::types::SlashCommandAction) -> CoreSlashAction {
    use artist::plugin::types::SlashCommandAction as W;
    match value {
        W::Input(content) => CoreSlashAction::Input {
            content: vec![artist_core::ContentPart::text(content)],
        },
        W::Steer(content) => CoreSlashAction::Steer {
            content: vec![artist_core::ContentPart::text(content)],
        },
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

fn core_tool_to_wit(tool: CoreTool) -> artist::plugin::types::ToolDefinition {
    artist::plugin::types::ToolDefinition {
        name: tool.name,
        description: tool.description,
        category: tool.category,
        input_schema: tool.input_schema.to_string(),
        output_schema: tool.output_schema.to_string(),
        effects: tool
            .effects
            .into_iter()
            .map(from_core_tool_effect)
            .collect(),
        annotations: artist::plugin::types::ToolAnnotations {
            read_only: tool.annotations.read_only,
            destructive: tool.annotations.destructive,
            idempotent: tool.annotations.idempotent,
            open_world: tool.annotations.open_world,
        },
    }
}

fn wit_next_action_to_core(
    hint: artist::plugin::types::NextActionHint,
) -> Result<CoreNextAction, String> {
    Ok(CoreNextAction {
        kind: hint.kind,
        label: hint.label,
        payload: serde_json::from_str(&hint.payload).map_err(|error| error.to_string())?,
    })
}

fn core_next_action_to_wit(hint: CoreNextAction) -> artist::plugin::types::NextActionHint {
    artist::plugin::types::NextActionHint {
        kind: hint.kind,
        label: hint.label,
        payload: hint.payload.to_string(),
    }
}

fn wit_failure_to_core(failure: artist::plugin::types::ToolFailure) -> CoreToolFailure {
    CoreToolFailure {
        code: failure.code,
        message: failure.message,
        retriable: failure.retriable,
        details: serde_json::from_str(&failure.details).unwrap_or(Value::String(failure.details)),
        violations: failure
            .violations
            .into_iter()
            .map(|violation| artist_resource::FieldViolation {
                path: violation.path,
                message: violation.message,
            })
            .collect(),
        next_actions: failure
            .next_actions
            .into_iter()
            .filter_map(|hint| wit_next_action_to_core(hint).ok())
            .collect(),
    }
}

fn wit_failure(
    code: impl Into<String>,
    message: impl Into<String>,
    retriable: bool,
) -> artist::plugin::types::ToolFailure {
    artist::plugin::types::ToolFailure {
        code: code.into(),
        message: message.into(),
        retriable,
        details: "null".into(),
        violations: Vec::new(),
        next_actions: Vec::new(),
    }
}

fn core_failure_to_wit(error: ToolError) -> artist::plugin::types::ToolFailure {
    match error {
        ToolError::Failed(failure) => {
            let failure = *failure;
            artist::plugin::types::ToolFailure {
                code: failure.code,
                message: failure.message,
                retriable: failure.retriable,
                details: failure.details.to_string(),
                violations: failure
                    .violations
                    .into_iter()
                    .map(|violation| artist::plugin::types::FieldViolation {
                        path: violation.path,
                        message: violation.message,
                    })
                    .collect(),
                next_actions: failure
                    .next_actions
                    .into_iter()
                    .map(core_next_action_to_wit)
                    .collect(),
            }
        }
        other => wit_failure("host-tool-error", other.to_string(), false),
    }
}

#[allow(clippy::result_large_err)] // the WIT-generated failure record is the ABI shape; boxing would diverge from the component contract
fn core_output_to_wit(
    output: CoreToolOutput,
) -> Result<artist::plugin::types::ToolSuccess, artist::plugin::types::ToolFailure> {
    Ok(artist::plugin::types::ToolSuccess {
        value: output.value.to_string(),
        content: output
            .content
            .into_iter()
            .map(core_content_to_wit)
            .collect::<Result<_, _>>()
            .map_err(|message| wit_failure("invalid-rich-content", message, false))?,
        next_actions: output
            .next_actions
            .into_iter()
            .map(core_next_action_to_wit)
            .collect(),
    })
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
fn core_content_to_wit(
    part: CoreContentPart,
) -> Result<artist::plugin::types::ContentPart, String> {
    use artist::plugin::types as w;
    Ok(match part {
        CoreContentPart::Text { text } => w::ContentPart::Text(text),
        CoreContentPart::Json { value } => w::ContentPart::Json(value.to_string()),
        CoreContentPart::Attachment { attachment } => w::ContentPart::Attachment(w::Attachment {
            blob: w::BlobRef {
                algorithm: attachment.blob.algorithm,
                digest: attachment.blob.digest,
                byte_length: attachment.blob.byte_length,
                media_type: attachment.blob.media_type,
                logical_name: attachment.blob.logical_name,
            },
            role: attachment.role,
            alternate_text: attachment.alternate_text,
            metadata: attachment
                .metadata
                .into_iter()
                .map(|(key, value)| w::MetadataEntry {
                    key,
                    value: value.to_string(),
                })
                .collect(),
        }),
        CoreContentPart::Reasoning { value } => w::ContentPart::Reasoning(value.to_string()),
        CoreContentPart::Opaque { kind, value } => w::ContentPart::Opaque(w::OpaqueContent {
            kind,
            value: value.to_string(),
        }),
    })
}

fn wit_content_to_core(
    part: artist::plugin::types::ContentPart,
) -> Result<CoreContentPart, String> {
    use artist::plugin::types::ContentPart as w;
    Ok(match part {
        w::Text(text) => CoreContentPart::Text { text },
        w::Json(value) => CoreContentPart::Json {
            value: serde_json::from_str(&value).map_err(|error| error.to_string())?,
        },
        w::Attachment(attachment) => {
            let metadata = attachment
                .metadata
                .into_iter()
                .map(|entry| {
                    Ok((
                        entry.key,
                        serde_json::from_str(&entry.value).map_err(|error| error.to_string())?,
                    ))
                })
                .collect::<Result<_, String>>()?;
            let blob = artist_core::BlobRef {
                algorithm: attachment.blob.algorithm,
                digest: attachment.blob.digest,
                byte_length: attachment.blob.byte_length,
                media_type: attachment.blob.media_type,
                logical_name: attachment.blob.logical_name,
            };
            blob.validate()?;
            CoreContentPart::Attachment {
                attachment: artist_core::Attachment {
                    blob,
                    role: attachment.role,
                    alternate_text: attachment.alternate_text,
                    metadata,
                },
            }
        }
        w::Reasoning(value) => CoreContentPart::Reasoning {
            value: serde_json::from_str(&value).map_err(|error| error.to_string())?,
        },
        w::Opaque(value) => CoreContentPart::Opaque {
            kind: value.kind,
            value: serde_json::from_str(&value.value).map_err(|error| error.to_string())?,
        },
    })
}

fn hook_phase_to_wit(phase: artist_kernel::HookPhase) -> artist::plugin::types::HookPhase {
    use artist::plugin::types::HookPhase as w;
    match phase {
        artist_kernel::HookPhase::BeforeModelRequest => w::BeforeModelRequest,
        artist_kernel::HookPhase::AfterModelResponse => w::AfterModelResponse,
        artist_kernel::HookPhase::BeforeToolExecution => w::BeforeToolExecution,
        artist_kernel::HookPhase::AfterToolResult => w::AfterToolResult,
        artist_kernel::HookPhase::RunCompleted => w::RunCompleted,
        artist_kernel::HookPhase::RunInterrupted => w::RunInterrupted,
        artist_kernel::HookPhase::RunFailed => w::RunFailed,
    }
}

fn invocation_for_scope(scope: InvocationScope) -> InvocationContext {
    let mut invocation = InvocationContext::root();
    invocation.scope = Some(scope);
    invocation
}

fn invocation_scope_to_wit(scope: &InvocationScope) -> artist::plugin::types::InvocationScope {
    artist::plugin::types::InvocationScope {
        session_id: scope.session_id.to_string(),
        run_id: scope.run_id.as_ref().map(ToString::to_string),
        call_id: scope.call_id.as_ref().map(ToString::to_string),
        correlation_id: scope.correlation_id.to_string(),
        parent_correlation_id: scope
            .parent_correlation_id
            .as_ref()
            .map(ToString::to_string),
    }
}

fn wit_scope_to_core(scope: artist::plugin::types::InvocationScope) -> InvocationScope {
    InvocationScope {
        session_id: artist_core::SessionId::new(scope.session_id),
        run_id: scope.run_id.map(artist_core::RunId::new),
        call_id: scope.call_id.map(artist_core::CallId::new),
        correlation_id: artist_core::CorrelationId::new(scope.correlation_id),
        parent_correlation_id: scope
            .parent_correlation_id
            .map(artist_core::CorrelationId::new),
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
            cursor,
        } => w::ResourceRequest::Poll(w::PollRequest {
            uri: uri.to_string(),
            match_: pattern,
            timeout_ms: timeout.map(|d| d.as_millis() as u64),
            cursor,
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
        R::Content(content) => CoreReply::Content {
            content: content
                .into_iter()
                .map(wit_content_to_core)
                .collect::<Result<_, _>>()
                .map_err(ResourceError::Provider)?,
        },
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
            next_cursor: p.next_cursor,
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
        CoreReply::Content { content } => R::Content(
            content
                .into_iter()
                .map(core_content_to_wit)
                .collect::<Result<_, _>>()
                .expect("canonical content always converts to WIT"),
        ),
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
        CoreReply::Poll {
            text,
            outcome,
            next_cursor,
        } => R::Poll(artist::plugin::types::PollReply {
            text,
            next_cursor,
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

    #[test]
    fn wasm_resource_provider_is_shareable_across_concurrent_router_calls() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WasmResource>();
    }

    #[test]
    fn concurrent_component_instances_share_namespaced_provider_state_atomically() {
        let state = ProviderState::default();
        let workers = (0..8)
            .map(|_| {
                let state = state.clone();
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        loop {
                            let expected = state.get("resource-a", "count").unwrap();
                            let current =
                                expected.as_deref().unwrap_or("0").parse::<u64>().unwrap();
                            if state
                                .compare_and_swap(
                                    "resource-a",
                                    "count".into(),
                                    expected,
                                    Some((current + 1).to_string()),
                                )
                                .unwrap()
                            {
                                break;
                            }
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(
            state.get("resource-a", "count").unwrap().as_deref(),
            Some("800")
        );
        assert_eq!(state.get("resource-b", "count").unwrap(), None);
        state
            .set("resource-b", "count".into(), "private".into())
            .unwrap();
        state.delete("resource-a", "count").unwrap();
        assert_eq!(state.get("resource-a", "count").unwrap(), None);
        assert_eq!(
            state.get("resource-b", "count").unwrap().as_deref(),
            Some("private")
        );
    }

    #[test]
    fn lifecycle_order_is_priority_then_id_not_activation_order() {
        let mut providers = vec![(10, "zeta"), (-5, "last-loaded"), (10, "alpha")];
        providers.sort_by(|left, right| lifecycle_order(left.0, left.1, right.0, right.1));
        assert_eq!(
            providers,
            [(-5, "last-loaded"), (10, "alpha"), (10, "zeta")]
        );
    }

    #[test]
    fn hook_stop_is_the_only_terminal_composition_decision() {
        assert!(!hook_is_terminal(&HookDecision::Proceed));
        assert!(!hook_is_terminal(&HookDecision::Rewrite("next".into())));
        assert!(hook_is_terminal(&HookDecision::Stop("done".into())));
    }

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
