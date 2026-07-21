use crate::event::{ContentBlock, Envelope, SessionEvent};
use crate::history::resolve_masks;

#[derive(Clone, Debug)]
pub struct CompactionCandidate {
    pub from_seq: u64,
    pub to_seq: u64,
    pub events: Vec<Envelope>,
}

/// Select old, currently-visible complete turns while preserving the newest
/// `preserve_user_turns` user turns verbatim.
pub fn select_compaction_candidate(
    events: &[Envelope],
    preserve_user_turns: usize,
) -> Option<CompactionCandidate> {
    let masks = resolve_masks(events, None);
    let visible: Vec<_> = events
        .iter()
        .filter(|event| event.lineage == crate::event::MAIN_LINEAGE && !masks.covers(event.seq))
        .collect();
    let user_positions: Vec<_> = visible
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            matches!(event.event(), SessionEvent::TurnUser(_)).then_some(index)
        })
        .collect();
    if user_positions.len() <= preserve_user_turns {
        return None;
    }
    let keep_from = user_positions[user_positions.len() - preserve_user_turns];
    let selected = &visible[..keep_from];
    let first = selected.first()?;
    let last = selected.last()?;
    // Rolling compaction must continue masking originals replaced by an older
    // summary. Masking only that summary would reactivate its original range.
    let from_seq = selected.iter().fold(first.seq, |start, envelope| {
        if let SessionEvent::HistoryCompact(compact) = envelope.event() {
            start.min(compact.from_seq)
        } else {
            start
        }
    });
    Some(CompactionCandidate {
        from_seq,
        to_seq: last.seq,
        events: selected.iter().map(|event| (*event).clone()).collect(),
    })
}

/// Render selected events as concise, role-labelled source for the summarizer.
pub fn render_compaction_source(candidate: &CompactionCandidate) -> String {
    let mut output = String::new();
    for envelope in &candidate.events {
        match envelope.event() {
            SessionEvent::TurnUser(turn) => {
                push(&mut output, "USER", &blocks_text(&turn.content));
            }
            SessionEvent::ModelTurn(turn) => {
                push(&mut output, "ASSISTANT", &blocks_text(&turn.content));
            }
            SessionEvent::ToolResult(result) => {
                push(
                    &mut output,
                    &format!("TOOL RESULT ({})", result.name),
                    &result.result,
                );
            }
            SessionEvent::HistoryCompact(compact) => {
                push(
                    &mut output,
                    "EARLIER SUMMARY",
                    &blocks_text(&compact.summary),
                );
            }
            SessionEvent::LegacyTurn(turn) => {
                push(&mut output, &turn.role.to_ascii_uppercase(), &turn.content)
            }
            _ => {}
        }
    }
    output
}

fn blocks_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } | ContentBlock::ReasoningSummary { text, .. } => {
                Some(text.clone())
            }
            ContentBlock::ToolCall {
                name, arguments, ..
            } => Some(format!("Tool call {name}: {arguments}")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn push(output: &mut String, role: &str, content: &str) {
    if !content.trim().is_empty() {
        output.push_str(&format!("[{role}]\n{}\n\n", content.trim()));
    }
}
