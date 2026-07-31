//! Candidate fallback: classification, health tracking, and the marker that
//! carries "this failure is worth trying elsewhere" out of a run.
//!
//! Only genuine availability failures advance to the next candidate. Malformed
//! requests, schema errors, and context-length overruns reproduce identically
//! on every candidate, so failing over on them burns the whole list and reports
//! the last error instead of the real one.

use dashmap::DashMap;
use rig_core::{
    agent::StreamingError,
    completion::{CompletionError, PromptError},
};
use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

/// Consecutive failures before a candidate is skipped.
const STRIKES: u32 = 3;
/// How long a tripped candidate stays skipped before it is tried again in its
/// normal priority position.
const COOLDOWN: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// An availability problem: authentication, payment, rate limiting, server
    /// errors, transport. Worth trying the next candidate.
    Unavailable,
    /// Reproduces identically on every candidate. Fail the run instead.
    Permanent,
}

/// Marker attached to a run error when the failure is worth retrying on
/// another candidate. The fallback loop downcasts to this rather than matching
/// on error strings.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct Unavailable(pub String);

pub(crate) fn classify(error: &StreamingError) -> Failure {
    match error {
        // A tool blew up inside our own loop; the provider is fine.
        StreamingError::Tool(_) => Failure::Permanent,
        StreamingError::Completion(error) => classify_completion(error),
        StreamingError::Prompt(error) => match error.as_ref() {
            PromptError::CompletionError(error) => classify_completion(error),
            // Turn limits, cancellations (including TTSR aborts), and tool
            // failures are all our own control flow, not provider health.
            _ => Failure::Permanent,
        },
    }
}

fn classify_completion(error: &CompletionError) -> Failure {
    if let Some(status) = error.provider_response_status() {
        // A 2xx here means the provider returned an error envelope alongside a
        // success status. Without parsing per-provider bodies we cannot tell a
        // rate limit from a bad request, so we do not burn the candidate list
        // on it.
        if status.is_success() {
            return Failure::Permanent;
        }
        return if status.is_server_error()
            || matches!(status.as_u16(), 401 | 402 | 403 | 408 | 409 | 425 | 429)
        {
            Failure::Unavailable
        } else {
            Failure::Permanent
        };
    }
    match error {
        // No status recovered: an HTTP-layer failure or a Rig-generated
        // transport diagnostic. Both are availability problems.
        CompletionError::HttpError(_) | CompletionError::ProviderError(_) => Failure::Unavailable,
        // Serialization, URL, request-construction and response-parse failures
        // are ours and will recur everywhere.
        _ => Failure::Permanent,
    }
}

/// Which candidate a health record belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CandidateKey {
    pub provider: String,
    pub model: String,
}

#[derive(Default)]
struct Health {
    consecutive: u32,
    tripped_at: Option<Instant>,
}

/// Per-process candidate health.
///
/// Deliberately in memory: a stale on-disk record outliving the outage it
/// describes is worse than re-learning the outage once per launch.
pub(crate) struct Breaker {
    health: DashMap<CandidateKey, Health>,
}

impl Breaker {
    fn new() -> Self {
        Self {
            health: DashMap::new(),
        }
    }

    pub fn global() -> &'static Breaker {
        static BREAKER: OnceLock<Breaker> = OnceLock::new();
        BREAKER.get_or_init(Breaker::new)
    }

    fn is_tripped_at(&self, key: &CandidateKey, now: Instant) -> bool {
        self.health
            .get(key)
            .and_then(|health| health.tripped_at)
            .is_some_and(|tripped| now.duration_since(tripped) < COOLDOWN)
    }

    pub fn is_tripped(&self, key: &CandidateKey) -> bool {
        self.is_tripped_at(key, Instant::now())
    }

    /// Record a failure. Returns whether the candidate is now tripped.
    pub fn record_failure(&self, key: &CandidateKey) -> bool {
        let mut health = self.health.entry(key.clone()).or_default();
        health.consecutive += 1;
        if health.consecutive >= STRIKES {
            health.tripped_at = Some(Instant::now());
            return true;
        }
        false
    }

    /// Any success clears the strike count — the counter is consecutive
    /// failures, not lifetime failures.
    pub fn record_success(&self, key: &CandidateKey) {
        self.health.remove(key);
    }

}

/// Why a candidate was not attempted, for the exhaustion report.
pub(crate) struct Skipped {
    pub candidate: String,
    pub reason: String,
}

/// How a candidate is named in diagnostics and downgrade notices.
pub(crate) fn candidate_label(candidate: &crate::profiles::Candidate) -> String {
    match (candidate.provider.as_deref(), candidate.model.as_deref()) {
        (Some(provider), Some(model)) => format!("{provider}/{model}"),
        (Some(provider), None) => provider.to_owned(),
        (None, Some(model)) => model.to_owned(),
        (None, None) => "session account".to_owned(),
    }
}

/// Build the error for a fully exhausted candidate list. The reported cause is
/// the most recent genuine provider failure, never the last skip reason, and
/// every candidate that was tried or skipped is named.
pub(crate) fn exhausted(profile: &str, last_error: Option<String>, skipped: &[Skipped]) -> String {
    let mut message = format!("profile {profile}: every routing candidate failed or was skipped");
    if let Some(error) = last_error {
        message.push_str(&format!("\n  last provider failure: {error}"));
    }
    for entry in skipped {
        message.push_str(&format!("\n  {}: {}", entry.candidate, entry.reason));
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::StatusCode;

    fn completion(status: StatusCode) -> StreamingError {
        StreamingError::Completion(CompletionError::from_http_response(status, "{}"))
    }

    #[test]
    fn availability_failures_advance_to_the_next_candidate() {
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::PAYMENT_REQUIRED,
            StatusCode::FORBIDDEN,
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert_eq!(
                classify(&completion(status)),
                Failure::Unavailable,
                "{status} should fail over"
            );
        }
    }

    /// These reproduce on every candidate. Failing over on them would burn the
    /// whole list and surface the last error rather than the real one.
    #[test]
    fn request_level_failures_do_not_burn_the_candidate_list() {
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::NOT_FOUND,
            StatusCode::PAYLOAD_TOO_LARGE,
            StatusCode::UNPROCESSABLE_ENTITY,
        ] {
            assert_eq!(
                classify(&completion(status)),
                Failure::Permanent,
                "{status} should not fail over"
            );
        }
    }

    /// Some providers return an error envelope with a success status; we cannot
    /// tell a rate limit from a bad request without parsing per-provider bodies.
    #[test]
    fn a_success_status_carrying_an_error_envelope_is_not_a_failover() {
        assert_eq!(classify(&completion(StatusCode::OK)), Failure::Permanent);
    }

    #[test]
    fn transport_diagnostics_without_a_status_fail_over() {
        let error = StreamingError::Completion(CompletionError::ProviderError(
            "connection reset by peer".into(),
        ));
        assert_eq!(classify(&error), Failure::Unavailable);
    }

    /// TTSR aborts the stream by cancelling the prompt. That is our own control
    /// flow and must never be read as the provider being unhealthy.
    #[test]
    fn a_cancelled_prompt_is_never_a_provider_failure() {
        let error = StreamingError::Prompt(Box::new(PromptError::PromptCancelled {
            chat_history: Vec::new(),
            reason: "stream rule".into(),
        }));
        assert_eq!(classify(&error), Failure::Permanent);
    }

    #[test]
    fn our_own_serialization_failures_are_not_failovers() {
        let json = serde_json::from_str::<serde_json::Value>("{ not json").unwrap_err();
        let error = StreamingError::Completion(CompletionError::JsonError(json));
        assert_eq!(classify(&error), Failure::Permanent);
    }

    fn key(model: &str) -> CandidateKey {
        CandidateKey {
            provider: "acct".into(),
            model: model.into(),
        }
    }

    #[test]
    fn a_candidate_trips_only_after_consecutive_strikes() {
        let breaker = Breaker::new();
        let key = key("trips-after-three");
        assert!(!breaker.record_failure(&key));
        assert!(!breaker.record_failure(&key));
        assert!(!breaker.is_tripped(&key));
        assert!(breaker.record_failure(&key));
        assert!(breaker.is_tripped(&key));
    }

    #[test]
    fn a_success_clears_the_strike_count() {
        let breaker = Breaker::new();
        let key = key("clears-on-success");
        breaker.record_failure(&key);
        breaker.record_failure(&key);
        breaker.record_success(&key);
        assert!(!breaker.record_failure(&key), "count should have restarted");
        assert!(!breaker.is_tripped(&key));
    }

    #[test]
    fn a_tripped_candidate_returns_after_its_cooldown() {
        let breaker = Breaker::new();
        let key = key("returns-after-cooldown");
        for _ in 0..STRIKES {
            breaker.record_failure(&key);
        }
        assert!(breaker.is_tripped(&key));
        let later = Instant::now() + COOLDOWN + Duration::from_secs(1);
        assert!(!breaker.is_tripped_at(&key, later));
    }

    #[test]
    fn exhaustion_reports_the_provider_failure_not_the_last_skip() {
        let message = exhausted(
            "resilient",
            Some("status 429: rate limited".into()),
            &[Skipped {
                candidate: "ollama-local/qwen3-coder".into(),
                reason: "no model configured".into(),
            }],
        );
        assert!(message.contains("rate limited"), "{message}");
        assert!(message.contains("ollama-local/qwen3-coder"), "{message}");
        assert!(message.contains("no model configured"), "{message}");
    }
}
