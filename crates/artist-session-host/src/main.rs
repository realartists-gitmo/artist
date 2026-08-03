use artist_session_host::{DaemonOptions, HostRegistry, SessionHostDaemon};
use std::{path::PathBuf, time::Duration};

fn main() -> anyhow::Result<()> {
    let session = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: artist-session-host <session-id>"))?;
    let project = std::env::var_os("ARTIST_HOST_PROJECT")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let executable = std::env::var_os("ARTIST_EXECUTABLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artist"));
    let registry = HostRegistry::platform_default()?;
    SessionHostDaemon::bind(
        &registry,
        DaemonOptions {
            session,
            project,
            executable,
            idle_timeout: Duration::from_secs(15 * 60),
        },
    )?
    .run()
}
