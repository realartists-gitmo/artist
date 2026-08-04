use anyhow::{Context, Result, bail};
use artist_tool_api::{
    ArtistDynamicTool, ArtistToolAnnotations, ArtistToolDefinition, ArtistToolOutput, ToolCategory,
    text_output_schema,
};
use rmcp::{RoleClient, ServiceExt, model::Tool as McpDefinition, service::RunningService};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, RwLock};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_OUTPUT: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Activation {
    Startup,
    #[default]
    Manual,
    OnCall,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ServerConfig {
    #[serde(default)]
    pub activation: Activation,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub url: Option<String>,
}
#[derive(Default, Deserialize)]
struct Config {
    #[serde(default)]
    servers: BTreeMap<String, ServerConfig>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedTool {
    name: String,
    #[serde(default)]
    title: Option<String>,
    description: String,
    #[serde(alias = "parameters")]
    input_schema: serde_json::Value,
    #[serde(default)]
    output_schema: Option<serde_json::Value>,
    #[serde(default)]
    annotations: ArtistToolAnnotations,
}
#[derive(Default, Serialize, Deserialize)]
struct Cache {
    servers: BTreeMap<String, Vec<CachedTool>>,
}

type Service = RunningService<RoleClient, ()>;
struct Server {
    config: ServerConfig,
    service: Option<Service>,
    tools: Vec<CachedTool>,
    error: Option<String>,
}
struct Inner {
    root: PathBuf,
    servers: RwLock<BTreeMap<String, Arc<Mutex<Server>>>>,
}
#[derive(Clone)]
pub struct McpManager(Arc<Inner>);

impl McpManager {
    pub async fn load(config_root: &Path) -> Result<Self> {
        let config_path = config_root.join("mcp.toml");
        let config: Config = if config_path.exists() {
            toml::from_str(&std::fs::read_to_string(&config_path).context("read mcp.toml")?)
                .context("parse mcp.toml")?
        } else {
            Config::default()
        };
        let cache: Cache = std::fs::read(config_root.join("mcp-cache.json"))
            .ok()
            .and_then(|v| serde_json::from_slice(&v).ok())
            .unwrap_or_default();
        let servers = config
            .servers
            .into_iter()
            .map(|(name, config)| {
                let tools = cache.servers.get(&name).cloned().unwrap_or_default();
                (
                    name,
                    Arc::new(Mutex::new(Server {
                        config,
                        service: None,
                        tools,
                        error: None,
                    })),
                )
            })
            .collect();
        let manager = Self(Arc::new(Inner {
            root: config_root.to_owned(),
            servers: RwLock::new(servers),
        }));
        manager.start_startup().await;
        Ok(manager)
    }
    async fn start_startup(&self) {
        let mut startup = Vec::new();
        for name in self.names().await {
            if self.activation(&name).await == Some(Activation::Startup) {
                startup.push(name);
            }
        }
        futures::future::join_all(startup.iter().map(|name| self.start(name))).await;
    }
    async fn activation(&self, name: &str) -> Option<Activation> {
        let server = self.0.servers.read().await.get(name)?.clone();
        Some(server.lock().await.config.activation)
    }
    async fn names(&self) -> Vec<String> {
        self.0.servers.read().await.keys().cloned().collect()
    }

    pub async fn server_names(&self) -> Vec<String> {
        self.names().await
    }

    pub async fn tool_names(&self) -> Vec<String> {
        self.tools()
            .await
            .into_iter()
            .map(|tool| tool.name().to_owned())
            .collect()
    }
    pub async fn start(&self, name: &str) -> Result<()> {
        let server = self
            .0
            .servers
            .read()
            .await
            .get(name)
            .cloned()
            .with_context(|| format!("unknown MCP server `{name}`"))?;
        let mut state = server.lock().await;
        if state
            .service
            .as_ref()
            .is_some_and(|service| service.is_transport_closed())
        {
            state.service = None;
        }
        if state.service.is_some() {
            return Ok(());
        }
        let result = tokio::time::timeout(CONNECT_TIMEOUT, async {
            let service = connect(&state.config).await?;
            let definitions = service
                .peer()
                .list_all_tools()
                .await
                .context("discover MCP tools")?;
            Ok::<_, anyhow::Error>((service, definitions))
        })
        .await
        .map_err(|_| anyhow::anyhow!("MCP connection timed out after {CONNECT_TIMEOUT:?}"))
        .and_then(|result| result);
        match result {
            Ok((service, definitions)) => {
                state.tools = definitions.iter().map(cached).collect();
                state.service = Some(service);
                state.error = None;
                drop(state);
                self.persist().await?;
                Ok(())
            }
            Err(error) => {
                state.error = Some(format!("{error:#}"));
                Err(error)
            }
        }
    }
    pub async fn stop(&self, name: &str) -> Result<()> {
        let server = self
            .0
            .servers
            .read()
            .await
            .get(name)
            .cloned()
            .with_context(|| format!("unknown MCP server `{name}`"))?;
        let mut state = server.lock().await;
        if let Some(mut service) = state.service.take() {
            service.close_with_timeout(Duration::from_secs(3)).await?;
        }
        Ok(())
    }
    pub async fn restart(&self, name: &str) -> Result<()> {
        self.stop(name).await?;
        self.start(name).await
    }
    pub async fn refresh(&self, name: &str) -> Result<()> {
        self.start(name).await?;
        let server = self
            .0
            .servers
            .read()
            .await
            .get(name)
            .with_context(|| format!("unknown MCP server: {name}"))?
            .clone();
        let mut state = server.lock().await;
        let defs = state
            .service
            .as_ref()
            .with_context(|| format!("MCP server {name} is not running"))?
            .peer()
            .list_all_tools()
            .await?;
        state.tools = defs.iter().map(cached).collect();
        drop(state);
        self.persist().await
    }
    pub async fn status(&self) -> Vec<String> {
        let mut out = Vec::new();
        for name in self.names().await {
            let Some(server) = self.0.servers.read().await.get(&name).cloned() else {
                continue;
            };
            let state = server.lock().await;
            out.push(format!(
                "{name}: {} ({:?}, {} tools){}",
                if state.service.is_some() {
                    "running"
                } else {
                    "stopped"
                },
                state.config.activation,
                state.tools.len(),
                state
                    .error
                    .as_ref()
                    .map(|e| format!(" - {e}"))
                    .unwrap_or_default()
            ));
        }
        if out.is_empty() {
            out.push("No MCP servers configured in mcp.toml.".into());
        }
        out
    }
    pub async fn tools(&self) -> Vec<ArtistDynamicTool> {
        let mut out = Vec::new();
        for server_name in self.names().await {
            let Some(server) = self.0.servers.read().await.get(&server_name).cloned() else {
                continue;
            };
            let state = server.lock().await;
            // Manual servers are deliberately invisible until the user starts them.
            // On-call servers retain their cached schemas while stopped so the model
            // can invoke a proxy and wake the underlying server.
            if state.config.activation != Activation::OnCall && state.service.is_none() {
                continue;
            }
            for tool in &state.tools {
                out.push(
                    McpProxyTool {
                        manager: self.clone(),
                        server: server_name.clone(),
                        tool: tool.clone(),
                    }
                    .into_dynamic(),
                );
            }
        }
        out
    }
    async fn persist(&self) -> Result<()> {
        let mut cache = Cache::default();
        for name in self.names().await {
            let Some(server) = self.0.servers.read().await.get(&name).cloned() else {
                continue;
            };
            cache
                .servers
                .insert(name, server.lock().await.tools.clone());
        }
        let path = self.0.root.join("mcp-cache.json");
        let temporary = self.0.root.join("mcp-cache.json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&cache)?)?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }
}

async fn connect(config: &ServerConfig) -> Result<Service> {
    if let Some(command) = &config.command {
        let mut process = tokio::process::Command::new(command);
        process
            .args(&config.args)
            .envs(&config.env)
            .stderr(Stdio::null());
        return Ok(
            ().serve(rmcp::transport::TokioChildProcess::new(process)?)
                .await?,
        );
    }
    if let Some(url) = &config.url {
        return Ok(()
            .serve(rmcp::transport::StreamableHttpClientTransport::from_uri(
                url.as_str(),
            ))
            .await?);
    }
    bail!("server requires `command` or `url`")
}
fn cached(tool: &McpDefinition) -> CachedTool {
    let annotations = tool.annotations.as_ref();
    CachedTool {
        name: tool.name.to_string(),
        title: tool
            .title
            .clone()
            .or_else(|| annotations.and_then(|value| value.title.clone())),
        description: tool.description.clone().unwrap_or_default().to_string(),
        input_schema: tool.schema_as_json_value(),
        output_schema: tool
            .output_schema
            .as_ref()
            .map(|schema| serde_json::Value::Object((**schema).clone())),
        annotations: ArtistToolAnnotations {
            read_only: annotations
                .and_then(|value| value.read_only_hint)
                .unwrap_or(false),
            destructive: annotations
                .and_then(|value| value.destructive_hint)
                .unwrap_or(true),
            idempotent: annotations
                .and_then(|value| value.idempotent_hint)
                .unwrap_or(false),
            open_world: annotations
                .and_then(|value| value.open_world_hint)
                .unwrap_or(true),
        },
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct McpCallError(String);

#[derive(Clone)]
pub struct McpProxyTool {
    manager: McpManager,
    server: String,
    tool: CachedTool,
}
impl McpProxyTool {
    fn into_dynamic(self) -> ArtistDynamicTool {
        let name = format!(
            "mcp__{}__{}",
            sanitize(&self.server),
            sanitize(&self.tool.name)
        );
        let definition = ArtistToolDefinition {
            title: self
                .tool
                .title
                .clone()
                .unwrap_or_else(|| artist_tool_api::humanize(&self.tool.name)),
            name,
            description: self.tool.description.clone(),
            input_schema: self.tool.input_schema.clone(),
            output_schema: self.tool.output_schema.clone().unwrap_or_else(|| {
                text_output_schema(&self.tool.name, "Result returned by the remote MCP tool.")
            }),
            category: ToolCategory::External,
            annotations: self.tool.annotations,
        };
        ArtistDynamicTool::new(definition, move |args| {
            let tool = self.clone();
            Box::pin(async move {
                tool.call_inner(args).await.map_err(|error| {
                    rig_core::tool::ToolExecutionError::from_error(McpCallError(format!(
                        "{error:#}"
                    )))
                })
            })
        })
    }

    async fn call_inner(&self, args: serde_json::Value) -> Result<ArtistToolOutput> {
        self.manager.start(&self.server).await?;
        let server = self
            .manager
            .0
            .servers
            .read()
            .await
            .get(&self.server)
            .with_context(|| format!("unknown MCP server: {}", self.server))?
            .clone();
        let state = server.lock().await;
        let peer = state
            .service
            .as_ref()
            .context("MCP server not running")?
            .peer()
            .clone();
        drop(state);
        let arguments = match args {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        };
        let request = rmcp::model::CallToolRequestParams::new(self.tool.name.clone());
        let request = if let Some(args) = arguments {
            request.with_arguments(args)
        } else {
            request
        };
        let result = tokio::time::timeout(CALL_TIMEOUT, peer.call_tool(request))
            .await
            .context("MCP tool timed out")??;
        let is_error = result.is_error.unwrap_or(false);
        let mut text_parts = Vec::new();
        let mut content_items = Vec::new();
        for block in result.content {
            match block {
                rmcp::model::ContentBlock::Text(text) => text_parts.push(text.text),
                rmcp::model::ContentBlock::Image(image) => {
                    content_items.push(mcp_image(&image.data, &image.mime_type));
                }
                other => text_parts.push(serde_json::to_string(&other)?),
            }
        }
        let mut text = text_parts.join("\n");
        if text.is_empty()
            && let Some(structured) = &result.structured_content
        {
            text = structured.to_string();
        }
        if text.len() > MAX_OUTPUT {
            let mut boundary = MAX_OUTPUT.min(text.len());
            while boundary > 0 && !text.is_char_boundary(boundary) {
                boundary -= 1;
            }
            let original_bytes = text.len();
            text.truncate(boundary);
            text.push_str(&format!(
                "\n[truncated remote MCP output: {original_bytes} bytes total]"
            ));
        }
        if is_error {
            bail!(if text.is_empty() {
                "remote MCP tool returned an error".to_owned()
            } else {
                text
            });
        }
        if !text.is_empty() {
            content_items.insert(
                0,
                rig_core::completion::message::ToolResultContent::text(text.clone()),
            );
        }
        let presentation = match rig_core::OneOrMany::many(content_items) {
            Ok(content) => rig_core::tool::ToolOutput::content(content),
            Err(_) => rig_core::tool::ToolOutput::text(text.clone()),
        };
        let structured = match result.structured_content {
            Some(serde_json::Value::Object(map)) => serde_json::Value::Object(map),
            Some(value) => serde_json::json!({"value": value}),
            None => serde_json::json!({"text": text}),
        };
        Ok(ArtistToolOutput {
            presentation,
            structured,
        })
    }
}

/// Convert an MCP image block into rig tool-result content.
///
/// An unrecognized MIME type still passes the payload through with no declared
/// media type rather than dropping it: providers that sniff the bytes handle it,
/// and dropping the only image a tool returned is the worse failure.
fn mcp_image(data: &str, mime_type: &str) -> rig_core::completion::message::ToolResultContent {
    use rig_core::completion::message::{DocumentSourceKind, Image, ImageMediaType};

    let media_type = match mime_type.trim().to_ascii_lowercase().as_str() {
        "image/png" => Some(ImageMediaType::PNG),
        "image/jpeg" | "image/jpg" => Some(ImageMediaType::JPEG),
        "image/gif" => Some(ImageMediaType::GIF),
        "image/webp" => Some(ImageMediaType::WEBP),
        "image/svg+xml" => Some(ImageMediaType::SVG),
        "image/heic" => Some(ImageMediaType::HEIC),
        "image/heif" => Some(ImageMediaType::HEIF),
        _ => None,
    };
    rig_core::completion::message::ToolResultContent::Image(Image {
        data: DocumentSourceKind::Base64(data.to_owned()),
        media_type,
        detail: None,
        additional_params: None,
    })
}

fn sanitize(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "_{byte:02x}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn namespace_is_safe() {
        assert_eq!(sanitize("foo-bar/x"), "foo_2dbar_2fx");
        assert_ne!(sanitize("foo-bar"), sanitize("foo_2dbar"));
    }
    #[test]
    fn parses_modes() {
        let c: Config = toml::from_str("[servers.x]\ncommand='x'\nactivation='on_call'").unwrap();
        assert_eq!(c.servers["x"].activation, Activation::OnCall);
    }

    #[test]
    fn defaults_to_manual_activation() {
        let c: Config = toml::from_str("[servers.x]\ncommand='x'").unwrap();
        assert_eq!(c.servers["x"].activation, Activation::Manual);
    }
}
