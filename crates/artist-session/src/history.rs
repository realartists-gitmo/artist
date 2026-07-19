//! The model-facing history projection: events → `Vec<rig Message>` with
//! full tool-call/result fidelity, honoring `history.rewind` and
//! `history.compact` masks.

use anyhow::Result;
use rig_core::OneOrMany;
use rig_core::completion::message::{
    AssistantContent, Message, Text, ToolResult, ToolResultContent, UserContent,
};

use crate::attachments::AttachmentStore;
use crate::convert::{blocks_to_assistant, blocks_to_user};
use crate::event::{ContentBlock, Envelope, SessionEvent};

/// Options for [`build`].
pub struct HistoryOptions<'a> {
    /// Only events with exactly this lineage are included.
    pub lineage: &'a str,
    /// Ignore events after this seq (inclusive bound). `None` = everything.
    pub up_to_seq: Option<u64>,
    /// Drop `ReasoningEncrypted` blocks (degrade path if the provider rejects
    /// replayed encrypted reasoning cross-process).
    pub drop_encrypted_reasoning: bool,
}

impl Default for HistoryOptions<'_> {
    fn default() -> Self {
        Self {
            lineage: crate::event::MAIN_LINEAGE,
            up_to_seq: None,
            drop_encrypted_reasoning: false,
        }
    }
}

/// A half-open set of masked seq ranges (inclusive bounds).
#[derive(Debug, Default)]
pub(crate) struct Masks(Vec<(u64, u64)>);

impl Masks {
    pub(crate) fn covers(&self, seq: u64) -> bool {
        self.0
            .iter()
            .any(|(start, end)| *start <= seq && seq <= *end)
    }
}

/// Resolve which rewind/compact control events are active. Processed from
/// latest to earliest: a control event that falls inside a later active
/// control's range is disabled (e.g. rewinding past a compaction restores
/// the originals the compaction had replaced).
pub(crate) fn resolve_masks(events: &[Envelope], up_to_seq: Option<u64>) -> Masks {
    let mut masks = Masks::default();
    for envelope in events.iter().rev() {
        if let Some(limit) = up_to_seq
            && envelope.seq > limit
        {
            continue;
        }
        let range = match envelope.event() {
            SessionEvent::HistoryRewind(rewind) => (rewind.to_seq.saturating_add(1), envelope.seq),
            SessionEvent::HistoryCompact(compact) => (compact.from_seq, compact.to_seq),
            _ => continue,
        };
        if masks.covers(envelope.seq) {
            continue;
        }
        if range.0 <= range.1 {
            masks.0.push(range);
        }
    }
    masks
}

/// Build the model-facing history from a session's events.
pub fn build(
    events: &[Envelope],
    attachments: &AttachmentStore,
    options: &HistoryOptions,
) -> Result<Vec<Message>> {
    let masks = resolve_masks(events, options.up_to_seq);
    // Tool-result images are logged separately from the text-only capture event
    // (rig's hook exposes only text); index them by internal_call_id to reattach.
    let mut result_images: std::collections::HashMap<String, Vec<ContentBlock>> =
        std::collections::HashMap::new();
    for envelope in events {
        if let SessionEvent::ToolResultImages(images) = envelope.event() {
            result_images.insert(images.internal_call_id, images.images);
        }
    }
    let mut messages: Vec<Message> = Vec::new();
    // ToolCall blocks of the latest assistant turn not yet fully answered:
    // (provider id, call_id, tool name). Results matched in arrival order.
    let mut unanswered: Vec<(String, Option<String>, String)> = Vec::new();
    // Tool results answering the current unanswered set, grouped into one
    // user message exactly as rig commits them.
    let mut pending_results: Vec<ToolResult> = Vec::new();

    for envelope in events {
        if let Some(limit) = options.up_to_seq
            && envelope.seq > limit
        {
            break;
        }
        if envelope.lineage != options.lineage || masks.covers(envelope.seq) {
            continue;
        }
        match envelope.event() {
            SessionEvent::TurnUser(turn) => {
                flush_results(&mut messages, &mut unanswered, &mut pending_results);
                let content = blocks_to_user(&turn.content, attachments)?;
                if let Some(content) = one_or_many(content) {
                    messages.push(Message::User { content });
                }
            }
            SessionEvent::ModelTurn(turn) => {
                flush_results(&mut messages, &mut unanswered, &mut pending_results);
                let mut blocks = turn.content.clone();
                if options.drop_encrypted_reasoning {
                    blocks
                        .retain(|block| !matches!(block, ContentBlock::ReasoningEncrypted { .. }));
                }
                if turn.partial {
                    // A cancelled turn has no tool round-trip to complete;
                    // replaying dangling tool calls would be rejected.
                    blocks.retain(|block| matches!(block, ContentBlock::Text { .. }));
                }
                let content = blocks_to_assistant(&blocks, attachments)?;
                for item in &content {
                    if let AssistantContent::ToolCall(call) = item {
                        unanswered.push((
                            call.id.clone(),
                            call.call_id.clone(),
                            call.function.name.clone(),
                        ));
                    }
                }
                if let Some(content) = one_or_many(content) {
                    messages.push(Message::Assistant { id: None, content });
                }
            }
            SessionEvent::ToolResult(result) => {
                // Ids come from the committed assistant ToolCall block, never
                // from the capture event, so the pairing rig validates on
                // replay always holds.
                // Skip a result that matches no pending call (by id/call_id, or
                // name) rather than defaulting to entry 0 and mis-pairing it
                // with an unrelated call — that would emit history a provider
                // rejects. Also covers the empty-`unanswered` case.
                let Some(position) = unanswered
                    .iter()
                    .position(|(id, call_id, _)| {
                        result.tool_call_id.as_deref() == Some(id)
                            || (result.tool_call_id.is_some() && result.tool_call_id == *call_id)
                    })
                    .or_else(|| {
                        unanswered
                            .iter()
                            .position(|(_, _, name)| *name == result.name)
                    })
                else {
                    continue;
                };
                let (id, call_id, _) = unanswered.remove(position);
                // Reattach any images recorded for this result (from the display
                // path) so an image-bearing tool result replays faithfully.
                let mut items =
                    vec![ToolResultContent::Text(Text::new(result.result.clone()))];
                if let Some(blocks) = result_images.get(&result.internal_call_id) {
                    items.extend(
                        blocks
                            .iter()
                            .filter_map(|block| crate::tool_image_from_block(block, attachments)),
                    );
                }
                pending_results.push(ToolResult {
                    id,
                    call_id,
                    content: OneOrMany::many(items).expect("at least the result text"),
                });
                if unanswered.is_empty() {
                    flush_results(&mut messages, &mut unanswered, &mut pending_results);
                }
            }
            SessionEvent::HistoryCompact(compact) => {
                flush_results(&mut messages, &mut unanswered, &mut pending_results);
                let content = blocks_to_user(&compact.summary, attachments)?;
                if let Some(content) = one_or_many(content) {
                    messages.push(Message::User { content });
                }
            }
            SessionEvent::LegacyTurn(turn) => {
                flush_results(&mut messages, &mut unanswered, &mut pending_results);
                match turn.role.as_str() {
                    "assistant" => messages.push(Message::assistant(&turn.content)),
                    _ => messages.push(Message::user(&turn.content)),
                }
            }
            // Lifecycle, steering-display, delegate, and rule events carry no
            // model-facing content (rule injections ride in turn.user /
            // tool.result text already).
            _ => {}
        }
    }
    flush_results(&mut messages, &mut unanswered, &mut pending_results);
    Ok(messages)
}

/// `OneOrMany` from a possibly-empty vec (`None` when empty).
fn one_or_many<T: Clone>(items: Vec<T>) -> Option<OneOrMany<T>> {
    OneOrMany::many(items).ok()
}

/// Commit pending tool results as one user message (rig's shape). Tool calls
/// that never received a result (cancelled mid-batch) get a synthetic
/// "cancelled" result so replayed history never carries an unanswered call.
fn flush_results(
    messages: &mut Vec<Message>,
    unanswered: &mut Vec<(String, Option<String>, String)>,
    pending_results: &mut Vec<ToolResult>,
) {
    if pending_results.is_empty() && unanswered.is_empty() {
        return;
    }
    for (id, call_id, _) in unanswered.drain(..) {
        pending_results.push(ToolResult {
            id,
            call_id,
            content: OneOrMany::one(ToolResultContent::Text(Text::new(
                "[tool call cancelled before completion]",
            ))),
        });
    }
    let results = pending_results
        .drain(..)
        .map(UserContent::ToolResult)
        .collect::<Vec<_>>();
    if let Some(content) = one_or_many(results) {
        messages.push(Message::User { content });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{
        HistoryCompact, HistoryRewind, LegacyTurn, ModelTurn, SCHEMA_VERSION, ToolOutcomeRecord,
        ToolResultEvent, ToolResultImagesEvent, TurnUser,
    };

    struct LogBuilder {
        events: Vec<Envelope>,
    }

    impl LogBuilder {
        fn new() -> Self {
            Self { events: Vec::new() }
        }

        fn push(&mut self, event: SessionEvent) -> u64 {
            self.push_lineage(event, crate::event::MAIN_LINEAGE)
        }

        fn push_lineage(&mut self, event: SessionEvent, lineage: &str) -> u64 {
            let seq = self.events.len() as u64;
            self.events.push(Envelope {
                v: SCHEMA_VERSION,
                seq,
                ts: 0,
                session: "s".into(),
                run: None,
                lineage: lineage.into(),
                kind: event.kind().to_owned(),
                payload: event.payload(),
            });
            seq
        }
    }

    fn attachments() -> (tempfile::TempDir, AttachmentStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(dir.path().join("attachments"));
        (dir, store)
    }

    fn user(text: &str) -> SessionEvent {
        SessionEvent::TurnUser(TurnUser {
            content: vec![ContentBlock::Text { text: text.into() }],
            display: None,
            source: "prompt".into(),
        })
    }

    fn assistant_text(text: &str) -> SessionEvent {
        SessionEvent::ModelTurn(ModelTurn {
            turn: 1,
            content: vec![ContentBlock::Text { text: text.into() }],
            total_tokens: 0,
            partial: false,
        })
    }

    fn assistant_tool_call(id: &str, name: &str) -> SessionEvent {
        SessionEvent::ModelTurn(ModelTurn {
            turn: 1,
            content: vec![ContentBlock::ToolCall {
                id: id.into(),
                call_id: Some(format!("call_{id}")),
                name: name.into(),
                arguments: serde_json::json!({}),
                signature: None,
            }],
            total_tokens: 0,
            partial: false,
        })
    }

    fn tool_result(name: &str, result: &str) -> SessionEvent {
        SessionEvent::ToolResult(ToolResultEvent {
            internal_call_id: "ic".into(),
            tool_call_id: None,
            name: name.into(),
            arguments: serde_json::json!({}),
            result: result.into(),
            outcome: ToolOutcomeRecord::Success,
            duration_ms: Some(3),
        })
    }

    #[test]
    fn tool_round_trip_pairs_ids_from_committed_call() {
        let (_dir, store) = attachments();
        let mut log = LogBuilder::new();
        log.push(user("read the file"));
        log.push(assistant_tool_call("fc_1", "read"));
        log.push(tool_result("read", "file contents"));
        log.push(assistant_text("done"));

        let history = build(&log.events, &store, &HistoryOptions::default()).unwrap();
        assert_eq!(history.len(), 4);
        match &history[2] {
            Message::User { content } => match content.first() {
                UserContent::ToolResult(result) => {
                    assert_eq!(result.id, "fc_1");
                    assert_eq!(result.call_id.as_deref(), Some("call_fc_1"));
                }
                other => panic!("expected tool result, got {other:?}"),
            },
            other => panic!("expected user message, got {other:?}"),
        }
    }

    #[test]
    fn tool_result_images_reattach_on_replay() {
        let (_dir, store) = attachments();
        let attachment = store.put(b"\x89PNG\r\n\x1a\n fake image bytes").unwrap();
        let mut log = LogBuilder::new();
        log.push(user("screenshot the page"));
        log.push(assistant_tool_call("fc_1", "screenshot"));
        log.push(tool_result("screenshot", "captured"));
        log.push(SessionEvent::ToolResultImages(ToolResultImagesEvent {
            internal_call_id: "ic".into(),
            images: vec![ContentBlock::Image {
                attachment,
                media_type: Some("png".into()),
            }],
        }));
        log.push(assistant_text("done"));

        let history = build(&log.events, &store, &HistoryOptions::default()).unwrap();
        match &history[2] {
            Message::User { content } => match content.first() {
                UserContent::ToolResult(result) => {
                    let images = result
                        .content
                        .iter()
                        .filter(|item| matches!(item, ToolResultContent::Image(_)))
                        .count();
                    assert_eq!(images, 1, "the image should replay on the tool result");
                }
                other => panic!("expected tool result, got {other:?}"),
            },
            other => panic!("expected user message, got {other:?}"),
        }
    }

    #[test]
    fn rewind_masks_events_and_later_rewind_wins() {
        let (_dir, store) = attachments();
        let mut log = LogBuilder::new();
        log.push(user("one"));
        log.push(assistant_text("first answer"));
        let after_first = log.events.last().unwrap().seq;
        log.push(user("two"));
        log.push(assistant_text("second answer"));
        log.push(SessionEvent::HistoryRewind(HistoryRewind {
            to_seq: after_first,
            reason: "retry".into(),
            by: "user".into(),
        }));
        log.push(user("two, better"));
        log.push(assistant_text("third answer"));

        let history = build(&log.events, &store, &HistoryOptions::default()).unwrap();
        let texts: Vec<String> = history
            .iter()
            .map(|message| match message {
                Message::User { content } => match content.first() {
                    UserContent::Text(text) => text.text.clone(),
                    _ => String::new(),
                },
                Message::Assistant { content, .. } => match content.first() {
                    AssistantContent::Text(text) => text.text.clone(),
                    _ => String::new(),
                },
                _ => String::new(),
            })
            .collect();
        assert_eq!(
            texts,
            vec!["one", "first answer", "two, better", "third answer"]
        );
    }

    #[test]
    fn rewind_past_compaction_restores_originals() {
        let (_dir, store) = attachments();
        let mut log = LogBuilder::new();
        let first = log.push(user("one"));
        let second = log.push(assistant_text("original answer"));
        log.push(SessionEvent::HistoryCompact(HistoryCompact {
            from_seq: first,
            to_seq: second,
            summary: vec![ContentBlock::Text {
                text: "Earlier: user asked one".into(),
            }],
        }));
        log.push(SessionEvent::HistoryRewind(HistoryRewind {
            to_seq: second,
            reason: "user rewind".into(),
            by: "user".into(),
        }));

        let history = build(&log.events, &store, &HistoryOptions::default()).unwrap();
        assert_eq!(history.len(), 2, "compaction summary must be gone");
        match &history[1] {
            Message::Assistant { content, .. } => match content.first() {
                AssistantContent::Text(text) => assert_eq!(text.text, "original answer"),
                other => panic!("unexpected {other:?}"),
            },
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn delegate_lineage_is_excluded_from_main_history() {
        let (_dir, store) = attachments();
        let mut log = LogBuilder::new();
        log.push(user("go"));
        log.push_lineage(assistant_text("delegate inner"), "main/delegate-1");
        log.push(assistant_text("main answer"));

        let history = build(&log.events, &store, &HistoryOptions::default()).unwrap();
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn cancelled_batch_synthesizes_results_for_unanswered_calls() {
        let (_dir, store) = attachments();
        let mut log = LogBuilder::new();
        log.push(user("go"));
        log.push(SessionEvent::ModelTurn(ModelTurn {
            turn: 1,
            content: vec![
                ContentBlock::ToolCall {
                    id: "fc_1".into(),
                    call_id: None,
                    name: "read".into(),
                    arguments: serde_json::json!({}),
                    signature: None,
                },
                ContentBlock::ToolCall {
                    id: "fc_2".into(),
                    call_id: None,
                    name: "grep".into(),
                    arguments: serde_json::json!({}),
                    signature: None,
                },
            ],
            total_tokens: 0,
            partial: false,
        }));
        log.push(tool_result("read", "ok"));
        // fc_2 never answered (cancelled) — next user turn arrives.
        log.push(user("never mind"));

        let history = build(&log.events, &store, &HistoryOptions::default()).unwrap();
        // user, assistant(2 calls), user(2 results incl. synthetic), user
        assert_eq!(history.len(), 4);
        match &history[2] {
            Message::User { content } => {
                let results: Vec<_> = content.iter().collect();
                assert_eq!(results.len(), 2);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn partial_turn_keeps_text_and_drops_tool_calls() {
        let (_dir, store) = attachments();
        let mut log = LogBuilder::new();
        log.push(user("go"));
        log.push(SessionEvent::ModelTurn(ModelTurn {
            turn: 1,
            content: vec![
                ContentBlock::Text {
                    text: "partial thoughts".into(),
                },
                ContentBlock::ToolCall {
                    id: "fc_1".into(),
                    call_id: None,
                    name: "read".into(),
                    arguments: serde_json::json!({}),
                    signature: None,
                },
            ],
            total_tokens: 0,
            partial: true,
        }));

        let history = build(&log.events, &store, &HistoryOptions::default()).unwrap();
        assert_eq!(history.len(), 2);
        match &history[1] {
            Message::Assistant { content, .. } => {
                assert_eq!(content.iter().count(), 1);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn drop_encrypted_reasoning_degrade_path() {
        let (_dir, store) = attachments();
        let mut log = LogBuilder::new();
        log.push(user("go"));
        log.push(SessionEvent::ModelTurn(ModelTurn {
            turn: 1,
            content: vec![
                ContentBlock::ReasoningEncrypted {
                    id: Some("rs_1".into()),
                    data: "gAAA".into(),
                },
                ContentBlock::Text {
                    text: "answer".into(),
                },
            ],
            total_tokens: 0,
            partial: false,
        }));

        let options = HistoryOptions {
            drop_encrypted_reasoning: true,
            ..Default::default()
        };
        let history = build(&log.events, &store, &options).unwrap();
        match &history[1] {
            Message::Assistant { content, .. } => assert_eq!(content.iter().count(), 1),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn legacy_turns_build_text_history() {
        let (_dir, store) = attachments();
        let mut log = LogBuilder::new();
        log.push(SessionEvent::LegacyTurn(LegacyTurn {
            role: "user".into(),
            content: "old question".into(),
        }));
        log.push(SessionEvent::LegacyTurn(LegacyTurn {
            role: "assistant".into(),
            content: "old answer".into(),
        }));
        let history = build(&log.events, &store, &HistoryOptions::default()).unwrap();
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn up_to_seq_snapshot_ignores_later_events() {
        let (_dir, store) = attachments();
        let mut log = LogBuilder::new();
        log.push(user("one"));
        let snapshot = log.push(assistant_text("answer"));
        log.push(user("two"));

        let options = HistoryOptions {
            up_to_seq: Some(snapshot),
            ..Default::default()
        };
        let history = build(&log.events, &store, &options).unwrap();
        assert_eq!(history.len(), 2);
    }
}
