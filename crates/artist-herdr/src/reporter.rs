use crate::{
    Activity, HerdrContext, HerdrState, TurnActivity,
    command::{CommandRunner, Report},
};
use std::time::Duration;
use tokio::{sync::watch, task::JoinHandle};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Desired {
    session_id: Option<String>,
    state: Option<HerdrState>,
    release: bool,
}

#[derive(Clone)]
pub struct HerdrHandle {
    desired: watch::Sender<Desired>,
    activity: Activity,
}

impl HerdrHandle {
    pub fn claim_idle(&self) {
        self.activity.claim_idle();
    }

    pub fn start_turn(&self) -> TurnActivity {
        self.activity.start_turn()
    }

    pub fn report_session(&self, session_id: impl Into<String>) {
        let session_id = session_id.into();
        self.desired.send_if_modified(|desired| {
            if desired.session_id.as_deref() == Some(&session_id) {
                return false;
            }
            desired.session_id = Some(session_id);
            true
        });
    }

    pub fn set_blocked(&self, reason: impl Into<String>, blocked: bool) {
        self.activity.set_blocked(reason, blocked);
    }
}

pub struct HerdrIntegration {
    handle: HerdrHandle,
    runner: CommandRunner,
    worker: Option<JoinHandle<()>>,
    fallback_release: bool,
}

impl HerdrIntegration {
    pub fn detect() -> Option<Self> {
        HerdrContext::detect().map(Self::start)
    }

    pub fn start(context: HerdrContext) -> Self {
        Self::start_with_runner(CommandRunner::new(context))
    }

    fn start_with_runner(runner: CommandRunner) -> Self {
        let (desired, receiver) = watch::channel(Desired::default());
        let state_sender = desired.clone();
        let activity = Activity::new(move |state| {
            state_sender.send_if_modified(|desired| {
                if desired.state == Some(state) {
                    return false;
                }
                desired.state = Some(state);
                true
            });
        });
        let worker_runner = runner.clone();
        let worker = tokio::spawn(async move { run_worker(worker_runner, receiver).await });
        Self {
            handle: HerdrHandle { desired, activity },
            runner,
            worker: Some(worker),
            fallback_release: true,
        }
    }

    pub fn handle(&self) -> HerdrHandle {
        self.handle.clone()
    }

    pub async fn shutdown(mut self) {
        self.request_release();
        if let Some(mut worker) = self.worker.take()
            && tokio::time::timeout(Duration::from_secs(2), &mut worker)
                .await
                .is_err()
        {
            worker.abort();
            self.runner.spawn_release();
        }
        self.fallback_release = false;
    }

    fn request_release(&self) {
        self.handle.desired.send_if_modified(|desired| {
            if desired.release {
                return false;
            }
            desired.release = true;
            true
        });
    }
}

impl Drop for HerdrIntegration {
    fn drop(&mut self) {
        if self.fallback_release {
            self.request_release();
            self.runner.spawn_release();
        }
    }
}

async fn run_worker(runner: CommandRunner, mut receiver: watch::Receiver<Desired>) {
    let mut sent_session = None;
    let mut sent_state = None;
    let mut failures = 0u32;
    loop {
        let desired = receiver.borrow_and_update().clone();
        if desired.release {
            let _ = runner.run(Report::Release).await;
            return;
        }

        let mut attempted = false;
        let mut failed = false;
        if desired.session_id != sent_session {
            attempted = true;
            let session_id = desired
                .session_id
                .as_deref()
                .expect("session changed to some");
            if runner.run(Report::Session(session_id)).await {
                sent_session = desired.session_id.clone();
            } else {
                failed = true;
            }
        }
        if desired.state != sent_state {
            attempted = true;
            let state = desired.state.expect("state changed to some");
            if runner
                .run(Report::State {
                    state,
                    session_id: desired.session_id.as_deref(),
                })
                .await
            {
                sent_state = Some(state);
            } else {
                failed = true;
            }
        }

        if failed {
            failures += 1;
            let delay = match failures {
                1 => Duration::from_millis(100),
                2 => Duration::from_millis(250),
                _ => Duration::from_secs(5),
            };
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                result = receiver.changed() => if result.is_err() { return },
            }
        } else if attempted {
            failures = 0;
        } else if receiver.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::Arc,
        time::Instant,
    };
    use tempfile::TempDir;

    struct Fake {
        _dir: TempDir,
        log: PathBuf,
        context: HerdrContext,
    }

    fn fake(body: &str) -> Fake {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let binary = dir.path().join("herdr");
        fs::write(
            &binary,
            format!(
                "#!/bin/sh\n{body}\nprintf '%s\\n' \"$*\" >> '{}'\n",
                log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let context = HerdrContext {
            pane_id: Arc::from("9-9"),
            binary: Arc::new(binary),
            socket_path: None,
        };
        Fake {
            _dir: dir,
            log,
            context,
        }
    }

    fn lines(path: &Path) -> Vec<String> {
        fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    async fn wait_for(path: &Path, predicate: impl Fn(&[String]) -> bool) -> Vec<String> {
        for _ in 0..200 {
            let current = lines(path);
            if predicate(&current) {
                return current;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for fake Herdr calls: {:?}", lines(path));
    }

    #[tokio::test]
    async fn reports_ordered_deduplicated_lifecycle_and_release() {
        let fake = fake("");
        let integration = HerdrIntegration::start(fake.context);
        let handle = integration.handle();
        handle.report_session("session-1");
        handle.claim_idle();
        wait_for(&fake.log, |calls| calls.len() >= 2).await;

        let turn = handle.start_turn();
        wait_for(&fake.log, |calls| {
            calls.iter().any(|line| line.contains("--state working"))
        })
        .await;
        handle.set_blocked("question", true);
        wait_for(&fake.log, |calls| {
            calls.iter().any(|line| line.contains("--state blocked"))
        })
        .await;
        handle.set_blocked("question", false);
        wait_for(&fake.log, |calls| {
            calls
                .iter()
                .filter(|line| line.contains("--state working"))
                .count()
                == 2
        })
        .await;
        turn.finish();
        wait_for(&fake.log, |calls| {
            calls
                .iter()
                .filter(|line| line.contains("--state idle"))
                .count()
                == 2
        })
        .await;
        for _ in 0..100 {
            handle.claim_idle();
        }
        integration.shutdown().await;

        let calls = lines(&fake.log);
        assert!(calls[0].contains("pane report-agent-session 9-9"));
        assert!(calls[0].contains("--source artist:extension --agent artist"));
        assert!(calls[0].contains("--agent-session-id session-1"));
        assert!(calls[1].contains("pane report-agent 9-9"));
        assert!(calls[1].contains("--state idle"));
        assert!(calls.last().unwrap().contains("pane release-agent 9-9"));
        let sequences = calls
            .iter()
            .map(|line| {
                let parts = line.split_whitespace().collect::<Vec<_>>();
                parts[parts.iter().position(|part| *part == "--seq").unwrap() + 1]
                    .parse::<u64>()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[tokio::test]
    async fn retries_after_cli_failure_and_recovers() {
        let fake = fake("if [ -f \"$0.fail\" ]; then exit 1; fi");
        let marker = PathBuf::from(format!("{}.fail", fake.context.binary().display()));
        fs::write(&marker, "fail").unwrap();
        let integration = HerdrIntegration::start(fake.context);
        integration.handle().claim_idle();
        tokio::time::sleep(Duration::from_millis(50)).await;
        fs::remove_file(marker).unwrap();
        wait_for(&fake.log, |calls| {
            calls.iter().any(|line| line.contains("--state idle"))
        })
        .await;
        integration.shutdown().await;
    }

    #[tokio::test]
    async fn command_timeout_never_blocks_shutdown() {
        let fake = fake("sleep 2");
        let runner = CommandRunner::with_timeout(fake.context, Duration::from_millis(25));
        let integration = HerdrIntegration::start_with_runner(runner);
        integration.handle().claim_idle();
        let started = Instant::now();
        integration.shutdown().await;
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
