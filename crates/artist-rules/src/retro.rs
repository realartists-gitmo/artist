//! Retroactive rule evaluation over a session's event log — on-demand only
//! (`/rules scan`, `/rules dry-run`). Findings are informational; they never
//! abort anything and never consume a rule's fire budget.

use artist_session::{ContentBlock, Envelope, SessionEvent, visible_events};
use rig_core::completion::message::{AssistantContent, Message};

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
/// rewound ranges are skipped.
pub fn scan(rules: &RuleSet, events: &[Envelope]) -> Vec<RetroFinding> {
    let mut findings = Vec::new();
    for envelope in visible_events(events) {
        let candidates = match envelope.event() {
            SessionEvent::ModelTurn(turn) => turn
                .content
                .iter()
                .filter_map(block_candidate)
                .collect::<Vec<_>>(),
            SessionEvent::ConversationMessages(batch) => batch
                .messages
                .iter()
                .skip(batch.display_from)
                .flat_map(message_candidates)
                .collect(),
            _ => continue,
        };
        for (target, text, tool) in candidates {
            for (rule, excerpt) in rules.scan_all(target, &text, tool.as_deref()) {
                // Wasm-backed rules judge their prefilter hits in scans too,
                // so a plugin's pass never shows up as a false finding.
                let firing = crate::types::Firing {
                    rule,
                    target,
                    matched: excerpt,
                    reminder: String::new(),
                    persistence: Default::default(),
                    fire: Default::default(),
                };
                let Some(firing) = rules.verdict(firing, 0) else {
                    continue;
                };
                findings.push(RetroFinding {
                    rule: firing.rule,
                    target,
                    seq: envelope.seq,
                    excerpt: firing.matched,
                });
            }
        }
    }
    findings
}

fn block_candidate(block: &ContentBlock) -> Option<(MatchTarget, String, Option<String>)> {
    match block {
        ContentBlock::Text { text } => Some((MatchTarget::AssistantText, text.clone(), None)),
        ContentBlock::ReasoningSummary { text, .. } => {
            Some((MatchTarget::ReasoningSummary, text.clone(), None))
        }
        ContentBlock::ToolCall {
            name, arguments, ..
        } => Some((
            MatchTarget::ToolArgs,
            arguments.to_string(),
            Some(name.clone()),
        )),
        _ => None,
    }
}

fn message_candidates(message: &Message) -> Vec<(MatchTarget, String, Option<String>)> {
    let Message::Assistant { content, .. } = message else {
        return Vec::new();
    };
    content
        .iter()
        .filter_map(|item| match item {
            AssistantContent::Text(text) => {
                Some((MatchTarget::AssistantText, text.text.clone(), None))
            }
            AssistantContent::Reasoning(reasoning) => Some((
                MatchTarget::ReasoningSummary,
                reasoning.display_text(),
                None,
            )),
            AssistantContent::ToolCall(call) => Some((
                MatchTarget::ToolArgs,
                call.function.arguments.to_string(),
                Some(call.function.name.clone()),
            )),
            _ => None,
        })
        .collect()
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
