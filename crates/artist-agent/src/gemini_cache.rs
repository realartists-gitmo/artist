//! Gemini explicit context caching: the preamble as a handle.
//!
//! Gemini has no conversation chaining, but it does let a prefix be uploaded
//! once and referenced by name. Applied to the system instruction that gives
//! the same two properties every other handle in this codebase gives: the bytes
//! stop being resent, and — the reason that matters more — content that is not
//! in the request cannot vary between requests. It is the protocol enforcing
//! what [`crate::prefix::PrefixFreezer`] enforces in our own code.
//!
//! Rig's `GenerateContentRequest` carries `#[serde(flatten)] additional_params`
//! and even leaves a `// cachedContent: Optional<String>` note where the field
//! would go, so the reference can be injected without vendoring the transport.
//!
//! # Failure is always inlining
//!
//! Every path here degrades to sending the preamble as usual. Gemini refuses a
//! cache below a model-dependent token floor, which means a short preamble is
//! indistinguishable from a model that does not support caching at all — and
//! both deserve the same answer, so neither is treated specially. One refusal
//! per session, remembered in [`artist_session::ProviderCapabilities`], and
//! nothing is attempted again.

use artist_session::{HandleLedger, ProviderCapabilities};
use serde_json::json;

/// How long a cached prefix is asked to live.
///
/// Long enough to cover a working session, short enough that an abandoned one
/// stops being billed. An expiry mid-session is not a problem to prevent: the
/// reference is refused, the handle is forgotten, and the next turn creates a
/// fresh cache.
const TTL_SECONDS: u64 = 3600;

/// Creates and reuses Gemini cached content for a session.
pub struct GeminiCache<'a> {
    pub http: &'a reqwest::Client,
    pub base_url: &'a str,
    pub api_key: &'a str,
    pub ledger: Option<&'a HandleLedger>,
    pub capabilities: &'a ProviderCapabilities,
}

impl GeminiCache<'_> {
    /// The cached-content name holding `system_instruction`, creating it if
    /// this is the first time it has been seen.
    ///
    /// `None` means "send the preamble inline", which is always correct.
    pub async fn ensure(
        &self,
        model: &str,
        system_instruction: &str,
        enabled: bool,
    ) -> Option<String> {
        if !enabled || system_instruction.is_empty() {
            return None;
        }
        let ledger = self.ledger?;
        // The model is part of the key, not just the content: a cache belongs
        // to the model it was created for and cannot be referenced from
        // another.
        let digest = artist_session::content_digest(
            format!("{model}\u{1f}{system_instruction}").as_bytes(),
        );
        let namespace = "gemini/cached-content";

        if let Some(name) = ledger.get(namespace, &digest) {
            return Some(name);
        }
        if !self.capabilities.may_cache_context() {
            return None;
        }
        match self.create(model, system_instruction).await {
            Some(name) => {
                let _ = ledger.put(namespace, &digest, &name);
                Some(name)
            }
            None => {
                self.capabilities.refuse_context_cache();
                None
            }
        }
    }

    /// Forget a cache the API would not accept, so the next turn rebuilds one.
    pub fn forget(&self, model: &str, system_instruction: &str) {
        if let Some(ledger) = self.ledger {
            let digest = artist_session::content_digest(
                format!("{model}\u{1f}{system_instruction}").as_bytes(),
            );
            ledger.forget("gemini/cached-content", &digest);
        }
    }

    async fn create(&self, model: &str, system_instruction: &str) -> Option<String> {
        let response = self
            .http
            .post(format!(
                "{}/cachedContents?key={}",
                self.base_url.trim_end_matches('/'),
                self.api_key
            ))
            .json(&json!({
                "model": qualified_model(model),
                "systemInstruction": {"parts": [{"text": system_instruction}]},
                "ttl": format!("{TTL_SECONDS}s"),
            }))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let wire: serde_json::Value = response.json().await.ok()?;
        wire.get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    }
}

/// Gemini names models as `models/<id>`; a bare id is rejected.
fn qualified_model(model: &str) -> String {
    if model.starts_with("models/") {
        model.to_owned()
    } else {
        format!("models/{model}")
    }
}

/// The request field that references a cached prefix.
///
/// Gemini rejects a request carrying both a cache and a system instruction, so
/// a caller that sets this must also stop sending the preamble — which is the
/// entire point.
pub fn reference(name: &str) -> serde_json::Value {
    json!({"cachedContent": name})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_model_id_is_qualified() {
        assert_eq!(qualified_model("gemini-2.5-pro"), "models/gemini-2.5-pro");
    }

    /// Already-qualified names must not be doubled up.
    #[test]
    fn an_already_qualified_model_is_left_alone() {
        assert_eq!(
            qualified_model("models/gemini-2.5-pro"),
            "models/gemini-2.5-pro"
        );
    }

    /// The shape rig flattens into `GenerateContentRequest`.
    #[test]
    fn the_reference_is_the_field_gemini_expects() {
        assert_eq!(
            reference("cachedContents/abc"),
            json!({"cachedContent": "cachedContents/abc"})
        );
    }

    /// Disabled by default: no request is made and the preamble goes inline.
    #[tokio::test]
    async fn caching_is_off_unless_asked_for() {
        let http = reqwest::Client::new();
        let capabilities = ProviderCapabilities::for_session();
        let dir = tempfile::tempdir().unwrap();
        let ledger = HandleLedger::new(dir.path());
        let cache = GeminiCache {
            http: &http,
            // Unreachable on purpose: if this is contacted the gate has failed.
            base_url: "http://127.0.0.1:1",
            api_key: "k",
            ledger: Some(&ledger),
            capabilities: &capabilities,
        };
        assert!(
            cache
                .ensure("gemini-2.5-pro", "be helpful", false)
                .await
                .is_none()
        );
        assert!(
            capabilities.may_cache_context(),
            "a disabled feature must not record a refusal"
        );
    }
}
