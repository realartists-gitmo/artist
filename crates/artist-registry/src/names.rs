//! The artist-name roster.
//!
//! Agents get famous names — Monet, Bach, Basquiat — because the internal actor
//! ids are short random tokens (`a-7f3`) that tokenize badly, read as noise in
//! a transcript, and are hard for a person to hold in working memory while
//! several agents are running. A name is a *rendering* of an actor id, never a
//! second identity: the id stays the key for workspace ownership, todo
//! ownership, and log lineage, and this table is the mapping between them.
//!
//! Names are machine-wide rather than per project, because `tell(Monet)` has to
//! name one agent on this machine. That makes allocation shared state, and
//! shared state between processes means disk.
//!
//! Unlike a delegation seat, a name is **not** released when its process dies.
//! A session in history still owns its name: a person returning to it expects
//! to still be talking to Monet. Release happens when the session can no longer
//! be resumed, which only its owner can know — so the roster is swept against a
//! liveness predicate the caller supplies rather than against process liveness.

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
    /// The display name: an entry from the roster, or the actor id itself when
    /// the roster is exhausted.
    pub name: String,
    /// The identity this renders — the session's actor id.
    pub actor: String,
    /// The session that owns it, so a sweep can ask whether that session is
    /// still resumable.
    pub session: String,
}

impl Name {
    /// Whether this name is a roster entry or the fallback rendering.
    ///
    /// The fallback is not a failure: past a few hundred concurrent agents a
    /// person has stopped addressing them individually, so only the aesthetic
    /// degrades while addressing keeps working.
    pub fn is_fallback(&self) -> bool {
        self.name == self.actor
    }
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
    pub fn claim(&self, session: &str, actor: &str) -> Result<Name> {
        fs::create_dir_all(&self.dir)?;
        let guard = Lock::take(&self.dir)?;

        let mut taken = std::collections::HashSet::new();
        for existing in self.read_all()? {
            if existing.session == session {
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
            // Exhaustion degrades to the actor id rather than failing: the
            // name is for human addressing, and an unnameable agent would be a
            // worse outcome than an ugly one.
            .unwrap_or(actor);

        let name = Name {
            name: chosen.to_owned(),
            actor: actor.to_owned(),
            session: session.to_owned(),
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

/// A cheap stable hash, used only to spread concurrent claims across the
/// roster. Collisions cost a scan, never correctness — the lock decides.
fn fingerprint(value: &str) -> u32 {
    value
        .bytes()
        .fold(2166136261u32, |hash, byte| (hash ^ byte as u32).wrapping_mul(16777619))
}

#[cfg(test)]
mod tests {
    use super::*;

    static SMALL: &[&str] = &["Monet", "Bach"];

    fn names(root: &Path) -> Names {
        Names::new(root.join("names")).with_roster(SMALL)
    }

    #[test]
    fn a_session_keeps_one_name_across_repeated_claims() {
        let root = tempfile::tempdir().unwrap();
        let roster = names(root.path());
        let first = roster.claim("s-1", "a-1").unwrap();
        let again = roster.claim("s-1", "a-1").unwrap();
        assert_eq!(first, again, "a resume must not consume a second name");
        assert_eq!(roster.list().unwrap().len(), 1);
    }

    #[test]
    fn distinct_sessions_get_distinct_names() {
        let root = tempfile::tempdir().unwrap();
        let roster = names(root.path());
        let a = roster.claim("s-1", "a-1").unwrap();
        let b = roster.claim("s-2", "a-2").unwrap();
        assert_ne!(a.name, b.name);
    }

    /// The name is a rendering of the actor id, so resolving must hand back
    /// the identity rather than just the label.
    #[test]
    fn resolving_a_name_yields_the_actor_behind_it() {
        let root = tempfile::tempdir().unwrap();
        let roster = names(root.path());
        let claimed = roster.claim("s-1", "a-7f3").unwrap();
        let found = roster.resolve(&claimed.name).unwrap().expect("bound");
        assert_eq!(found.actor, "a-7f3");
        assert_eq!(found.session, "s-1");
    }

    /// Exhaustion must not fail an agent's startup — it degrades to the id.
    #[test]
    fn an_exhausted_roster_falls_back_to_the_actor_id() {
        let root = tempfile::tempdir().unwrap();
        let roster = names(root.path());
        roster.claim("s-1", "a-1").unwrap();
        roster.claim("s-2", "a-2").unwrap();

        let overflow = roster.claim("s-3", "a-3").unwrap();
        assert_eq!(overflow.name, "a-3");
        assert!(overflow.is_fallback());
        assert_eq!(
            roster.resolve("a-3").unwrap().unwrap().actor,
            "a-3",
            "a fallback name still has to be addressable"
        );
    }

    /// A closed terminal must not free a name: the session is still resumable
    /// and a person returning to it expects the same agent.
    #[test]
    fn a_name_outlives_its_process_and_is_freed_only_by_the_sweep() {
        let root = tempfile::tempdir().unwrap();
        let roster = names(root.path());
        let held = roster.claim("s-1", "a-1").unwrap();

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
        roster.claim("s-1", "a-1").unwrap();
        roster.claim("s-2", "a-2").unwrap();
        assert!(
            roster.claim("s-3", "a-3").unwrap().is_fallback(),
            "the two-name roster should be exhausted"
        );
        roster.release("s-3").unwrap();

        roster.release("s-1").unwrap();
        let reclaimed = roster.claim("s-4", "a-4").unwrap();
        assert!(
            !reclaimed.is_fallback(),
            "releasing must put the name back in the pool, got {reclaimed:?}"
        );
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
        let claimed = mine.claim("s-1", "a-1").unwrap();
        assert_eq!(
            theirs.resolve(&claimed.name).unwrap().unwrap().session,
            "s-1"
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
        assert!(roster.len() > 500, "roster is {}", roster.len());
    }
}
