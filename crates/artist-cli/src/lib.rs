//! Shared command-line process wiring for Artist.
//!
//! Domain behavior remains in `artist-agent`; this crate owns executable
//! defaults and transport setup so binaries and embedding callers use the
//! same daemon lifecycle.

use std::path::PathBuf;
use std::sync::Arc;

use artist_agent::daemon::{Daemon, DaemonRpcHandler};
use artist_agent::rpc::serve_ndjson;

pub mod tools;

pub use tools::{ToolRunner, validate_root};

pub fn default_state_root() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from(".artist"))
        .join("artist")
}

pub async fn run_daemon(state_root: PathBuf) -> anyhow::Result<()> {
    let daemon = Arc::new(Daemon::open(state_root)?);
    let handler = DaemonRpcHandler::new(daemon);
    serve_ndjson(tokio::io::stdin(), tokio::io::stdout(), &handler).await?;
    Ok(())
}
