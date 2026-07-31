use crate::{
    Activity, HerdrContext, HerdrState, TurnActivity,
    command::{CommandRunner, Report},
};
use std::{sync::Mutex, time::Duration};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

enum Update {
    State(HerdrState),
    Session(String),
}

#[derive(Clone)]
pub struct HerdrHandle {
    updates: mpsc::Sender<Update>,
    session_id: std::sync::Arc<Mutex<Option<String>>>,
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
        let mut last = self
            .session_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.as_deref() == Some(&session_id) {
            return;
        }
        if self
            .updates
            .try_send(Update::Session(session_id.clone()))
            .is_ok()
        {
            *last = Some(session_id);
        }
    }

    pub fn set_blocked(&self, reason: impl Into<String>, blocked: bool) {
        self.activity.set_blocked(reason, blocked)
    }
}

pub struct HerdrIntegration {
    handle: HerdrHandle,
    runner: CommandRunner,
    release: watch::Sender<bool>,
    worker: Option<JoinHandle<bool>>,
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
        let (updates, receiver) = mpsc::channel(64);
        let (release, release_receiver) = watch::channel(false);
        let state_updates = updates.clone();
        let activity = Activity::new(move |state| {
            let _ = state_updates.try_send(Update::State(state));
        });
        let worker_runner = runner.clone();
        let worker =
            tokio::spawn(
                async move { run_worker(worker_runner, receiver, release_receiver).await },
            );
        Self {
            handle: HerdrHandle {
                updates,
                session_id: Default::default(),
                activity,
            },
            runner,
            release,
            worker: Some(worker),
            fallback_release: true,
        }
    }

    pub fn handle(&self) -> HerdrHandle {
        self.handle.clone()
    }

    pub async fn shutdown(mut self) {
        self.release.send_replace(true);
        let released = if let Some(mut worker) = self.worker.take() {
            match tokio::time::timeout(Duration::from_secs(3), &mut worker).await {
                Ok(Ok(released)) => released,
                _ => {
                    worker.abort();
                    false
                }
            }
        } else {
            false
        };
        if !released && !self.runner.run(Report::Release).await {
            self.runner.spawn_release();
        }
        self.fallback_release = false;
    }
}

impl Drop for HerdrIntegration {
    fn drop(&mut self) {
        if self.fallback_release {
            if let Some(worker) = self.worker.take() {
                worker.abort();
            }
            self.runner.spawn_release();
        }
    }
}

async fn run_worker(
    runner: CommandRunner,
    mut updates: mpsc::Receiver<Update>,
    mut release: watch::Receiver<bool>,
) -> bool {
    let mut session_id = None;
    loop {
        if *release.borrow() {
            return release_with_retry(&runner).await;
        }
        let update = tokio::select! {
            biased;
            result = release.changed() => {
                if result.is_err() || *release.borrow() {
                    return release_with_retry(&runner).await;
                }
                continue;
            }
            update = updates.recv() => match update {
                Some(update) => update,
                None => return false,
            },
        };
        let mut failures = 0u32;
        loop {
            let report = match &update {
                Update::State(state) => Report::State {
                    state: *state,
                    session_id: session_id.as_deref(),
                },
                Update::Session(id) => Report::Session(id),
            };
            if runner.run(report).await {
                if let Update::Session(id) = &update {
                    session_id = Some(id.clone());
                }
                break;
            }
            failures += 1;
            let delay = match failures {
                1 => Duration::from_millis(100),
                2 => Duration::from_millis(250),
                _ => Duration::from_secs(5),
            };
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                result = release.changed() => {
                    if result.is_err() || *release.borrow() {
                        return release_with_retry(&runner).await;
                    }
                }
            }
        }
    }
}

async fn release_with_retry(runner: &CommandRunner) -> bool {
    for delay in [0, 100, 250] {
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        if runner.run(Report::Release).await {
            return true;
        }
    }
    false
}
