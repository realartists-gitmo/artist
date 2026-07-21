//! The on-disk event schema: envelope, event kinds, and content blocks.
//!
//! Every line of `events.jsonl` is one [`Envelope`]. The envelope is frozen;
//! only payloads version (via `v`). Unknown kinds and unknown payload fields
//! are tolerated on read so an older binary can still open a session touched
//! by a newer one.

use serde::{Deserialize, Serialize};

/// Current payload schema version written by this binary.
pub const SCHEMA_VERSION: u32 = 1;

/// Lineage of the main agent.
pub const MAIN_LINEAGE: &str = "main";

/// One line of `events.jsonl`, as stored.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    pub v: u32,
    pub seq: u64,
    /// Unix milliseconds.
    pub ts: u64,
    pub session: String,
    /// One `stream_chat` invocation. TTSR retries mint a new run id, so
    /// aborted branches stay distinguishable in the log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// Agent scope: `main` or `main/delegate-<uuid>`.
    pub lineage: String,
    pub kind: String,
    pub payload: serde_json::Value,
}

impl Envelope {
    /// Decode the payload into a typed event. Unknown kinds or undecodable
    /// payloads yield [`SessionEvent::Unknown`] — never an error.
    pub fn event(&self) -> SessionEvent {
        SessionEvent::decode(&self.kind, &self.payload)
    }
}

/// A typed session event. `kind` strings are stable; payload shapes may grow
/// fields but existing fields never change meaning within a schema version.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    SessionCreated(SessionCreated),
    RunStarted(RunStarted),
    RunFinished(RunFinished),
    TurnUser(TurnUser),
    ModelTurn(ModelTurn),
    ToolResult(ToolResultEvent),
    SteeringDelivered(SteeringDelivered),
    DelegateStarted(DelegateStarted),
    DelegateFinished(DelegateFinished),
    HistoryRewind(HistoryRewind),
    LegacyTurn(LegacyTurn),
    RuleFired(RuleFired),
    RuleInjection(RuleInjection),
    RuleRetroFindings(RuleRetroFindings),
    /// Forward-compat: a kind this binary does not understand.
    Unknown {
        kind: String,
    },
}

macro_rules! event_kinds {
    ($(($variant:ident, $ty:ty, $kind:literal)),+ $(,)?) => {
        impl SessionEvent {
            /// The stable `kind` string for this event.
            pub fn kind(&self) -> &str {
                match self {
                    $(SessionEvent::$variant(_) => $kind,)+
                    SessionEvent::Unknown { kind } => kind,
                }
            }

            /// Serialize the payload half of the envelope.
            pub fn payload(&self) -> serde_json::Value {
                match self {
                    $(SessionEvent::$variant(payload) => {
                        serde_json::to_value(payload).expect("event payloads always serialize")
                    })+
                    SessionEvent::Unknown { .. } => serde_json::Value::Null,
                }
            }

            fn decode(kind: &str, payload: &serde_json::Value) -> SessionEvent {
                match kind {
                    $($kind => match serde_json::from_value(payload.clone()) {
                        Ok(payload) => SessionEvent::$variant(payload),
                        Err(_) => SessionEvent::Unknown { kind: kind.to_owned() },
                    },)+
                    _ => SessionEvent::Unknown { kind: kind.to_owned() },
                }
            }
        }

        $(impl From<$ty> for SessionEvent {
            fn from(payload: $ty) -> Self {
                SessionEvent::$variant(payload)
            }
        })+
    };
}

event_kinds!(
    (SessionCreated, SessionCreated, "session.created"),
    (RunStarted, RunStarted, "run.started"),
    (RunFinished, RunFinished, "run.finished"),
    (TurnUser, TurnUser, "turn.user"),
    (ModelTurn, ModelTurn, "model.turn"),
    (ToolResult, ToolResultEvent, "tool.result"),
    (SteeringDelivered, SteeringDelivered, "steering.delivered"),
    (DelegateStarted, DelegateStarted, "delegate.started"),
    (DelegateFinished, DelegateFinished, "delegate.finished"),
    (HistoryRewind, HistoryRewind, "history.rewind"),
    (LegacyTurn, LegacyTurn, "legacy.turn"),
    (RuleFired, RuleFired, "rule.fired"),
    (RuleInjection, RuleInjection, "rule.injection"),
    (RuleRetroFindings, RuleRetroFindings, "rule.retro_findings"),
);

/// One content block inside a message. Structurally mirrors rig's content
/// types but with explicit tags so the on-disk format survives rig upgrades.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        name: String,
        arguments: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    ReasoningSummary {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        text: String,
    },
    ReasoningText {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    ReasoningEncrypted {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        data: String,
    },
    ReasoningRedacted {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        data: String,
    },
    /// Content-addressed reference into the session's `attachments/` store.
    Image {
        attachment: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
    },
    /// Escape hatch: verbatim rig serde for content we do not model.
    Opaque {
        rig: serde_json::Value,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SessionCreated {
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub artist_version: String,
    /// Present when this session was forked from another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RunStarted {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunFinished {
    Completed,
    Cancelled,
    Error { error: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TurnUser {
    pub content: Vec<ContentBlock>,
    /// Pre-expansion text as typed in the TUI, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// "prompt" | "queued" | "rule"
    pub source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ModelTurn {
    /// One-based index of the model call within its run.
    pub turn: u32,
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub total_tokens: u64,
    /// True when synthesized from accumulated deltas after a cancel.
    #[serde(default)]
    pub partial: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolResultEvent {
    pub internal_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub name: String,
    pub arguments: serde_json::Value,
    /// The model-visible result text (after any steering rewrite).
    pub result: String,
    pub outcome: ToolOutcomeRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolOutcomeRecord {
    Success,
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        message: String,
    },
    Skipped {
        reason: String,
    },
    Denied {
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SteeringDelivered {
    pub content: String,
    pub after_internal_call_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DelegateStarted {
    pub prompt: String,
    pub read_only: bool,
    pub fork: bool,
    pub background: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DelegateFinished {
    pub outcome: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HistoryRewind {
    /// Events with `to_seq < seq <= rewind event's seq` are masked.
    pub to_seq: u64,
    pub reason: String,
    /// "user" | "stream-rules"
    pub by: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LegacyTurn {
    /// "user" | "assistant"
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RuleFired {
    pub rule: String,
    pub target: String,
    pub matched: String,
    pub turn: u32,
    /// Whether the rule re-arms each user turn. Recorded so a resumed session
    /// can re-arm per-turn rules instead of leaving them permanently fired.
    /// Absent in pre-existing logs (treated as `false`, i.e. once-per-session).
    #[serde(default)]
    pub per_turn: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RuleInjection {
    pub rule: String,
    pub reminder: String,
    /// Whether this reminder persists for the whole session (vs the single
    /// retry message). Recorded so resume only re-activates session-persistent
    /// injections. Absent in pre-existing logs (treated as `false`).
    #[serde(default)]
    pub session_persistent: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RuleRetroFindings {
    pub rule: String,
    pub count: u64,
    pub examples: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(kind: &str, payload: serde_json::Value) -> Envelope {
        Envelope {
            v: SCHEMA_VERSION,
            seq: 7,
            ts: 1_752_669_000_123,
            session: "s-1".into(),
            run: Some("r-1".into()),
            lineage: MAIN_LINEAGE.into(),
            kind: kind.into(),
            payload,
        }
    }

    #[test]
    fn envelope_round_trips_through_jsonl() {
        let event = SessionEvent::from(ModelTurn {
            turn: 3,
            content: vec![
                ContentBlock::ReasoningEncrypted {
                    id: Some("rs_1".into()),
                    data: "gAAAA".into(),
                },
                ContentBlock::Text {
                    text: "hello".into(),
                },
                ContentBlock::ToolCall {
                    id: "fc_1".into(),
                    call_id: Some("call_1".into()),
                    name: "read".into(),
                    arguments: serde_json::json!({"path": "src/lib.rs"}),
                    signature: None,
                },
            ],
            total_tokens: 42,
            partial: false,
        });
        let stored = envelope(event.kind(), event.payload());
        let line = serde_json::to_string(&stored).unwrap();
        let parsed: Envelope = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed, stored);
        assert_eq!(parsed.event(), event);
    }

    #[test]
    fn unknown_kind_decodes_to_unknown_not_error() {
        let stored = envelope("future.thing", serde_json::json!({"x": 1}));
        assert_eq!(
            stored.event(),
            SessionEvent::Unknown {
                kind: "future.thing".into()
            }
        );
    }

    #[test]
    fn unknown_payload_fields_are_tolerated() {
        let stored = envelope(
            "turn.user",
            serde_json::json!({
                "content": [{"type": "text", "text": "hi"}],
                "source": "prompt",
                "some_future_field": true,
            }),
        );
        match stored.event() {
            SessionEvent::TurnUser(turn) => {
                assert_eq!(turn.content, vec![ContentBlock::Text { text: "hi".into() }]);
            }
            other => panic!("expected TurnUser, got {other:?}"),
        }
    }

    #[test]
    fn undecodable_known_kind_degrades_to_unknown() {
        let stored = envelope("model.turn", serde_json::json!({"turn": "not a number"}));
        assert_eq!(
            stored.event(),
            SessionEvent::Unknown {
                kind: "model.turn".into()
            }
        );
    }
}
