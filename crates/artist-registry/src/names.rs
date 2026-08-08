//! The artist-name roster.
//!
//! Agents get memorable roster names — Monet, Bach, Basquiat — while internal actor
//! ids remain harness-only implementation details. The bare roster name is the durable
//! model-visible Artist identity used by universal `send` and MCP identity handles.
//!
//! Names are machine-wide rather than per project so a bare Artist identity resolves
//! consistently from any process. Allocation is therefore shared durable state.
//!
//! Unlike a delegation seat, a name is **not** released when its process dies. A retained
//! identity/session record continues to reserve it; only normal retention pruning releases
//! the corresponding lease.

use std::{
    fs,
    path::{Path, PathBuf},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{Result, permits::write_atomic};

/// A name bound to a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Name {
    /// The display name: always an entry from the artist roster.
    pub name: String,
    /// The identity this renders — the session's actor id.
    pub actor: String,
    /// The session that owns it, so a sweep can ask whether that session is
    /// still resumable.
    pub session: String,
    /// Which worktree this agent is working in, so "everyone on this repo" is
    /// answerable. Absent for a record written before the directory existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The profile it is running, so "every reviewer" is answerable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// The *name* of the agent that spawned it, so descendants are walkable.
    /// `None` for a session root, which nobody spawned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// What is known about an agent when it claims a name.
///
/// A struct rather than positional arguments because the directory is only as
/// useful as the attributes recorded in it, and an argument list that grows by
/// one `Option<String>` at a time is how attributes get silently dropped at one
/// of the two call sites.
#[derive(Clone, Debug, Default)]
pub struct Registration {
    pub session: String,
    pub actor: String,
    pub project: Option<String>,
    pub profile: Option<String>,
    pub parent: Option<String>,
}

/// The machine-wide roster.
#[derive(Clone, Debug)]
pub struct Names {
    dir: PathBuf,
    roster: &'static [&'static str],
}

impl Names {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            roster: &crate::roster::ROSTER,
        }
    }

    /// Use a different roster. For tests that need exhaustion to be reachable.
    pub fn with_roster(mut self, roster: &'static [&'static str]) -> Self {
        self.roster = roster;
        self
    }

    /// Bind a name to a session, or return the one it already holds.
    ///
    /// Idempotent on `session`: re-opening a session must not consume a second
    /// name, and must return the same one, or addressing would break across a
    /// resume.
    pub fn claim(&self, registration: &Registration) -> Result<Name> {
        let session = registration.session.as_str();
        let actor = registration.actor.as_str();
        fs::create_dir_all(&self.dir)?;
        let guard = Lock::take(&self.dir)?;

        let mut taken = std::collections::HashSet::new();
        for existing in self.read_all()? {
            if existing.session == session {
                // Re-registering refreshes the attributes: a session that hands
                // off keeps its name (invariant 2) but is now running a
                // different profile, and a directory that said otherwise would
                // route "every reviewer" to the wrong agents.
                if existing.profile != registration.profile
                    || existing.project != registration.project
                {
                    let refreshed = Name {
                        profile: registration.profile.clone(),
                        project: registration.project.clone(),
                        ..existing
                    };
                    write_atomic(
                        &self.path(&refreshed.name),
                        &serde_json::to_vec(&refreshed)?,
                    )?;
                    return Ok(refreshed);
                }
                return Ok(existing);
            }
            taken.insert(existing.name.clone());
        }

        // Offset the scan by the session so concurrent claims tend to land on
        // different names rather than all contending for the first free one.
        let start = fingerprint(session) as usize;
        let chosen = (0..self.roster.len())
            .map(|i| self.roster[(start + i) % self.roster.len()])
            .find(|candidate| !taken.contains(*candidate))
            .ok_or_else(|| crate::Error::Corrupt("artist-name roster exhausted".into()))?;

        let name = Name {
            name: chosen.to_owned(),
            actor: actor.to_owned(),
            session: session.to_owned(),
            project: registration.project.clone(),
            profile: registration.profile.clone(),
            parent: registration.parent.clone(),
        };
        write_atomic(&self.path(&name.name), &serde_json::to_vec(&name)?)?;
        drop(guard);
        Ok(name)
    }

    /// Look a name up for routing a message to it.
    pub fn resolve(&self, name: &str) -> Result<Option<Name>> {
        match fs::read(self.path(name)) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Every currently bound name.
    pub fn list(&self) -> Result<Vec<Name>> {
        let _guard = Lock::take(&self.dir).ok();
        self.read_all()
    }

    /// Release the name held by a session. Called when that session becomes
    /// unresumable — never merely because its process exited.
    pub fn release(&self, session: &str) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        let _guard = Lock::take(&self.dir)?;
        for existing in self.read_all()? {
            if existing.session == session {
                let _ = fs::remove_file(self.path(&existing.name));
            }
        }
        Ok(())
    }

    /// Release every name whose session no longer exists.
    ///
    /// The predicate is the caller's because only the session store knows what
    /// "still resumable" means; this crate deliberately does not guess from
    /// process liveness, which would free a name the moment a terminal closed.
    pub fn sweep(&self, resumable: impl Fn(&str) -> bool) -> Result<usize> {
        fs::create_dir_all(&self.dir)?;
        let _guard = Lock::take(&self.dir)?;
        let mut released = 0;
        for existing in self.read_all()? {
            if !resumable(&existing.session) {
                let _ = fs::remove_file(self.path(&existing.name));
                released += 1;
            }
        }
        Ok(released)
    }

    fn read_all(&self) -> Result<Vec<Name>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        Ok(entries
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                if path.file_name()? == LOCK {
                    return None;
                }
                serde_json::from_slice(&fs::read(path).ok()?).ok()
            })
            .collect())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(sanitize(name))
    }
}

const LOCK: &str = ".lock";

struct Lock(fs::File);

impl Lock {
    fn take(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LOCK))?;
        FileExt::lock_exclusive(&file)?;
        Ok(Self(file))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

/// A name reaches `resolve` from a tool argument, so it must not name a path
/// outside the roster directory.
fn sanitize(name: &str) -> String {
    let cleaned: String = name
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

/// Walk parent links to decide whether `candidate` sits beneath `root`.
///
/// Bounded by the directory size rather than trusting the links to be acyclic:
/// a crash between two claims could in principle leave a cycle, and a
/// group-membership query is not the place to discover it by hanging.
fn fingerprint(value: &str) -> u32 {
    value.bytes().fold(2166136261u32, |hash, byte| {
        (hash ^ byte as u32).wrapping_mul(16777619)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    static SMALL: &[&str] = &["Monet", "Bach"];

    fn names(root: &Path) -> Names {
        Names::new(root.join("names")).with_roster(SMALL)
    }

    fn reg(session: &str, actor: &str) -> Registration {
        Registration {
            session: session.into(),
            actor: actor.into(),
            ..Registration::default()
        }
    }

    #[test]
    fn a_session_keeps_one_name_across_repeated_claims() {
        let root = tempfile::tempdir().unwrap();
        let roster = names(root.path());
        let first = roster.claim(&reg("s-1", "a-1")).unwrap();
        let again = roster.claim(&reg("s-1", "a-1")).unwrap();
        assert_eq!(first, again, "a resume must not consume a second name");
        assert_eq!(roster.list().unwrap().len(), 1);
    }

    #[test]
    fn distinct_sessions_get_distinct_names() {
        let root = tempfile::tempdir().unwrap();
        let roster = names(root.path());
        let a = roster.claim(&reg("s-1", "a-1")).unwrap();
        let b = roster.claim(&reg("s-2", "a-2")).unwrap();
        assert_ne!(a.name, b.name);
    }

    /// The name is a rendering of the actor id, so resolving must hand back
    /// the identity rather than just the label.
    #[test]
    fn resolving_a_name_yields_the_actor_behind_it() {
        let root = tempfile::tempdir().unwrap();
        let roster = names(root.path());
        let claimed = roster.claim(&reg("s-1", "a-7f3")).unwrap();
        let found = roster.resolve(&claimed.name).unwrap().expect("bound");
        assert_eq!(found.actor, "a-7f3");
        assert_eq!(found.session, "s-1");
    }

    /// Public Artist identities are always roster names. Exhaustion is explicit rather
    /// than leaking the internal actor id into the model-facing namespace.
    #[test]
    fn an_exhausted_roster_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let roster = names(root.path());
        roster.claim(&reg("s-1", "a-1")).unwrap();
        roster.claim(&reg("s-2", "a-2")).unwrap();
        let error = roster.claim(&reg("s-3", "a-3")).unwrap_err();
        assert!(error.to_string().contains("roster exhausted"));
        assert!(roster.resolve("a-3").unwrap().is_none());
    }

    /// A closed terminal must not free a name: the session is still resumable
    /// and a person returning to it expects the same agent.
    #[test]
    fn a_name_outlives_its_process_and_is_freed_only_by_the_sweep() {
        let root = tempfile::tempdir().unwrap();
        let roster = names(root.path());
        let held = roster.claim(&reg("s-1", "a-1")).unwrap();

        assert_eq!(roster.sweep(|_| true).unwrap(), 0, "still resumable");
        assert!(roster.resolve(&held.name).unwrap().is_some());

        assert_eq!(roster.sweep(|s| s != "s-1").unwrap(), 1);
        assert!(roster.resolve(&held.name).unwrap().is_none());
    }

    /// Freed names return to circulation, or a long-lived machine slowly runs
    /// out despite nothing being held.
    ///
    /// Asserted by exhausting the roster first: a released name is back in
    /// circulation exactly when the next claim gets a roster entry instead of
    /// the fallback. Which specific entry it gets is not a property — claims
    /// are offset across the roster so that concurrent ones do not all contend
    /// for the same first-free name.
    #[test]
    fn a_released_name_returns_to_circulation() {
        let root = tempfile::tempdir().unwrap();
        let roster = names(root.path());
        roster.claim(&reg("s-1", "a-1")).unwrap();
        roster.claim(&reg("s-2", "a-2")).unwrap();
        assert!(
            roster.claim(&reg("s-3", "a-3")).is_err(),
            "the two-name roster should be exhausted"
        );

        roster.release("s-1").unwrap();
        let reclaimed = roster.claim(&reg("s-4", "a-4")).unwrap();
        assert!(SMALL.contains(&reclaimed.name.as_str()));
    }

    #[test]
    fn a_hostile_name_cannot_escape_the_roster_directory() {
        let root = tempfile::tempdir().unwrap();
        let roster = names(root.path());
        assert!(roster.resolve("../../etc/passwd").unwrap().is_none());
    }

    /// Two processes on one machine share the roster; that is the whole point.
    #[test]
    fn a_second_process_sees_the_first_ones_claims() {
        let root = tempfile::tempdir().unwrap();
        let mine = names(root.path());
        let theirs = names(root.path());
        let claimed = mine.claim(&reg("s-1", "a-1")).unwrap();
        assert_eq!(
            theirs.resolve(&claimed.name).unwrap().unwrap().session,
            "s-1"
        );
    }

    /// A handoff keeps the name but changes the profile, and the directory has
    /// to follow or "every reviewer" routes to the wrong agents.
    #[test]
    fn re_registering_refreshes_the_profile_without_taking_a_new_name() {
        let root = tempfile::tempdir().unwrap();
        let roster = names(root.path());
        let before = roster
            .claim(&Registration {
                session: "s-1".into(),
                actor: "a-1".into(),
                profile: Some("planner".into()),
                ..Registration::default()
            })
            .unwrap();
        let after = roster
            .claim(&Registration {
                session: "s-1".into(),
                actor: "a-1".into(),
                profile: Some("reviewer".into()),
                ..Registration::default()
            })
            .unwrap();

        assert_eq!(before.name, after.name, "a handoff keeps the name");
        assert_eq!(after.profile.as_deref(), Some("reviewer"));
        assert_eq!(
            roster.list().unwrap().len(),
            1,
            "and consumes no second name"
        );
    }

    /// The full shipped roster has to be big enough that exhaustion is
    /// theoretical, and free of duplicates that would make two sessions
    /// collide on one name.
    #[test]
    fn the_shipped_roster_is_large_and_unique() {
        let roster: &[&str] = &crate::roster::ROSTER;
        let unique: std::collections::BTreeSet<_> = roster.iter().collect();
        assert_eq!(unique.len(), roster.len(), "duplicate name in the roster");
        assert_eq!(roster.len(), 736, "shipped roster size changed");
    }
}
