use std::path::PathBuf;

use anyhow::Context;
use artist_mcp_server::{Allow, McpDaemon};
use clap::{Args, Parser, Subcommand};

/// Artist as an MCP server: the harness's own tools over MCP, so ChatGPT (via
/// an OpenAI Secure MCP Tunnel) or any MCP client can drive a project the same
/// way the CLI does.
#[derive(Parser)]
#[command(name = "artist-mcp", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Shared options, identical for both transports.
#[derive(Args, Clone)]
struct Common {
    /// The project the tools operate on. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    project: PathBuf,
    /// Directory for durable state (anchors, envelopes, session logs). Defaults
    /// to `<config>/artist/tools/<hash-of-project>`, the same place the CLI
    /// keeps per-project tool state.
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// The agent profile whose tool policy applies to the surface.
    #[arg(long, default_value = "worker")]
    profile: String,
    /// Actor name this connection runs as — the per-actor anchor state, and
    /// the key reconnects claim their durable identity under.
    #[arg(long, default_value = "mcp")]
    actor: String,
    /// Log level (trace, debug, info, warn, error).
    #[arg(long, default_value = "warn")]
    log: String,
    /// Bring up computer use (drives the machine this process runs on).
    #[arg(long)]
    allow_computer: bool,
    /// Bring up the canvas server, started lazily when the model reaches for it.
    #[arg(long)]
    allow_canvas: bool,
    /// Bring up memory (facts + code retrieval). Off unless configured in
    /// settings and this flag is set.
    #[arg(long)]
    allow_memory: bool,
    /// Bring up subagents (needs a configured provider account).
    #[arg(long)]
    allow_subagent: bool,
    /// Claim a durable identity and expose the tell/query/reply message tools.
    #[arg(long)]
    allow_comms: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the harness tools over MCP stdio.
    ///
    /// This is the transport a tunnel (`tunnel-client` with `MCP_COMMAND` set
    /// to this binary) or a local MCP client spawns. One invocation serves one
    /// connection; the durable state is on disk, so a respawned connection
    /// picks up where the last one left off.
    Serve(Common),
    /// Serve the harness tools over MCP Streamable HTTP on a loopback port.
    ///
    /// The process is long-lived: it owns the recorder, computer registry,
    /// canvas, memory, and identity, so a reconnecting web session keeps them
    /// across the connection rather than rebuilding them each time.
    Daemon(Common),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let (common, http) = match cli.command {
        Command::Serve(common) => (common, None),
        Command::Daemon(common) => (common, Some(daemon_addr()?)),
    };
    tracing_subscriber::fmt()
        .with_env_filter(format!(
            "artist_mcp_server={},artist_agent={}",
            common.log, common.log
        ))
        .with_writer(std::io::stderr)
        .init();
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(run(common, http))
}

/// Where the daemon listens. Loopback only: the client that wants it is on the
/// same machine (a local tunnel, or the user's browser). Nothing else should
/// be able to reach the process that drives this machine.
fn daemon_addr() -> anyhow::Result<std::net::SocketAddr> {
    let host = std::env::var("ARTIST_MCP_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned());
    let port = std::env::var("ARTIST_MCP_PORT").unwrap_or_else(|_| "8317".to_owned());
    parse_daemon_addr(&host, &port)
}

fn parse_daemon_addr(host: &str, port: &str) -> anyhow::Result<std::net::SocketAddr> {
    let host: std::net::IpAddr = host
        .parse()
        .context("ARTIST_MCP_HOST is not an IP address")?;
    anyhow::ensure!(
        host.is_loopback(),
        "ARTIST_MCP_HOST must be a loopback address"
    );
    let port: u16 = port
        .parse()
        .context("ARTIST_MCP_PORT is not a port number")?;
    Ok(std::net::SocketAddr::new(host, port))
}

fn config_root() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("ARTIST_CONFIG_DIR") {
        return Ok(PathBuf::from(path));
    }
    dirs::config_dir()
        .map(|dir| dir.join("artist"))
        .context("no config directory")
}

/// Per-project state, keyed by canonical path — the same convention the CLI
/// uses so the MCP server shares anchor/drift state with local sessions on the
/// same project rather than inventing a second location.
fn project_state_dir(project: &std::path::Path) -> anyhow::Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    project.hash(&mut hasher);
    let config_root = config_root()?;
    Ok(config_root
        .join("tools")
        .join(format!("{:x}", hasher.finish())))
}

async fn run(common: Common, http: Option<std::net::SocketAddr>) -> anyhow::Result<()> {
    let project = std::fs::canonicalize(&common.project).context("canonicalize project root")?;
    let state_dir = match &common.state_dir {
        Some(dir) => dir.clone(),
        None => project_state_dir(&project)?,
    };
    tracing::info!(
        actor = %common.actor,
        profile = %common.profile,
        project = %project.display(),
        state = %state_dir.display(),
        "serving artist over MCP"
    );

    let allow = Allow {
        computer: common.allow_computer,
        canvas: common.allow_canvas,
        memory: common.allow_memory,
        subagent: common.allow_subagent,
        comms: common.allow_comms,
    };
    let daemon =
        McpDaemon::build(&project, &state_dir, &common.profile, &common.actor, allow).await?;
    match http {
        Some(addr) => daemon.serve_http(addr).await,
        None => daemon.serve_stdio().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_accepts_ipv4_and_ipv6_loopback() {
        assert_eq!(
            parse_daemon_addr("127.0.0.1", "8317").unwrap(),
            "127.0.0.1:8317".parse().unwrap()
        );
        assert_eq!(
            parse_daemon_addr("::1", "8317").unwrap(),
            "[::1]:8317".parse().unwrap()
        );
    }

    #[test]
    fn daemon_rejects_non_loopback_hosts() {
        let error = parse_daemon_addr("0.0.0.0", "8317").unwrap_err();
        assert!(error.to_string().contains("loopback"));
    }
}
