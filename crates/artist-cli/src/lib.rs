//! Shared command-line process wiring for Artist.
//!
//! Domain behavior remains in `artist-agent`; this crate owns executable
//! defaults and transport setup so binaries and embedding callers use the
//! same daemon lifecycle.

use std::path::PathBuf;
use std::sync::Arc;

use artist_agent::daemon::{Daemon, DaemonRpcHandler};
use artist_agent::rpc::serve_ndjson_with_events;

pub mod runtime;
pub mod tools;

pub use tools::{ToolRunner, validate_root};

pub fn default_state_root() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from(".artist"))
        .join("artist")
}

pub async fn run_daemon(state_root: PathBuf) -> anyhow::Result<()> {
    let daemon = Arc::new(Daemon::open(state_root)?);
    if let Ok(extension_root) = tools::extension_root() {
        let factory = Arc::new(runtime::ComponentRuntimeFactory::from_environment(
            extension_root,
        ));
        daemon.set_runtime_factory(factory);
    }
    let handler = DaemonRpcHandler::new(daemon);
    serve_ndjson_with_events(tokio::io::stdin(), tokio::io::stdout(), &handler).await?;
    Ok(())
}
