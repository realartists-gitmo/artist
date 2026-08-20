//! Small, dependency-light observability primitives for daemon integrations.
//!
//! Metrics are transport-neutral. Redaction is opt-in and is for
//! diagnostics/projections; authoritative event/resource payloads remain
//! unchanged unless the caller explicitly applies it.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use serde_json::Value;

#[derive(Default)]
pub struct AgentMetrics {
    turns_started: AtomicU64,
    turns_completed: AtomicU64,
    tool_calls: AtomicU64,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct MetricsSnapshot {
    pub turns_started: u64,
    pub turns_completed: u64,
    pub tool_calls: u64,
}

impl AgentMetrics {
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            turns_started: self.turns_started.load(Ordering::Relaxed),
            turns_completed: self.turns_completed.load(Ordering::Relaxed),
            tool_calls: self.tool_calls.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn turn_started(&self) {
        self.turns_started.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn turn_completed(&self) {
        self.turns_completed.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn tool_call(&self) {
        self.tool_calls.fetch_add(1, Ordering::Relaxed);
    }
}

const SECRET_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "authorization",
    "credential",
    "password",
    "secret",
    "token",
];

/// Replace values under conventional secret-bearing object keys in-place.
pub fn redact_json(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object.iter_mut() {
                let normalized = key.to_ascii_lowercase().replace('-', "_");
                if SECRET_KEYS.contains(&normalized.as_str()) {
                    *value = Value::String("[REDACTED]".into());
                } else {
                    redact_json(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(redact_json),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_is_recursive_and_does_not_touch_unrelated_fields() {
        let mut value = serde_json::json!({
            "Authorization": "Bearer x",
            "nested": [{"api-key": "abc", "label": "keep"}]
        });
        redact_json(&mut value);
        assert_eq!(value["Authorization"], "[REDACTED]");
        assert_eq!(value["nested"][0]["api-key"], "[REDACTED]");
        assert_eq!(value["nested"][0]["label"], "keep");
    }

    #[test]
    fn metrics_snapshot_is_stable_and_serializable() {
        let metrics = AgentMetrics::default();
        metrics.turn_started();
        metrics.tool_call();
        metrics.turn_completed();
        assert_eq!(
            metrics.snapshot(),
            MetricsSnapshot {
                turns_started: 1,
                turns_completed: 1,
                tool_calls: 1
            }
        );
        assert!(serde_json::to_value(metrics.snapshot()).is_ok());
    }
}
