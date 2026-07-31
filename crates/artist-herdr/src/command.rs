use crate::{AGENT, HerdrContext, HerdrState, LIFECYCLE_SOURCE};
use std::{
    ffi::OsString,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{process::Command, time::timeout};

#[derive(Clone)]
pub(crate) struct CommandRunner {
    context: HerdrContext,
    next_sequence: Arc<AtomicU64>,
    release_active: Arc<AtomicBool>,
    timeout: Duration,
}

struct ReleaseGuard(Arc<AtomicBool>);

impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
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
            release_active: Arc::new(AtomicBool::new(false)),
            timeout: Duration::from_millis(750),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_timeout(context: HerdrContext, timeout: Duration) -> Self {
        Self {
            context,
            next_sequence: Arc::new(AtomicU64::new(1)),
            release_active: Arc::new(AtomicBool::new(false)),
            timeout,
        }
    }

    pub(crate) async fn run(&self, report: Report<'_>) -> bool {
        let _release = if matches!(&report, Report::Release) {
            Some(self.acquire_release().await)
        } else {
            None
        };
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
        let runner = self.clone();
        // This path runs only while unwinding or after bounded async release
        // retries fail. Spawn the child inside its reaper so thread creation
        // failure cannot leave a child that nobody waits on.
        let _ = std::thread::Builder::new()
            .name("artist-herdr-release".into())
            .spawn(move || {
                let _release = runner.acquire_release_blocking();
                let mut command = std::process::Command::new(runner.context.binary());
                command
                    .args(runner.args(Report::Release))
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                if let Ok(mut child) = command.spawn() {
                    let _ = child.wait();
                }
            });
    }

    async fn acquire_release(&self) -> ReleaseGuard {
        while self
            .release_active
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        ReleaseGuard(self.release_active.clone())
    }

    fn acquire_release_blocking(&self) -> ReleaseGuard {
        while self
            .release_active
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        ReleaseGuard(self.release_active.clone())
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
