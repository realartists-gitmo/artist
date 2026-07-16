use dashmap::DashMap;
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

    pub async fn start<F>(&self, prompt: String, future: F) -> String
    where
        F: Future<Output = Result<String, String>> + Send + 'static,
    {
        self.cleanup().await;
        let task_id = format!("delegate-{}", uuid::Uuid::new_v4().simple());
        let job = Arc::new(Job {
            prompt,
            state: RwLock::new(JobState::Running),
            done: Notify::new(),
            abort: Mutex::new(None),
        });
        self.jobs.insert(task_id.clone(), job.clone());
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
        json!({"taskId":task_id,"status":"running"}).to_string()
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
        Ok(json!({"taskId":id,"status":status_name(&state)}).to_string())
    }

    pub async fn read(&self, id: &str) -> Result<String, String> {
        let job = self.job(id)?;
        let state = job.state.read().await;
        Ok(match &*state {
            JobState::Running => json!({"taskId":id,"status":"running"}),
            JobState::Completed(output) => {
                json!({"taskId":id,"status":"completed","output":output})
            }
            JobState::Failed(error) => json!({"taskId":id,"status":"failed","error":error}),
            JobState::Cancelled => json!({"taskId":id,"status":"cancelled"}),
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
                json!({"taskId":id,"status":status_name(&state),"prompt":shorten(&job.prompt,100)}),
            );
        }
        Value::Array(output).to_string()
    }

    fn job(&self, id: &str) -> Result<Arc<Job>, String> {
        self.jobs
            .get(id)
            .map(|entry| entry.clone())
            .ok_or_else(|| format!("unknown delegate task: {id}"))
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
            .start("work".into(), async { Ok("finished".into()) })
            .await;
        let id = serde_json::from_str::<Value>(&started).unwrap()["taskId"]
            .as_str()
            .unwrap()
            .to_owned();
        let result = jobs.wait(&id, Some(1_000)).await.unwrap();
        assert!(result.contains("completed"));
        assert!(result.contains("finished"));

        let started = jobs
            .start("never".into(), async {
                std::future::pending::<Result<String, String>>().await
            })
            .await;
        let id = serde_json::from_str::<Value>(&started).unwrap()["taskId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(jobs.cancel(&id).await.unwrap().contains("cancelled"));
    }
}
