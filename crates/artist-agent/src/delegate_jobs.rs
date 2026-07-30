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
    jobs: Arc<DashMap<String, Arc<Job>>>,
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

type Registry = Arc<DashMap<String, Arc<Job>>>;
static REGISTRIES: OnceLock<DashMap<PathBuf, Registry>> = OnceLock::new();

impl DelegateJobs {
    pub fn for_project(root: &Path) -> Self {
        let registries = REGISTRIES.get_or_init(DashMap::new);
        let jobs = registries
            .entry(root.to_owned())
            .or_insert_with(|| Arc::new(DashMap::new()))
            .clone();
        Self { jobs }
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
            if let Entry::Vacant(entry) = self.jobs.entry(candidate.clone()) {
                entry.insert(job.clone());
                break candidate;
            }
        };
        let future = build(task_id.clone());
        let running_job = job.clone();
        let handle = tokio::spawn(async move {
            let next = match future.await {
                Ok(output) => JobState::Completed(output),
                Err(error) => JobState::Failed(error),
            };
            let mut current = running_job.state.write().await;
            if matches!(*current, JobState::Running) {
                *current = next;
                running_job.done.notify_waiters();
            }
        });
        *job.abort.lock().unwrap_or_else(|error| error.into_inner()) = Some(handle.abort_handle());
        json!({"taskId":task_id,"role":job.role,"status":"running"}).to_string()
    }

    pub async fn wait(&self, id: &str, wait_ms: Option<u64>) -> Result<String, String> {
        let job = self.job(id)?;
        let notified = job.done.notified();
        if matches!(*job.state.read().await, JobState::Running) {
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(wait_ms.unwrap_or(30_000).min(30_000)),
                notified,
            )
            .await;
        }
        self.read(id).await
    }

    pub async fn cancel(&self, id: &str) -> Result<String, String> {
        let job = self.job(id)?;
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
            *state = JobState::Cancelled;
            job.done.notify_waiters();
        }
        drop(state);
        self.read(id).await
    }

    pub async fn status(&self, id: &str) -> Result<String, String> {
        let job = self.job(id)?;
        let state = job.state.read().await;
        Ok(json!({"taskId":id,"role":job.role,"status":status_name(&state)}).to_string())
    }

    pub async fn read(&self, id: &str) -> Result<String, String> {
        let job = self.job(id)?;
        let state = job.state.read().await;
        Ok(match &*state {
            JobState::Running => json!({"taskId":id,"role":job.role,"status":"running"}),
            JobState::Completed(output) => {
                let output = serde_json::from_str::<Value>(output)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("output")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| output.clone());
                json!({"taskId":id,"role":job.role,"status":"completed","output":output})
            }
            JobState::Failed(error) => {
                json!({"taskId":id,"role":job.role,"status":"failed","error":error})
            }
            JobState::Cancelled => json!({"taskId":id,"role":job.role,"status":"cancelled"}),
        }
        .to_string())
    }

    pub async fn list(&self) -> String {
        let jobs = self
            .jobs
            .iter()
            .map(|item| (item.key().clone(), item.value().clone()))
            .collect::<Vec<_>>();
        let mut output = Vec::with_capacity(jobs.len());
        for (id, job) in jobs {
            let state = job.state.read().await;
            output.push(
                json!({"taskId":id,"role":job.role,"status":status_name(&state),"prompt":shorten(&job.prompt,100)}),
            );
        }
        Value::Array(output).to_string()
    }

    fn job(&self, id: &str) -> Result<Arc<Job>, String> {
        self.jobs
            .get(id)
            .map(|entry| entry.clone())
            .ok_or_else(|| format!("unknown subagent task: {id}"))
    }

    async fn cleanup(&self) {
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

fn status_name(state: &JobState) -> &'static str {
    match state {
        JobState::Running => "running",
        JobState::Completed(_) => "completed",
        JobState::Failed(_) => "failed",
        JobState::Cancelled => "cancelled",
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
}
