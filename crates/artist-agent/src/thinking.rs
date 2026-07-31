//! Translation from a profile's normalized thinking configuration into
//! provider request parameters.
//!
//! Providers spell reasoning differently and constrain the combination
//! differently, so validity is a function of (provider, mode, level) together
//! rather than of each field independently. This is the single place that
//! knows the differences.

use crate::profiles::{Thinking, ThinkingLevel, ThinkingMode};
use llm_provider::ProviderKind;
use serde_json::{Value, json};

/// Build the provider-specific request parameters for a run.
///
/// Returns `None` for providers with no reasoning controls and no cache key to
/// set — sending a reasoning block to a provider that does not accept one is a
/// request error, so silence is the correct output rather than a best guess.
pub(crate) fn request_params(
    provider: ProviderKind,
    cache_key: &str,
    thinking: Thinking,
) -> Option<Value> {
    match provider {
        ProviderKind::Chatgpt => {
            let mut params = json!({ "prompt_cache_key": cache_key });
            // Request a provider-generated trace for the live UI even when the
            // model's default effort is in use. Rig's memory policy is
            // independent: streaming this summary does not make the CLI
            // responsible for model context.
            //
            // The Codex backend always reasons, so `mode: off` cannot disable
            // it here; it only means "do not ask for a specific effort".
            params["reasoning"] = match (thinking.mode, thinking.level) {
                (ThinkingMode::On, Some(level)) => {
                    json!({ "effort": level.as_str(), "summary": "auto" })
                }
                _ => json!({ "summary": "auto" }),
            };
            Some(params)
        }
        ProviderKind::Anthropic => {
            let mut params = json!({});
            match thinking.mode {
                ThinkingMode::On => {
                    params["thinking"] = json!({ "type": "adaptive" });
                    if let Some(level) = thinking.level {
                        params["output_config"] = json!({ "effort": level.as_str() });
                    }
                }
                ThinkingMode::Off => {
                    params["thinking"] = json!({ "type": "disabled" });
                    // Disabled thinking is only accepted at `high` or below on
                    // current Anthropic models; pairing it with `xhigh`/`max`
                    // is rejected outright. Drop the level rather than send a
                    // request we know the provider will refuse.
                    if let Some(level) =
                        thinking.level.filter(|level| *level <= ThinkingLevel::High)
                    {
                        params["output_config"] = json!({ "effort": level.as_str() });
                    }
                }
            }
            Some(params)
        }
        // Everything else either has no reasoning controls or spells them in a
        // way that errors when sent to a non-reasoning model. A provider
        // without an equivalent concept ignores the level.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thinking(mode: ThinkingMode, level: Option<ThinkingLevel>) -> Thinking {
        Thinking { mode, level }
    }

    #[test]
    fn chatgpt_always_carries_the_cache_key_and_a_summary() {
        let params = request_params(
            ProviderKind::Chatgpt,
            "artist-1",
            thinking(ThinkingMode::On, None),
        )
        .unwrap();
        assert_eq!(params["prompt_cache_key"], "artist-1");
        assert_eq!(params["reasoning"]["summary"], "auto");
        assert!(params["reasoning"]["effort"].is_null());
    }

    #[test]
    fn chatgpt_sends_the_level_as_effort() {
        let params = request_params(
            ProviderKind::Chatgpt,
            "artist-1",
            thinking(ThinkingMode::On, Some(ThinkingLevel::Xhigh)),
        )
        .unwrap();
        assert_eq!(params["reasoning"]["effort"], "xhigh");
    }

    #[test]
    fn anthropic_maps_mode_and_level_onto_separate_fields() {
        let params = request_params(
            ProviderKind::Anthropic,
            "artist-1",
            thinking(ThinkingMode::On, Some(ThinkingLevel::High)),
        )
        .unwrap();
        assert_eq!(params["thinking"]["type"], "adaptive");
        assert_eq!(params["output_config"]["effort"], "high");
        // The cache key is an OpenAI-shaped parameter; Anthropic caches via
        // explicit breakpoints, so it must not leak into the request.
        assert!(params["prompt_cache_key"].is_null());
    }

    /// Disabling thinking above `high` effort is a 400 on current Anthropic
    /// models. The level is dropped rather than sent.
    #[test]
    fn anthropic_drops_a_level_that_cannot_pair_with_disabled_thinking() {
        for level in [ThinkingLevel::Xhigh, ThinkingLevel::Max] {
            let params = request_params(
                ProviderKind::Anthropic,
                "artist-1",
                thinking(ThinkingMode::Off, Some(level)),
            )
            .unwrap();
            assert_eq!(params["thinking"]["type"], "disabled");
            assert!(
                params["output_config"].is_null(),
                "{level:?} must not be sent with disabled thinking"
            );
        }
    }

    #[test]
    fn anthropic_keeps_a_level_that_can_pair_with_disabled_thinking() {
        let params = request_params(
            ProviderKind::Anthropic,
            "artist-1",
            thinking(ThinkingMode::Off, Some(ThinkingLevel::Medium)),
        )
        .unwrap();
        assert_eq!(params["thinking"]["type"], "disabled");
        assert_eq!(params["output_config"]["effort"], "medium");
    }

    #[test]
    fn providers_without_reasoning_controls_get_nothing() {
        for provider in [
            ProviderKind::Ollama,
            ProviderKind::Openai,
            ProviderKind::Groq,
            ProviderKind::Llamafile,
        ] {
            assert!(
                request_params(
                    provider,
                    "artist-1",
                    thinking(ThinkingMode::On, Some(ThinkingLevel::Max))
                )
                .is_none(),
                "{provider:?} should receive no reasoning parameters"
            );
        }
    }
}
