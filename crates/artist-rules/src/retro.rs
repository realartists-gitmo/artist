//! Retroactive rule evaluation over a session's event log — on-demand only
//! (`/rules scan`, `/rules dry-run`). Findings are informational; they never
//! abort anything and never consume a rule's fire budget.

use artist_session::{ContentBlock, Envelope, SessionEvent, visible_events};

use crate::matcher::RuleSet;
use crate::types::{MatchTarget, RuleId};

#[derive(Clone, Debug, PartialEq)]
pub struct RetroFinding {
    pub rule: RuleId,
    pub target: MatchTarget,
    /// Seq of the event the match was found in.
    pub seq: u64,
    pub excerpt: String,
}

/// Scan committed model output (assistant text, reasoning summaries,
/// tool-call arguments) with the given rule set. Rule-injection turns and
/// masked (rewound/compacted) ranges are skipped.
pub fn scan(rules: &RuleSet, events: &[Envelope]) -> Vec<RetroFinding> {
    let mut findings = Vec::new();
    for envelope in visible_events(events) {
        let SessionEvent::ModelTurn(turn) = envelope.event() else {
            continue;
        };
        for block in &turn.content {
            let matched = match block {
                ContentBlock::Text { text } => {
                    rules.scan_all(MatchTarget::AssistantText, text, None)
                }
                ContentBlock::ReasoningSummary { text, .. } => {
                    rules.scan_all(MatchTarget::ReasoningSummary, text, None)
                }
                ContentBlock::ToolCall {
                    name, arguments, ..
                } => rules.scan_all(MatchTarget::ToolArgs, &arguments.to_string(), Some(name)),
                _ => Vec::new(),
            };
            let target = match block {
                ContentBlock::Text { .. } => MatchTarget::AssistantText,
                ContentBlock::ReasoningSummary { .. } => MatchTarget::ReasoningSummary,
                _ => MatchTarget::ToolArgs,
            };
            for (rule, excerpt) in matched {
                findings.push(RetroFinding {
                    rule,
                    target,
                    seq: envelope.seq,
                    excerpt,
                });
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declarative::parse_parts;
    use artist_session::{ModelTurn, SCHEMA_VERSION, TurnUser};
    use std::sync::Arc;

    fn envelope(seq: u64, event: SessionEvent) -> Envelope {
        Envelope {
            v: SCHEMA_VERSION,
            seq,
            ts: 0,
            session: "s".into(),
            run: None,
            lineage: "main".into(),
            kind: event.kind().to_owned(),
            payload: event.payload(),
        }
    }

    fn rules(specs: &[(&str, &str)]) -> Arc<RuleSet> {
        Arc::new(RuleSet::compile(
            specs
                .iter()
                .map(|(name, extra)| {
                    parse_parts(
                        &format!("name: {name}\ndescription: d\n{extra}"),
                        "reminder",
                        None,
                    )
                    .unwrap()
                })
                .collect(),
        ))
    }

    #[test]
    fn finds_past_matches_and_skips_rule_injections() {
        let events = vec![
            envelope(
                0,
                SessionEvent::ModelTurn(ModelTurn {
                    turn: 1,
                    content: vec![
                        ContentBlock::Text {
                            text: "let me mock the data".into(),
                        },
                        ContentBlock::ToolCall {
                            id: "fc".into(),
                            call_id: None,
                            name: "write".into(),
                            arguments: serde_json::json!({"content": "except: pass"}),
                            signature: None,
                        },
                    ],
                    total_tokens: 0,
                    partial: false,
                }),
            ),
            // A rule injection containing the pattern must NOT match.
            envelope(
                1,
                SessionEvent::TurnUser(TurnUser {
                    content: vec![ContentBlock::Text {
                        text: "reminder about mock the data".into(),
                    }],
                    display: None,
                    source: "rule".into(),
                }),
            ),
        ];
        let set = rules(&[
            ("no-mock", "patterns: ['mock the data']"),
            (
                "no-swallow",
                "targets: [tool-args]\npatterns: ['except: pass']\ntools: [write]",
            ),
        ]);
        let findings = scan(&set, &events);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].rule, RuleId("no-mock".into()));
        assert_eq!(findings[1].rule, RuleId("no-swallow".into()));
        assert_eq!(findings[1].target, MatchTarget::ToolArgs);
    }
}
