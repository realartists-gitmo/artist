//! The long-lived artist MCP process.
//!
//! Stdio mode spawns one process per connection, so anything a connection needs
//! beyond the envelope must survive that respawn on disk — which is what the
//! outbox does. But recorder, computer registry, canvas, memory, and identity
//! are process-lifetime resources: they should survive a tunnel reconnect
//! *in memory*, not be rebuilt from scratch each time. This module is the
//! daemon that owns them once and serves one or many connections against the
//! resulting surface, over stdio or Streamable HTTP on a loopback port.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, anyhow};
use artist_agent::{
    SessionHandles,
    profiles::Profiles,
    tool_set::{McpDelegation, McpIdentity, McpSurface},
};
use artist_canvas::server::Lazy;
use artist_tools::Workspace;
use llm_provider::{ProviderId, ProviderSet, SavedProvider};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use serde::Deserialize;

use crate::canvas_host::McpCanvasHost;
use crate::{
    http_server::{HttpMcpServer, SurfaceFactory},
    server::McpServer,
};

/// Which subsystems a daemon may bring up, one field per command-line flag.
#[derive(Clone, Copy, Debug, Default)]
pub struct Allow {
    pub memory: bool,
    pub subagent: bool,
}

/// Builds one MCP server per logical transport session.

/// A running artist MCP process: shared resources, session-specific identities.
pub struct McpDaemon {
    factory: SurfaceFactory,
    actor: String,
    project: String,
    profile: String,
    profile_instructions: String,
    identities: artist_registry::HttpIdentities,
    /// Held so the recorder's writer task stays alive for the daemon's life.
    _writer: Option<artist_session::WriterTask>,
}

impl McpDaemon {
    /// Build the shared process resources. A concrete server is created when a
    /// transport session starts, so two web sessions never share an identity.
    pub async fn build(
        project: &Path,
        state_dir: &Path,
        profile_name: &str,
        actor: &str,
        allow: Allow,
    ) -> anyhow::Result<Self> {
        let project = std::fs::canonicalize(project).context("canonicalize project root")?;
        let profiles = Profiles::discover(&project);
        let profile = profiles.get(profile_name).map_err(|error| anyhow!(error))?;
        let config_root = config_root()?;
        let settings = FileSettings::load(&config_root, &project);

        let conversation_id = format!("mcp:{actor}");
        let session_dir = state_dir.join("sessions").join(actor);
        let delegation_attachments =
            artist_session::AttachmentStore::new(session_dir.join("attachments"));
        let (recorder, writer) =
            match artist_session::EventLogWriter::open(&session_dir, &conversation_id) {
                Ok(writer) => {
                    let (recorder, task) = artist_session::spawn_writer(writer, None);
                    (recorder, Some(task))
                }
                Err(error) => {
                    tracing::warn!(
                        session = %session_dir.display(),
                        "could not open the session log: {error:#}; recording disabled"
                    );
                    (artist_session::Recorder::noop(), None)
                }
            };

        // Resource construction is not a visibility gate. The selected profile is the
        // only authority deciding whether `computer`, `canvas`, `ask`, etc. are listed.
        let computer = Some(artist_computer::SurfaceRegistry::for_project(
            &project,
            screen(&settings.computer),
        ));
        let ask = artist_session::AskRegistry::for_project(&project, Some(recorder.clone()));
        let memory_config = settings.memory();
        let memory = match allow.memory {
            true => open_memory(&config_root, &memory_config).await,
            false => None,
        };
        let delegation = if allow.subagent {
            build_delegation(
                &config_root,
                &conversation_id,
                &recorder,
                delegation_attachments,
                computer.clone(),
                memory.clone(),
                &profiles,
            )
        } else {
            None
        };

        let project_text = project.display().to_string();
        let state_dir = state_dir.to_path_buf();
        let profile_name = profile_name.to_owned();
        let factory_project = project.clone();
        let factory_workspace = Workspace::open(&factory_project, &state_dir, actor)?;
        let factory_pages = artist_agent::pagination::PageStore::open(Some(&state_dir))?;
        let factory_config_root = config_root.clone();
        let factory_profile = profile.clone();
        let profile_instructions = profile.instructions.clone();
        let factory_profile_instructions = profile_instructions.clone();
        let factory_recorder = recorder.clone();
        let factory_ask = ask.clone();
        let factory_computer = computer.clone();
        let factory_memory = memory.clone();
        let factory_delegation = delegation.clone();
        let factory_profile_name = profile_name.clone();
        let factory: SurfaceFactory = Arc::new(move |identity: Option<McpIdentity>| {
            let actor_key = identity
                .as_ref()
                .map(|identity| identity.actor.as_str())
                .unwrap_or("mcp-http-discovery");
            let artist_name = identity
                .as_ref()
                .map(|identity| identity.name.as_str())
                .unwrap_or("anonymous");
            let workspace = factory_workspace.with_actor(actor_key)?;
            let session_conversation = format!("mcp:{actor_key}");
            let session_attachments = artist_session::AttachmentStore::new(
                state_dir
                    .join("sessions")
                    .join(actor_key)
                    .join("attachments"),
            );
            let memory_writer = factory_memory.as_ref().map(|handle| {
                handle.writer(factory_recorder.clone(), session_conversation.clone())
            });
            let canvas_host = Arc::new(McpCanvasHost::new(
                factory_ask.clone(),
                artist_name,
                &factory_project,
                &factory_profile_name,
            ));
            let canvas = Some(Lazy::new(
                factory_project.clone(),
                Arc::clone(&canvas_host) as Arc<dyn artist_canvas::bridge::CanvasHost>,
            ));
            let mut tools = artist_agent::tool_set::mcp_surface(McpSurface {
                workspace,
                profile: factory_profile.clone(),
                recorder: Some(factory_recorder.clone()),
                ask: Some(factory_ask.clone()),
                attachments: Some(session_attachments),
                computer: factory_computer.clone(),
                canvas,
                memory: memory_writer,
                delegation: factory_delegation.clone(),
                identity: identity.clone(),
                pages: factory_pages.clone(),
            });
            if factory_profile.permits("workspace") {
                tools.push(crate::admin::workspace_tool(
                    crate::admin::WorkspaceStore::new(&factory_config_root, &factory_project),
                ));
            }
            let permit_operation = factory_profile.permits("operation");
            let permit_page = factory_profile.permits("page");
            let identity = identity.unwrap_or_else(|| McpIdentity {
                actor: "mcp-http-discovery".into(),
                profile: factory_profile_name.clone(),
                project: factory_project.display().to_string(),
                name: "anonymous".into(),
            });
            let server = McpServer::with_identity_profile_and_visibility(
                tools,
                Some(&state_dir),
                identity,
                factory_profile_instructions.clone(),
                permit_operation,
                permit_page,
            )?;
            canvas_host.attach(server.canvas_dispatcher());
            Ok(server)
        });

        Ok(Self {
            factory,
            actor: actor.to_owned(),
            project: project_text.clone(),
            profile: profile_name,
            profile_instructions,
            identities: artist_registry::Registry::for_project(&project).http_identities(),
            _writer: writer,
        })
    }

    /// Build the server for the durable stdio actor.
    pub fn server(&self) -> anyhow::Result<McpServer> {
        let claimed = artist_registry::names().claim(&artist_registry::Registration {
            session: self.actor.clone(),
            actor: self.actor.clone(),
            project: Some(self.project.clone()),
            profile: Some(self.profile.clone()),
            parent: None,
        })?;
        (self.factory)(Some(McpIdentity {
            actor: self.actor.clone(),
            profile: self.profile.clone(),
            project: self.project.clone(),
            name: claimed.name,
        }))
    }

    /// Serve one stdio connection against its durable actor and exit.
    pub async fn serve_stdio(self) -> anyhow::Result<()> {
        self.server()?.serve_stdio().await
    }

    /// Serve Streamable HTTP on a loopback port. Transport sessions are anonymous;
    /// durable Artist identity is established only by the explicit `identity` tool.
    pub async fn serve_http(self, addr: std::net::SocketAddr) -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("bind {addr}"))?;
        let http = HttpMcpServer::new(
            Arc::clone(&self.factory),
            self.identities.clone(),
            self.profile.clone(),
            self.project.clone(),
            self.profile_instructions.clone(),
        )?;
        let session_manager = Arc::new(LocalSessionManager::default());
        let legacy_server = http.clone();
        let legacy = StreamableHttpService::new(
            move || Ok::<_, std::io::Error>(legacy_server.clone()),
            Arc::clone(&session_manager),
            StreamableHttpServerConfig::default(),
        );
        let modern_server = http.clone();
        let modern = StreamableHttpService::new(
            move || Ok::<_, std::io::Error>(modern_server.clone()),
            session_manager,
            StreamableHttpServerConfig::default()
                .with_stateful_mode(false)
                .with_json_response(true),
        );
        let mut discovery_blocks = vec![
            crate::server::INSTRUCTIONS.to_owned(),
            crate::server::HTTP_INSTRUCTIONS.to_owned(),
        ];
        if !self.profile_instructions.trim().is_empty() {
            discovery_blocks.push(self.profile_instructions.clone());
        }
        let discover = crate::discover::DiscoverReply::new(
            "Artist".to_owned(),
            format!(
                "Artist MCP harness for {} using profile {}",
                self.project, self.profile
            ),
            discovery_blocks.join("\n\n"),
        );
        let service = crate::discover::McpGatewayService::new(legacy, modern, discover);
        let router = axum::Router::new()
            .route(
                "/.well-known/oauth-protected-resource/mcp",
                axum::routing::get(no_oauth_metadata),
            )
            .route(
                "/.well-known/oauth-protected-resource",
                axum::routing::get(no_oauth_metadata),
            )
            .nest_service("/mcp", service);
        tracing::info!(addr = %listener.local_addr()?, "serving artist over MCP http");
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(|error| anyhow!(error))
    }
}

async fn no_oauth_metadata() -> (axum::http::StatusCode, &'static str) {
    (
        axum::http::StatusCode::NOT_FOUND,
        "OAuth protected resource metadata is not configured for this MCP server.",
    )
}
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(error) => {
                tracing::warn!("failed to install SIGTERM handler: {error}");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    tracing::warn!("failed to receive interrupt signal: {error}");
                }
            }
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::warn!("failed to receive shutdown signal: {error}");
    }
}

/// The config root: `$ARTIST_CONFIG_DIR` or the platform config dir's
/// `artist/`, the same place the CLI keeps providers and settings.
fn config_root() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("ARTIST_CONFIG_DIR") {
        return Ok(PathBuf::from(path));
    }
    dirs::config_dir()
        .map(|dir| dir.join("artist"))
        .context("no config directory")
}

/// Settings the daemon reads from `settings.toml` (global + project). A
/// reimplementation of the CLI's [`settings::load_effective`] for the handful
/// of fields MCP needs — the CLI's settings module is private to its crate.
#[derive(Debug, Default, Deserialize)]
struct FileSettings {
    #[serde(default)]
    computer: ComputerSettings,
    #[serde(default)]
    memory: MemorySettings,
}

#[derive(Debug, Default, Deserialize)]
struct ComputerSettings {
    #[serde(default)]
    screen: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct MemorySettings {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default, alias = "modelDir")]
    model_dir: Option<String>,
    #[serde(default)]
    dim: Option<usize>,
}

#[derive(Debug, Clone)]
struct ResolvedMemory {
    enabled: bool,
    model_dir: Option<String>,
    dim: usize,
}

impl FileSettings {
    /// Load and merge the global and project settings files, project layer
    /// winning — the same precedence the CLI applies. A missing or malformed
    /// file is ignored: settings are optional configuration, not a gate.
    fn load(config_root: &Path, project: &Path) -> Self {
        let global = load_file(&config_root.join("settings.toml"));
        let project_layer = load_file(&project.join(".artist").join("settings.toml"));
        Self {
            computer: ComputerSettings {
                screen: project_layer.computer.screen.or(global.computer.screen),
            },
            memory: MemorySettings {
                enabled: project_layer.memory.enabled.or(global.memory.enabled),
                model_dir: project_layer.memory.model_dir.or(global.memory.model_dir),
                dim: project_layer.memory.dim.or(global.memory.dim),
            },
        }
    }

    fn memory(&self) -> ResolvedMemory {
        ResolvedMemory {
            enabled: self.memory.enabled.unwrap_or(false),
            model_dir: self.memory.model_dir.clone(),
            dim: self.memory.dim.unwrap_or(768),
        }
    }
}

fn load_file(path: &Path) -> FileSettings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

/// The virtual screen size: a `WIDTHxHEIGHT` string or a preset name, defaulting
/// to the same `desktop` size the CLI uses. Malformed values fall back silently.
fn screen(settings: &ComputerSettings) -> (i32, i32) {
    let Some(value) = &settings.screen else {
        return artist_computer::host::DEFAULT_SCREEN;
    };
    let value = value.trim();
    if let Some((_, size)) = SCREEN_PRESETS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(value))
    {
        return *size;
    }
    let Some((width, height)) = value.split_once(['x', 'X']) else {
        return artist_computer::host::DEFAULT_SCREEN;
    };
    match (width.trim().parse(), height.trim().parse()) {
        (Ok(width), Ok(height)) => (width, height),
        _ => artist_computer::host::DEFAULT_SCREEN,
    }
}

const SCREEN_PRESETS: &[(&str, (i32, i32))] = &[
    ("desktop", (1920, 1080)),
    ("laptop", (1440, 900)),
    ("tablet", (820, 1180)),
    ("mobile", (390, 844)),
];

/// Open the memory subsystem, or `None` when it is off or unusable.
///
/// Every failure degrades to "no memory" rather than aborting the daemon: a
/// missing embedding model or a locked store should cost recall, not the whole
/// MCP server.
async fn open_memory(
    config_root: &Path,
    settings: &ResolvedMemory,
) -> Option<artist_agent::memory::MemoryHandle> {
    if !settings.enabled {
        return None;
    }
    let memory = match artist_memory::Memory::open(config_root).await {
        Ok(memory) => memory,
        Err(error) => {
            if is_locked(&error) {
                tracing::warn!(
                    store = %artist_memory::facts_path(config_root).display(),
                    "another artist holds the memory store, so this daemon runs WITHOUT memory"
                );
            } else {
                tracing::warn!("memory disabled, could not open the store: {error:#}");
            }
            return None;
        }
    };
    let model_dir = match settings.model_dir.as_ref() {
        Some(dir) => {
            let path = Path::new(dir);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                config_root.join(path)
            }
        }
        None => config_root.join("models").join("code-embed"),
    };
    let embedder = match artist_memory::Embedder::load(&model_dir, settings.dim).await {
        Ok(embedder) => Some(embedder),
        Err(error) => {
            tracing::warn!(
                model = %model_dir.display(),
                "memory recall will be lexical only, no embedding model: {error:#}"
            );
            None
        }
    };
    Some(artist_agent::memory::MemoryHandle::new(memory, embedder))
}

/// Does the open failure mean someone else holds the store? Matched on the
/// message because the lock failure surfaces from RocksDB as a C++ status
/// string through two layers of wrapping, with no typed error to match on.
fn is_locked(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}").to_ascii_lowercase();
    text.contains("lock") || text.contains("no locks available") || text.contains("resource busy")
}

/// Load the configured provider accounts, reimplementing the CLI's
/// `providers.toml` decode (including the legacy credentials migration).
fn load_providers(config_root: &Path) -> (ProviderSet, Option<SavedProvider>) {
    let path = config_root.join("providers.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return (ProviderSet::default(), None),
    };
    let mut document = match toml::from_str::<toml::Value>(&text) {
        Ok(document) => document,
        Err(error) => {
            tracing::warn!("providers.toml unreadable: {error}");
            return (ProviderSet::default(), None);
        }
    };
    migrate_credentials(&mut document);
    #[derive(Deserialize)]
    struct ProviderFile {
        #[serde(default)]
        default_provider: Option<ProviderId>,
        #[serde(default)]
        providers: Vec<SavedProvider>,
    }
    let file: ProviderFile = match document.try_into() {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!("providers.toml could not be decoded: {error}");
            return (ProviderSet::default(), None);
        }
    };
    let selected = file
        .default_provider
        .as_ref()
        .and_then(|id| {
            file.providers
                .iter()
                .position(|provider| provider.id.as_str() == id.as_str())
        })
        .or_else(|| (!file.providers.is_empty()).then_some(0))
        .and_then(|index| file.providers.get(index).cloned());
    (ProviderSet::new(file.providers), selected)
}

/// Version 4 credentials are explicitly tagged; older files put the kind in
/// the key name or omitted it entirely. Same transformation the CLI applies.
fn migrate_credentials(document: &mut toml::Value) {
    let Some(table) = document.as_table_mut() else {
        return;
    };
    let previous_version = table
        .get("version")
        .and_then(toml::Value::as_integer)
        .unwrap_or(1);
    if previous_version >= 4 {
        return;
    }
    table.insert("version".into(), toml::Value::Integer(4));
    let Some(providers) = table
        .get_mut("providers")
        .and_then(toml::Value::as_array_mut)
    else {
        return;
    };
    for provider in providers {
        let Some(provider) = provider.as_table_mut() else {
            continue;
        };
        let credentials = provider
            .remove("credentials")
            .or_else(|| provider.remove("auth"));
        let Some(mut credentials) = credentials else {
            continue;
        };
        let is_legacy_chatgpt = credentials
            .get("type")
            .and_then(toml::Value::as_str)
            .is_none();
        if is_legacy_chatgpt {
            if let Some(auth) = credentials.as_table_mut() {
                auth.insert("type".into(), toml::Value::String("chatgpt".into()));
            }
        }
        provider.insert("credentials".into(), credentials);
        provider.entry("provider").or_insert_with(|| {
            toml::Value::String(
                if is_legacy_chatgpt {
                    "chatgpt"
                } else {
                    "openai"
                }
                .into(),
            )
        });
    }
}

/// Build the delegation environment for subagents, or `None` when no provider
/// account is configured. The parent provider is the session's default account
/// (or the first one); a delegated profile naming another provider routes
/// against the full set.
fn build_delegation(
    config_root: &Path,
    conversation_id: &str,
    recorder: &artist_session::Recorder,
    attachments: artist_session::AttachmentStore,
    computer: Option<artist_computer::SurfaceRegistry>,
    memory: Option<artist_agent::memory::MemoryHandle>,
    profiles: &Profiles,
) -> Option<McpDelegation> {
    let (providers, parent) = load_providers(config_root);
    let parent = parent?;
    let (events, _display) = tokio::sync::mpsc::unbounded_channel();
    let handles = SessionHandles {
        recorder: recorder.clone(),
        conversation_id: conversation_id.to_owned(),
        providers,
        attachments: Some(attachments),
        computer,
        durable_memory: memory,
        ..SessionHandles::default()
    };
    Some(McpDelegation {
        provider: parent,
        context: Arc::new(Vec::new()),
        handles,
        events,
        profiles: profiles.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_identity(project: &str, index: usize) -> McpIdentity {
        McpIdentity {
            actor: format!("a-http-test-{index}"),
            profile: "default".into(),
            project: project.into(),
            name: format!("ArtistTest{index}"),
        }
    }

    #[tokio::test]
    async fn anonymous_http_factory_claims_no_identity_and_explicit_identities_are_distinct() {
        let project = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let base = artist_tools::short_id("daemon-test");
        let daemon = McpDaemon::build(
            project.path(),
            state.path(),
            "default",
            &base,
            Allow::default(),
        )
        .await
        .unwrap();

        let anonymous = (daemon.factory)(None).unwrap();
        assert_eq!(anonymous.identity().name, "anonymous");

        let first = fake_identity(&daemon.project, 1);
        let second = fake_identity(&daemon.project, 2);
        let first_server = (daemon.factory)(Some(first.clone())).unwrap();
        let second_server = (daemon.factory)(Some(second.clone())).unwrap();
        assert_eq!(first_server.identity().name, first.name);
        assert_eq!(second_server.identity().name, second.name);
        assert_ne!(
            first_server.identity().actor,
            second_server.identity().actor
        );
    }

    #[cfg(target_os = "linux")]
    fn hashline_descriptor_count(state: &Path) -> usize {
        std::fs::read_dir("/proc/self/fd")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| std::fs::read_link(entry.path()).ok())
            .filter(|target| {
                target.starts_with(state)
                    && target
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("hashlines.sqlite3"))
            })
            .count()
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn http_identities_share_one_project_workspace() {
        let project = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let base = artist_tools::short_id("daemon-workspace-test");
        let daemon = McpDaemon::build(
            project.path(),
            state.path(),
            "default",
            &base,
            Allow::default(),
        )
        .await
        .unwrap();
        let baseline = hashline_descriptor_count(state.path());
        assert!(
            baseline > 0,
            "the shared workspace must own the hashline database"
        );

        let servers: Vec<_> = (0..32)
            .map(|index| (daemon.factory)(Some(fake_identity(&daemon.project, index))).unwrap())
            .collect();

        assert_eq!(
            hashline_descriptor_count(state.path()),
            baseline,
            "per-identity servers must derive actor views from one shared workspace"
        );
        drop(servers);
    }
}
