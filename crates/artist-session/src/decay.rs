//! Replacing stale observations with stubs to reclaim model context.
//!
//! Observations are the largest thing a computer-use session puts into context,
//! and almost all of them are worthless within a few turns: the screen they
//! describe has already been acted on and changed. Keeping them costs the whole
//! window; the model only ever needs the current one plus the last few for
//! continuity.
//!
//! This is deliberately *not* part of compaction. The compaction planner picks a
//! single positional cut point and has no per-message concept at all; teaching
//! it one would change its contract and every test around it. Decay instead
//! rewrites content in place under one strict invariant:
//!
//! > Only the content blocks of a `UserContent::ToolResult` change. No message
//! > or tool result is added, removed or reordered, and no `id` or `call_id` is
//! > ever altered.
//!
//! That invariant is what keeps a provider's tool-call/tool-result pairing valid
//! and what keeps this out of the compaction planner's business.

use rig_core::completion::Message;
use rig_core::completion::message::{AssistantContent, ToolResultContent, UserContent};

/// The marker a computer observation opens with.
///
/// Load-bearing rather than decorative: it is half of how a decayable result is
/// recognized.
const SENTINEL: &str = "<observation surface=";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecayPolicy {
    /// How many observations stay materialized. `0` disables decay entirely.
    pub keep_recent: usize,
    /// Results smaller than this are left alone — the stub would not save
    /// enough to be worth the churn in the log.
    pub min_bytes: usize,
}

impl Default for DecayPolicy {
    fn default() -> Self {
        Self {
            keep_recent: 3,
            min_bytes: 256,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecayOutcome {
    pub elided: u32,
    pub bytes_saved: u64,
}

/// Replace stale observation tool results with stubs.
///
/// Returns `None` when nothing changed, so the caller can skip writing a
/// snapshot at all — decay runs before every turn and most turns have nothing
/// to do.
pub fn decay_observations(messages: &mut [Message], policy: &DecayPolicy) -> Option<DecayOutcome> {
    if policy.keep_recent == 0 {
        return None;
    }

    // A tool result carries no tool name, so the name is recovered by walking
    // the assistant tool calls that precede it. Belt and braces with the
    // sentinel: the id map alone is fragile across compaction snapshots, and
    // the sentinel alone would let a `read` of this very file trigger an
    // elision of its own output.
    let names = tool_call_names(messages);

    let mut candidates = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        let Message::User { content } = message else {
            continue;
        };
        for (block_index, item) in content.iter().enumerate() {
            let UserContent::ToolResult(result) = item else {
                continue;
            };
            if names.get(&result.id).map(String::as_str) != Some("computer") {
                continue;
            }
            if !result
                .content
                .iter()
                .any(|block| block.as_text().is_some_and(|text| text.contains(SENTINEL)))
            {
                continue;
            }
            candidates.push((message_index, block_index));
        }
    }

    if candidates.len() <= policy.keep_recent {
        return None;
    }
    let stale = &candidates[..candidates.len() - policy.keep_recent];

    let mut outcome = DecayOutcome::default();
    for (message_index, block_index) in stale {
        let Some(Message::User { content }) = messages.get_mut(*message_index) else {
            continue;
        };
        let Some(UserContent::ToolResult(result)) = content.iter_mut().nth(*block_index) else {
            continue;
        };

        let before = result_bytes(&result.content);
        if before < policy.min_bytes || is_already_elided(&result.content) {
            continue;
        }

        let stub = stub_for(&result.content);
        // Arity is preserved: replacing the block list wholesale would change
        // how many content items the tool result has, which some providers
        // validate.
        let mut replaced = false;
        for block in result.content.iter_mut() {
            // Anything that is not the observation survives verbatim. The user
            // may have steered mid-call, and `SteeringHook` merges that into
            // this very result — eliding it would silently delete the user's
            // instruction from model context while the transcript still shows
            // it, so nobody would notice the agent had stopped honouring it.
            if let Some(preserved) = non_observation_text(block) {
                *block = ToolResultContent::text(preserved);
                continue;
            }
            *block = if replaced {
                // Trailing blocks (images, extra text) collapse to a marker
                // rather than vanishing.
                ToolResultContent::text("[elided]")
            } else {
                replaced = true;
                ToolResultContent::text(stub.clone())
            };
        }

        let after = result_bytes(&result.content);
        outcome.elided += 1;
        outcome.bytes_saved += before.saturating_sub(after) as u64;
    }

    (outcome.elided > 0).then_some(outcome)
}

/// Text in a tool result that did not come from the observation.
///
/// Today that means user steering, which `SteeringHook` appends to whichever
/// block was last when the result was produced. Returns the preserved text —
/// steering only, with the observation body stripped — or `None` when the block
/// is purely observation.
fn non_observation_text(block: &ToolResultContent) -> Option<String> {
    const OPEN: &str = "<user_steering>";
    const CLOSE: &str = "</user_steering>";

    let text = block.as_text()?;
    let start = text.find(OPEN)?;
    // Everything from the first steering block onward. Steering is always
    // appended, so this keeps every message and drops the observation before it.
    let preserved = text[start..].trim();
    // A well-formed steering block is the only thing worth preserving; a bare
    // mention without a close tag is page content, not an instruction.
    preserved.contains(CLOSE).then(|| preserved.to_owned())
}

fn is_already_elided(content: &rig_core::OneOrMany<ToolResultContent>) -> bool {
    content.iter().any(|block| {
        block
            .as_text()
            .is_some_and(|text| text.contains("elided=\"true\""))
    })
}

fn result_bytes(content: &rig_core::OneOrMany<ToolResultContent>) -> usize {
    content
        .iter()
        .map(|block| match block {
            ToolResultContent::Text(text) => text.text.len(),
            ToolResultContent::Json { value } => value.to_string().len(),
            // An image is already externalized to a reference by this point, so
            // its in-context cost is the reference, not the pixels.
            ToolResultContent::Image(_) => 64,
        })
        .sum()
}

/// Build the stub, preserving the surface, epoch and image digest so the frame
/// stays recoverable by `artist computer frame` and by the inspector.
fn stub_for(content: &rig_core::OneOrMany<ToolResultContent>) -> String {
    let header = content
        .iter()
        .find_map(|block| block.as_text())
        .and_then(|text| text.lines().next())
        .unwrap_or("<observation>");

    let surface = attribute(header, "surface").unwrap_or_else(|| "?".to_owned());
    let epoch = attribute(header, "epoch").unwrap_or_else(|| "?".to_owned());
    let image = attribute(header, "img");

    let mut stub = format!("<observation surface=\"{surface}\" epoch=\"{epoch}\" elided=\"true\"");
    if let Some(digest) = image {
        stub.push_str(&format!(" img=\"{digest}\""));
    }
    stub.push_str(
        ">\n[elided to save context — call computer with mode=\"observe\" to refresh this surface]",
    );
    stub
}

fn attribute(header: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = header.find(&needle)? + needle.len();
    let rest = &header[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// Map tool-call id to tool name by walking assistant turns forward.
fn tool_call_names(messages: &[Message]) -> std::collections::HashMap<String, String> {
    let mut names = std::collections::HashMap::new();
    for message in messages {
        let Message::Assistant { content, .. } = message else {
            continue;
        };
        for item in content.iter() {
            if let AssistantContent::ToolCall(call) = item {
                names.insert(call.id.clone(), call.function.name.clone());
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig_core::OneOrMany;
    use rig_core::completion::message::{ToolCall, ToolFunction, ToolResult};

    fn call(id: &str, name: &str) -> Message {
        Message::Assistant {
            id: None,
            content: OneOrMany::one(AssistantContent::ToolCall(ToolCall::new(
                id.to_owned(),
                ToolFunction::new(name.to_owned(), serde_json::json!({})),
            ))),
        }
    }

    fn observation(id: &str, epoch: u64, filler: usize) -> Message {
        let body = "row \"data\" (anchor)\n".repeat(filler);
        Message::User {
            content: OneOrMany::one(UserContent::ToolResult(ToolResult {
                id: id.to_owned(),
                call_id: Some(format!("call_{id}")),
                content: OneOrMany::one(ToolResultContent::text(format!(
                    "<observation surface=\"win:3\" epoch=\"{epoch}\" img=\"ab12cd\">\n{body}"
                ))),
            })),
        }
    }

    /// `n` observations, each preceded by its tool call.
    fn history(count: usize) -> Vec<Message> {
        let mut messages = Vec::new();
        for index in 0..count {
            let id = format!("fc_{index}");
            messages.push(call(&id, "computer"));
            messages.push(observation(&id, index as u64, 40));
        }
        messages
    }

    fn texts(message: &Message) -> Vec<String> {
        let Message::User { content } = message else {
            return Vec::new();
        };
        content
            .iter()
            .filter_map(|item| match item {
                UserContent::ToolResult(result) => Some(
                    result
                        .content
                        .iter()
                        .filter_map(|block| block.as_text().map(str::to_owned))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn only_observations_beyond_the_keep_window_are_elided() {
        let mut messages = history(6);
        let outcome = decay_observations(&mut messages, &DecayPolicy::default()).unwrap();

        assert_eq!(outcome.elided, 3, "6 observations, keep 3");
        assert!(outcome.bytes_saved > 0);

        // The three oldest are stubs; the three newest are untouched.
        for index in 0..3 {
            assert!(texts(&messages[index * 2 + 1])[0].contains("elided=\"true\""));
        }
        for index in 3..6 {
            assert!(!texts(&messages[index * 2 + 1])[0].contains("elided=\"true\""));
        }
    }

    #[test]
    fn nothing_happens_while_within_the_keep_window() {
        let mut messages = history(3);
        assert_eq!(
            decay_observations(&mut messages, &DecayPolicy::default()),
            None
        );
    }

    #[test]
    fn the_stub_keeps_the_surface_epoch_and_image_digest() {
        let mut messages = history(5);
        decay_observations(&mut messages, &DecayPolicy::default()).unwrap();
        let stub = &texts(&messages[1])[0];

        assert!(stub.contains("surface=\"win:3\""));
        assert!(stub.contains("epoch=\"0\""));
        assert!(
            stub.contains("img=\"ab12cd\""),
            "the frame must stay recoverable: {stub}"
        );
        assert!(stub.contains("mode=\"observe\""));
    }

    #[test]
    fn structure_is_never_altered() {
        let mut messages = history(6);
        let before: Vec<_> = messages
            .iter()
            .map(|message| match message {
                Message::User { content } => (
                    "user",
                    content.len(),
                    content
                        .iter()
                        .filter_map(|item| match item {
                            UserContent::ToolResult(result) => Some((
                                result.id.clone(),
                                result.call_id.clone(),
                                result.content.len(),
                            )),
                            _ => None,
                        })
                        .collect::<Vec<_>>(),
                ),
                Message::Assistant { content, .. } => ("assistant", content.len(), Vec::new()),
                Message::System { .. } => ("system", 0, Vec::new()),
            })
            .collect();

        decay_observations(&mut messages, &DecayPolicy::default()).unwrap();

        let after: Vec<_> = messages
            .iter()
            .map(|message| match message {
                Message::User { content } => (
                    "user",
                    content.len(),
                    content
                        .iter()
                        .filter_map(|item| match item {
                            UserContent::ToolResult(result) => Some((
                                result.id.clone(),
                                result.call_id.clone(),
                                result.content.len(),
                            )),
                            _ => None,
                        })
                        .collect::<Vec<_>>(),
                ),
                Message::Assistant { content, .. } => ("assistant", content.len(), Vec::new()),
                Message::System { .. } => ("system", 0, Vec::new()),
            })
            .collect();

        assert_eq!(
            before, after,
            "decay must never change message count, arity, ids or call_ids"
        );
    }

    #[test]
    fn a_read_result_that_merely_mentions_the_sentinel_is_left_alone() {
        // Reading this very source file would put the sentinel in a `read`
        // result. Matching on the sentinel alone would then elide it.
        let mut messages = history(5);
        messages.push(call("fc_read", "read"));
        messages.push(Message::User {
            content: OneOrMany::one(UserContent::ToolResult(ToolResult {
                id: "fc_read".into(),
                call_id: None,
                content: OneOrMany::one(ToolResultContent::text(format!(
                    "{}\n{}",
                    "<observation surface=\"quoted\" epoch=\"1\">",
                    "x".repeat(4000)
                ))),
            })),
        });

        decay_observations(&mut messages, &DecayPolicy::default()).unwrap();

        let read_result = &texts(messages.last().unwrap())[0];
        assert!(
            !read_result.contains("elided=\"true\""),
            "a read result must never be decayed: {read_result}"
        );
    }

    #[test]
    fn decay_never_destroys_user_steering() {
        // The user steered while a computer call was in flight, so the message
        // rode into that observation's text block. Eliding the observation must
        // not take the instruction with it.
        let mut messages = history(5);
        let Message::User { content } = &mut messages[1] else {
            panic!("expected a tool result");
        };
        let UserContent::ToolResult(result) = content.first_mut() else {
            panic!("expected a tool result");
        };
        let ToolResultContent::Text(text) = result.content.first_mut() else {
            panic!("expected text");
        };
        text.text
            .push_str("\n\n<user_steering>\ndo not touch the billing tab\n</user_steering>");

        decay_observations(&mut messages, &DecayPolicy::default()).unwrap();

        let surviving = &texts(&messages[1])[0];
        assert!(
            surviving.contains("do not touch the billing tab"),
            "the user's instruction must survive decay: {surviving}"
        );
        assert!(
            !surviving.contains("row \"data\""),
            "the observation body should still be gone: {surviving}"
        );
    }

    #[test]
    fn a_page_that_merely_mentions_the_steering_tag_is_not_preserved() {
        // Page content containing the literal opening tag is not an
        // instruction; only a well-formed block counts.
        let block = ToolResultContent::text("<observation surface=\"x\">\n<user_steering> hi");
        assert_eq!(non_observation_text(&block), None);
    }

    #[test]
    fn decay_is_idempotent() {
        let mut messages = history(6);
        decay_observations(&mut messages, &DecayPolicy::default()).unwrap();
        let once = messages.clone();

        // A second pass has nothing left to do: already-elided results are
        // recognized and skipped, so re-running cannot cascade.
        assert_eq!(
            decay_observations(&mut messages, &DecayPolicy::default()),
            None
        );
        assert_eq!(messages, once);
    }

    #[test]
    fn small_results_are_not_worth_eliding() {
        let mut messages = Vec::new();
        for index in 0..8 {
            let id = format!("fc_{index}");
            messages.push(call(&id, "computer"));
            messages.push(observation(&id, index as u64, 0));
        }
        assert_eq!(
            decay_observations(&mut messages, &DecayPolicy::default()),
            None,
            "stubbing a tiny result costs a log write and saves nothing"
        );
    }

    #[test]
    fn a_zero_keep_window_disables_decay_entirely() {
        let mut messages = history(20);
        let policy = DecayPolicy {
            keep_recent: 0,
            ..DecayPolicy::default()
        };
        assert_eq!(decay_observations(&mut messages, &policy), None);
    }
}
