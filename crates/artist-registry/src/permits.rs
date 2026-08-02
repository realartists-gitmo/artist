//! A counting semaphore that spans processes.
//!
//! `max_concurrent` is a statement about a project — how many subagents may run
//! against this worktree at once. Enforcing it with an in-process semaphore
//! made it a statement about a process instead, so N terminals running artist
//! on one repository got N times the configured concurrency and contended for
//! the same CPU, the same build lock, and the same files.
//!
//! Seats are files in a directory. Holding one is holding a file; releasing is
//! deleting it; crashing leaves it behind for the next caller to reclaim once
//! the owning process is gone. The directory lock is only ever held across the
//! scan-and-create, never across the work the seat authorises.

use std::{
    fs,
    path::{Path, PathBuf},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, process::Owner};

/// The file written for one held seat.
#[derive(Serialize, Deserialize)]
struct Claim {
    owner: Owner,
    /// Free-text, for `artist` diagnostics: which run took this seat.
    #[serde(default)]
    label: String,
}

/// A project's seat pool.
#[derive(Clone, Debug)]
pub struct Permits {
    dir: PathBuf,
    max: usize,
}

impl Permits {
    pub(crate) fn new(dir: PathBuf, max: usize) -> Self {
        Self { dir, max }
    }

    /// Take a seat, or report that every one is held.
    ///
    /// Non-blocking by design: a fan-out beyond the limit should fail loudly
    /// and immediately rather than queue invisibly, which is the behaviour the
    /// in-process semaphore had and the behaviour callers were written against.
    pub fn try_acquire(&self, label: &str) -> Result<Option<Seat>> {
        fs::create_dir_all(&self.dir)?;
        let guard = DirLock::take(&self.dir)?;

        let mut live = 0usize;
        for entry in fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.file_name().is_some_and(|name| name == LOCK) {
                continue;
            }
            match read_claim(&path) {
                // A seat whose owner is gone is not a seat. Reclaiming here
                // rather than in a sweeper means the cost is paid by whoever
                // needs the seat, and only when they need it.
                Some(claim) if claim.owner.is_alive() => live += 1,
                _ => {
                    let _ = fs::remove_file(&path);
                }
            }
        }

        if live >= self.max {
            return Ok(None);
        }

        let path = self.dir.join(format!("{}-{}", std::process::id(), unique()));
        write_atomic(
            &path,
            &serde_json::to_vec(&Claim {
                owner: Owner::current(),
                label: label.to_owned(),
            })?,
        )?;
        drop(guard);
        Ok(Some(Seat { path }))
    }

    /// Seats currently held by live processes, reclaiming any that are not.
    pub fn live(&self) -> Result<usize> {
        fs::create_dir_all(&self.dir)?;
        let _guard = DirLock::take(&self.dir)?;
        let mut live = 0;
        for entry in fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.file_name().is_some_and(|name| name == LOCK) {
                continue;
            }
            match read_claim(&path) {
                Some(claim) if claim.owner.is_alive() => live += 1,
                _ => {
                    let _ = fs::remove_file(&path);
                }
            }
        }
        Ok(live)
    }

    pub fn max(&self) -> usize {
        self.max
    }
}

/// A held seat. Dropping it releases.
///
/// Release is best-effort: if the file cannot be removed the seat is reclaimed
/// by liveness the moment this process exits, so a failed delete costs a seat
/// until then rather than permanently.
#[derive(Debug)]
pub struct Seat {
    path: PathBuf,
}

impl Drop for Seat {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

const LOCK: &str = ".lock";

/// An exclusive lock over the seat directory, held only across a scan.
///
/// `flock` is released by the kernel when the holding process dies, so a crash
/// mid-scan cannot wedge every other process out of the pool — which a
/// lockfile-by-existence scheme would.
struct DirLock(fs::File);

impl DirLock {
    fn take(dir: &Path) -> Result<Self> {
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LOCK))?;
        FileExt::lock_exclusive(&file)?;
        Ok(Self(file))
    }
}

impl Drop for DirLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn read_claim(path: &Path) -> Option<Claim> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Enough uniqueness to name a file within one locked section — the lock, not
/// this value, is what actually prevents collisions.
fn unique() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Write through a temporary and rename, so a reader never sees a half-written
/// claim and mistake it for a corrupt one worth reclaiming.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Error::Corrupt(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permits(dir: &Path, max: usize) -> Permits {
        Permits::new(dir.join("permits"), max)
    }

    #[test]
    fn the_pool_hands_out_exactly_its_limit() {
        let root = tempfile::tempdir().unwrap();
        let pool = permits(root.path(), 2);

        let first = pool.try_acquire("a").unwrap();
        let second = pool.try_acquire("b").unwrap();
        assert!(first.is_some() && second.is_some());
        assert_eq!(pool.live().unwrap(), 2);
        assert!(pool.try_acquire("c").unwrap().is_none());
    }

    #[test]
    fn releasing_returns_the_seat() {
        let root = tempfile::tempdir().unwrap();
        let pool = permits(root.path(), 1);

        let held = pool.try_acquire("a").unwrap().expect("a free pool has one");
        assert!(pool.try_acquire("b").unwrap().is_none());
        drop(held);
        assert!(pool.try_acquire("b").unwrap().is_some());
    }

    /// The whole point of putting seats on disk: a second process sees the
    /// first one's seats. Simulated by pointing two `Permits` at one directory,
    /// which is exactly what two processes on one project do.
    #[test]
    fn a_second_holder_sees_the_first_holders_seats() {
        let root = tempfile::tempdir().unwrap();
        let mine = permits(root.path(), 2);
        let theirs = permits(root.path(), 2);

        let _a = mine.try_acquire("mine").unwrap().unwrap();
        let _b = theirs.try_acquire("theirs").unwrap().unwrap();
        assert!(
            mine.try_acquire("mine-again").unwrap().is_none(),
            "the limit is the project's, not each process's"
        );
    }

    /// A crashed process must not pin a seat forever, or one bad exit
    /// permanently lowers the project's concurrency.
    #[test]
    fn a_seat_held_by_a_dead_process_is_reclaimed() {
        let root = tempfile::tempdir().unwrap();
        let pool = permits(root.path(), 1);
        fs::create_dir_all(&pool.dir).unwrap();
        write_atomic(
            &pool.dir.join("ghost"),
            &serde_json::to_vec(&Claim {
                owner: Owner {
                    pid: u32::MAX - 1,
                    started: None,
                },
                label: "crashed".into(),
            })
            .unwrap(),
        )
        .unwrap();

        assert_eq!(pool.live().unwrap(), 0);
        assert!(pool.try_acquire("survivor").unwrap().is_some());
    }

    /// Reclaiming on unreadable data is the safe direction: a corrupt claim we
    /// cannot attribute would otherwise consume a seat that nothing releases.
    #[test]
    fn an_unreadable_claim_is_reclaimed_rather_than_counted() {
        let root = tempfile::tempdir().unwrap();
        let pool = permits(root.path(), 1);
        fs::create_dir_all(&pool.dir).unwrap();
        fs::write(pool.dir.join("garbage"), b"not json").unwrap();

        assert_eq!(pool.live().unwrap(), 0);
        assert!(pool.try_acquire("survivor").unwrap().is_some());
    }

    /// The lock file lives in the same directory as the seats and must never be
    /// mistaken for one.
    #[test]
    fn the_directory_lock_is_not_counted_as_a_seat() {
        let root = tempfile::tempdir().unwrap();
        let pool = permits(root.path(), 1);
        let held = pool.try_acquire("a").unwrap();
        assert!(held.is_some());
        assert_eq!(pool.live().unwrap(), 1);
    }
}
