use std::{path::Path, time::Duration};

use llm_provider::ProviderKind;

const MAX_OVERLOAD_RETRIES: u32 = 5;

/// State for retrying a provider overload before the failed attempt has emitted
/// anything observable. This is deliberately separate from stream-rule retries.
pub(crate) struct OverloadRetry {
    base_cache_key: String,
    active_cache_key: String,
    retries: u32,
}

impl OverloadRetry {
    pub(crate) fn new(project: &Path, model: &str, lineage: &str) -> Self {
        let base_cache_key = prompt_cache_key(project, model, lineage);
        Self {
            active_cache_key: base_cache_key.clone(),
            base_cache_key,
            retries: 0,
        }
    }

    pub(crate) fn cache_key(&self) -> &str {
        &self.active_cache_key
    }

    /// Returns a jittered exponential delay and rotates cache affinity.
    pub(crate) fn schedule(&mut self) -> Option<Duration> {
        if self.retries >= MAX_OVERLOAD_RETRIES {
            return None;
        }
        let floor_ms = 500u64 << self.retries;
        let jitter = u64::from_le_bytes(
            uuid::Uuid::new_v4().as_bytes()[..8]
                .try_into()
                .expect("UUID contains eight bytes"),
        ) % floor_ms;
        self.retries += 1;
        self.active_cache_key = format!(
            "{}-r-{}",
            self.base_cache_key,
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        Some(Duration::from_millis(floor_ms + jitter))
    }
}

/// Restrict retries to the provider's explicit transient-overload response.
/// Rig does not currently expose the nested provider code as a stable typed
/// value, so match the serialized response rather than generic error prose.
pub(crate) fn is_overload(provider: ProviderKind, error: &impl std::fmt::Display) -> bool {
    if provider != ProviderKind::Chatgpt {
        return false;
    }
    let message = error.to_string().to_ascii_lowercase();
    message.contains("server_is_overloaded")
        || message.contains("our servers are currently overloaded")
}

fn prompt_cache_key(project: &Path, model: &str, lineage: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    project.hash(&mut hasher);
    model.hash(&mut hasher);
    lineage.hash(&mut hasher);
    format!("artist-{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_affinity_is_per_lineage() {
        let project = Path::new("/tmp/project");
        assert_eq!(
            prompt_cache_key(project, "model", "main:one"),
            prompt_cache_key(project, "model", "main:one")
        );
        assert_ne!(
            prompt_cache_key(project, "model", "main:one"),
            prompt_cache_key(project, "model", "main:two")
        );
    }

    #[test]
    fn overload_budget_is_bounded_and_rotates_affinity() {
        let mut retry = OverloadRetry::new(Path::new("/tmp/project"), "model", "main:one");
        let original = retry.cache_key().to_owned();
        for attempt in 0..MAX_OVERLOAD_RETRIES {
            let delay = retry.schedule().unwrap();
            let floor = 500u64 << attempt;
            assert!(delay >= Duration::from_millis(floor));
            assert!(delay < Duration::from_millis(floor * 2));
            assert_ne!(retry.cache_key(), original);
        }
        assert!(retry.schedule().is_none());
    }

    #[test]
    fn only_chatgpt_explicit_overload_is_retryable() {
        assert!(is_overload(
            ProviderKind::Chatgpt,
            &"response code: server_is_overloaded"
        ));
        assert!(!is_overload(
            ProviderKind::Openai,
            &"response code: server_is_overloaded"
        ));
        assert!(is_overload(
            ProviderKind::Chatgpt,
            &"CompletionError: ProviderError: Our servers are currently overloaded. Please try again later."
        ));
        assert!(!is_overload(ProviderKind::Chatgpt, &"service unavailable"));
    }
}
