//! Which provider-side handles a session is permitted to use.
//!
//! Each of these replaces something artist would otherwise restate on every
//! request — the conversation, the system prompt — with a reference to
//! something the provider is already holding. They share a shape: a durable,
//! billed artifact on the provider's side, created on the user's account, in
//! exchange for not resending content that cannot then drift between requests.
//!
//! That trade is the user's to make, so every one of these is off by default.
//!
//! # Why this is configuration and not an environment variable
//!
//! These began as `std::env::var` reads, which was wrong in a way worth
//! recording. A process-global switch is invisible to the type system, cannot
//! be scoped to a project, and — the part that actually bit — makes test
//! isolation a discipline rather than a property. One test setting a flag
//! silently changed the behaviour of another running beside it, and the
//! resulting failure pointed at an assertion three files away from the cause.
//!
//! Passing the decision in makes each of those impossible: a caller that did
//! not ask cannot be affected by a caller that did.

/// Resolved statefulness policy for one session.
///
/// `Default` is every handle disabled, which is exactly today's behaviour:
/// content is restated on each request and nothing durable is created.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Statefulness {
    /// Continue a provider-held conversation with `previous_response_id`
    /// instead of restating history.
    ///
    /// Needs an endpoint that retains responses; the ChatGPT/Codex backend
    /// does not. Getting the accompanying input trimming wrong shows the model
    /// a conversation that never happened, which is why this is not a default
    /// even where it works.
    pub chaining: bool,
    /// Reference the system prompt by id rather than sending its text.
    pub stored_prompt: bool,
    /// Hold the system prompt in a Gemini explicit context cache.
    pub gemini_cache: bool,
}

impl Statefulness {
    /// Every handle enabled. For tests and for a caller that has verified all
    /// three against a live endpoint.
    pub fn all() -> Self {
        Self {
            chaining: true,
            stored_prompt: true,
            gemini_cache: true,
        }
    }

    pub fn with_chaining(mut self, enabled: bool) -> Self {
        self.chaining = enabled;
        self
    }

    pub fn with_stored_prompt(mut self, enabled: bool) -> Self {
        self.stored_prompt = enabled;
        self
    }

    pub fn with_gemini_cache(mut self, enabled: bool) -> Self {
        self.gemini_cache = enabled;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default must be the behaviour artist had before any of this existed:
    /// nothing referenced, nothing created on the user's account.
    #[test]
    fn nothing_is_enabled_by_default() {
        let policy = Statefulness::default();
        assert!(!policy.chaining);
        assert!(!policy.stored_prompt);
        assert!(!policy.gemini_cache);
    }

    /// Enabling one must not enable another — they need different endpoints and
    /// carry different risks.
    #[test]
    fn each_handle_is_independent() {
        let policy = Statefulness::default().with_chaining(true);
        assert!(policy.chaining);
        assert!(!policy.stored_prompt);
        assert!(!policy.gemini_cache);
    }

    #[test]
    fn all_enables_everything() {
        let policy = Statefulness::all();
        assert!(policy.chaining && policy.stored_prompt && policy.gemini_cache);
    }
}
