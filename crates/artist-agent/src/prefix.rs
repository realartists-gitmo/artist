//! The stable prompt-cache prefix.
//!
//! Prompt caching keys on a prefix of the request, so the preamble is the one
//! string in a turn that must not move. Change a byte of it and every cached
//! token behind it — the whole conversation — is repriced at full rate.
//!
//! That rule is easy to state and very easy to break by accident, because the
//! preamble is assembled from disk: project instruction files, the skill
//! catalogue, the profile. Anything that re-reads disk on a turn boundary can
//! change it, and nothing about `format!` warns you. Reviewing for it does not
//! scale; the mistake looks like ordinary code.
//!
//! So it is enforced by discarding rather than by review. [`PrefixFreezer`]
//! keeps the first preamble it is shown for a given key and returns *that*
//! forever after, whatever it is handed later. A field added in future that
//! varies per turn cannot invalidate the cache — it simply never reaches the
//! model through this channel. The freezer reports that it happened, and the
//! caller routes the difference to the append-only channel, where changing
//! content costs nothing.
//!
//! The failure mode is therefore "your change arrives as an appended note
//! instead of in the preamble", which is what we would have wanted anyway.
//!
//! Keyed by profile as well as conversation: a handoff is deliberately a fresh
//! start with a different persona and tool surface, documented in the loop as
//! functionally `/clear` followed by a new session. Repricing there is correct.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use similar::TextDiff;

/// How much of a turn a superseded-preamble report may take.
///
/// Matched to `artist_tools::drift::DRIFT_BUDGET`, which solves the same
/// problem on the same channel: a report that rides the user turn competes with
/// the user's actual message for room, and one that is unbounded gets cut at an
/// arbitrary point rather than a chosen one.
pub const SUPERSEDED_BUDGET: usize = 8 * 1024;

/// Holds the first preamble seen for each key.
///
/// Cloning shares the store, so every turn of a session — and every retry
/// within a turn — sees the same frozen text.
#[derive(Clone, Default)]
pub struct PrefixFreezer(Arc<Mutex<HashMap<String, Arc<str>>>>);

/// The preamble that will actually be sent, and what it displaced.
pub struct Frozen {
    text: Arc<str>,
    superseded: Option<String>,
}

impl Frozen {
    /// The bytes to send. Identical for every turn under one key.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Rendered account of what this turn's preamble would have said
    /// differently, or `None` when nothing moved — which is the overwhelmingly
    /// common case, and must stay free.
    pub fn superseded_note(&self) -> Option<String> {
        let fresh = self.superseded.as_deref()?;
        Some(render_superseded(&self.text, fresh, SUPERSEDED_BUDGET))
    }
}

impl PrefixFreezer {
    /// One per session, cloned into every turn.
    ///
    /// Named for its scope because the scope is the whole mechanism: a freezer
    /// constructed per turn freezes nothing, and reads perfectly well at the
    /// call site. Clone a session-lived one in — as the tool registry and the
    /// surface registry beside it already do.
    pub fn for_session() -> Self {
        Self::default()
    }

    /// Return the preamble for `key`, freezing `fresh` if this is the first
    /// sight of that key and discarding it otherwise.
    pub fn freeze(&self, key: &str, fresh: String) -> Frozen {
        // A poisoned lock means some other turn panicked while holding it. The
        // stored text is immutable once inserted, so the map is still sound —
        // and refusing to serve a preamble would fail this turn to no purpose.
        let mut held = match self.0.lock() {
            Ok(held) => held,
            Err(poisoned) => poisoned.into_inner(),
        };
        match held.get(key) {
            Some(frozen) if **frozen == *fresh => Frozen {
                text: Arc::clone(frozen),
                superseded: None,
            },
            Some(frozen) => Frozen {
                text: Arc::clone(frozen),
                superseded: Some(fresh),
            },
            None => {
                let text: Arc<str> = Arc::from(fresh);
                held.insert(key.to_owned(), Arc::clone(&text));
                Frozen {
                    text,
                    superseded: None,
                }
            }
        }
    }
}

/// The key a preamble is frozen under.
///
/// Profile is part of it because a handoff legitimately changes the persona and
/// the tool surface; conversation is part of it because two sessions in one
/// process must not share.
pub fn key(conversation: &str, profile: &str) -> String {
    format!("{conversation}\u{1f}{profile}")
}

/// Say what the frozen preamble no longer matches.
///
/// A diff rather than the new text in full: the changed region is what the
/// model needs, and instruction files run to tens of kilobytes. Deliberately
/// content-blind — it knows nothing about `AGENTS.md` or skills, so a preamble
/// input added years from now is reported by this code without being taught
/// about it.
fn render_superseded(frozen: &str, fresh: &str, budget: usize) -> String {
    let diff = TextDiff::from_lines(frozen, fresh)
        .unified_diff()
        .context_radius(2)
        .to_string();

    let mut out = String::from(
        "\n\n--- your instructions changed on disk since this session started ---\n\
         The system prompt is frozen for the session, so the version above is \
         still what it says. Treat the change below as authoritative where the \
         two disagree.\n\n",
    );
    if diff.len() <= budget {
        out.push_str(&diff);
        return out;
    }
    let mut kept = String::new();
    let mut lines = diff.lines();
    for line in lines.by_ref() {
        if kept.len() + line.len() + 1 > budget {
            out.push_str(&kept);
            out.push_str(&format!(
                "  … {} more changed line(s) not shown\n",
                lines.count() + 1
            ));
            return out;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    out.push_str(&kept);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point: a second, different preamble under the same key does
    /// not reach the model. If this ever fails, prompt caching is broken and
    /// every turn of every session repays the full conversation.
    #[test]
    fn the_first_preamble_wins_forever() {
        let freezer = PrefixFreezer::for_session();
        let first = freezer.freeze("k", "original".into());
        assert_eq!(first.as_str(), "original");

        let second = freezer.freeze("k", "something else entirely".into());
        assert_eq!(second.as_str(), "original");
    }

    /// Discarding silently would be worse than the bug it fixes: the model
    /// would be running on instructions the user believes they replaced.
    #[test]
    fn a_discarded_change_is_reported() {
        let freezer = PrefixFreezer::for_session();
        freezer.freeze("k", "be terse\n".into());
        let note = freezer
            .freeze("k", "be terse\nalso be funny\n".into())
            .superseded_note()
            .expect("supersession reported");

        assert!(note.contains("changed on disk"), "{note}");
        assert!(note.contains("+also be funny"), "{note}");
    }

    /// The common case is nothing changed, and it runs on every turn.
    #[test]
    fn an_unchanged_preamble_reports_nothing() {
        let freezer = PrefixFreezer::for_session();
        freezer.freeze("k", "same".into());
        assert!(freezer.freeze("k", "same".into()).superseded_note().is_none());
    }

    /// A handoff is a deliberate fresh start, so it gets its own frozen text
    /// rather than inheriting the previous profile's.
    #[test]
    fn a_different_profile_freezes_separately() {
        let freezer = PrefixFreezer::for_session();
        freezer.freeze(&key("c", "default"), "default persona".into());
        let other = freezer.freeze(&key("c", "reviewer"), "reviewer persona".into());
        assert_eq!(other.as_str(), "reviewer persona");
    }

    /// Two sessions in one process must not share a preamble.
    #[test]
    fn a_different_conversation_freezes_separately() {
        let freezer = PrefixFreezer::for_session();
        freezer.freeze(&key("one", "default"), "first".into());
        let second = freezer.freeze(&key("two", "default"), "second".into());
        assert_eq!(second.as_str(), "second");
    }

    /// The report rides the user turn, so it cannot be allowed to crowd out the
    /// user's own message.
    #[test]
    fn an_enormous_change_is_bounded() {
        let freezer = PrefixFreezer::for_session();
        freezer.freeze("k", "a\n".repeat(5_000));
        let note = freezer
            .freeze("k", "b\n".repeat(5_000))
            .superseded_note()
            .expect("supersession reported");

        assert!(note.contains("more changed line(s) not shown"), "truncation missing");
        assert!(note.len() < SUPERSEDED_BUDGET * 2, "budget overrun: {}", note.len());
    }
}
