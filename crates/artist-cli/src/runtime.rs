//! Default daemon runtime assembly.
//!
//! The daemon owns session lifetime, but this module owns the concrete
//! component selection used by the shipped executable. Keeping this assembly
//! here prevents provider/profile/tool policy from leaking into the kernel or
//! the generic agent state machine.

use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use artist_agent::daemon::{
    EngineTurnRunner, SessionRuntimeFactory, SessionTurnRunner, SharedProvider,
};
use artist_agent::{
    AgentEngine, AgentError, ForkExecutor, ForkIngress, HandoffHandler, HandoffPlan, HarnessPolicy,
    ProviderResolver, ToolSurface, TurnControl, TurnRequest,
};
use artist_component::{
    CompactionComponent, ComponentHost, ComponentToolRegistry, CompositionInput,
    PermissionRegistry, ProfileDocument, UrlCompositionSource,
};
use artist_kernel::{FilesNamespace, Kernel, ResourceUri};
use artist_session::{EventLog, EventLogTranscript, Workspace};
use async_trait::async_trait;
use llm_provider::{
    Message, ModelProvider, ProviderAuth, ProviderCatalog, ProviderConfig, ProviderConfigError,
    ProviderConfigId, ProviderError, ResolvedResource, ResourceResolver, Role, ToolDefinition,
};

pub struct ComponentRuntimeFactory {
    extension_root: PathBuf,
    configurations: Arc<RwLock<BTreeMap<String, ProviderConfig>>>,
    hosts: tokio::sync::Mutex<BTreeMap<String, Arc<ComponentHost>>>,
}

/// Executes profile-local forks in the same workspace and component universe
/// as their parent. Each child gets its own durable fork log and session-local
/// tool surface, while provider/resource components remain shared and safely
/// concurrent.
struct ComponentForkExecutor {
    provider: Arc<SharedProvider>,
    tools: ComponentToolRegistry,
    definitions: Vec<ToolDefinition>,
    host: Arc<ComponentHost>,
    composition_input: CompositionInput,
    model: String,
    profile_id: String,
    permissions: PermissionRegistry,
    harness: HarnessPolicy,
    compaction: Option<Arc<dyn CompactionComponent>>,
    compaction_context_limit: Option<u64>,
    workspace_root: PathBuf,
}

impl ComponentForkExecutor {
    async fn execute_child(
        &self,
        fork_id: String,
        ingress: ForkIngress,
        task: String,
        control: TurnControl,
    ) -> Result<serde_json::Value, AgentError> {
        let log_path = self
            .workspace_root
            .join(".artist/forks")
            .join(format!("{fork_id}.jsonl"));
        let log = Arc::new(EventLog::open(log_path, fork_id.clone())?);
        let surface = ToolSurface::new(self.tools.clone())
            .with_log(Arc::clone(&log))
            .with_permissions(self.profile_id.clone(), self.permissions.clone());
        surface
            .replace_static_definitions(self.definitions.clone())
            .map_err(|error| AgentError::ToolSurface(error.to_string()))?;
        // Forks must finish through yield. Nested fork/handoff execution is
        // intentionally disabled here so a child cannot create an unbounded
        // tree or violate the parent's terminal contract.
        surface.configure_harness(HarnessPolicy {
            yield_schema: self.harness.yield_schema.clone(),
            allow_fork: false,
            allow_handoff: false,
        });
        let surface = Arc::new(surface);
        let mut engine = AgentEngine::new(
            Arc::clone(&self.provider),
            Arc::clone(&surface),
            Arc::clone(&log),
        )
        .as_fork();
        if let Some(compaction) = &self.compaction {
            engine = engine.with_compaction(
                Arc::clone(compaction),
                self.compaction_context_limit,
                serde_json::json!({}),
            );
        }
        let result = engine
            .run_turn(TurnRequest {
                model: self.model.clone(),
                user: Message::text(Role::User, task),
                tools: self.definitions.clone(),
                tool_surface: Some(Arc::clone(&surface)),
                composition_updater: Some(
                    Arc::clone(&self.host) as Arc<dyn artist_component::CompositionUpdater>
                ),
                composition_input: Some(self.composition_input.clone()),
                context: Some(ingress.snapshot),
                initial_messages: Some(ingress.messages),
                control,
            })
            .await;
        match result {
            Ok(outcome) => {
                let yielded = engine.replay_events()?.into_iter().any(|event| {
                    matches!(
                        event,
                        artist_agent::AgentEvent::Yielded { payload }
                            if payload
                                .get("complete")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false)
                    )
                });
                if !yielded {
                    return Ok(serde_json::json!({
                        "status": "failed",
                        "error": "fork terminated without yield.complete=true",
                        "response": outcome.response,
                    }));
                }
                Ok(serde_json::json!({
                    "status": "completed",
                    "response": outcome.response,
                    "usage": outcome.usage,
                    "tool_rounds": outcome.tool_rounds,
                }))
            }
            Err(AgentError::Cancelled) => Err(AgentError::Cancelled),
            Err(AgentError::DeadlineExceeded) => Err(AgentError::DeadlineExceeded),
            Err(error) => Ok(serde_json::json!({
                "status": "failed",
                "error": error.to_string(),
            })),
        }
    }
}

#[async_trait]
impl ForkExecutor for ComponentForkExecutor {
    async fn execute(
        &self,
        fork_id: String,
        context: artist_session::Snapshot,
        task: String,
        control: TurnControl,
    ) -> Result<serde_json::Value, AgentError> {
        self.execute_child(
            fork_id,
            ForkIngress {
                snapshot: context,
                messages: Vec::new(),
            },
            task,
            control,
        )
        .await
    }

    async fn execute_with_ingress(
        &self,
        fork_id: String,
        ingress: ForkIngress,
        task: String,
        control: TurnControl,
    ) -> Result<serde_json::Value, AgentError> {
        self.execute_child(fork_id, ingress, task, control).await
    }
}

impl ComponentRuntimeFactory {
    pub fn new(extension_root: impl Into<PathBuf>) -> Self {
        Self {
            extension_root: extension_root.into(),
            configurations: Arc::new(RwLock::new(BTreeMap::new())),
            hosts: tokio::sync::Mutex::new(BTreeMap::new()),
        }
    }

    /// Install a provider configuration by stable identity. The secret stays
    /// in this process-local component configuration and is never copied into
    /// session metadata or event payloads.
    pub fn register_provider_config(
        &self,
        config: ProviderConfig,
    ) -> Result<(), ProviderConfigError> {
        config.validate()?;
        let id = config.id.to_string();
        let mut configurations = self.configurations.write().unwrap();
        if configurations.contains_key(&id) {
            return Err(ProviderConfigError::Duplicate(config.id));
        }
        configurations.insert(id, config);
        Ok(())
    }

    pub fn from_environment(extension_root: impl Into<PathBuf>) -> Self {
        let factory = Self::new(extension_root);
        let Ok(secret) = env::var("OPENAI_API_KEY") else {
            return factory;
        };
        let id = ProviderConfigId::new("openai-default").expect("static provider id is valid");
        let config = ProviderConfig {
            id,
            provider_type: "openai".into(),
            endpoint: env::var("ARTIST_OPENAI_ENDPOINT").ok(),
            default_model: env::var("ARTIST_MODEL").ok(),
            options: serde_json::Value::Null,
            auth: ProviderAuth::ApiKey { secret },
        };
        let _ = factory.register_provider_config(config);
        factory
    }

    async fn host_for(&self, workspace: &Workspace) -> Result<Arc<ComponentHost>, AgentError> {
        let key = workspace.id.to_string();
        let mut hosts = self.hosts.lock().await;
        if let Some(host) = hosts.get(&key).cloned() {
            return Ok(host);
        }
        if !self.extension_root.is_dir() {
            return Err(AgentError::Composition(format!(
                "extension root does not exist: {}",
                self.extension_root.display()
            )));
        }
        let kernel = Arc::new(Kernel::with_agents());
        kernel.register(FilesNamespace::new(workspace.root.clone()));
        let local_root = workspace.root.join(".artist");
        artist_component::install_prompt_view(
            &kernel,
            self.extension_root.join(".artist/prompt"),
            local_root.join("prompt"),
        );
        artist_component::install_profile_view(
            &kernel,
            self.extension_root.join(".artist/profile"),
            local_root.join("profile"),
        );
        let source = UrlCompositionSource::new(
            self.extension_root.clone(),
            workspace.root.join(".artist/url"),
        );
        let host = Arc::new(
            ComponentHost::start(kernel, source)
                .await
                .map_err(|error| AgentError::Composition(error.to_string()))?,
        );
        hosts.insert(key, Arc::clone(&host));
        Ok(host)
    }

    fn provider_config(
        &self,
        requested: Option<&str>,
        provider_type: Option<&str>,
    ) -> Result<ProviderConfig, AgentError> {
        let configurations = self.configurations.read().unwrap();
        let id = if let Some(requested) = requested {
            requested.to_owned()
        } else {
            let candidates: Vec<_> = configurations
                .iter()
                .filter(|(_, config)| {
                    provider_type.is_none_or(|provider_type| config.provider_type == provider_type)
                })
                .map(|(id, _)| id.clone())
                .collect();
            match candidates.as_slice() {
                [id] => id.clone(),
                [] => {
                    return Err(AgentError::ProviderSelection(
                        provider_type
                            .map(|provider_type| {
                                format!(
                                    "profile selected provider type {provider_type:?}, but no configuration is installed"
                                )
                            })
                            .unwrap_or_else(|| {
                                "no provider configuration is selected; configure a stable provider id"
                                    .into()
                            }),
                    ));
                }
                _ => {
                    return Err(AgentError::ProviderSelection(
                        "multiple provider configurations match; select a stable provider id"
                            .into(),
                    ));
                }
            }
        };
        configurations.get(&id).cloned().ok_or_else(|| {
            AgentError::ProviderSelection(format!("provider configuration {id:?} is unavailable"))
        })
    }

    async fn profile(
        host: &ComponentHost,
        resource_id: Option<&str>,
        profile_id: Option<&str>,
    ) -> Result<Option<ProfileDocument>, AgentError> {
        let Some(profile_id) = profile_id else {
            return Ok(None);
        };
        let resource_id = resource_id.unwrap_or("profile://filesystem");
        let component = host
            .profiles()
            .selected(Some(resource_id))
            .map_err(|error| AgentError::Composition(error.to_string()))?
            .ok_or_else(|| {
                AgentError::Composition(format!("profile component {resource_id:?} is unavailable"))
            })?;
        component
            .resolve(profile_id)
            .await
            .map(Some)
            .map_err(|error| AgentError::Composition(error.to_string()))
    }

    fn definitions(host: &ComponentHost) -> Vec<ToolDefinition> {
        host.tools()
            .all_names()
            .into_iter()
            .map(|name| ToolDefinition {
                description: Some(format!("Component-provided {name} operation")),
                input_schema: schema_for(&name),
                name,
            })
            .collect()
    }

    async fn build_runtime(
        &self,
        workspace: Workspace,
        metadata: artist_agent::daemon::SessionMetadata,
        log: Arc<EventLog>,
    ) -> Result<Arc<dyn SessionTurnRunner>, AgentError> {
        let metadata = metadata_after_profile_changes(metadata, &log)?;
        let host = self.host_for(&workspace).await?;
        host.kernel()
            .register_agent_transcript(
                metadata.session_id.clone(),
                EventLogTranscript::new(Arc::clone(&log)),
            )
            .map_err(|error| AgentError::Context(error.to_string()))?;
        let profile = Self::profile(
            &host,
            metadata.profile_resource_id.as_deref(),
            metadata.profile_id.as_deref(),
        )
        .await?;
        let profile_name = metadata
            .profile_id
            .clone()
            .unwrap_or_else(|| "default".into());
        let permissions = profile
            .as_ref()
            .map(|profile| profile.permissions.clone())
            .unwrap_or_default();
        let definitions = Self::definitions(&host);
        if let Some(profile) = &profile {
            for required in &profile.required_tools {
                if !definitions
                    .iter()
                    .any(|definition| &definition.name == required)
                {
                    return Err(AgentError::ToolSurface(format!(
                        "profile requires unavailable tool {required:?}"
                    )));
                }
            }
        }
        let policy = HarnessPolicy {
            yield_schema: profile
                .as_ref()
                .map(ProfileDocument::yield_schema)
                .unwrap_or_else(|| HarnessPolicy::default().yield_schema),
            allow_fork: profile.as_ref().is_some_and(|profile| profile.allow_fork),
            allow_handoff: profile
                .as_ref()
                .is_some_and(|profile| profile.allow_handoff),
        };
        let surface = ToolSurface::new(host.tools())
            .with_log(Arc::clone(&log))
            .with_permissions(profile_name.clone(), permissions.clone());
        surface
            .replace_static_definitions(definitions.clone())
            .map_err(|error| AgentError::ToolSurface(error.to_string()))?;
        surface.configure_harness(policy.clone());

        let identity = profile
            .as_ref()
            .and_then(|profile| profile.identity.clone())
            .or(metadata.identity.clone())
            .unwrap_or_else(|| metadata.session_id.clone());
        let composition_input = CompositionInput {
            identity,
            profile: metadata.profile_id.clone(),
            system: None,
            agent_instructions: profile
                .as_ref()
                .and_then(|profile| profile.post_system.clone()),
            profile_content: profile.as_ref().map(|profile| profile.prompt.clone()),
            visible_resources: Vec::new(),
            tool_events: definitions
                .iter()
                .filter_map(|definition| serde_json::to_string(definition).ok())
                .collect(),
        };
        let context = host
            .compose_initial(composition_input.clone())
            .await
            .map_err(|error| AgentError::Composition(error.to_string()))?;

        let config = self.provider_config(
            metadata.provider_config_id.as_deref().or_else(|| {
                profile
                    .as_ref()
                    .and_then(|profile| profile.provider_config.as_deref())
            }),
            profile
                .as_ref()
                .and_then(|profile| profile.provider.as_deref()),
        )?;
        let resource_resolver: Arc<dyn ResourceResolver> = Arc::new(KernelResourceResolver {
            kernel: Arc::clone(host.kernel()),
        });
        let provider = host
            .provider_catalog()
            .configure_refreshed_with_resource_resolver(
                &config,
                Some(Arc::clone(&resource_resolver)),
            )
            .await
            .map_err(|error| AgentError::ProviderSelection(error.to_string()))?;
        let provider_resolver = Arc::new(ConfigProviderResolver {
            catalog: host.provider_catalog().clone(),
            configurations: Arc::clone(&self.configurations),
            resource_resolver,
        });
        let handoff = Arc::new(ProfileHandoffHandler {
            host: Arc::clone(&host),
            configurations: Arc::clone(&self.configurations),
            resource_id: metadata
                .profile_resource_id
                .unwrap_or_else(|| "profile://filesystem".into()),
            identity: metadata
                .identity
                .unwrap_or_else(|| metadata.session_id.clone()),
        });
        let model = metadata
            .model
            .or_else(|| profile.as_ref().and_then(|profile| profile.model.clone()))
            .or(config.default_model)
            .unwrap_or_else(|| "gpt-4.1-mini".into());
        let compaction = if let Some(resource_id) = metadata.compaction_resource_id.as_deref() {
            host.compaction()
                .selected(Some(resource_id))
                .map_err(|error| AgentError::Compaction(error.to_string()))?
        } else {
            None
        };
        let shared_provider = Arc::new(SharedProvider(provider));
        let fork_executor = Arc::new(ComponentForkExecutor {
            provider: Arc::clone(&shared_provider),
            tools: host.tools(),
            definitions: definitions.clone(),
            host: Arc::clone(&host),
            composition_input: composition_input.clone(),
            model: model.clone(),
            profile_id: profile_name,
            permissions,
            harness: policy,
            compaction: compaction.clone(),
            compaction_context_limit: None,
            workspace_root: workspace.root.clone(),
        });
        let mut runner =
            EngineTurnRunner::new(Arc::clone(&shared_provider), Arc::new(surface.clone()))
                .with_default_model(model)
                .with_context(context)
                .with_tool_surface(Arc::new(surface))
                .with_composition(host.clone(), composition_input)
                .with_provider_resolver(provider_resolver)
                .with_handoff_handler(handoff)
                .with_fork_executor(fork_executor);
        if let Some(compaction) = compaction {
            runner = runner.with_compaction(compaction, None);
        }
        Ok(Arc::new(runner))
    }
}

#[async_trait]
impl SessionRuntimeFactory for ComponentRuntimeFactory {
    async fn resolve(
        &self,
        workspace: Workspace,
        metadata: artist_agent::daemon::SessionMetadata,
        log: Arc<EventLog>,
    ) -> Result<Arc<dyn SessionTurnRunner>, AgentError> {
        // Resolve on every turn. A handoff changes the active provider,
        // profile, permissions, and composition input durably; rebuilding the
        // small session runner from the log is what makes the next turn and a
        // daemon restart observe the same active state.
        self.build_runtime(workspace, metadata, log).await
    }
}

fn metadata_after_profile_changes(
    mut metadata: artist_agent::daemon::SessionMetadata,
    log: &Arc<EventLog>,
) -> Result<artist_agent::daemon::SessionMetadata, AgentError> {
    for record in log.records()? {
        if record.event_type != "agent.profile_changed" {
            continue;
        }
        let event: artist_agent::AgentEvent =
            serde_json::from_value(record.payload).map_err(|error| {
                AgentError::Context(format!(
                    "invalid profile transition at sequence {}: {error}",
                    record.sequence
                ))
            })?;
        if let artist_agent::AgentEvent::ProfileChanged {
            profile_id,
            provider_id,
            model,
        } = event
        {
            metadata.profile_id = Some(profile_id);
            if provider_id.is_some() {
                metadata.provider_config_id = provider_id;
            }
            if model.is_some() {
                metadata.model = model;
            }
        }
    }
    Ok(metadata)
}

struct ConfigProviderResolver {
    catalog: ProviderCatalog,
    configurations: Arc<RwLock<BTreeMap<String, ProviderConfig>>>,
    resource_resolver: Arc<dyn ResourceResolver>,
}

#[async_trait]
impl ProviderResolver for ConfigProviderResolver {
    async fn resolve(
        &self,
        provider_config_id: &str,
    ) -> Result<Arc<dyn ModelProvider>, AgentError> {
        let config = self
            .configurations
            .read()
            .unwrap()
            .get(provider_config_id)
            .cloned()
            .ok_or_else(|| {
                AgentError::ProviderSelection(format!(
                    "provider configuration {provider_config_id:?} is unavailable"
                ))
            })?;
        self.catalog
            .configure_refreshed_with_resource_resolver(
                &config,
                Some(Arc::clone(&self.resource_resolver)),
            )
            .await
            .map_err(|error| AgentError::ProviderSelection(error.to_string()))
    }
}

struct KernelResourceResolver {
    kernel: Arc<Kernel>,
}

#[async_trait]
impl ResourceResolver for KernelResourceResolver {
    async fn resolve(&self, uri: &str) -> Result<ResolvedResource, ProviderError> {
        const MAX_PROVIDER_RESOURCE: u64 = 64 * 1024 * 1024;
        let uri: ResourceUri = uri.parse().map_err(|error| ProviderError::Request {
            message: format!("invalid resource reference: {error}"),
        })?;
        let attrs = self
            .kernel
            .attrs_uri(&uri)
            .await
            .map_err(|error| ProviderError::Request {
                message: format!("resource lookup failed: {error}"),
            })?;
        if attrs.size > MAX_PROVIDER_RESOURCE {
            return Err(ProviderError::Unsupported {
                feature: format!("resource {uri} exceeds provider input capacity"),
            });
        }
        let size = u32::try_from(attrs.size).map_err(|_| ProviderError::Unsupported {
            feature: format!("resource {uri} exceeds provider read range capacity"),
        })?;
        let bytes =
            self.kernel
                .read_uri(&uri, 0, size)
                .await
                .map_err(|error| ProviderError::Request {
                    message: format!("resource read failed: {error}"),
                })?;
        Ok(ResolvedResource {
            mime_type: mime_for_uri(uri.path()),
            bytes,
        })
    }
}

fn mime_for_uri(path: &str) -> Option<String> {
    let extension = path
        .rsplit('/')
        .next()?
        .rsplit_once('.')?
        .1
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "txt" | "md" | "rs" | "js" | "ts" | "tsx" | "jsx" | "py" | "go" | "json" | "toml"
        | "yaml" | "yml" | "xml" | "html" | "css" => "text/plain",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        _ => return None,
    };
    Some(mime.into())
}

struct ProfileHandoffHandler {
    host: Arc<ComponentHost>,
    configurations: Arc<RwLock<BTreeMap<String, ProviderConfig>>>,
    resource_id: String,
    identity: String,
}

#[async_trait]
impl HandoffHandler for ProfileHandoffHandler {
    async fn resolve(&self, profile_id: &str, brief: &str) -> Result<HandoffPlan, AgentError> {
        let profile =
            ComponentRuntimeFactory::profile(&self.host, Some(&self.resource_id), Some(profile_id))
                .await?
                .ok_or_else(|| AgentError::Composition("handoff profile is unavailable".into()))?;
        let definitions = ComponentRuntimeFactory::definitions(&self.host);
        for required in &profile.required_tools {
            if !definitions
                .iter()
                .any(|definition| &definition.name == required)
            {
                return Err(AgentError::ToolSurface(format!(
                    "profile requires unavailable tool {required:?}"
                )));
            }
        }
        let input = CompositionInput {
            identity: profile
                .identity
                .clone()
                .unwrap_or_else(|| self.identity.clone()),
            profile: Some(profile_id.into()),
            system: None,
            agent_instructions: profile.post_system.clone(),
            profile_content: profile.prompt.clone().into(),
            visible_resources: Vec::new(),
            tool_events: definitions
                .iter()
                .filter_map(|definition| serde_json::to_string(definition).ok())
                .collect(),
        };
        let context = self
            .host
            .compose_initial(input.clone())
            .await
            .map_err(|error| AgentError::Composition(error.to_string()))?;
        // The brief is recorded by AgentEngine as the durable handoff
        // ingress. Keeping it out of the replacement snapshot prevents the
        // same user message from appearing once in the context prefix and
        // again in the replayed conversation.
        let _ = brief;
        let yield_schema = profile.yield_schema();
        let permissions = profile.permissions.clone();
        let provider_config_id = select_profile_provider_config(
            &self.configurations,
            profile.provider_config.as_deref(),
            profile.provider.as_deref(),
        )?;
        let model = profile.model.clone().or_else(|| {
            provider_config_id.as_deref().and_then(|id| {
                self.configurations
                    .read()
                    .unwrap()
                    .get(id)
                    .and_then(|config| config.default_model.clone())
            })
        });
        Ok(HandoffPlan {
            profile_id: profile_id.into(),
            context,
            composition_input: Some(input),
            model,
            provider_config_id,
            tool_definitions: Some(definitions),
            permissions: Some((profile_id.into(), permissions)),
            harness: Some(HarnessPolicy {
                yield_schema,
                allow_fork: profile.allow_fork,
                allow_handoff: profile.allow_handoff,
            }),
        })
    }
}

fn select_profile_provider_config(
    configurations: &Arc<RwLock<BTreeMap<String, ProviderConfig>>>,
    requested: Option<&str>,
    provider_type: Option<&str>,
) -> Result<Option<String>, AgentError> {
    let configurations = configurations.read().unwrap();
    if let Some(requested) = requested {
        if configurations.contains_key(requested) {
            return Ok(Some(requested.to_owned()));
        }
        return Err(AgentError::ProviderSelection(format!(
            "provider configuration {requested:?} is unavailable"
        )));
    }
    let Some(provider_type) = provider_type else {
        return Ok(None);
    };
    let candidates: Vec<_> = configurations
        .iter()
        .filter(|(_, config)| config.provider_type == provider_type)
        .map(|(id, _)| id.clone())
        .collect();
    match candidates.as_slice() {
        [id] => Ok(Some(id.clone())),
        [] => Err(AgentError::ProviderSelection(format!(
            "profile selected provider type {provider_type:?}, but no configuration is installed"
        ))),
        _ => Err(AgentError::ProviderSelection(format!(
            "multiple provider configurations match provider type {provider_type:?}; select a stable provider id"
        ))),
    }
}

fn schema_for(name: &str) -> serde_json::Value {
    match name {
        "read" => serde_json::json!({
            "type":"object",
            "properties":{
                "uri":{"type":"string"},
                "range":{"type":"string","description":"Relative line range such as -20..+40"}
            },
            "required":["uri"],"additionalProperties":false
        }),
        "write" => serde_json::json!({
            "type":"object","properties":{"uri":{"type":"string"},"content":{"type":"string"}},
            "required":["uri","content"],"additionalProperties":false
        }),
        "move" => serde_json::json!({
            "type":"object","properties":{"source":{"type":"string"},"destination":{"type":["string","null"]}},
            "required":["source"],"additionalProperties":false
        }),
        "edit" => serde_json::json!({
            "type":"object",
            "properties":{
                "uri":{"type":"string"},
                "changes":{
                    "type":"array",
                    "items":{
                        "type":"object",
                        "properties":{"anchor":{"type":"string"},"replacement":{"type":"string"}},
                        "required":["anchor","replacement"],"additionalProperties":false
                    }
                }
            },
            "required":["uri","changes"],"additionalProperties":false
        }),
        "find" => serde_json::json!({
            "type":"object",
            "properties":{
                "uri":{"type":"string"},
                "query":{"type":"string"},
                "limit":{"type":"integer","minimum":1,"maximum":500},
                "mode":{"type":"string","enum":["literal","glob","fuzzy"]}
            },
            "required":["uri","query"],"additionalProperties":false
        }),
        "grep" => serde_json::json!({
            "type":"object",
            "properties":{
                "uri":{"type":"string"},
                "query":{"type":"string"},
                "limit":{"type":"integer","minimum":1,"maximum":500},
                "mode":{"type":"string","enum":["literal","plain","regex"]}
            },
            "required":["uri","query"],"additionalProperties":false
        }),
        "run" => serde_json::json!({
            "type":"object",
            "properties":{
                "executable":{"type":"string"},
                "target":{"type":"string"},
                "arguments":{"type":"array","items":{"type":"string"}},
                "working_directory":{"type":"string"},
                "environment":{"type":"object","additionalProperties":{"type":"string"}}
            },
            "anyOf":[{"required":["executable"]},{"required":["target"]}],
            "additionalProperties":false
        }),
        "poll" => serde_json::json!({
            "type":"object",
            "properties":{
                "target":{"type":"string"},
                "match":{"type":"string"},
                "timeout_ms":{"type":"integer","minimum":0,"maximum":60000}
            },
            "required":["target","timeout_ms"],"additionalProperties":false
        }),
        "signal" => serde_json::json!({
            "type":"object",
            "properties":{"process":{"type":"string"},"signal":{"type":"string"}},
            "required":["process","signal"],"additionalProperties":false
        }),
        _ => serde_json::json!({"type":"object"}),
    }
}
