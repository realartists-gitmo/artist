/// Resolves for process-level termination requests. Interactive Ctrl-C is read
/// by Crossterm in raw mode; one-shot Ctrl-C and SIGTERM take this path so the
/// Herdr reporter and other top-level resources still shut down normally.
pub(crate) async fn requested() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let terminate = async {
            match signal(SignalKind::terminate()) {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(_) => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
