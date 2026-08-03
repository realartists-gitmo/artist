//! The transcript document — the reduction both frontends render.
//!
//! [`PromptEvent`] is already semantic: it says a tool started, not that a
//! spinner should spin. So the shared reduction takes that stream directly and
//! produces [`Block`]s, and the TUI and GUI each decide what a block looks like.
//!
//! Nothing here measures or wraps. A [`Block::Message`] holds *logical* lines;
//! the terminal wraps them to columns and the GUI wraps them to pixels.

use serde::{Deserialize, Serialize};

use crate::{inline::InlineLine, markdown::MarkdownStream};

/// Stable wire vocabulary consumed by every frontend.
///
/// This intentionally mirrors the agent's display event JSON without depending
/// on the agent crate. Keeping that dependency out prevents provider, computer,
/// and async-runtime features from leaking into native frontend processes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum PromptEvent {
    ReasoningSummaryDelta(String),
    TextDelta(String),
    SubagentStarted {
        id: String,
        role: String,
        prompt: String,
    },
    SubagentEvent {
        id: String,
        event: Box<PromptEvent>,
    },
    SubagentFinished {
        id: String,
        outcome: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    ToolExecutionStart {
        id: String,
        name: String,
    },
    ToolResult {
        id: String,
        content: String,
        outcome: Option<serde_json::Value>,
        duration_ms: Option<u64>,
        images: Vec<ToolImage>,
    },
    CompletionUsage {
        total_tokens: u64,
        cached_input_tokens: u64,
    },
    RuleFired {
        rule: String,
        matched: String,
    },
    ProviderFallback {
        from: String,
        reason: String,
    },
    HandedOff {
        from: String,
        to: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolImage {
    pub attachment: String,
    pub media_type: Option<String>,
    pub bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolStatus {
    /// The model asked for the call; it has not started.
    Requested,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCard {
    pub id: String,
    pub name: String,
    /// Pretty-printed arguments. Kept as text because every view shows them as
    /// text, and keeping `serde_json::Value` here would push formatting choices
    /// into two places.
    pub arguments: String,
    pub status: ToolStatus,
    pub result: Option<String>,
    pub duration_ms: Option<u64>,
    /// Digests of images the tool returned. The payloads live in the session
    /// attachment store. A GUI resolves and draws them; the TUI shows a count.
    /// Neither is the fallback — see the capability note on [`Transcript`].
    pub images: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoticeKind {
    /// A stream rule matched and the run retried.
    RuleFired,
    /// Routing moved to another provider.
    ProviderFallback,
    /// The session changed profile.
    HandedOff,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub kind: NoticeKind,
    pub title: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subagent {
    pub id: String,
    pub role: String,
    pub prompt: String,
    pub blocks: Vec<Block>,
    pub outcome: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Block {
    Message {
        role: Role,
        /// Original text, retained so rich frontends can render Markdown while
        /// terminal frontends continue consuming semantic logical lines.
        #[serde(default)]
        source: String,
        lines: Vec<InlineLine>,
    },
    /// Model reasoning summary. Visually distinct in both frontends, but the
    /// core does not say how.
    Reasoning {
        lines: Vec<InlineLine>,
    },
    Tool(ToolCard),
    Notice(Notice),
    Subagent(Subagent),
}

/// Token accounting for the run in progress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub total_tokens: u64,
    /// Zero means "the provider said nothing", not "measured zero".
    pub cached_input_tokens: u64,
}

/// The reduced conversation.
///
/// On capabilities: where the two frontends genuinely differ — inline images,
/// hover, smooth scrolling — the core carries *enough for either* rather than
/// the intersection. `ToolCard::images` holds digests whether or not the view
/// can draw them. That is what stops the TUI becoming a lossy GUI.
#[derive(Default)]
pub struct Transcript {
    blocks: Vec<Block>,
    usage: Usage,
    stream: MarkdownStream,
    /// Index of the assistant message currently being streamed into.
    open_message: Option<usize>,
    /// Whether the last streamed line ended without a newline, so the next
    /// chunk continues it rather than starting a line.
    line_open: bool,
    reasoning: MarkdownStream,
    open_reasoning: Option<usize>,
    reasoning_line_open: bool,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn usage(&self) -> Usage {
        self.usage
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Record something the user sent. User text is not model output, so it is
    /// not run through the markdown reader — it is shown as typed.
    pub fn push_user(&mut self, text: &str) {
        self.close_turn();
        self.blocks.push(Block::Message {
            role: Role::User,
            source: text.to_owned(),
            lines: text
                .split('\n')
                .map(|line| InlineLine {
                    inlines: vec![crate::inline::Inline::prose(line.to_owned())],
                    code: false,
                })
                .collect(),
        });
    }

    /// Fold one agent event into the document.
    pub fn apply(&mut self, event: &PromptEvent) {
        match event {
            PromptEvent::TextDelta(delta) => self.append_text(delta),
            PromptEvent::ReasoningSummaryDelta(delta) => self.append_reasoning(delta),
            PromptEvent::ToolCall {
                id,
                name,
                arguments,
            } => {
                self.close_turn();
                self.blocks.push(Block::Tool(ToolCard {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: serde_json::to_string_pretty(arguments)
                        .unwrap_or_else(|_| arguments.to_string()),
                    status: ToolStatus::Requested,
                    result: None,
                    duration_ms: None,
                    images: Vec::new(),
                }));
            }
            PromptEvent::ToolExecutionStart { id, .. } => {
                if let Some(card) = self.tool_mut(id) {
                    card.status = ToolStatus::Running;
                }
            }
            PromptEvent::ToolResult {
                id,
                content,
                outcome,
                duration_ms,
                images,
            } => {
                let failed = outcome.as_ref().is_some_and(|outcome| {
                    outcome.to_string().to_ascii_lowercase().contains("error")
                });
                if let Some(card) = self.tool_mut(id) {
                    card.status = if failed {
                        ToolStatus::Failed
                    } else {
                        ToolStatus::Done
                    };
                    card.result = Some(content.clone());
                    card.duration_ms = *duration_ms;
                    card.images = images
                        .iter()
                        .map(|image| image.attachment.clone())
                        .collect();
                }
            }
            PromptEvent::CompletionUsage {
                total_tokens,
                cached_input_tokens,
            } => {
                self.usage = Usage {
                    total_tokens: *total_tokens,
                    cached_input_tokens: *cached_input_tokens,
                };
            }
            PromptEvent::RuleFired { rule, matched } => {
                // A rule firing means the partial output was abandoned and the
                // run retried, so the open message must go with it.
                self.discard_open_message();
                self.blocks.push(Block::Notice(Notice {
                    kind: NoticeKind::RuleFired,
                    title: rule.clone(),
                    detail: matched.clone(),
                }));
            }
            PromptEvent::ProviderFallback { from, reason } => {
                self.close_turn();
                self.blocks.push(Block::Notice(Notice {
                    kind: NoticeKind::ProviderFallback,
                    title: from.clone(),
                    detail: reason.clone(),
                }));
            }
            PromptEvent::HandedOff { from, to } => {
                self.close_turn();
                self.blocks.push(Block::Notice(Notice {
                    kind: NoticeKind::HandedOff,
                    title: format!("{from} → {to}"),
                    detail: String::new(),
                }));
            }
            PromptEvent::SubagentStarted { id, role, prompt } => {
                self.close_turn();
                self.blocks.push(Block::Subagent(Subagent {
                    id: id.clone(),
                    role: role.clone(),
                    prompt: prompt.clone(),
                    blocks: Vec::new(),
                    outcome: None,
                }));
            }
            PromptEvent::SubagentEvent { id, event } => {
                // A delegate's events reduce with the same rules, into its own
                // nested transcript, so nesting depth costs nothing here.
                if let Some(subagent) = self.subagent_mut(id) {
                    let mut nested = Transcript {
                        blocks: std::mem::take(&mut subagent.blocks),
                        ..Transcript::default()
                    };
                    nested.apply(event);
                    let blocks = nested.into_blocks();
                    if let Some(subagent) = self.subagent_mut(id) {
                        subagent.blocks = blocks;
                    }
                }
            }
            PromptEvent::SubagentFinished { id, outcome } => {
                if let Some(subagent) = self.subagent_mut(id) {
                    subagent.outcome = Some(outcome.clone());
                }
            }
        }
    }

    /// End the assistant's turn, so the next delta starts a new message.
    pub fn close_turn(&mut self) {
        self.open_message = None;
        self.open_reasoning = None;
        self.line_open = false;
        self.reasoning_line_open = false;
        self.stream.reset();
        self.reasoning.reset();
    }

    pub fn into_blocks(self) -> Vec<Block> {
        self.blocks
    }

    fn append_text(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let produced = self.stream.push(delta);
        let continues = self.line_open;
        self.line_open = !delta.ends_with('\n');

        let index = match self.open_message {
            Some(index) => index,
            None => {
                self.blocks.push(Block::Message {
                    role: Role::Assistant,
                    source: String::new(),
                    lines: Vec::new(),
                });
                let index = self.blocks.len() - 1;
                self.open_message = Some(index);
                index
            }
        };
        if let Some(Block::Message { source, lines, .. }) = self.blocks.get_mut(index) {
            source.push_str(delta);
            append_lines(lines, continues, produced);
        }
    }

    fn append_reasoning(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let produced = self.reasoning.push(delta);
        let continues = self.reasoning_line_open;
        self.reasoning_line_open = !delta.ends_with('\n');

        let index = match self.open_reasoning {
            Some(index) => index,
            None => {
                self.blocks.push(Block::Reasoning { lines: Vec::new() });
                let index = self.blocks.len() - 1;
                self.open_reasoning = Some(index);
                index
            }
        };
        if let Some(Block::Reasoning { lines }) = self.blocks.get_mut(index) {
            append_lines(lines, continues, produced);
        }
    }

    fn discard_open_message(&mut self) {
        if let Some(index) = self.open_message.take()
            && index < self.blocks.len()
        {
            self.blocks.remove(index);
        }
        self.close_turn();
    }

    fn tool_mut(&mut self, id: &str) -> Option<&mut ToolCard> {
        self.blocks.iter_mut().rev().find_map(|block| match block {
            Block::Tool(card) if card.id == id => Some(card),
            _ => None,
        })
    }

    fn subagent_mut(&mut self, id: &str) -> Option<&mut Subagent> {
        self.blocks.iter_mut().rev().find_map(|block| match block {
            Block::Subagent(subagent) if subagent.id == id => Some(subagent),
            _ => None,
        })
    }
}

/// Join a chunk's lines onto the message, continuing the open line if the
/// previous chunk stopped mid-line.
fn append_lines(lines: &mut Vec<InlineLine>, continues: bool, produced: Vec<InlineLine>) {
    for (index, line) in produced.into_iter().enumerate() {
        match lines.last_mut() {
            Some(last) if index == 0 && continues => last.inlines.extend(line.inlines),
            _ => lines.push(line),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(block: &Block) -> String {
        match block {
            Block::Message { lines, .. } | Block::Reasoning { lines } => lines
                .iter()
                .map(InlineLine::plain)
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        }
    }

    #[test]
    fn joins_deltas_that_stop_mid_line() {
        // The provider splits tokens wherever it likes; a line broken across
        // three deltas must still be one line.
        let mut transcript = Transcript::new();
        for delta in ["Hel", "lo, ", "world"] {
            transcript.apply(&PromptEvent::TextDelta(delta.to_owned()));
        }
        assert_eq!(transcript.blocks().len(), 1);
        assert_eq!(text(&transcript.blocks()[0]), "Hello, world");
    }

    #[test]
    fn retains_raw_markdown_across_streamed_deltas() {
        let mut transcript = Transcript::new();
        transcript.apply(&PromptEvent::TextDelta("## Head".to_owned()));
        transcript.apply(&PromptEvent::TextDelta("ing\n\n- item".to_owned()));
        let Block::Message { source, .. } = &transcript.blocks()[0] else {
            panic!("expected a message");
        };
        assert_eq!(source, "## Heading\n\n- item");
    }

    #[test]
    fn starts_a_new_line_after_a_newline() {
        let mut transcript = Transcript::new();
        transcript.apply(&PromptEvent::TextDelta("first\n".to_owned()));
        transcript.apply(&PromptEvent::TextDelta("second".to_owned()));
        assert_eq!(text(&transcript.blocks()[0]), "first\nsecond");
    }

    #[test]
    fn keeps_fence_state_across_deltas() {
        let mut transcript = Transcript::new();
        transcript.apply(&PromptEvent::TextDelta("```rust\nfn ".to_owned()));
        transcript.apply(&PromptEvent::TextDelta("main() {}\n```".to_owned()));

        let Block::Message { lines, .. } = &transcript.blocks()[0] else {
            panic!("expected a message");
        };
        assert_eq!(lines[1].plain(), "fn main() {}");
        assert!(lines[1].code, "the code line is inside the fence");
        assert!(!lines[0].code, "the opening delimiter is not");
    }

    #[test]
    fn tool_lifecycle_updates_one_card() {
        let mut transcript = Transcript::new();
        transcript.apply(&PromptEvent::ToolCall {
            id: "t1".into(),
            name: "read".into(),
            arguments: serde_json::json!({"path": "a.rs"}),
        });
        transcript.apply(&PromptEvent::ToolExecutionStart {
            id: "t1".into(),
            name: "read".into(),
        });
        transcript.apply(&PromptEvent::ToolResult {
            id: "t1".into(),
            content: "contents".into(),
            outcome: None,
            duration_ms: Some(12),
            images: Vec::new(),
        });

        assert_eq!(transcript.blocks().len(), 1);
        let Block::Tool(card) = &transcript.blocks()[0] else {
            panic!("expected a tool card");
        };
        assert_eq!(card.status, ToolStatus::Done);
        assert_eq!(card.result.as_deref(), Some("contents"));
        assert_eq!(card.duration_ms, Some(12));
    }

    #[test]
    fn a_rule_firing_discards_the_abandoned_output() {
        // The run retries from the same point, so leaving the partial text on
        // screen would show the user output the model never committed to.
        let mut transcript = Transcript::new();
        transcript.push_user("go");
        transcript.apply(&PromptEvent::TextDelta("partial answer".to_owned()));
        transcript.apply(&PromptEvent::RuleFired {
            rule: "no-secrets".into(),
            matched: "AKIA...".into(),
        });

        assert_eq!(transcript.blocks().len(), 2);
        assert!(matches!(transcript.blocks()[1], Block::Notice(_)));
        assert!(
            !transcript
                .blocks()
                .iter()
                .any(|block| text(block).contains("partial answer"))
        );
    }

    #[test]
    fn text_after_a_tool_call_starts_a_new_message() {
        let mut transcript = Transcript::new();
        transcript.apply(&PromptEvent::TextDelta("before".to_owned()));
        transcript.apply(&PromptEvent::ToolCall {
            id: "t1".into(),
            name: "read".into(),
            arguments: serde_json::json!({}),
        });
        transcript.apply(&PromptEvent::TextDelta("after".to_owned()));

        assert_eq!(transcript.blocks().len(), 3);
        assert_eq!(text(&transcript.blocks()[0]), "before");
        assert_eq!(text(&transcript.blocks()[2]), "after");
    }

    #[test]
    fn subagent_events_reduce_into_the_nested_transcript() {
        let mut transcript = Transcript::new();
        transcript.apply(&PromptEvent::SubagentStarted {
            id: "s1".into(),
            role: "Explore".into(),
            prompt: "find it".into(),
        });
        transcript.apply(&PromptEvent::SubagentEvent {
            id: "s1".into(),
            event: Box::new(PromptEvent::TextDelta("nested".to_owned())),
        });
        transcript.apply(&PromptEvent::SubagentFinished {
            id: "s1".into(),
            outcome: "completed".into(),
        });

        let Block::Subagent(subagent) = &transcript.blocks()[0] else {
            panic!("expected a subagent");
        };
        assert_eq!(subagent.outcome.as_deref(), Some("completed"));
        assert_eq!(text(&subagent.blocks[0]), "nested");
    }

    #[test]
    fn usage_is_recorded() {
        let mut transcript = Transcript::new();
        transcript.apply(&PromptEvent::CompletionUsage {
            total_tokens: 1200,
            cached_input_tokens: 900,
        });
        assert_eq!(transcript.usage().total_tokens, 1200);
        assert_eq!(transcript.usage().cached_input_tokens, 900);
    }

    #[test]
    fn frontend_wire_format_accepts_newtype_events() {
        let encoded = serde_json::to_string(&PromptEvent::TextDelta("hello".into())).unwrap();
        assert_eq!(encoded, r#"{"type":"text_delta","data":"hello"}"#);
        assert_eq!(
            serde_json::from_str::<PromptEvent>(&encoded).unwrap(),
            PromptEvent::TextDelta("hello".into())
        );
    }
}
