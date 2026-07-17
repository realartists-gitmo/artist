use anyhow::{Context, Result, bail};
use rig_core::tool::Tool;
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
    description: String,
    parameters: serde_json::Value,
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
        let names = self.names().await;
        for name in names {
            if self.activation(&name).await == Some(Activation::Startup) {
                let _ = self.start(&name).await;
            }
        }
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
            .map(|tool| tool.name())
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
        let server = self.0.servers.read().await.get(name).unwrap().clone();
        let mut state = server.lock().await;
        let defs = state
            .service
            .as_ref()
            .unwrap()
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
            let server = self.0.servers.read().await[&name].clone();
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
    pub async fn tools(&self) -> Vec<McpProxyTool> {
        let mut out = Vec::new();
        for server_name in self.names().await {
            let server = self.0.servers.read().await[&server_name].clone();
            let state = server.lock().await;
            // Manual servers are deliberately invisible until the user starts them.
            // On-call servers retain their cached schemas while stopped so the model
            // can invoke a proxy and wake the underlying server.
            if state.config.activation != Activation::OnCall && state.service.is_none() {
                continue;
            }
            for tool in &state.tools {
                out.push(McpProxyTool {
                    manager: self.clone(),
                    server: server_name.clone(),
                    tool: tool.clone(),
                });
            }
        }
        out
    }
    async fn persist(&self) -> Result<()> {
        let mut cache = Cache::default();
        for name in self.names().await {
            let server = self.0.servers.read().await[&name].clone();
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
    CachedTool {
        name: tool.name.to_string(),
        description: tool.description.clone().unwrap_or_default().to_string(),
        parameters: tool.schema_as_json_value(),
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
impl Tool for McpProxyTool {
    const NAME: &'static str = "mcp";
    type Error = McpCallError;
    type Args = serde_json::Value;
    type Output = String;
    fn name(&self) -> String {
        format!(
            "mcp__{}__{}",
            sanitize(&self.server),
            sanitize(&self.tool.name)
        )
    }
    fn description(&self) -> String {
        self.tool.description.clone()
    }
    fn parameters(&self) -> serde_json::Value {
        self.tool.parameters.clone()
    }
    async fn call(&self, args: Self::Args) -> std::result::Result<String, McpCallError> {
        self.call_inner(args)
            .await
            .map_err(|error| McpCallError(format!("{error:#}")))
    }
}
impl McpProxyTool {
    async fn call_inner(&self, args: serde_json::Value) -> Result<String> {
        self.manager.start(&self.server).await?;
        let server = self.manager.0.servers.read().await[&self.server].clone();
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
        let mut text = serde_json::to_string(&result)?;
        if text.len() > MAX_OUTPUT {
            let boundary = text
                .char_indices()
                .map(|(index, _)| index)
                .take_while(|index| *index <= MAX_OUTPUT)
                .last()
                .unwrap_or(0);
            text.truncate(boundary);
            text.push_str("\n[output truncated]");
        }
        Ok(text)
    }
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
