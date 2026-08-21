//! Default daemon runtime assembly.
//!
//! The daemon owns session lifetime, but this module owns the concrete
//! component selection used by the shipped executable. Keeping this assembly
//! here prevents provider/profile/tool policy from leaking into the kernel or
//! the generic agent state machine.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use artist_agent::daemon::{
    EngineTurnRunner, SessionRuntimeFactory, SessionTurnRunner, SharedProvider,
};
use artist_agent::{
    AgentEngine, AgentError, ForkExecutor, ForkIngress, HandoffHandler, HandoffPlan, HarnessPolicy,
    ProviderResolver, ToolSurface, TurnControl, TurnRequest,
};
use artist_component::{
    CompactionComponent, ComponentHost, ComponentToolRegistry, CompositionInput,
    EventLogTranscript, PermissionRegistry, ProfileDocument, ProviderSocket, UrlCompositionSource,
};
use artist_kernel::{FilesNamespace, Kernel, ResourceSignal, ResourceUri};
use artist_session::{EventLog, Workspace};
use async_trait::async_trait;
use llm_provider::{
    Message, ModelProvider, ProviderConfig, ProviderConfigError, ProviderError, ResolvedResource,
    ResourceResolver, Role, ToolDefinition,
};

pub struct ComponentRuntimeFactory {
    extension_root: PathBuf,
    providers: Arc<ProviderSocket>,
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
        if let Some(result) = completed_fork_from_log(&log)? {
            return Ok(result);
        }
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
                        artist_agent::AgentEvent::HarnessCompleted {
                            operation: Some(artist_component::HarnessOperation::Yield {
                                complete: true,
                                ..
                            }),
                            result,
                            ..
                        } if !result.is_error
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

fn completed_fork_from_log(log: &EventLog) -> Result<Option<serde_json::Value>, AgentError> {
    let mut yielded = false;
    let mut response = None;
    for record in log.records()? {
        let event: artist_agent::AgentEvent = match record.event_type.as_str() {
            value if value.starts_with("agent.") => serde_json::from_value(record.payload)
                .map_err(|error| AgentError::Context(error.to_string()))?,
            _ => continue,
        };
        match event {
            artist_agent::AgentEvent::HarnessCompleted {
                operation: Some(artist_component::HarnessOperation::Yield { complete: true, .. }),
                result,
                ..
            } if !result.is_error => yielded = true,
            artist_agent::AgentEvent::TurnCompleted { response: value } => response = Some(value),
            _ => {}
        }
    }
    Ok(yielded.then(|| {
        serde_json::json!({
            "status": "completed",
            "response": response,
            "usage": llm_provider::Usage::default(),
            "tool_rounds": 0,
        })
    }))
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
            providers: Arc::new(ProviderSocket::builtins()),
            hosts: tokio::sync::Mutex::new(BTreeMap::new()),
        }
    }

    /// Install a provider configuration through the provider configuration
    /// component. The secret never enters session metadata or event payloads.
    pub fn register_provider_config(
        &self,
        config: ProviderConfig,
    ) -> Result<(), ProviderConfigError> {
        self.providers.register(config)
    }

    pub fn from_environment(extension_root: impl Into<PathBuf>) -> Self {
        Self {
            extension_root: extension_root.into(),
            providers: Arc::new(ProviderSocket::from_environment()),
            hosts: tokio::sync::Mutex::new(BTreeMap::new()),
        }
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
        let kernel = Arc::new(Kernel::with_url());
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
            ComponentHost::start_with_provider_socket(kernel, source, (*self.providers).clone())
                .await
                .map_err(|error| AgentError::Composition(error.to_string()))?,
        );
        hosts.insert(key, Arc::clone(&host));
        Ok(host)
    }

    fn provider_config(
        providers: &ProviderSocket,
        requested: Option<&str>,
        provider_type: Option<&str>,
    ) -> Result<ProviderConfig, AgentError> {
        providers
            .select(requested, provider_type)
            .map_err(|error| AgentError::ProviderSelection(error.to_string()))
    }

    async fn profile(
        host: &ComponentHost,
        resource_id: Option<&str>,
        profile_id: Option<&str>,
    ) -> Result<Option<ProfileDocument>, AgentError> {
        let Some(profile_id) = profile_id else {
            return Ok(None);
        };
        let component = host
            .profiles()
            .selected(resource_id)
            .map_err(|error| AgentError::Composition(error.to_string()))?
            .ok_or_else(|| {
                AgentError::Composition(format!(
                    "no profile component is selected for {profile_id:?}"
                ))
            })?;
        component
            .resolve(profile_id)
            .await
            .map(Some)
            .map_err(|error| AgentError::Composition(error.to_string()))
    }

    fn definitions(host: &ComponentHost) -> Vec<ToolDefinition> {
        host.tools().definitions()
    }

    async fn build_runtime(
        &self,
        workspace: Workspace,
        metadata: artist_agent::daemon::SessionMetadata,
        log: Arc<EventLog>,
    ) -> Result<Arc<dyn SessionTurnRunner>, AgentError> {
        let metadata = metadata_after_profile_changes(metadata, &log)?;
        let host = self.host_for(&workspace).await?;
        host.register_agent_transcript(
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
        if let Some(profile) = &profile {
            let enabled = surface
                .definitions()
                .into_iter()
                .map(|definition| definition.name)
                .collect::<std::collections::BTreeSet<_>>();
            for required in &profile.required_tools {
                if !enabled.contains(required) {
                    return Err(AgentError::ToolSurface(format!(
                        "profile requires unavailable or denied tool {required:?}"
                    )));
                }
            }
        }

        let identity = profile
            .as_ref()
            .and_then(|profile| profile.identity.clone())
            .or(metadata.identity.clone())
            .unwrap_or_else(|| metadata.session_id.clone());
        let composition_input = CompositionInput {
            identity,
            profile: metadata.profile_id.clone(),
            profile_resource: metadata.profile_resource_id.clone(),
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

        let config = Self::provider_config(
            host.providers(),
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
            .providers()
            .configure(&config, Some(Arc::clone(&resource_resolver)))
            .await
            .map_err(|error| AgentError::ProviderSelection(error.to_string()))?;
        let provider_resolver = Arc::new(ConfigProviderResolver {
            providers: host.providers().clone(),
            resource_resolver,
        });
        let handoff = Arc::new(ProfileHandoffHandler {
            host: Arc::clone(&host),
            providers: host.providers().clone(),
            resource_id: metadata.profile_resource_id,
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

    async fn write_resource(
        &self,
        workspace: Workspace,
        uri: ResourceUri,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<u32, AgentError> {
        let host = self.host_for(&workspace).await?;
        host.kernel()
            .write_uri(&uri, offset, &data)
            .await
            .map_err(|error| AgentError::Context(error.to_string()))
    }

    async fn signal_resource(
        &self,
        workspace: Workspace,
        uri: ResourceUri,
        signal: ResourceSignal,
    ) -> Result<(), AgentError> {
        let host = self.host_for(&workspace).await?;
        host.kernel()
            .signal_uri(&uri, signal)
            .await
            .map_err(|error| AgentError::Context(error.to_string()))
    }

    async fn move_resource(
        &self,
        workspace: Workspace,
        source: ResourceUri,
        destination: ResourceUri,
    ) -> Result<(), AgentError> {
        let host = self.host_for(&workspace).await?;
        host.kernel()
            .move_uri(&source, &destination)
            .await
            .map_err(|error| AgentError::Context(error.to_string()))
    }
}

fn metadata_after_profile_changes(
    mut metadata: artist_agent::daemon::SessionMetadata,
    log: &Arc<EventLog>,
) -> Result<artist_agent::daemon::SessionMetadata, AgentError> {
    for record in log.records()? {
        if !matches!(
            record.event_type.as_str(),
            "agent.profile_changed" | "agent.handoff" | "agent.harness_completed"
        ) {
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
        } else if let artist_agent::AgentEvent::Handoff { profile_id, .. } = event {
            // Legacy logs used a separate handoff projection. New logs use
            // HarnessCompleted as the sole transaction record below.
            metadata.profile_id = Some(profile_id);
        } else if let artist_agent::AgentEvent::HarnessCompleted {
            operation: Some(artist_component::HarnessOperation::Handoff { profile, .. }),
            result,
            ..
        } = event
        {
            if result.is_error {
                continue;
            }
            metadata.profile_id = Some(profile);
            let output = result.content.iter().find_map(|part| match part {
                llm_provider::ContentPart::Text { text } => {
                    serde_json::from_str::<artist_component::ToolResultEnvelope>(text)
                        .ok()
                        .and_then(|envelope| envelope.output)
                }
                _ => None,
            });
            if let Some(output) = output {
                if let Some(provider) = output.get("provider").and_then(serde_json::Value::as_str) {
                    metadata.provider_config_id = Some(provider.to_owned());
                }
                if let Some(model) = output.get("model").and_then(serde_json::Value::as_str) {
                    metadata.model = Some(model.to_owned());
                }
            }
        }
    }
    Ok(metadata)
}

struct ConfigProviderResolver {
    providers: ProviderSocket,
    resource_resolver: Arc<dyn ResourceResolver>,
}

#[async_trait]
impl ProviderResolver for ConfigProviderResolver {
    async fn resolve(
        &self,
        provider_config_id: &str,
    ) -> Result<Arc<dyn ModelProvider>, AgentError> {
        let config = self
            .providers
            .configuration(provider_config_id)
            .map_err(|error| AgentError::ProviderSelection(error.to_string()))?;
        self.providers
            .configure(&config, Some(Arc::clone(&self.resource_resolver)))
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
    providers: ProviderSocket,
    resource_id: Option<String>,
    identity: String,
}

#[async_trait]
impl HandoffHandler for ProfileHandoffHandler {
    async fn resolve(&self, profile_id: &str, brief: &str) -> Result<HandoffPlan, AgentError> {
        let profile = ComponentRuntimeFactory::profile(
            &self.host,
            self.resource_id.as_deref(),
            Some(profile_id),
        )
        .await?
        .ok_or_else(|| AgentError::Composition("handoff profile is unavailable".into()))?;
        let definitions = ComponentRuntimeFactory::definitions(&self.host);
        let permissions = profile.permissions.clone();
        let policy = HarnessPolicy {
            yield_schema: profile.yield_schema(),
            allow_fork: profile.allow_fork,
            allow_handoff: profile.allow_handoff,
        };
        let validation_surface = ToolSurface::new(self.host.tools())
            .with_permissions(profile_id.to_owned(), permissions.clone());
        validation_surface
            .replace_static_definitions(definitions.clone())
            .map_err(|error| AgentError::ToolSurface(error.to_string()))?;
        validation_surface.configure_harness(policy.clone());
        let enabled = validation_surface
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect::<std::collections::BTreeSet<_>>();
        for required in &profile.required_tools {
            if !enabled.contains(required) {
                return Err(AgentError::ToolSurface(format!(
                    "profile requires unavailable or denied tool {required:?}"
                )));
            }
        }
        let input = CompositionInput {
            identity: profile
                .identity
                .clone()
                .unwrap_or_else(|| self.identity.clone()),
            profile: Some(profile_id.into()),
            profile_resource: self.resource_id.clone(),
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
        let provider_config_id = select_profile_provider_config(
            &self.providers,
            profile.provider_config.as_deref(),
            profile.provider.as_deref(),
        )?;
        let model = profile.model.clone().or_else(|| {
            provider_config_id
                .as_deref()
                .and_then(|id| self.providers.default_model(id))
        });
        Ok(HandoffPlan {
            profile_id: profile_id.into(),
            context,
            composition_input: Some(input),
            model,
            provider_config_id,
            tool_definitions: Some(definitions),
            permissions: Some((profile_id.into(), permissions)),
            harness: Some(policy),
        })
    }
}

fn select_profile_provider_config(
    providers: &ProviderSocket,
    requested: Option<&str>,
    provider_type: Option<&str>,
) -> Result<Option<String>, AgentError> {
    if requested.is_none() && provider_type.is_none() {
        return Ok(None);
    }
    providers
        .select(requested, provider_type)
        .map(|config| Some(config.id.to_string()))
        .map_err(|error| AgentError::ProviderSelection(error.to_string()))
}
