//! Background subagent jobs.
//!
//! Two layers, because a job has two halves that are scoped differently. Its
//! *execution* is a task in this process's runtime — abortable, and reachable
//! only from here. Its *record* belongs to the project: `list` should show what
//! is running against this worktree no matter which terminal started it, and a
//! wait should not fail because the job belongs to a sibling process.
//!
//! So the live handle stays in memory and the record is mirrored to
//! [`artist_registry::Jobs`]. Reads prefer the local handle when we own the job
//! — it is exact and instant — and fall back to the shared table otherwise.
//! Cancel is the one operation that cannot cross: aborting a task requires the
//! handle, so cancelling a foreign job says so rather than silently doing
//! nothing.

use artist_registry::{Outcome, Registry as SharedRegistry};
use dashmap::{DashMap, mapref::entry::Entry};
use serde_json::{Value, json};
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
use tokio::sync::{Notify, RwLock};

#[derive(Clone)]
pub struct DelegateJobs {
    /// Jobs this process owns, with the handles needed to abort and notify.
    jobs: Arc<DashMap<String, Arc<Job>>>,
    /// Every job on this project, from every process.
    table: artist_registry::Jobs,
}

struct Job {
    prompt: String,
    role: String,
    state: RwLock<JobState>,
    done: Notify,
    abort: Mutex<Option<tokio::task::AbortHandle>>,
}

#[derive(Clone)]
enum JobState {
    Running,
    Completed(String),
    Failed(String),
    Cancelled,
}

type LiveJobs = Arc<DashMap<String, Arc<Job>>>;
static REGISTRIES: OnceLock<DashMap<PathBuf, LiveJobs>> = OnceLock::new();

/// How long a foreign job is re-read while waiting on it.
///
/// Polling rather than a channel: jobs run for minutes, so the cost is
/// irrelevant, and it means there is no inter-process connection to keep alive,
/// reconnect, or leak when either side dies.
const FOREIGN_POLL: std::time::Duration = std::time::Duration::from_millis(250);

impl DelegateJobs {
    pub fn for_project(root: &Path) -> Self {
        let registries = REGISTRIES.get_or_init(DashMap::new);
        let jobs = registries
            .entry(root.to_owned())
            .or_insert_with(|| Arc::new(DashMap::new()))
            .clone();
        Self {
            jobs,
            table: SharedRegistry::for_project(root).jobs(),
        }
    }

    pub async fn start<B, F>(&self, prompt: String, role: String, build: B) -> String
    where
        B: FnOnce(String) -> F + Send,
        F: Future<Output = Result<String, String>> + Send + 'static,
    {
        self.cleanup().await;
        let job = Arc::new(Job {
            prompt,
            role,
            state: RwLock::new(JobState::Running),
            done: Notify::new(),
            abort: Mutex::new(None),
        });
        let task_id = loop {
            let candidate = artist_tools::short_id("a");
            // Uniqueness has to hold across processes now, so a candidate that
            // is free locally but taken on disk is rejected too.
            if self.table.get(&candidate).ok().flatten().is_some() {
                continue;
            }
            if let Entry::Vacant(entry) = self.jobs.entry(candidate.clone()) {
                entry.insert(job.clone());
                break candidate;
            }
        };
        // Best-effort: a project whose state directory is unwritable still runs
        // its jobs, it just loses cross-process visibility of them.
        let _ = self.table.start(&task_id, &job.role, &job.prompt);

        let future = build(task_id.clone());
        let running_job = job.clone();
        let table = self.table.clone();
        let recorded_id = task_id.clone();
        let handle = tokio::spawn(async move {
            let next = match future.await {
                Ok(output) => JobState::Completed(output),
                Err(error) => JobState::Failed(error),
            };
            let mut current = running_job.state.write().await;
            if matches!(*current, JobState::Running) {
                let _ = table.finish(&recorded_id, outcome_of(&next));
                *current = next;
                running_job.done.notify_waiters();
            }
        });
        *job.abort.lock().unwrap_or_else(|error| error.into_inner()) = Some(handle.abort_handle());
        json!({"taskId":task_id,"role":job.role,"status":"running"}).to_string()
    }

    pub async fn wait(&self, id: &str, wait_ms: Option<u64>) -> Result<String, String> {
        let budget = std::time::Duration::from_millis(wait_ms.unwrap_or(30_000).min(30_000));
        let Ok(job) = self.local(id) else {
            return self.wait_foreign(id, budget).await;
        };
        let notified = job.done.notified();
        if matches!(*job.state.read().await, JobState::Running) {
            let _ = tokio::time::timeout(budget, notified).await;
        }
        self.read(id).await
    }

    /// Wait on a job owned by another process by re-reading its record.
    ///
    /// Terminates on an abandoned job as well as a finished one: a record left
    /// running by a process that died is resolved by the registry, so this
    /// cannot hang on work that will never complete.
    async fn wait_foreign(
        &self,
        id: &str,
        budget: std::time::Duration,
    ) -> Result<String, String> {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let job = self
                .table
                .get(id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("unknown subagent task: {id}"))?;
            if !job.is_running() || tokio::time::Instant::now() >= deadline {
                return self.read(id).await;
            }
            tokio::time::sleep(FOREIGN_POLL.min(deadline - tokio::time::Instant::now())).await;
        }
    }

    pub async fn cancel(&self, id: &str) -> Result<String, String> {
        let Ok(job) = self.local(id) else {
            // Deliberately an error rather than a no-op. Aborting a task needs
            // its handle, and reporting "cancelled" for work that is still
            // running would be worse than refusing.
            return Err(match self.table.get(id).ok().flatten() {
                Some(_) => format!(
                    "subagent task {id} belongs to another artist process and cannot be \
                     cancelled from here"
                ),
                None => format!("unknown subagent task: {id}"),
            });
        };
        let mut state = job.state.write().await;
        if matches!(*state, JobState::Running) {
            if let Some(handle) = job
                .abort
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                handle.abort();
            }
            let _ = self.table.finish(id, Outcome::Cancelled);
            *state = JobState::Cancelled;
            job.done.notify_waiters();
        }
        drop(state);
        self.read(id).await
    }

    pub async fn status(&self, id: &str) -> Result<String, String> {
        if let Ok(job) = self.local(id) {
            let state = job.state.read().await;
            return Ok(
                json!({"taskId":id,"role":job.role,"status":status_name(&state)}).to_string(),
            );
        }
        let job = self.foreign(id)?;
        Ok(json!({"taskId":id,"role":job.role,"status":foreign_status(&job)}).to_string())
    }

    pub async fn read(&self, id: &str) -> Result<String, String> {
        if let Ok(job) = self.local(id) {
            let state = job.state.read().await;
            return Ok(match &*state {
                JobState::Running => json!({"taskId":id,"role":job.role,"status":"running"}),
                JobState::Completed(output) => {
                    json!({"taskId":id,"role":job.role,"status":"completed","output":unwrap_output(output)})
                }
                JobState::Failed(error) => {
                    json!({"taskId":id,"role":job.role,"status":"failed","error":error})
                }
                JobState::Cancelled => json!({"taskId":id,"role":job.role,"status":"cancelled"}),
            }
            .to_string());
        }

        let job = self.foreign(id)?;
        Ok(match job.resolved() {
            artist_registry::JobState::Running => {
                json!({"taskId":id,"role":job.role,"status":"running","foreign":true})
            }
            artist_registry::JobState::Done { outcome } => match outcome {
                Outcome::Finished { output } => {
                    json!({"taskId":id,"role":job.role,"status":"completed","output":unwrap_output(&output),"foreign":true})
                }
                Outcome::Failed { error } => {
                    json!({"taskId":id,"role":job.role,"status":"failed","error":error,"foreign":true})
                }
                Outcome::Cancelled => {
                    json!({"taskId":id,"role":job.role,"status":"cancelled","foreign":true})
                }
                // Surfaced as its own status rather than folded into "failed":
                // the job may well have done its work, and only the record of
                // how it ended was lost with the process.
                Outcome::Abandoned => json!({
                    "taskId": id,
                    "role": job.role,
                    "status": "abandoned",
                    "error": "the artist process running this task exited before it finished",
                    "foreign": true,
                }),
            },
        }
        .to_string())
    }

    /// Every job on this project, ours and other processes'.
    ///
    /// Sourced from the shared table rather than the local map so that a fresh
    /// process sees work already in flight — which is the reason the table
    /// exists.
    pub async fn list(&self) -> String {
        let Ok(shared) = self.table.list() else {
            return Value::Array(Vec::new()).to_string();
        };
        let mut output = Vec::with_capacity(shared.len());
        for job in shared {
            // Prefer our own view where we have it: the local state is exact,
            // while the record can trail it by a write.
            let status = match self.local(&job.id) {
                Ok(local) => {
                    let state = local.state.read().await;
                    status_name(&state).to_owned()
                }
                Err(_) => foreign_status(&job).to_owned(),
            };
            output.push(json!({
                "taskId": job.id,
                "role": job.role,
                "status": status,
                "prompt": shorten(&job.summary, 100),
                "foreign": !job.is_ours(),
            }));
        }
        Value::Array(output).to_string()
    }

    fn local(&self, id: &str) -> Result<Arc<Job>, String> {
        self.jobs
            .get(id)
            .map(|entry| entry.clone())
            .ok_or_else(|| format!("unknown subagent task: {id}"))
    }

    fn foreign(&self, id: &str) -> Result<artist_registry::Job, String> {
        self.table
            .get(id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("unknown subagent task: {id}"))
    }

    async fn cleanup(&self) {
        // Records outlive the process, so the shared table needs its own
        // horizon. A day keeps a session's own history readable while stopping
        // a long-lived worktree from accumulating them forever.
        const KEEP: u64 = 60 * 60 * 24;
        let horizon = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|now| now.as_secs().saturating_sub(KEEP))
            .unwrap_or(0);
        let _ = self.table.prune(horizon);

        if self.jobs.len() < 64 {
            return;
        }
        let jobs = self
            .jobs
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<Vec<_>>();
        for (id, job) in jobs {
            if !matches!(*job.state.read().await, JobState::Running) {
                self.jobs.remove(&id);
                if self.jobs.len() < 64 {
                    break;
                }
            }
        }
    }
}

fn outcome_of(state: &JobState) -> Outcome {
    match state {
        JobState::Completed(output) => Outcome::Finished {
            output: unwrap_output(output),
        },
        JobState::Failed(error) => Outcome::Failed {
            error: error.clone(),
        },
        JobState::Cancelled | JobState::Running => Outcome::Cancelled,
    }
}

/// A completed subagent returns a JSON envelope; readers want the text inside
/// it. Applied before the record is written so a foreign reader gets the same
/// value a local one does rather than the envelope.
fn unwrap_output(output: &str) -> String {
    serde_json::from_str::<Value>(output)
        .ok()
        .and_then(|value| {
            value
                .get("output")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| output.to_owned())
}

fn status_name(state: &JobState) -> &'static str {
    match state {
        JobState::Running => "running",
        JobState::Completed(_) => "completed",
        JobState::Failed(_) => "failed",
        JobState::Cancelled => "cancelled",
    }
}

fn foreign_status(job: &artist_registry::Job) -> &'static str {
    match job.resolved() {
        artist_registry::JobState::Running => "running",
        artist_registry::JobState::Done { outcome } => match outcome {
            Outcome::Finished { .. } => "completed",
            Outcome::Failed { .. } => "failed",
            Outcome::Cancelled => "cancelled",
            Outcome::Abandoned => "abandoned",
        },
    }
}

fn shorten(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn background_jobs_complete_and_cancel() {
        let root = tempfile::tempdir().unwrap();
        let jobs = DelegateJobs::for_project(root.path());
        let started = jobs
            .start("work".into(), "worker".into(), |_| async {
                Ok("finished".into())
            })
            .await;
        let started = serde_json::from_str::<Value>(&started).unwrap();
        assert_eq!(started["role"], "worker");
        let id = started["taskId"].as_str().unwrap().to_owned();
        let result =
            serde_json::from_str::<Value>(&jobs.wait(&id, Some(1_000)).await.unwrap()).unwrap();
        assert_eq!(result["taskId"], id);
        assert_eq!(result["role"], "worker");
        assert_eq!(result["status"], "completed");
        assert_eq!(result["output"], "finished");

        let started = jobs
            .start("never".into(), "explorer".into(), |_| async {
                std::future::pending::<Result<String, String>>().await
            })
            .await;
        let id = serde_json::from_str::<Value>(&started).unwrap()["taskId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(jobs.cancel(&id).await.unwrap().contains("cancelled"));
    }

    #[tokio::test]
    async fn start_passes_the_reserved_task_id_to_the_job() {
        let root = tempfile::tempdir().unwrap();
        let jobs = DelegateJobs::for_project(root.path());
        let (sent_id, received_id) = tokio::sync::oneshot::channel();

        let started = jobs
            .start("work".into(), "worker".into(), move |task_id| async move {
                sent_id.send(task_id).unwrap();
                Ok("finished".into())
            })
            .await;
        let returned_id = serde_json::from_str::<Value>(&started).unwrap()["taskId"]
            .as_str()
            .unwrap()
            .to_owned();

        assert_eq!(received_id.await.unwrap(), returned_id);
    }

    /// A second process on the same worktree must see the job and be able to
    /// read its result — the reason the record is on disk at all. Simulated by
    /// building a second `DelegateJobs` whose local map is empty, which is
    /// exactly the state a sibling process is in.
    #[tokio::test]
    async fn another_process_sees_and_reads_the_job() {
        let root = tempfile::tempdir().unwrap();
        let mine = DelegateJobs::for_project(root.path());
        let started = mine
            .start("work".into(), "worker".into(), |_| async {
                Ok("the answer".into())
            })
            .await;
        let id = serde_json::from_str::<Value>(&started).unwrap()["taskId"]
            .as_str()
            .unwrap()
            .to_owned();
        mine.wait(&id, Some(1_000)).await.unwrap();

        let theirs = DelegateJobs {
            jobs: Arc::new(DashMap::new()),
            table: SharedRegistry::for_project(root.path()).jobs(),
        };
        let seen = serde_json::from_str::<Value>(&theirs.read(&id).await.unwrap()).unwrap();
        assert_eq!(seen["status"], "completed");
        assert_eq!(seen["output"], "the answer");
        assert_eq!(seen["foreign"], true);

        let listed = serde_json::from_str::<Value>(&theirs.list().await).unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["taskId"], id.as_str());
    }

    /// Waiting across processes has to terminate on the record rather than on a
    /// notify we never receive.
    #[tokio::test]
    async fn waiting_on_a_foreign_job_resolves_from_the_record() {
        let root = tempfile::tempdir().unwrap();
        let mine = DelegateJobs::for_project(root.path());
        let started = mine
            .start("work".into(), "worker".into(), |_| async {
                Ok("done".into())
            })
            .await;
        let id = serde_json::from_str::<Value>(&started).unwrap()["taskId"]
            .as_str()
            .unwrap()
            .to_owned();

        let theirs = DelegateJobs {
            jobs: Arc::new(DashMap::new()),
            table: SharedRegistry::for_project(root.path()).jobs(),
        };
        let result =
            serde_json::from_str::<Value>(&theirs.wait(&id, Some(2_000)).await.unwrap()).unwrap();
        assert_eq!(result["status"], "completed");
    }

    /// Cancel cannot cross a process boundary, and must say so rather than
    /// report success for work that is still running.
    #[tokio::test]
    async fn cancelling_a_foreign_job_is_refused_not_faked() {
        let root = tempfile::tempdir().unwrap();
        let mine = DelegateJobs::for_project(root.path());
        let started = mine
            .start("never".into(), "worker".into(), |_| async {
                std::future::pending::<Result<String, String>>().await
            })
            .await;
        let id = serde_json::from_str::<Value>(&started).unwrap()["taskId"]
            .as_str()
            .unwrap()
            .to_owned();

        let theirs = DelegateJobs {
            jobs: Arc::new(DashMap::new()),
            table: SharedRegistry::for_project(root.path()).jobs(),
        };
        let error = theirs.cancel(&id).await.unwrap_err();
        assert!(error.contains("another artist process"), "{error}");
        assert!(theirs.cancel("a-nope").await.unwrap_err().contains("unknown"));
    }
}
