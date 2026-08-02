//! When a provider-side conversation chain may be used, and what kills it.
//!
//! A stateless completion API resends the whole conversation every turn. On a
//! real 48-turn session in this repository that came to 4,295,905 bytes uploaded
//! for a final context state of 198,432 — 95.4% of the traffic was a resend of
//! something the provider had already been given, and that session contained no
//! images at all.
//!
//! Providers that keep the conversation server-side let a request name its
//! predecessor instead of restating it. Beyond the bandwidth, this is the
//! strongest available form of the rule that a cached prefix must not move:
//! content that is not in the request cannot vary between requests. What is not
//! sent cannot drift.
//!
//! # The chain is a cache, never the source of truth
//!
//! Artist does not accumulate history — it rewrites it. A stream rule aborts a
//! run mid-flight and replays from an earlier point with a reminder spliced in;
//! compaction replaces a hundred thousand tokens with a summary; a handoff is
//! functionally `/clear`. A server-side chain is append-only with fork, and
//! expresses none of those.
//!
//! So the chain is held optimistically over the local event log, which stays
//! authoritative. Anything that rewrites history drops the chain and the next
//! request goes up in full. That keeps replay exact, keeps the log the only
//! thing that has to be right, and makes the optimisation safe to abandon at
//! any moment — including permanently, if a provider withdraws it.
//!
//! Erring toward [`Send::Full`] costs bandwidth. Erring toward [`Send::Chained`]
//! sends the model a conversation that never happened. The asymmetry is the
//! reason every uncertain case here resolves to `Full`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Why a chain was dropped. Carried for diagnosis: a chain that never survives
/// two turns is a bug, and without a reason it is an invisible one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Broke {
    /// A stream rule fired and the run is replaying from an earlier point.
    RuleReplay,
    /// History was replaced by a summary.
    Compacted,
    /// The session changed profile, which resets the conversation.
    HandedOff,
    /// A turn ended without a usable response id — cancelled, failed, or
    /// interrupted mid-stream.
    TurnIncomplete,
    /// The provider rejected the chain: expired, unknown, or unsupported.
    ProviderRefused,
    /// Something else edited the history the log holds.
    HistoryRewritten,
}

/// What the next request should carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Send {
    /// Restate the whole conversation. Always correct.
    Full,
    /// Reference the previous response and send only what is new.
    Chained { previous_response_id: String },
}

impl Send {
    pub fn is_chained(&self) -> bool {
        matches!(self, Send::Chained { .. })
    }
}

/// Per-conversation chain positions, shared across the turns of a session.
///
/// Keyed by provider namespace as well as conversation because a response id
/// means nothing to an account that did not issue it — the same reason
/// [`crate::HandleLedger`] is namespaced.
///
/// Deliberately in memory only. A resumed session starts cold: response ids
/// expire on the provider's schedule, and one cheap full request beats
/// resurrecting a chain that may no longer exist.
#[derive(Clone, Default)]
pub struct ChainState(Arc<Mutex<HashMap<String, Entry>>>);

#[derive(Clone, Debug)]
struct Entry {
    response_id: String,
    /// Turns this chain has survived. Only ever grows while chaining holds.
    depth: u32,
}

impl ChainState {
    pub fn new() -> Self {
        Self::default()
    }

    /// What the next request under this key should carry.
    ///
    /// `supported` is the provider capability answer; a provider that cannot
    /// chain always gets [`Send::Full`] without needing its own bookkeeping.
    pub fn plan(&self, key: &str, supported: bool) -> Send {
        if !supported {
            return Send::Full;
        }
        match self.entry(key) {
            Some(entry) => Send::Chained {
                previous_response_id: entry.response_id,
            },
            None => Send::Full,
        }
    }

    /// A turn completed cleanly and the provider named the response.
    ///
    /// An empty id is treated as no id: chaining from nothing would send the
    /// next request with a reference the provider cannot resolve.
    pub fn advance(&self, key: &str, response_id: &str) {
        if response_id.is_empty() {
            self.invalidate(key, Broke::TurnIncomplete);
            return;
        }
        let mut held = self.lock();
        let depth = held.get(key).map(|entry| entry.depth + 1).unwrap_or(0);
        held.insert(
            key.to_owned(),
            Entry {
                response_id: response_id.to_owned(),
                depth,
            },
        );
    }

    /// Drop the chain. The next request restates everything.
    ///
    /// Idempotent, and safe to call on a key that never had a chain — callers
    /// invalidate from error paths where they cannot know.
    pub fn invalidate(&self, key: &str, _reason: Broke) {
        self.lock().remove(key);
    }

    /// Turns the current chain has survived, or `None` when cold. For tests and
    /// diagnostics.
    pub fn depth(&self, key: &str) -> Option<u32> {
        self.entry(key).map(|entry| entry.depth)
    }

    fn entry(&self, key: &str) -> Option<Entry> {
        self.lock().get(key).cloned()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        // A poisoned lock means a turn panicked while holding it. Losing the
        // chain is the safe direction, and the map is otherwise sound.
        match self.0.lock() {
            Ok(held) => held,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// The key a chain is tracked under.
pub fn key(conversation: &str, namespace: &str) -> String {
    format!("{conversation}\u{1f}{namespace}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "conv\u{1f}openai";

    /// Cold start restates everything. There is nothing to chain from.
    #[test]
    fn a_cold_chain_sends_everything() {
        assert_eq!(ChainState::new().plan(KEY, true), Send::Full);
    }

    #[test]
    fn a_completed_turn_chains_the_next_one() {
        let chain = ChainState::new();
        chain.advance(KEY, "resp_1");
        assert_eq!(
            chain.plan(KEY, true),
            Send::Chained {
                previous_response_id: "resp_1".into()
            }
        );
    }

    /// A provider without chaining needs no special handling anywhere else.
    #[test]
    fn an_unsupporting_provider_never_chains() {
        let chain = ChainState::new();
        chain.advance(KEY, "resp_1");
        assert_eq!(chain.plan(KEY, false), Send::Full);
    }

    /// The core safety property. Every one of these rewrites history, and a
    /// chain that survived any of them would present the model a conversation
    /// that did not happen.
    #[test]
    fn every_history_rewrite_drops_the_chain() {
        for reason in [
            Broke::RuleReplay,
            Broke::Compacted,
            Broke::HandedOff,
            Broke::TurnIncomplete,
            Broke::ProviderRefused,
            Broke::HistoryRewritten,
        ] {
            let chain = ChainState::new();
            chain.advance(KEY, "resp_1");
            assert!(chain.plan(KEY, true).is_chained(), "{reason:?} setup");

            chain.invalidate(KEY, reason);
            assert_eq!(chain.plan(KEY, true), Send::Full, "survived {reason:?}");
        }
    }

    /// A turn that produced no response id cannot be chained from — the
    /// cancelled and provider-error paths both land here.
    #[test]
    fn a_turn_without_a_response_id_breaks_the_chain() {
        let chain = ChainState::new();
        chain.advance(KEY, "resp_1");
        chain.advance(KEY, "");
        assert_eq!(chain.plan(KEY, true), Send::Full);
    }

    /// Recovery: a dropped chain rebuilds from the next clean turn rather than
    /// staying cold for the rest of the session.
    #[test]
    fn a_broken_chain_recovers_on_the_next_clean_turn() {
        let chain = ChainState::new();
        chain.advance(KEY, "resp_1");
        chain.invalidate(KEY, Broke::RuleReplay);
        chain.advance(KEY, "resp_2");
        assert_eq!(
            chain.plan(KEY, true),
            Send::Chained {
                previous_response_id: "resp_2".into()
            }
        );
        assert_eq!(chain.depth(KEY), Some(0), "depth restarts after a break");
    }

    /// Error paths invalidate without knowing whether a chain existed.
    #[test]
    fn invalidating_a_cold_chain_is_harmless() {
        let chain = ChainState::new();
        chain.invalidate(KEY, Broke::ProviderRefused);
        chain.invalidate(KEY, Broke::ProviderRefused);
        assert_eq!(chain.plan(KEY, true), Send::Full);
    }

    /// A response id from one account means nothing to another.
    #[test]
    fn chains_do_not_leak_across_namespaces() {
        let chain = ChainState::new();
        chain.advance(&key("conv", "openai:one"), "resp_1");
        assert_eq!(chain.plan(&key("conv", "openai:two"), true), Send::Full);
        assert_eq!(chain.plan(&key("other", "openai:one"), true), Send::Full);
    }

    /// The chain always points at the most recent turn, not the first.
    #[test]
    fn the_chain_follows_the_latest_response() {
        let chain = ChainState::new();
        chain.advance(KEY, "resp_1");
        chain.advance(KEY, "resp_2");
        chain.advance(KEY, "resp_3");
        assert_eq!(
            chain.plan(KEY, true),
            Send::Chained {
                previous_response_id: "resp_3".into()
            }
        );
        assert_eq!(chain.depth(KEY), Some(2));
    }

    /// Turns run through cloned handles; they must share one chain.
    #[test]
    fn clones_share_one_chain() {
        let chain = ChainState::new();
        let turn = chain.clone();
        turn.advance(KEY, "resp_1");
        assert!(chain.plan(KEY, true).is_chained());
    }
}
