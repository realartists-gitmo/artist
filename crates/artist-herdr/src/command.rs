use crate::{AGENT, HerdrContext, HerdrState, LIFECYCLE_SOURCE};
use std::{
    ffi::OsString,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{process::Command, time::timeout};

#[derive(Clone)]
pub(crate) struct CommandRunner {
    context: HerdrContext,
    next_sequence: Arc<AtomicU64>,
    timeout: Duration,
}

pub(crate) enum Report<'a> {
    State {
        state: HerdrState,
        session_id: Option<&'a str>,
    },
    Session(&'a str),
    Release,
}

impl CommandRunner {
    pub(crate) fn new(context: HerdrContext) -> Self {
        Self {
            context,
            next_sequence: Arc::new(AtomicU64::new(1)),
            timeout: Duration::from_millis(750),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_timeout(context: HerdrContext, timeout: Duration) -> Self {
        Self {
            context,
            next_sequence: Arc::new(AtomicU64::new(1)),
            timeout,
        }
    }

    pub(crate) async fn run(&self, report: Report<'_>) -> bool {
        let mut command = self.command(report);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let Ok(mut child) = command.spawn() else {
            return false;
        };
        match timeout(self.timeout, child.wait()).await {
            Ok(Ok(status)) => status.success(),
            Ok(Err(_)) => false,
            Err(_) => {
                let _ = child.start_kill();
                let _ = timeout(Duration::from_millis(100), child.wait()).await;
                false
            }
        }
    }

    pub(crate) fn spawn_release(&self) {
        let mut command = std::process::Command::new(self.context.binary());
        command
            .args(self.args(Report::Release))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Ok(mut child) = command.spawn() {
            // This path runs only while unwinding or after bounded async release
            // retries fail. Reap without delaying Artist's own shutdown.
            let _ = std::thread::Builder::new()
                .name("artist-herdr-release".into())
                .spawn(move || {
                    let _ = child.wait();
                });
        }
    }

    fn command(&self, report: Report<'_>) -> Command {
        let mut command = Command::new(self.context.binary());
        command.args(self.args(report));
        command
    }

    fn args(&self, report: Report<'_>) -> Vec<OsString> {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let mut args = vec![OsString::from("pane")];
        match report {
            Report::State { state, session_id } => {
                args.extend(self.common("report-agent", sequence));
                args.extend([OsString::from("--state"), OsString::from(state.as_str())]);
                if let Some(session_id) = session_id {
                    args.extend([
                        OsString::from("--agent-session-id"),
                        OsString::from(session_id),
                    ]);
                }
            }
            Report::Session(session_id) => {
                args.extend(self.common("report-agent-session", sequence));
                args.extend([
                    OsString::from("--agent-session-id"),
                    OsString::from(session_id),
                ]);
            }
            Report::Release => args.extend(self.common("release-agent", sequence)),
        }
        args
    }

    fn common(&self, command: &str, sequence: u64) -> Vec<OsString> {
        vec![
            OsString::from(command),
            OsString::from(self.context.pane_id()),
            OsString::from("--source"),
            OsString::from(LIFECYCLE_SOURCE),
            OsString::from("--agent"),
            OsString::from(AGENT),
            OsString::from("--seq"),
            OsString::from(sequence.to_string()),
        ]
    }
}
