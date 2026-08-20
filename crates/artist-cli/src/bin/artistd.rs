//! Minimal cross-platform Artist daemon entrypoint.
//!
//! Stdio is the initial transport because it works uniformly on Linux,
//! macOS, and Windows and lets a parent client own process lifecycle. The
//! protocol and domain handler are shared with future socket transports.

use artist_cli::{default_state_root, run_daemon};
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "artistd", about = "Run the Artist local agent daemon")]
struct Args {
    /// Durable daemon state directory.
    #[arg(long, value_name = "PATH")]
    state_root: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    run_daemon(args.state_root.unwrap_or_else(default_state_root)).await
}
