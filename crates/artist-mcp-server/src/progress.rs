//! MCP progress reporting backed by Artist's transport-neutral reporter.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use artist_tool_api::{ProgressEvent, ProgressReporter, ToolCallContext};
use rmcp::{
    RoleServer,
    model::{Meta, ProgressNotificationParam},
    service::Peer,
};
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

pub struct ProgressSession {
    reporter: ProgressReporter,
    stop: CancellationToken,
    heartbeat: Option<JoinHandle<()>>,
    relay: JoinHandle<()>,
    phase: String,
}

impl ProgressSession {
    pub fn start(
        meta: &Meta,
        peer: Peer<RoleServer>,
        operation_id: String,
        tool: &str,
        arguments: &Value,
    ) -> (ToolCallContext, Option<Self>) {
        let Some(token) = meta.get_progress_token() else {
            return (
                ToolCallContext {
                    operation_id: Some(operation_id),
                    progress: ProgressReporter::default(),
                },
                None,
            );
        };

        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<ProgressEvent>();
        let sequence = Arc::new(AtomicU64::new(0));
        let reporter = ProgressReporter::new({
            let sequence = Arc::clone(&sequence);
            move |mut event: ProgressEvent| {
                event.progress = sequence.fetch_add(1, Ordering::Relaxed) as f64 + 1.0;
                let _ = sender.send(event);
            }
        });
        let relay = tokio::spawn(async move {
            while let Some(event) = receiver.recv().await {
                let mut notification =
                    ProgressNotificationParam::new(token.clone(), event.progress);
                notification.total = event.total;
                notification.message = event.message.or_else(|| Some(event.phase.clone()));
                let mut event_meta = serde_json::Map::new();
                event_meta.insert("artistPhase".into(), Value::String(event.phase));
                if let Some(unit) = event.unit {
                    event_meta.insert("artistUnit".into(), Value::String(unit));
                }
                notification.meta = Some(Meta(event_meta));
                if let Err(error) = peer.notify_progress(notification).await {
                    tracing::debug!("progress notification ended: {error}");
                    break;
                }
            }
        });

        let phase = phase(tool, arguments);
        reporter.emit(ProgressEvent {
            progress: 0.0,
            total: None,
            phase: phase.clone(),
            message: Some(format!("Started {tool}.")),
            unit: None,
        });

        let stop = CancellationToken::new();
        let heartbeat = is_long_running(tool).then(|| {
            let reporter = reporter.clone();
            let stop = stop.clone();
            let tool = tool.to_owned();
            let phase = phase.clone();
            tokio::spawn(async move {
                let started = std::time::Instant::now();
                let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
                interval.tick().await;
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => break,
                        _ = interval.tick() => {
                            reporter.emit(ProgressEvent {
                                progress: 0.0,
                                total: None,
                                phase: phase.clone(),
                                message: Some(format!(
                                    "{tool} is still running ({} seconds elapsed).",
                                    started.elapsed().as_secs()
                                )),
                                unit: Some("heartbeat".into()),
                            });
                        }
                    }
                }
            })
        });

        (
            ToolCallContext {
                operation_id: Some(operation_id),
                progress: reporter.clone(),
            },
            Some(Self {
                reporter,
                stop,
                heartbeat,
                relay,
                phase,
            }),
        )
    }

    pub async fn finish(self, tool: &str, succeeded: bool) {
        self.stop.cancel();
        if let Some(heartbeat) = self.heartbeat {
            let _ = heartbeat.await;
        }
        self.reporter.emit(ProgressEvent {
            progress: 0.0,
            total: None,
            phase: self.phase,
            message: Some(format!(
                "{tool} {}.",
                if succeeded { "completed" } else { "failed" }
            )),
            unit: Some("completion".into()),
        });
        drop(self.reporter);
        let _ = self.relay.await;
    }
}

fn is_long_running(tool: &str) -> bool {
    matches!(
        tool,
        "bash"
            | "computer"
            | "subagent"
            | "code_search"
            | "code_related"
            | "memory"
            | "code_map"
            | "code_surface"
            | "code_impact"
    )
}

fn phase(tool: &str, arguments: &Value) -> String {
    match tool {
        "bash" => "shell".to_owned(),
        "computer" => "computer".to_owned(),
        "subagent" => "subagent".to_owned(),
        "code_search" | "code_related" | "memory" => "indexing".to_owned(),
        _ => arguments
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_running_surface_is_explicit() {
        assert!(is_long_running("bash"));
        assert!(is_long_running("computer"));
        assert!(is_long_running("subagent"));
        assert!(!is_long_running("read"));
    }
}
