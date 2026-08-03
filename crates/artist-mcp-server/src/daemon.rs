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
use crate::server::McpServer;

/// Which subsystems a daemon may bring up, one field per command-line flag.
#[derive(Clone, Copy, Debug, Default)]
pub struct Allow {
    pub computer: bool,
    pub canvas: bool,
    pub memory: bool,
    pub subagent: bool,
    pub comms: bool,
}

/// A running artist MCP process: one surface, any number of connections.
pub struct McpDaemon {
    server: McpServer,
    /// Held so the recorder's writer task stays alive for the daemon's life.
    _writer: Option<artist_session::WriterTask>,
}

impl McpDaemon {
    /// Build the full surface and the server over it, opening every subsystem
    /// `allow` asks for. Subsystems degrade rather than fail the daemon — a
    /// locked memory store or an in-use session log costs recall and recording,
    /// not the ability to work.
    pub async fn build(
        project: &Path,
        state_dir: &Path,
        profile_name: &str,
        actor: &str,
        allow: Allow,
    ) -> anyhow::Result<Self> {
        let project = std::fs::canonicalize(project).context("canonicalize project root")?;
        let workspace = Workspace::open(&project, state_dir, actor)?;
        let profiles = Profiles::discover(&project);
        let profile = profiles.get(profile_name).map_err(|error| anyhow!(error))?;
        let outbox = artist_session::AskOutbox::open(Some(state_dir))?;
        let config_root = config_root()?;
        let settings = FileSettings::load(&config_root, &project);

        // The session log everything records into. Degrades to a noop recorder
        // when another process holds the same session dir's writer lock, which
        // for stdio mode means the previous connection is still flushing.
        let conversation_id = format!("mcp:{actor}");
        let session_dir = state_dir.join("sessions").join(actor);
        let attachments = artist_session::AttachmentStore::new(session_dir.join("attachments"));
        let (recorder, writer) = match artist_session::EventLogWriter::open(
            &session_dir,
            &conversation_id,
        ) {
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

        // Computer: drives the machine this daemon runs on. Cheap to construct;
        // binding a surface is only the model reaching for the tool.
        let computer = allow
            .computer
            .then(|| artist_computer::SurfaceRegistry::for_project(&project, screen(&settings.computer)));

        // Memory: one undivided store, exclusive lock. `None` (off, or locked
        // by another artist) also gates code_search/code_related.
        let memory_config = settings.memory();
        let memory = match allow.memory {
            true => open_memory(&config_root, &memory_config).await,
            false => None,
        };
        let memory_writer = memory
            .as_ref()
            .map(|handle| handle.writer(recorder.clone(), conversation_id.clone()));

        // Canvas: lazy — the page only comes up when the model reaches for it.
        let canvas_host = allow
            .canvas
            .then(|| Arc::new(McpCanvasHost::new(outbox.clone(), actor, &project, profile_name)));
        let canvas = canvas_host.as_ref().map(|host| {
            Lazy::new(
                project.clone(),
                Arc::clone(host) as Arc<dyn artist_canvas::bridge::CanvasHost>,
            )
        });

        // Delegation: subagent needs a configured account. None of them, no
        // tool — the honest availability answer.
        let delegation = if allow.subagent {
            build_delegation(
                &config_root,
                &conversation_id,
                &recorder,
                attachments.clone(),
                computer.clone(),
                memory.clone(),
                &profiles,
            )
        } else {
            None
        };

        // Comms: claim the durable identity once; the model re-claims on `init`.
        let identity = allow.comms.then(|| {
            let name = artist_registry::names()
                .claim(&artist_registry::Registration {
                    session: actor.to_owned(),
                    actor: actor.to_owned(),
                    project: Some(project.display().to_string()),
                    profile: Some(profile_name.to_owned()),
                    parent: None,
                })
                .map(|name| name.name)
                .unwrap_or_else(|_| actor.to_owned());
            McpIdentity {
                actor: actor.to_owned(),
                profile: profile_name.to_owned(),
                project: project.display().to_string(),
                name,
            }
        });

        let tools = artist_agent::tool_set::mcp_surface(McpSurface {
            workspace,
            profile,
            recorder: Some(recorder),
            outbox: Some(outbox),
            attachments: Some(attachments),
            computer,
            canvas,
            memory: memory_writer,
            delegation,
            identity,
        });
        if tools.is_empty() {
            return Err(anyhow!(
                "profile {profile_name:?} permits no tools in this environment"
            ));
        }
        let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
        tracing::info!(tools = ?names, "built tool surface");

        let server = McpServer::new(tools, Some(state_dir))?;
        if let Some(host) = &canvas_host {
            host.attach(server.clone());
        }
        Ok(Self {
            server,
            _writer: writer,
        })
    }

    /// The server, cloned for whichever connection needs it.
    pub fn server(&self) -> McpServer {
        self.server.clone()
    }

    /// Serve one stdio connection against this surface and exit. The durable
    /// state (outbox, envelope, session log) is on disk, so a respawned
    /// connection picks up where the last one left off.
    pub async fn serve_stdio(self) -> anyhow::Result<()> {
        self.server.serve_stdio().await
    }

    /// Serve Streamable HTTP on a loopback port. Each MCP session gets its own
    /// [`McpServer`] clone over the one shared surface, so a reconnect to the
    /// daemon keeps the recorder, computer registry, canvas, memory, and
    /// identity alive across it.
    pub async fn serve_http(self, addr: std::net::SocketAddr) -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("bind {addr}"))?;
        let service = StreamableHttpService::new(
            || Ok(self.server()),
            Arc::new(LocalSessionManager::default()),
            // Loopback-only hosts by default, which also blocks DNS-rebinding
            // attacks against a daemon running on the user's machine.
            StreamableHttpServerConfig::default(),
        );
        let router = axum::Router::new().nest_service("/mcp", service);
        tracing::info!(addr = %listener.local_addr()?, "serving artist over MCP http");
        axum::serve(listener, router)
            .await
            .map_err(|error| anyhow!(error))
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

#[derive(Debug, Clone, Copy)]
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
                screen: project_layer
                    .computer
                    .screen
                    .or(global.computer.screen),
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
        if is_legacy_chatgpt && let Some(auth) = credentials.as_table_mut() {
            auth.insert("type".into(), toml::Value::String("chatgpt".into()));
        }
        provider.insert("credentials".into(), credentials);
        provider.entry("provider").or_insert_with(|| {
            toml::Value::String(if is_legacy_chatgpt { "chatgpt" } else { "openai" }.into())
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
    let mut handles = SessionHandles::default();
    handles.recorder = recorder.clone();
    handles.conversation_id = conversation_id.to_owned();
    handles.providers = providers;
    handles.attachments = Some(attachments);
    handles.computer = computer;
    handles.durable_memory = memory;
    Some(McpDelegation {
        provider: parent,
        context: Arc::new(Vec::new()),
        handles,
        events,
        profiles: profiles.clone(),
    })
}
