//! Who an agent is.
//!
//! Agents get famous names because the internal actor ids (`a-7f3`) tokenize
//! badly, read as noise in a transcript, and are hard to hold in working memory
//! while several agents run at once. The name is a *rendering* of the actor id,
//! not a second identity — the id stays the key for workspace ownership, todo
//! ownership, and log lineage.
//!
//! Placement in the prompt is the load-bearing detail here. Prompt caching is a
//! prefix match, so a name near the top gives every agent a different prefix
//! and no two agents ever share a cached prompt — which is worst in exactly our
//! case, a fan-out of subagents spawned together off one base prompt. The name
//! therefore goes last, after every byte that is shared, so only the tail is
//! reprocessed. See [`Identity::prompt_block`].


/// A resolved agent identity.
#[derive(Clone, Debug)]
pub(crate) struct Identity {
    pub name: String,
    pub actor: String,
}

impl Identity {
    /// The trailing system-prompt block.
    ///
    /// Returned separately from the rest of the prompt, and appended by the
    /// caller *after* everything shared, so the shared body stays one cacheable
    /// prefix across every agent on the machine. Anything placed after this is
    /// per-agent too and belongs here rather than above it.
    pub fn prompt_block(&self) -> String {
        format!(
            "\n\nYou are {}, an Artist in the so-named agentic coding harness.",
            self.name
        )
    }
}

/// Claim the roster name for a durable Artist identity.
pub(crate) fn for_session(
    registration: artist_registry::Registration,
) -> Result<Identity, artist_registry::Error> {
    let name = artist_registry::names().claim(&registration)?;
    Ok(Identity {
        name: name.name,
        actor: name.actor,
    })
}

/// What a session records about itself in the directory.
///
/// The attributes are what make a predicate answerable: without `project` there
/// is no "everyone on this repo", without `profile` no "every reviewer", and
/// without `parent` no "everyone under Monet". A directory that records only
/// names can be read but not queried.
pub(crate) fn session(
    session: &str,
    actor: &str,
    project: &std::path::Path,
    profile: &str,
) -> Result<Identity, artist_registry::Error> {
    for_session(artist_registry::Registration {
        session: session.to_owned(),
        actor: actor.to_owned(),
        project: Some(project.display().to_string()),
        profile: Some(profile.to_owned()),
        parent: None,
    })
}

/// Reconstitute the identity stored in a lineage's `run.started` event.
/// Registration is deliberately skipped: the durable log, not the transient
/// name roster, is authoritative when resuming an existing agent.
pub(crate) fn recorded(name: &str, actor: &str) -> Identity {
    Identity {
        name: name.to_owned(),
        actor: actor.to_owned(),
    }
}

/// Claim a durable name for a subagent identity. The lease is deliberately not
/// released when the child process/run ends; registry retention owns release.
pub(crate) fn for_run(
    actor: &str,
    project: &std::path::Path,
    profile: &str,
    parent: Option<&str>,
) -> Result<RunIdentity, artist_registry::Error> {
    Ok(RunIdentity {
        identity: for_session(artist_registry::Registration {
            session: actor.to_owned(),
            actor: actor.to_owned(),
            project: Some(project.display().to_string()),
            profile: Some(profile.to_owned()),
            parent: parent.map(str::to_owned),
        })?,
        lease_key: actor.to_owned(),
    })
}

#[derive(Clone)]
pub(crate) struct RunIdentity {
    identity: Identity,
    /// Internal roster lease key; never model-facing. The session registry keeps
    /// this key until its retained record is pruned.
    pub(crate) lease_key: String,
}

impl std::ops::Deref for RunIdentity {
    type Target = Identity;

    fn deref(&self) -> &Self::Target {
        &self.identity
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    /// Serialised: these share one process-wide roster directory via
    /// `ARTIST_STATE_DIR`, and `set_var` is process-global.
    fn with_roster<T>(body: impl FnOnce() -> T) -> T {
        static GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _held = GUARD.lock().unwrap_or_else(|error| error.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: single-threaded within the guard, and the value is only read
        // by this crate's registry lookups.
        unsafe { std::env::set_var("ARTIST_STATE_DIR", dir.path()) };
        body()
    }

    #[test]
    fn a_session_keeps_its_name_across_repeated_resolution() {
        with_roster(|| {
            let first = session("s-1", "s-1", std::path::Path::new("/p"), "default");
            let again = session("s-1", "s-1", std::path::Path::new("/p"), "default");
            assert_eq!(first.name, again.name);
            assert_eq!(first.actor, "s-1");
        });
    }

    /// The prompt block must be appendable to a shared prefix without changing
    /// a byte of it — that is the whole reason it is a separate string.
    #[test]
    fn the_identity_block_is_a_pure_suffix() {
        with_roster(|| {
            let shared = "shared system prompt";
            let one = format!(
                "{shared}{}",
                session("s-1", "s-1", std::path::Path::new("/p"), "default").prompt_block()
            );
            let two = format!(
                "{shared}{}",
                session("s-2", "s-2", std::path::Path::new("/p"), "default").prompt_block()
            );

            assert!(one.starts_with(shared) && two.starts_with(shared));
            assert_ne!(one, two, "two agents must not be the same agent");
            let common = one
                .bytes()
                .zip(two.bytes())
                .take_while(|(a, b)| a == b)
                .count();
            assert!(
                common >= shared.len(),
                "the shared prefix must survive intact so it can cache across agents"
            );
        });
    }

    #[test]
    fn the_block_names_the_agent() {
        with_roster(|| {
            let identity = session("s-1", "a-1", std::path::Path::new("/p"), "default");
            assert!(identity.prompt_block().contains(&identity.name));
            assert!(identity.prompt_block().contains("Artist"));
        });
    }

    /// A subagent is not resumable, so its name goes back to the pool when the
    /// run ends — otherwise a fan-out drains the roster permanently.
    #[test]
    fn a_run_name_is_released_when_the_run_ends() {
        with_roster(|| {
            let name = {
                let run = for_run(
                    "a-child",
                    std::path::Path::new("/p"),
                    "worker",
                    Some("Monet"),
                );
                let held = run.name.clone();
                assert!(artist_registry::names().resolve(&held).unwrap().is_some());
                held
            };
            assert!(
                artist_registry::names().resolve(&name).unwrap().is_none(),
                "the run's name should be back in the pool"
            );
        });
    }

    /// Cloning the identity into a child environment must not release the name
    /// when the clone goes out of scope.
    #[test]
    fn a_cloned_run_identity_holds_the_name_until_the_last_copy_drops() {
        with_roster(|| {
            let run = for_run(
                "a-child",
                std::path::Path::new("/p"),
                "worker",
                Some("Monet"),
            );
            let name = run.name.clone();
            let clone = run.clone();
            drop(run);
            assert!(artist_registry::names().resolve(&name).unwrap().is_some());
            drop(clone);
            assert!(artist_registry::names().resolve(&name).unwrap().is_none());
        });
    }
}
