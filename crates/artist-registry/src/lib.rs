//! The machine-wide registry: what artist knows about work that outlives a
//! single process.
//!
//! Three things were held in `OnceLock` statics keyed by project root — the
//! delegation semaphore, the background-job table, and (once agents are named)
//! the name roster. A `OnceLock` is per process, so each of them silently meant
//! "per process, per project" rather than "per project":
//!
//! * `max_concurrent = 4` became 4×N across N artist processes on one
//!   repository, all contending for the same CPU and the same build lock.
//! * A job started by one process was invisible to another, so `await` could
//!   not reach it and `list` did not show it.
//! * Two processes would independently hand out the same agent name.
//!
//! All three are the same problem and get the same fix: state on disk, keyed by
//! what it is actually scoped to, with liveness rather than cleanup as the
//! correctness rule. Nothing here assumes a graceful exit, because a harness
//! that spawns subagents is a harness that gets `SIGKILL`ed.
//!
//! Scope differs by concern, and the layout says so: seats and jobs live under
//! the project, because that is what bounds them; names live under the user's
//! state directory, because a name must be unique across every project at once.

use std::path::{Path, PathBuf};

mod jobs;
mod messages;
mod names;
mod permits;
mod process;
mod roster;

pub use jobs::{Job, JobState, Jobs, Outcome};
pub use messages::{Audience, Group, Message, Messages, now};
pub use names::{Name, Names, Registration, Selector};
pub use permits::{Permits, Seat};
pub use process::Owner;
pub use roster::ROSTER;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("registry io: {0}")]
    Io(#[from] std::io::Error),
    /// A record that could not be read. Always recoverable: the caller either
    /// reclaims the slot or ignores the record, never fails the run.
    #[error("registry record unreadable: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Per-project registry state.
///
/// Lives inside the project's own ignored state directory rather than a
/// machine-global one so that the path is derivable from the project root
/// alone — every process working on that worktree computes the same location
/// without a lookup table, which is what makes cross-process agreement
/// automatic rather than configured.
#[derive(Clone, Debug)]
pub struct Registry {
    root: PathBuf,
}

impl Registry {
    pub fn for_project(project: &Path) -> Self {
        Self {
            root: project.join(".artist/state/registry"),
        }
    }

    /// Point the registry at an explicit directory. For tests, and for the
    /// case where a project's state directory is relocated.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The delegation seat pool for this project.
    pub fn permits(&self, max: usize) -> Permits {
        Permits::new(self.root.join("permits"), max)
    }

    /// The background job table for this project.
    pub fn jobs(&self) -> Jobs {
        Jobs::new(self.root.join("jobs"))
    }

    /// A message store rooted here rather than machine-wide.
    ///
    /// Only for tests: real delivery is machine-wide, because an agent name is
    /// unique across the machine and a project-scoped mailbox would make
    /// inter-repository coordination impossible.
    /// A name roster rooted here rather than machine-wide.
    ///
    /// Only for tests and isolated harnesses that must not mutate the user's
    /// machine-wide roster.
    pub fn names_for_test(&self) -> Names {
        Names::new(self.root.join("names"))
    }

    /// A message store rooted here rather than machine-wide.
    ///
    /// Only for tests: real delivery is machine-wide, because an agent name is
    /// unique across the machine and a project-scoped mailbox would make
    /// inter-repository coordination impossible.
    pub fn messages_for_test(&self) -> Messages {
        Messages::new(self.root.join("messages"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// The machine-wide name roster.
///
/// Not per project: two agents in different repositories must not both be
/// Monet, because `tell(Monet)` has to mean one agent on this machine.
pub fn names() -> Names {
    Names::new(names_root())
}

/// The machine-wide mailbox and group store.
///
/// Machine-wide for the same reason names are: a message is addressed to an
/// agent, and agents are unique across the machine rather than within a
/// project. Inter-repository coordination is a real want, so scoping delivery
/// to a project would make the common case work and the interesting one
/// impossible.
pub fn messages() -> Messages {
    Messages::new(names_root().with_file_name("messages"))
}

fn names_root() -> PathBuf {
    // `ARTIST_STATE_DIR` first so a test or a sandboxed run can redirect the
    // roster without touching the user's real one.
    if let Some(dir) = std::env::var_os("ARTIST_STATE_DIR") {
        return PathBuf::from(dir).join("names");
    }
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("artist/names")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both processes must derive the same path from the same worktree, or
    /// none of the cross-process guarantees hold.
    #[test]
    fn the_project_path_is_derived_not_configured() {
        let project = Path::new("/tmp/some/project");
        assert_eq!(
            Registry::for_project(project).root(),
            Registry::for_project(project).root()
        );
        assert!(
            Registry::for_project(project)
                .root()
                .starts_with("/tmp/some/project/.artist")
        );
    }

    /// Seats are bounded by the project; names are not, so they must not share
    /// a root or two repositories would each get their own Monet.
    #[test]
    fn names_live_outside_any_project() {
        let project = Path::new("/tmp/some/project");
        assert!(!names_root().starts_with(project));
    }
}
