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
