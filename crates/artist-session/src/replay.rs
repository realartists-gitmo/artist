//! Display projections: the TUI resume replay and the markdown transcript.
//! Both are derived views over the event log — the log is truth.

use crate::conversation_replay::{ConversationReplay, markdown_messages, user_message_text};
use crate::event::{ContentBlock, Envelope, SessionEvent};
use crate::history::resolve_masks;

/// One renderable item for TUI resume replay, in transcript order.
#[derive(Clone, Debug, PartialEq)]
pub enum ReplayItem {
    User(String),
    Assistant(String),
    Reasoning(String),
    Tool { name: String, preview: String },
    Steering(String),
    RuleFired { rule: String, matched: String },
}

const TOOL_PREVIEW_CAP: usize = 160;

/// Build the resume replay from a session's events (main lineage, masked
/// ranges honored) — unlike the old markdown parser, this shows tool
/// activity, reasoning, and rule firings.
pub fn replay_for_ui(events: &[Envelope]) -> Vec<ReplayItem> {
    let masks = resolve_masks(events, None);
    let mut items = Vec::new();
    let mut conversation = ConversationReplay::default();
    for envelope in events {
        if envelope.lineage != crate::event::MAIN_LINEAGE || masks.covers(envelope.seq) {
            continue;
        }
        match envelope.event() {
            SessionEvent::ConversationMessages(batch) => {
                if batch.reset && batch.display_from == 0 {
                    items.clear();
                    conversation.clear();
                }
                for message in batch.messages.iter().skip(batch.display_from) {
                    conversation.push(message, &mut items);
                }
            }
            SessionEvent::TurnUser(turn) => {
                let text = turn.display.unwrap_or_else(|| blocks_text(&turn.content));
                if !text.is_empty() {
                    items.push(ReplayItem::User(text));
                }
            }
            SessionEvent::ModelTurn(turn) => {
                let mut reasoning = String::new();
                let mut text = String::new();
                for block in &turn.content {
                    match block {
                        ContentBlock::ReasoningSummary { text: value, .. } => {
                            reasoning.push_str(value);
                        }
                        ContentBlock::Text { text: value } => text.push_str(value),
                        _ => {}
                    }
                }
                if !reasoning.is_empty() {
                    items.push(ReplayItem::Reasoning(reasoning));
                }
                if !text.is_empty() {
                    items.push(ReplayItem::Assistant(text));
                }
            }
            SessionEvent::ToolResult(result) => {
                let preview: String = result
                    .result
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(TOOL_PREVIEW_CAP)
                    .collect();
                items.push(ReplayItem::Tool {
                    name: result.name,
                    preview,
                });
            }
            SessionEvent::SteeringDelivered(steering) => {
                items.push(ReplayItem::Steering(steering.content));
            }
            SessionEvent::RuleFired(fired) => {
                items.push(ReplayItem::RuleFired {
                    rule: fired.rule,
                    matched: fired.matched,
                });
            }
            SessionEvent::LegacyTurn(turn) => match turn.role.as_str() {
                "assistant" => items.push(ReplayItem::Assistant(turn.content)),
                _ => items.push(ReplayItem::User(turn.content)),
            },
            _ => {}
        }
    }
    items
}

/// The user prompt texts, oldest first (feeds the TUI prompt-recall
/// history).
pub fn user_prompts(events: &[Envelope]) -> Vec<String> {
    let masks = resolve_masks(events, None);
    let mut prompts = Vec::new();
    for envelope in events {
        if envelope.lineage != crate::event::MAIN_LINEAGE || masks.covers(envelope.seq) {
            continue;
        }
        match envelope.event() {
            SessionEvent::ConversationMessages(batch) => {
                if batch.reset && batch.display_from == 0 {
                    prompts.clear();
                }
                prompts.extend(
                    batch
                        .messages
                        .iter()
                        .skip(batch.display_from)
                        .filter_map(user_message_text)
                        .filter(|text| !text.starts_with("<system-reminder")),
                );
            }
            SessionEvent::TurnUser(turn) if turn.source == "prompt" || turn.source == "queued" => {
                prompts.push(turn.display.unwrap_or_else(|| blocks_text(&turn.content)));
            }
            SessionEvent::LegacyTurn(turn) if turn.role == "user" => prompts.push(turn.content),
            _ => {}
        }
    }
    prompts
}

/// Events not hidden by rewind masks — the "current" view of
/// the session that on-demand scans should see.
pub fn visible_events(events: &[Envelope]) -> Vec<&Envelope> {
    let masks = resolve_masks(events, None);
    events
        .iter()
        .filter(|envelope| !masks.covers(envelope.seq))
        .collect()
}

/// User-turn rewind targets: `(seq, display)` in transcript order, masked
/// ranges excluded (an already-rewound turn is not offered again).
pub fn rewind_targets(events: &[Envelope]) -> Vec<(u64, String)> {
    let masks = resolve_masks(events, None);
    let mut targets = Vec::new();
    for envelope in events {
        if envelope.lineage != crate::event::MAIN_LINEAGE || masks.covers(envelope.seq) {
            continue;
        }
        match envelope.event() {
            SessionEvent::ConversationMessages(batch) => {
                if batch.reset && batch.display_from == 0 {
                    targets.clear();
                }
                if let Some(text) = batch
                    .messages
                    .iter()
                    .skip(batch.display_from)
                    .filter_map(user_message_text)
                    .rfind(|text| !text.starts_with("<system-reminder"))
                {
                    targets.push((envelope.seq, text));
                }
            }
            SessionEvent::TurnUser(turn) if turn.source == "prompt" || turn.source == "queued" => {
                targets.push((
                    envelope.seq,
                    turn.display.unwrap_or_else(|| blocks_text(&turn.content)),
                ));
            }
            _ => {}
        }
    }
    targets
}

fn blocks_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The markdown transcript fragment for one event, if it renders. The
/// writer task appends these incrementally; `render_markdown` regenerates
/// the whole file from the log.
pub fn markdown_fragment(envelope: &Envelope) -> Option<String> {
    if envelope.lineage != crate::event::MAIN_LINEAGE {
        return None;
    }
    match envelope.event() {
        SessionEvent::SessionCreated(created) => Some(format!(
            "# Artist session\n\n- Project: `{}`{}\n",
            created.project,
            created
                .parent_session
                .map(|parent| format!("\n- Forked from: `{parent}`"))
                .unwrap_or_default(),
        )),
        SessionEvent::ConversationMessages(batch) => {
            let fragment = markdown_messages(&batch.messages[batch.display_from..]);
            (!fragment.is_empty()).then_some(fragment)
        }
        SessionEvent::TurnUser(turn) => {
            let text = turn.display.unwrap_or_else(|| blocks_text(&turn.content));
            match turn.source.as_str() {
                "rule" => None, // rendered by the RuleInjection fragment
                _ => Some(format!("\n## User\n\n{text}\n")),
            }
        }
        SessionEvent::ModelTurn(turn) => {
            let text = blocks_text(&turn.content);
            let calls: Vec<String> = turn
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolCall { name, .. } => Some(name.clone()),
                    _ => None,
                })
                .collect();
            let mut fragment = String::new();
            if !text.is_empty() {
                let marker = if turn.partial { " (interrupted)" } else { "" };
                fragment.push_str(&format!("\n## Assistant{marker}\n\n{text}\n"));
            }
            if !calls.is_empty() {
                fragment.push_str(&format!("\n> tools: {}\n", calls.join(", ")));
            }
            (!fragment.is_empty()).then_some(fragment)
        }
        SessionEvent::SteeringDelivered(steering) => {
            Some(format!("\n## User (steering)\n\n{}\n", steering.content))
        }
        SessionEvent::RuleFired(fired) => Some(format!(
            "\n> ⚠ stream rule `{}` fired on `{}` — rewound and retried\n",
            fired.rule,
            fired.matched.replace('`', "'")
        )),
        SessionEvent::LegacyTurn(turn) => Some(format!(
            "\n## {}\n\n{}\n",
            if turn.role == "assistant" {
                "Assistant"
            } else {
                "User"
            },
            turn.content
        )),
        _ => None,
    }
}

/// Regenerate the full transcript markdown from the log (`artist sessions
/// render`, and the recovery path when transcript.md is stale or missing).
pub fn render_markdown(events: &[Envelope]) -> String {
    let masks = resolve_masks(events, None);
    let mut header = String::new();
    let mut fragments = Vec::new();
    for envelope in events {
        if envelope.lineage != crate::event::MAIN_LINEAGE || masks.covers(envelope.seq) {
            continue;
        }
        let event = envelope.event();
        if matches!(event, SessionEvent::SessionCreated(_)) {
            header = markdown_fragment(envelope).unwrap_or_default();
            continue;
        }
        if matches!(
            event,
            SessionEvent::ConversationMessages(ref batch)
                if batch.reset && batch.display_from == 0
        ) {
            fragments.clear();
        }
        if let Some(fragment) = markdown_fragment(envelope) {
            fragments.push(fragment);
        }
    }
    header + &fragments.concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{
        ModelTurn, RuleFired, SCHEMA_VERSION, ToolOutcomeRecord, ToolResultEvent, TurnUser,
    };

    fn envelope(seq: u64, lineage: &str, event: SessionEvent) -> Envelope {
        Envelope {
            v: SCHEMA_VERSION,
            seq,
            ts: 0,
            session: "s".into(),
            run: None,
            lineage: lineage.into(),
            kind: event.kind().to_owned(),
            payload: event.payload(),
        }
    }

    #[test]
    fn replay_shows_tools_reasoning_and_rules() {
        let events = vec![
            envelope(
                0,
                "main",
                SessionEvent::TurnUser(TurnUser {
                    content: vec![ContentBlock::Text {
                        text: "fix it".into(),
                    }],
                    display: Some("fix it".into()),
                    source: "prompt".into(),
                }),
            ),
            envelope(
                1,
                "main",
                SessionEvent::ModelTurn(ModelTurn {
                    turn: 1,
                    content: vec![
                        ContentBlock::ReasoningSummary {
                            id: None,
                            text: "thinking".into(),
                        },
                        ContentBlock::ToolCall {
                            id: "fc1".into(),
                            call_id: None,
                            name: "read".into(),
                            arguments: serde_json::json!({}),
                            signature: None,
                        },
                    ],
                    total_tokens: 0,
                    partial: false,
                }),
            ),
            envelope(
                2,
                "main",
                SessionEvent::ToolResult(ToolResultEvent {
                    internal_call_id: "ic".into(),
                    tool_call_id: None,
                    name: "read".into(),
                    arguments: serde_json::json!({}),
                    result: "line one\nline two".into(),
                    outcome: ToolOutcomeRecord::Success,
                    duration_ms: None,
                }),
            ),
            envelope(
                3,
                "main/delegate-1",
                SessionEvent::ModelTurn(ModelTurn {
                    turn: 1,
                    content: vec![ContentBlock::Text {
                        text: "hidden".into(),
                    }],
                    total_tokens: 0,
                    partial: false,
                }),
            ),
            envelope(
                4,
                "main",
                SessionEvent::RuleFired(RuleFired {
                    rule: "no-mock".into(),
                    target: "assistant-text".into(),
                    matched: "mock data".into(),
                    turn: 2,
                    per_turn: false,
                }),
            ),
        ];
        let items = replay_for_ui(&events);
        assert_eq!(
            items,
            vec![
                ReplayItem::User("fix it".into()),
                ReplayItem::Reasoning("thinking".into()),
                ReplayItem::Tool {
                    name: "read".into(),
                    preview: "line one".into()
                },
                ReplayItem::RuleFired {
                    rule: "no-mock".into(),
                    matched: "mock data".into()
                },
            ]
        );
    }

    #[test]
    fn markdown_renders_and_rule_turns_are_not_duplicated() {
        let events = vec![
            envelope(
                0,
                "main",
                SessionEvent::TurnUser(TurnUser {
                    content: vec![ContentBlock::Text { text: "hi".into() }],
                    display: None,
                    source: "prompt".into(),
                }),
            ),
            envelope(
                1,
                "main",
                SessionEvent::TurnUser(TurnUser {
                    content: vec![ContentBlock::Text {
                        text: "<system-reminder>…</system-reminder>".into(),
                    }],
                    display: Some("rule: r".into()),
                    source: "rule".into(),
                }),
            ),
        ];
        let markdown = render_markdown(&events);
        assert!(markdown.contains("## User\n\nhi"));
        assert!(!markdown.contains("system-reminder"));
    }

    #[test]
    fn user_prompts_skip_rule_injections() {
        let events = vec![
            envelope(
                0,
                "main",
                SessionEvent::TurnUser(TurnUser {
                    content: vec![ContentBlock::Text { text: "one".into() }],
                    display: None,
                    source: "prompt".into(),
                }),
            ),
            envelope(
                1,
                "main",
                SessionEvent::TurnUser(TurnUser {
                    content: vec![ContentBlock::Text {
                        text: "rule".into(),
                    }],
                    display: None,
                    source: "rule".into(),
                }),
            ),
        ];
        assert_eq!(user_prompts(&events), vec!["one".to_owned()]);
    }
}
