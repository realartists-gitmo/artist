//! Background jobs, visible to every process working on the project.
//!
//! A job's *execution* cannot cross a process — the work is a task in the
//! owning process's runtime. Its *record* can, and that is what callers
//! actually need: `list` should show what is running on this worktree, `await`
//! should be able to wait on it, and a status read should not depend on which
//! terminal started it.
//!
//! So the record lives in a file the owner writes and anyone reads. A reader
//! polls; jobs run for minutes, so polling costs nothing and buys the absence
//! of any inter-process channel to keep alive.

use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{Result, permits::write_atomic, process::Owner};

/// How a job ended.
///
/// Mirrors the terminal states the session log already records, so a caller
/// waiting on a job and a reader replaying the log describe the same outcome
/// with the same word.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// Ran to completion and produced this.
    Finished { output: String },
    /// Failed on its own terms — a provider error, an exhausted candidate list.
    Failed { error: String },
    /// Stopped by a person or by the harness.
    Cancelled,
    /// The owning process disappeared before recording an outcome. Inferred by
    /// a reader rather than written by the owner, because by definition the
    /// owner was not there to write it.
    Abandoned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JobState {
    Running,
    Done { outcome: Outcome },
}

/// One job's durable record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    /// The profile it runs, for display in `list`.
    pub role: String,
    /// First line of the task, for display.
    pub summary: String,
    pub owner: Owner,
    pub state: JobState,
    /// Seconds since the epoch, for ordering a `list` across processes that do
    /// not share a clock source beyond the wall clock.
    pub started_at: u64,
}

impl Job {
    /// The outcome a reader should act on, which is not always the one written.
    ///
    /// A record left in `Running` by a process that no longer exists is not
    /// running — nothing will ever finish it, so a waiter that trusted the file
    /// would block forever. Resolving that here means every reader gets the
    /// same answer without each one reimplementing the check.
    pub fn resolved(&self) -> JobState {
        match &self.state {
            JobState::Running if !self.owner.is_alive() => JobState::Done {
                outcome: Outcome::Abandoned,
            },
            other => other.clone(),
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.resolved(), JobState::Running)
    }

    /// Whether this process is the one that can still act on the job.
    pub fn is_ours(&self) -> bool {
        self.owner == Owner::current()
    }
}

/// The project's job table.
#[derive(Clone, Debug)]
pub struct Jobs {
    dir: PathBuf,
}

impl Jobs {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Record a job as running. Called by the owner at spawn.
    pub fn start(&self, id: &str, role: &str, summary: &str) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        self.write(&Job {
            id: id.to_owned(),
            role: role.to_owned(),
            summary: summary.chars().take(200).collect(),
            owner: Owner::current(),
            state: JobState::Running,
            started_at: now(),
        })
    }

    /// Record a terminal outcome. Called by the owner when the job ends.
    pub fn finish(&self, id: &str, outcome: Outcome) -> Result<()> {
        let Some(mut job) = self.get(id)? else {
            return Ok(());
        };
        job.state = JobState::Done { outcome };
        self.write(&job)
    }

    pub fn get(&self, id: &str) -> Result<Option<Job>> {
        match fs::read(self.path(id)) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Every job on this project, newest first, from every process.
    ///
    /// Unreadable records are skipped rather than failing the listing: one
    /// corrupt file must not hide the other jobs from the user.
    pub fn list(&self) -> Result<Vec<Job>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut jobs: Vec<Job> = entries
            .filter_map(|entry| fs::read(entry.ok()?.path()).ok())
            .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
            .collect();
        jobs.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(a.id.cmp(&b.id)));
        Ok(jobs)
    }

    /// Drop records for jobs that finished before `keep_since`, so a
    /// long-lived worktree does not accumulate them forever. Running jobs are
    /// never removed regardless of age.
    pub fn prune(&self, keep_since: u64) -> Result<usize> {
        let mut removed = 0;
        for job in self.list()? {
            if !job.is_running() && job.started_at < keep_since {
                let _ = fs::remove_file(self.path(&job.id));
                removed += 1;
            }
        }
        Ok(removed)
    }

    fn write(&self, job: &Job) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        write_atomic(&self.path(&job.id), &serde_json::to_vec(job)?)
    }

    fn path(&self, id: &str) -> PathBuf {
        // Ids are harness-generated (`artist_tools::short_id`), but a job id
        // reaches this from a tool argument, so a traversal must not.
        self.dir.join(sanitize(id))
    }
}

/// Reduce an id to something that can only name a file directly inside the job
/// directory.
fn sanitize(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(96)
        .collect();
    if cleaned.is_empty() {
        "-".to_owned()
    } else {
        cleaned
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Jobs {
    #[cfg(test)]
    fn dir(&self) -> &std::path::Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn jobs(root: &Path) -> Jobs {
        Jobs::new(root.join("jobs"))
    }

    #[test]
    fn a_job_round_trips_through_the_table() {
        let root = tempfile::tempdir().unwrap();
        let table = jobs(root.path());
        table.start("a-1", "worker", "refactor the parser").unwrap();

        let job = table.get("a-1").unwrap().expect("just started");
        assert_eq!(job.role, "worker");
        assert!(job.is_running());
        assert!(job.is_ours());

        table
            .finish(
                "a-1",
                Outcome::Finished {
                    output: "done".into(),
                },
            )
            .unwrap();
        assert!(!table.get("a-1").unwrap().unwrap().is_running());
    }

    /// The reason the table exists: another process's jobs are visible.
    #[test]
    fn a_second_reader_sees_the_first_writers_jobs() {
        let root = tempfile::tempdir().unwrap();
        jobs(root.path()).start("a-1", "worker", "theirs").unwrap();
        assert_eq!(jobs(root.path()).list().unwrap().len(), 1);
    }

    /// A waiter must never block forever on a job whose owner died — the whole
    /// value of `await` across processes depends on this resolving.
    #[test]
    fn a_job_whose_owner_died_resolves_as_abandoned() {
        let root = tempfile::tempdir().unwrap();
        let table = jobs(root.path());
        table.start("a-1", "worker", "orphan").unwrap();

        let mut job = table.get("a-1").unwrap().unwrap();
        job.owner = Owner {
            pid: u32::MAX - 1,
            started: None,
        };
        table.write(&job).unwrap();

        let seen = table.get("a-1").unwrap().unwrap();
        assert!(!seen.is_running());
        assert_eq!(
            seen.resolved(),
            JobState::Done {
                outcome: Outcome::Abandoned
            }
        );
        assert!(
            !seen.is_ours(),
            "an abandoned job must not look actionable to us"
        );
    }

    /// A finished job keeps its recorded outcome even after its owner exits —
    /// abandonment is only inferred for work that never finished.
    #[test]
    fn a_finished_job_is_not_reinterpreted_when_its_owner_exits() {
        let root = tempfile::tempdir().unwrap();
        let table = jobs(root.path());
        table.start("a-1", "worker", "finished").unwrap();
        table
            .finish(
                "a-1",
                Outcome::Finished {
                    output: "result".into(),
                },
            )
            .unwrap();

        let mut job = table.get("a-1").unwrap().unwrap();
        job.owner = Owner {
            pid: u32::MAX - 1,
            started: None,
        };
        table.write(&job).unwrap();

        assert_eq!(
            table.get("a-1").unwrap().unwrap().resolved(),
            JobState::Done {
                outcome: Outcome::Finished {
                    output: "result".into()
                }
            }
        );
    }

    /// Job ids reach this from a tool argument, so they must not be able to
    /// name a path outside the table.
    #[test]
    fn an_id_cannot_escape_the_job_directory() {
        let root = tempfile::tempdir().unwrap();
        let table = jobs(root.path());
        table.start("../../etc/passwd", "worker", "hostile").unwrap();

        // The property, not the exact spelling: whatever the id flattens to,
        // it is one entry directly inside the table and nothing was written
        // anywhere else.
        let written: Vec<_> = fs::read_dir(table.dir())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].parent(), Some(table.dir()));
        assert!(!root.path().join("etc").exists());
        assert!(!root.path().parent().unwrap().join("etc").exists());
    }

    #[test]
    fn listing_survives_a_corrupt_record() {
        let root = tempfile::tempdir().unwrap();
        let table = jobs(root.path());
        table.start("a-1", "worker", "good").unwrap();
        fs::write(table.dir().join("a-2"), b"not json").unwrap();
        assert_eq!(table.list().unwrap().len(), 1);
    }

    #[test]
    fn pruning_keeps_running_jobs_whatever_their_age() {
        let root = tempfile::tempdir().unwrap();
        let table = jobs(root.path());
        table.start("a-1", "worker", "still going").unwrap();
        table.start("a-2", "worker", "over").unwrap();
        table.finish("a-2", Outcome::Cancelled).unwrap();

        assert_eq!(table.prune(now() + 60).unwrap(), 1);
        assert!(table.get("a-1").unwrap().is_some());
        assert!(table.get("a-2").unwrap().is_none());
    }
}
