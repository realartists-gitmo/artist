//! Single-flight for expensive, mutually-exclusive commands.
//!
//! Several agents share one worktree, so several agents run `cargo test`
//! against one `target/`. Today that goes badly twice over. Cargo takes an
//! exclusive lock on the build directory, so the second build does not run in
//! parallel — it blocks; the choice was never "parallel or serial", it was
//! already serial. And the first build is already invalid: cargo fingerprints
//! and reads source progressively, so a tree that mutates underneath it yields
//! artifacts for a state that never existed on disk, plus fingerprints that do
//! not match them, which degrades the *next* incremental build too.
//!
//! So the status quo pays full serialization and gets an incoherent answer for
//! it. This module replaces queueing with coalescing: one run per key, shared
//! by every waiter, superseded rather than queued when a newer request arrives.
//!
//! Two ideas carry the design, and conflating them is the easy mistake:
//!
//! * **Exclusion domain** — what cannot run concurrently. `cargo build` and
//!   `cargo test` share one `target/` lock, so they serialize.
//! * **Coalescing key** — what may share a *result*. Those two do not: a build
//!   result is not a test result. Two identical `cargo test -p x` share both.
//!
//! Collapse them and you either hand someone a result for a command they did
//! not run, or serialize things that could have shared. The first failure is
//! silent, which is why they are separate types here.
//!
//! Scope note: coalescing is per process. Cargo's own build lock still
//! serializes across processes, so the cross-process case degrades to today's
//! behaviour rather than breaking — and the motivating case, several subagents
//! under one harness, is in-process.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// What a command is, for scheduling purposes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeJob {
    /// What must not run concurrently — usually a directory that gets locked
    /// or mutated.
    pub domain: String,
    /// What may share a result. Strictly finer than the domain.
    pub key: String,
    /// Whether a running instance can be killed and restarted.
    ///
    /// False for anything that mutates a shared tree in place rather than a
    /// cache: a killed `npm install` leaves `node_modules/` half-written in a
    /// way nothing downstream detects, so a matching request queues instead of
    /// superseding.
    pub preemptible: bool,
}

/// How a request wants to be scheduled.
///
/// Only a blocking request may supersede. Backgrounding stays legal and needs
/// no justification — it simply does not carry the right to kill a job other
/// agents are parked on, which is what keeps the no-starvation argument intact:
/// every agent that can preempt is an agent that is currently blocked, so with
/// `N` agents the worst case is all `N` parking and the run completing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Blocking,
    Background,
}

/// The outcome of one run, handed to every waiter on it.
#[derive(Clone, Debug)]
pub struct Shared<T> {
    pub value: T,
    /// How many callers received this exact run's result. One means it was not
    /// actually shared; more is a saving worth reporting to the model, since a
    /// result covering someone else's edits is a result they need to know is
    /// not only theirs.
    pub waiters: usize,
    /// True when this run superseded an earlier one, meaning its tree state is
    /// newer than the request that started that one.
    pub superseded_earlier: bool,
}

struct Run {
    generation: u64,
    cancel: CancellationToken,
    done: Arc<Notify>,
    outcome: Arc<Mutex<Option<Arc<RunResult>>>>,
    waiters: Arc<Mutex<usize>>,
}

struct RunResult {
    value: Result<String, String>,
    superseded_earlier: bool,
}

#[derive(Default)]
struct Cell {
    generation: u64,
    current: Option<Arc<Run>>,
}

/// The process's coalescing state.
///
/// Global rather than threaded through every caller because the property it
/// enforces — one run per key — is a property of the machine's work, not of any
/// one agent's call stack. Two subagents that never share a Rust value still
/// share a `target/`.
#[derive(Default)]
pub struct Coalescer {
    cells: Mutex<HashMap<String, Arc<Mutex<Cell>>>>,
    domains: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

pub fn global() -> &'static Coalescer {
    static COALESCER: OnceLock<Coalescer> = OnceLock::new();
    COALESCER.get_or_init(Coalescer::default)
}

impl Coalescer {
    /// Run `job`, sharing an in-flight run where the rules allow.
    ///
    /// `execute` receives a cancellation token and must stop when it fires;
    /// a run that ignores it delays its own supersession rather than breaking
    /// correctness.
    pub async fn run<F, Fut>(
        &self,
        job: &TreeJob,
        mode: Mode,
        execute: F,
    ) -> Result<Shared<String>, String>
    where
        F: FnOnce(CancellationToken) -> Fut + Send,
        Fut: std::future::Future<Output = Result<String, String>> + Send,
    {
        let cell = self.cell(&job.key);

        enum Plan {
            Join(Arc<Run>),
            Start { generation: u64, superseded: bool },
        }

        let plan = {
            let mut guard = cell.lock().expect("coalescer cell poisoned");
            match guard.current.clone() {
                // A background request never cancels: it takes the run as it
                // stands, which may predate its own edits. That is the cost of
                // not blocking, and it is paid by the requester rather than by
                // the agents already parked on the run.
                Some(run) if mode == Mode::Background || !job.preemptible => {
                    *run.waiters.lock().expect("waiter count poisoned") += 1;
                    Plan::Join(run)
                }
                Some(run) => {
                    // Supersede. Existing waiters are carried onto the new
                    // generation by the follow loop below: they observe their
                    // run cancelled and re-read the cell rather than returning
                    // a result that was thrown away.
                    run.cancel.cancel();
                    guard.generation += 1;
                    Plan::Start {
                        generation: guard.generation,
                        superseded: true,
                    }
                }
                None => {
                    guard.generation += 1;
                    Plan::Start {
                        generation: guard.generation,
                        superseded: false,
                    }
                }
            }
        };

        match plan {
            Plan::Join(run) => self.follow(&cell, run).await,
            Plan::Start {
                generation,
                superseded,
            } => {
                let run = Arc::new(Run {
                    generation,
                    cancel: CancellationToken::new(),
                    done: Arc::new(Notify::new()),
                    outcome: Arc::new(Mutex::new(None)),
                    waiters: Arc::new(Mutex::new(1)),
                });
                cell.lock().expect("coalescer cell poisoned").current = Some(run.clone());

                // The domain lock is taken *after* the cell lock is released,
                // so superseding never waits behind the run it just cancelled.
                let domain = self.domain(&job.domain);
                let held = domain.lock().await;

                // Between cancelling and acquiring the domain, someone newer
                // may have superseded us in turn. Their run is the one that
                // should proceed.
                if !self.still_current(&cell, run.generation) {
                    // Released before following, without exception. A run that
                    // keeps the domain while waiting on the run that superseded
                    // it deadlocks the pair: the newer run cannot start until
                    // the older one lets go, and the older one is waiting for
                    // the newer one to finish.
                    drop(held);
                    return self.follow_current(&cell).await;
                }

                let value = execute(run.cancel.clone()).await;
                drop(held);
                let waiters = *run.waiters.lock().expect("waiter count poisoned");
                let result = Arc::new(RunResult {
                    value: value.clone(),
                    superseded_earlier: superseded,
                });
                *run.outcome.lock().expect("outcome poisoned") = Some(result);
                {
                    let mut guard = cell.lock().expect("coalescer cell poisoned");
                    if guard
                        .current
                        .as_ref()
                        .is_some_and(|c| c.generation == run.generation)
                    {
                        guard.current = None;
                    }
                }
                run.done.notify_waiters();

                if run.cancel.is_cancelled() {
                    // We were superseded while running; our result is for a
                    // tree state that has already moved on.
                    return self.follow_current(&cell).await;
                }
                value.map(|value| Shared {
                    value,
                    waiters,
                    superseded_earlier: superseded,
                })
            }
        }
    }

    /// Wait on a run, following supersession rather than returning a result
    /// that was thrown away.
    async fn follow(
        &self,
        cell: &Arc<Mutex<Cell>>,
        run: Arc<Run>,
    ) -> Result<Shared<String>, String> {
        // Registered before the outcome is checked, not after. `notified()`
        // does not subscribe until first polled, so checking first and awaiting
        // second loses a notification that lands in between — and the waiter
        // then sleeps forever on a run that already finished.
        let notified = run.done.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        if run.outcome.lock().expect("outcome poisoned").is_none() {
            notified.await;
        }
        let outcome = run.outcome.lock().expect("outcome poisoned").clone();
        match outcome {
            Some(result) if !run.cancel.is_cancelled() => {
                let waiters = *run.waiters.lock().expect("waiter count poisoned");
                result.value.clone().map(|value| Shared {
                    value,
                    waiters,
                    superseded_earlier: result.superseded_earlier,
                })
            }
            // Cancelled, or woken without a result: the cell has moved on, so
            // attach to whatever is there now. The recursion through
            // `follow_current` is what carries a superseded waiter forward —
            // iterating here as well would be a second way to express it.
            _ => Box::pin(self.follow_current(cell)).await,
        }
    }

    /// Attach to whatever run the cell holds now, starting one is not our job
    /// here — a superseding caller has already put theirs in place.
    async fn follow_current(&self, cell: &Arc<Mutex<Cell>>) -> Result<Shared<String>, String> {
        let current = {
            let guard = cell.lock().expect("coalescer cell poisoned");
            guard.current.clone()
        };
        match current {
            Some(run) => {
                *run.waiters.lock().expect("waiter count poisoned") += 1;
                Box::pin(self.follow(cell, run)).await
            }
            // The superseding run finished before we looked. Nothing to attach
            // to and nothing to report — the caller retries, which is one extra
            // command in a case that needs two racing supersessions to reach.
            None => Err("the tree moved on before this run finished".into()),
        }
    }

    fn still_current(&self, cell: &Arc<Mutex<Cell>>, generation: u64) -> bool {
        cell.lock()
            .expect("coalescer cell poisoned")
            .current
            .as_ref()
            .is_some_and(|run| run.generation == generation)
    }

    fn cell(&self, key: &str) -> Arc<Mutex<Cell>> {
        self.cells
            .lock()
            .expect("coalescer poisoned")
            .entry(key.to_owned())
            .or_default()
            .clone()
    }

    fn domain(&self, domain: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.domains
            .lock()
            .expect("coalescer poisoned")
            .entry(domain.to_owned())
            .or_default()
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn cargo_test() -> TreeJob {
        TreeJob {
            domain: "cargo:/p/target".into(),
            key: "cargo:test".into(),
            preemptible: true,
        }
    }

    /// The saving: two callers asking for the same thing run it once.
    #[tokio::test]
    async fn a_background_request_joins_the_run_in_flight() {
        let coalescer = Arc::new(Coalescer::default());
        let runs = Arc::new(AtomicUsize::new(0));

        let gate = Arc::new(Notify::new());
        let first = {
            let coalescer = coalescer.clone();
            let runs = runs.clone();
            let gate = gate.clone();
            let job = cargo_test();
            async move {
                coalescer
                    .run(&job, Mode::Blocking, |_| async move {
                        runs.fetch_add(1, Ordering::SeqCst);
                        gate.notified().await;
                        Ok("built".into())
                    })
                    .await
            }
        };
        let second = {
            let coalescer = coalescer.clone();
            let job = cargo_test();
            async move {
                // Let the first run reach its gate before joining.
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                let result = coalescer.run(&job, Mode::Background, |_| async move {
                    panic!("a joining request must not start its own run")
                });
                gate.notify_waiters();
                result.await
            }
        };

        let (a, b) = tokio::join!(first, second);
        assert_eq!(a.unwrap().value, "built");
        assert_eq!(b.unwrap().value, "built");
        assert_eq!(runs.load(Ordering::SeqCst), 1, "the work ran once");
    }

    /// A blocking request supersedes, because its tree state is newer than the
    /// run already in flight.
    #[tokio::test]
    async fn a_blocking_request_supersedes_and_carries_the_waiter_over() {
        let coalescer = Arc::new(Coalescer::default());
        let started = Arc::new(Notify::new());

        let first = {
            let coalescer = coalescer.clone();
            let started = started.clone();
            tokio::spawn(async move {
                coalescer
                    .run(&cargo_test(), Mode::Blocking, |cancel| async move {
                        started.notify_waiters();
                        cancel.cancelled().await;
                        Err("cancelled".into())
                    })
                    .await
            })
        };
        started.notified().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let second = coalescer
            .run(&cargo_test(), Mode::Blocking, |_| async {
                Ok("fresh".into())
            })
            .await
            .unwrap();

        assert_eq!(second.value, "fresh");
        assert!(second.superseded_earlier);
        assert_eq!(
            first.await.unwrap().unwrap().value,
            "fresh",
            "the superseded caller is carried onto the newer run, not failed"
        );
    }

    /// Backgrounding stays legal but must not kill a run others are parked on
    /// — that is what keeps the bound on starvation.
    #[tokio::test]
    async fn a_background_request_never_supersedes() {
        let coalescer = Arc::new(Coalescer::default());
        let started = Arc::new(Notify::new());
        let cancelled = Arc::new(AtomicUsize::new(0));

        let first = {
            let coalescer = coalescer.clone();
            let started = started.clone();
            let cancelled = cancelled.clone();
            tokio::spawn(async move {
                coalescer
                    .run(&cargo_test(), Mode::Blocking, |cancel| async move {
                        started.notify_waiters();
                        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                        if cancel.is_cancelled() {
                            cancelled.fetch_add(1, Ordering::SeqCst);
                        }
                        Ok("original".into())
                    })
                    .await
            })
        };
        started.notified().await;

        let joined = coalescer
            .run(&cargo_test(), Mode::Background, |_| async {
                panic!("must join, not start")
            })
            .await
            .unwrap();

        assert_eq!(joined.value, "original");
        assert_eq!(cancelled.load(Ordering::SeqCst), 0);
        assert_eq!(first.await.unwrap().unwrap().value, "original");
        assert_eq!(joined.waiters, 2, "both callers shared one run");
    }

    /// A non-preemptible job queues rather than being killed mid-write — a
    /// half-written `node_modules/` is undetectable downstream.
    #[tokio::test]
    async fn a_non_preemptible_job_is_never_killed() {
        let coalescer = Arc::new(Coalescer::default());
        let job = TreeJob {
            domain: "npm:/p/node_modules".into(),
            key: "npm:install".into(),
            preemptible: false,
        };
        let started = Arc::new(Notify::new());

        let first = {
            let coalescer = coalescer.clone();
            let started = started.clone();
            let job = job.clone();
            tokio::spawn(async move {
                coalescer
                    .run(&job, Mode::Blocking, |cancel| async move {
                        started.notify_waiters();
                        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                        assert!(!cancel.is_cancelled(), "install must not be preempted");
                        Ok("installed".into())
                    })
                    .await
            })
        };
        started.notified().await;

        let second = coalescer
            .run(&job, Mode::Blocking, |_| async {
                panic!("must join the install already running")
            })
            .await
            .unwrap();
        assert_eq!(second.value, "installed");
        assert_eq!(first.await.unwrap().unwrap().value, "installed");
    }

    /// Sharing a domain forces serialization; it must not cause a result to be
    /// shared, because a build result is not a test result.
    #[tokio::test]
    async fn two_keys_in_one_domain_serialize_without_sharing_a_result() {
        let coalescer = Arc::new(Coalescer::default());
        let build = TreeJob {
            key: "cargo:build".into(),
            ..cargo_test()
        };
        let overlapping = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let run = |job: TreeJob, label: &'static str| {
            let coalescer = coalescer.clone();
            let overlapping = overlapping.clone();
            let peak = peak.clone();
            async move {
                coalescer
                    .run(&job, Mode::Blocking, move |_| async move {
                        let now = overlapping.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                        overlapping.fetch_sub(1, Ordering::SeqCst);
                        Ok(label.to_owned())
                    })
                    .await
            }
        };

        let (a, b) = tokio::join!(run(cargo_test(), "tested"), run(build, "built"));
        assert_eq!(a.unwrap().value, "tested");
        assert_eq!(b.unwrap().value, "built", "results must not be shared");
        assert_eq!(peak.load(Ordering::SeqCst), 1, "one domain, one at a time");
    }

    /// Different domains are independent and must actually run concurrently,
    /// or coalescing would have serialized unrelated projects.
    #[tokio::test]
    async fn different_domains_run_concurrently() {
        let coalescer = Arc::new(Coalescer::default());
        let peak = Arc::new(AtomicUsize::new(0));
        let live = Arc::new(AtomicUsize::new(0));

        let run = |domain: &'static str| {
            let coalescer = coalescer.clone();
            let peak = peak.clone();
            let live = live.clone();
            async move {
                let job = TreeJob {
                    domain: domain.into(),
                    key: format!("{domain}:test"),
                    preemptible: true,
                };
                coalescer
                    .run(&job, Mode::Blocking, move |_| async move {
                        let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                        live.fetch_sub(1, Ordering::SeqCst);
                        Ok("done".to_owned())
                    })
                    .await
            }
        };

        let _ = tokio::join!(run("a"), run("b"));
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }

    /// A failure has to reach the caller rather than being swallowed by the
    /// sharing machinery.
    #[tokio::test]
    async fn a_failing_run_reports_to_every_waiter() {
        let coalescer = Arc::new(Coalescer::default());
        let started = Arc::new(Notify::new());

        let first = {
            let coalescer = coalescer.clone();
            let started = started.clone();
            tokio::spawn(async move {
                coalescer
                    .run(&cargo_test(), Mode::Blocking, |_| async move {
                        started.notify_waiters();
                        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                        Err("compile error".to_owned())
                    })
                    .await
            })
        };
        started.notified().await;
        let joined = coalescer
            .run(&cargo_test(), Mode::Background, |_| async {
                panic!("must join")
            })
            .await;

        assert_eq!(joined.unwrap_err(), "compile error");
        assert_eq!(first.await.unwrap().unwrap_err(), "compile error");
    }
}
