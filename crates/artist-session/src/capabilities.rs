//! What this endpoint turned out not to support.
//!
//! Statefulness is negotiated by trying: there is no discovery call that says
//! whether a backend keeps files, stores prompts, or retains responses. So each
//! is attempted once and, if refused, not attempted again.
//!
//! "Again" has to mean *for the session*, which is why this lives here rather
//! than on the client. A provider client is rebuilt for every user turn, so
//! flags held on one are forgotten between turns — an endpoint without a files
//! endpoint would be probed once per turn forever, and a conversation chain
//! would never survive long enough to save anything. A test caught exactly that
//! by counting probes across two turns and finding two.
//!
//! Every flag is one-way. Nothing here ever moves back to supported: a capability
//! that returns mid-session is not worth the request it would cost to discover,
//! and a new session finds out anyway.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Per-session record of refused capabilities, shared across every turn.
#[derive(Clone, Debug, Default)]
pub struct ProviderCapabilities {
    uploads_refused: Arc<AtomicBool>,
    prompts_refused: Arc<AtomicBool>,
    context_cache_refused: Arc<AtomicBool>,
}

impl ProviderCapabilities {
    pub fn for_session() -> Self {
        Self::default()
    }

    /// Whether the files endpoint is worth trying.
    pub fn may_upload(&self) -> bool {
        !self.uploads_refused.load(Ordering::Relaxed)
    }

    pub fn refuse_uploads(&self) {
        self.uploads_refused.store(true, Ordering::Relaxed);
    }

    /// Whether storing the system prompt is worth trying.
    pub fn may_store_prompts(&self) -> bool {
        !self.prompts_refused.load(Ordering::Relaxed)
    }

    pub fn refuse_prompts(&self) {
        self.prompts_refused.store(true, Ordering::Relaxed);
    }

    /// Whether an explicit context cache is worth creating.
    ///
    /// Gemini rejects a cache below a model-dependent token floor, so a small
    /// preamble refuses here as surely as a backend that has no such feature.
    /// Both warrant the same response, which is why they are not distinguished.
    pub fn may_cache_context(&self) -> bool {
        !self.context_cache_refused.load(Ordering::Relaxed)
    }

    pub fn refuse_context_cache(&self) {
        self.context_cache_refused.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_is_worth_trying_once() {
        let capabilities = ProviderCapabilities::for_session();
        assert!(capabilities.may_upload());
        assert!(capabilities.may_store_prompts());
        assert!(capabilities.may_cache_context());
    }

    #[test]
    fn a_refusal_is_remembered() {
        let capabilities = ProviderCapabilities::for_session();
        capabilities.refuse_uploads();
        assert!(!capabilities.may_upload());
    }

    /// Refusing one capability must not disable the others: a backend can hold
    /// files and not prompts.
    #[test]
    fn refusals_are_independent() {
        let capabilities = ProviderCapabilities::for_session();
        capabilities.refuse_uploads();
        assert!(capabilities.may_store_prompts());

        assert!(capabilities.may_cache_context());

        let other = ProviderCapabilities::for_session();
        other.refuse_prompts();
        assert!(other.may_upload());

        let third = ProviderCapabilities::for_session();
        third.refuse_context_cache();
        assert!(third.may_upload() && third.may_store_prompts());
    }

    /// The reason this type exists: a client rebuilt for the next turn must see
    /// the previous turn's refusal, or it probes a missing endpoint forever.
    #[test]
    fn a_refusal_survives_into_a_cloned_handle() {
        let session = ProviderCapabilities::for_session();
        let first_turn = session.clone();
        first_turn.refuse_uploads();

        let second_turn = session.clone();
        assert!(
            !second_turn.may_upload(),
            "a later turn re-probed an endpoint already known to refuse"
        );
    }
}
