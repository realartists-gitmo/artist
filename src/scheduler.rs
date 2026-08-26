use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    domain::{EventData, RunId, RunStatus},
    runtime::{Result, Runtime},
};

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SchedulerReport {
    pub interrupted: Vec<RunId>,
    pub restarted: Vec<RunId>,
    pub joined: Vec<RunId>,
    pub runnable: Vec<RunId>,
}

/// Small durable scheduler. It changes persisted run states; an outer service
/// chooses how many runnable agent loops to execute concurrently.
pub struct Scheduler<'a> {
    runtime: &'a Runtime,
}

impl<'a> Scheduler<'a> {
    pub fn new(runtime: &'a Runtime) -> Self {
        Self { runtime }
    }

    /// Repairs actions that began before a process crash but never completed.
    /// It never replays Python code automatically.
    pub fn recover(&self) -> Result<SchedulerReport> {
        let mut report = SchedulerReport::default();
        for run in self.runtime.store().list_runs()? {
            if run.status == RunStatus::Running
                && let Some(action) = self.unfinished_action(run.id)?
            {
                self.runtime.interrupt(
                    run.id,
                    action,
                    "harness stopped before action completed",
                )?;
                report.interrupted.push(run.id);
            }
        }
        for run in self.runtime.store().list_runs()? {
            if run.status == RunStatus::Interrupted {
                self.runtime.restart(run.id, "harness recovery")?;
                report.restarted.push(run.id);
            }
        }
        let tick = self.tick()?;
        report.joined = tick.joined;
        report.runnable = tick.runnable;
        Ok(report)
    }

    /// Wakes completed joins and reports runs that an agent worker may execute.
    pub fn tick(&self) -> Result<SchedulerReport> {
        let mut report = SchedulerReport::default();
        for run in self.runtime.store().list_runs()? {
            if run.status == RunStatus::WaitingForChildren
                && self.runtime.try_join(run.id)?.is_some()
            {
                report.joined.push(run.id);
            }
        }
        report.runnable = self
            .runtime
            .store()
            .list_runs()?
            .into_iter()
            .filter(|run| run.status == RunStatus::Running)
            .map(|run| run.id)
            .collect();
        Ok(report)
    }

    fn unfinished_action(&self, run_id: RunId) -> Result<Option<String>> {
        let events = self.runtime.store().events(run_id)?;
        let mut model = HashSet::<Uuid>::new();
        let mut python = HashSet::<Uuid>::new();
        for event in events {
            match event.data {
                EventData::ModelRequested { request_id } => {
                    model.insert(request_id);
                }
                EventData::ModelResponse { request_id, .. } => {
                    model.remove(&request_id);
                }
                EventData::PythonRequested { request_id, .. } => {
                    python.insert(request_id);
                }
                EventData::PythonCompleted { request_id, .. } => {
                    python.remove(&request_id);
                }
                _ => {}
            }
        }
        Ok(python
            .into_iter()
            .next()
            .map(|id| format!("python:{id}"))
            .or_else(|| model.into_iter().next().map(|id| format!("model:{id}"))))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{Node, SqliteStore};

    #[test]
    fn recovery_marks_and_surfaces_unfinished_model_call() {
        let runtime = Runtime::new(SqliteStore::memory().unwrap(), "s", "a");
        let node = Node::new("n", "n", json!({}), json!({}));
        runtime.register_node(&node).unwrap();
        let run = runtime.start(node.id, json!({})).unwrap();
        runtime.begin_model_request(run.run_id).unwrap();
        let report = Scheduler::new(&runtime).recover().unwrap();
        assert_eq!(report.interrupted, [run.run_id]);
        assert_eq!(report.restarted, [run.run_id]);
        assert_eq!(
            runtime.store().run(run.run_id).unwrap().status,
            RunStatus::Running
        );
    }
}
