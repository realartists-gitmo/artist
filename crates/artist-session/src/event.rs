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
    RunUsage(RunUsage),
    RunFinished(RunFinished),
    TaskStarted(TaskStarted),
    TaskUpdated(TaskUpdated),
    TaskFinished(TaskFinished),
    ChangeRecorded(ChangeRecorded),
    TurnUser(TurnUser),
    ModelTurn(ModelTurn),
    ToolResult(ToolResultEvent),
    ToolResultImages(ToolResultImagesEvent),
    SteeringDelivered(SteeringDelivered),
    DelegateStarted(DelegateStarted),
    DelegateFinished(DelegateFinished),
    ConversationMessages(ConversationMessages),
    ConversationCompacted(ConversationCompacted),
    HistoryRewind(HistoryRewind),
    LegacyTurn(LegacyTurn),
    RuleFired(RuleFired),
    RuleInjection(RuleInjection),
    RuleRetroFindings(RuleRetroFindings),
    HandoffPerformed(HandoffPerformed),
    TodoUpdated(TodoUpdated),
    ProviderContext(ProviderContext),
    CanvasCreated(CanvasCreated),
    CanvasOpened(CanvasOpened),
    CanvasState(CanvasState),
    AskPosted(AskPosted),
    AskAnswered(AskAnswered),
    MemoryWritten(MemoryWritten),
    ComputerLaunched(ComputerLaunched),
    ComputerStageOpened(ComputerStageOpened),
    ComputerStageClosed(ComputerStageClosed),
    ComputerObserved(ComputerObserved),
    ComputerActed(ComputerActed),
    ComputerElided(ComputerElided),
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
    (RunUsage, RunUsage, "run.usage"),
    (RunFinished, RunFinished, "run.finished"),
    (TaskStarted, TaskStarted, "task.started"),
    (TaskUpdated, TaskUpdated, "task.updated"),
    (TaskFinished, TaskFinished, "task.finished"),
    (ChangeRecorded, ChangeRecorded, "change.recorded"),
    (TurnUser, TurnUser, "turn.user"),
    (ModelTurn, ModelTurn, "model.turn"),
    (ToolResult, ToolResultEvent, "tool.result"),
    (
        ToolResultImages,
        ToolResultImagesEvent,
        "tool.result.images"
    ),
    (SteeringDelivered, SteeringDelivered, "steering.delivered"),
    (DelegateStarted, DelegateStarted, "delegate.started"),
    (DelegateFinished, DelegateFinished, "delegate.finished"),
    (
        ConversationMessages,
        ConversationMessages,
        "conversation.messages"
    ),
    (
        ConversationCompacted,
        ConversationCompacted,
        "conversation.compacted"
    ),
    (HistoryRewind, HistoryRewind, "history.rewind"),
    (LegacyTurn, LegacyTurn, "legacy.turn"),
    (RuleFired, RuleFired, "rule.fired"),
    (RuleInjection, RuleInjection, "rule.injection"),
    (RuleRetroFindings, RuleRetroFindings, "rule.retro_findings"),
    (HandoffPerformed, HandoffPerformed, "handoff.performed"),
    (TodoUpdated, TodoUpdated, "todo.updated"),
    (ProviderContext, ProviderContext, "provider.context.v1"),
    (CanvasCreated, CanvasCreated, "canvas.created"),
    (CanvasOpened, CanvasOpened, "canvas.opened"),
    (CanvasState, CanvasState, "canvas.state"),
    (AskPosted, AskPosted, "ask.posted"),
    (AskAnswered, AskAnswered, "ask.answered"),
    (MemoryWritten, MemoryWritten, "memory.written"),
    (
        ComputerStageOpened,
        ComputerStageOpened,
        "computer.stage_opened"
    ),
    (
        ComputerStageClosed,
        ComputerStageClosed,
        "computer.stage_closed"
    ),
    (ComputerLaunched, ComputerLaunched, "computer.launched"),
    (ComputerObserved, ComputerObserved, "computer.observed"),
    (ComputerActed, ComputerActed, "computer.acted"),
    (ComputerElided, ComputerElided, "computer.elided"),
);

/// An isolated graphical session started on this machine.
///
/// Recorded so a transcript shows when the agent acquired a display of its own —
/// the moment its blast radius changed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComputerStageOpened {
    pub stage: String,
    /// Which `Stage` implementation backs it (`wayland`, `pty`).
    pub backend: String,
    /// The stage's own display, never the user's.
    pub display: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComputerStageClosed {
    pub stage: String,
    pub reason: String,
}

/// One observation of a surface.
///
/// Deliberately metadata only: the node text already reaches the model as the
/// tool result, and duplicating it here would make the log grow with every look
/// at an unchanged screen. `image` names the frame in the attachment store.
/// A program was started on a surface.
///
/// Recorded because without it a distilled macro says *what was done* but not
/// *what to do it to*: the surface id it carries belonged to a session that is
/// gone, and there was no other record of the command. Replaying then needed the
/// program supplied by hand, which made the artifact half an artifact.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComputerLaunched {
    /// The surface the launch produced.
    pub surface: String,
    /// The program, as invoked.
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// True for a graphical launch onto the stage, false for a terminal.
    pub gui: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComputerObserved {
    /// Correlates to the tool row, the attachment, and `artist computer log`.
    pub internal_call_id: String,
    pub surface: String,
    pub epoch: u64,
    pub rung: u8,
    /// False for a delta observation.
    pub full: bool,
    pub nodes: u32,
    pub bytes: u64,
    pub image: Option<String>,
}

/// One step of an action program, as executed.
///
/// `label` is what the model claimed the target was called and `resolved_name`
/// is what it actually was: the pair is the audit trail for the cross-check, and
/// a mismatch is the single most useful thing to see when a run goes wrong.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComputerStep {
    pub action: String,
    pub anchor: Option<String>,
    pub label: Option<String>,
    pub resolved_name: Option<String>,
    /// The text typed or the chord pressed. Load-bearing for `distill`: a step
    /// without it cannot be rebuilt, and replays as a silent no-op.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
    pub outcome: String,
}

/// An action program ran against a surface. This is the distillation input:
/// the `(anchor, action, expect)` triples here are what a replayable macro is
/// built from.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComputerActed {
    pub internal_call_id: String,
    pub surface: String,
    pub epoch: u64,
    pub steps: Vec<ComputerStep>,
    pub settled_ms: Option<u64>,
    pub expect: Option<String>,
    pub expect_met: Option<bool>,
    /// Index of the step that failed, if the program stopped early.
    pub failed_step: Option<u32>,
}

/// Stale observations were replaced with stubs to reclaim model context.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComputerElided {
    pub count: u32,
    pub bytes_saved: u64,
}

/// A durable memory was recorded, or an existing one retired.
///
/// The memory database is a rebuildable projection, not the system of record:
/// its backend commits without syncing the WAL, and the durability opt-in does
/// not exist in the pinned release. Recording the write here is what makes the
/// store reconstructible, and — because replay runs over `visible_events` — is
/// also what makes memory rewind-aware without any extra machinery, exactly as
/// with [`TodoUpdated`].
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MemoryWritten {
    /// The **proposition** id — 32 hex digits of the content hash.
    ///
    /// Not the observation. `replaces:` is a claim that a *belief* is wrong,
    /// which is a statement about the proposition; retiring one peer's
    /// observation while four others stayed live would be incoherent. Hex
    /// rather than a pair of integers because the log is read by people.
    pub fact_id: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    /// The prose the model actually sees on recall.
    pub text: String,
    /// Which write point produced this: `tool`, `correction`, `output`, `commit`.
    pub origin: String,
    /// Set when this write retires an earlier belief.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded: Option<String>,
    /// The vector, so a peer replaying this log can index the fact without
    /// re-deriving it.
    ///
    /// It used to be omitted as "derivable", which held while the only replayer
    /// was the machine that wrote it. Once logs replicate it stops holding:
    /// deriving costs a forward pass at ~6-7 sequences/sec, so a peer import
    /// turns into an hours-long re-embed of things already embedded once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub embedding: Vec<f32>,
    /// Which embedding space [`Self::embedding`] belongs to.
    ///
    /// Without it the vector is unusable on any machine that might be running a
    /// different model: same width, different space, silently wrong neighbours.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub embedder: String,
}

/// A canvas was scaffolded into the project.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CanvasCreated {
    pub slug: String,
    pub title: String,
    /// Which starter it was grown from, for the record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

/// A canvas was handed to the user.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CanvasOpened {
    pub slug: String,
    /// Deliberately not the URL: it carries the session key, and an event log
    /// is copied into forks and read by tooling.
    #[serde(default)]
    pub port: u16,
}

/// A canvas's shared state after a write.
///
/// A snapshot rather than a delta, mirroring `todo.updated`: replay is
/// last-writer-wins per canvas, which keeps rewind correct without folding an
/// edit history. Like todos, this is harness-owned and survives the context
/// wipe a handoff performs — a canvas is a durable app, not conversation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CanvasState {
    pub slug: String,
    pub rev: u64,
    pub entries: serde_json::Value,
}

/// A question was put to the user.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AskPosted {
    pub question: crate::ask::Question,
}

/// ...and answered, or dismissed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AskAnswered {
    pub answer: crate::ask::Answer,
    /// Human decisions and deterministic auto-resolution are both durable.
    #[serde(default = "default_answer_source")]
    pub source: crate::ask::AnswerSource,
    /// Which surface the user answered on — `tui`, `canvas:<slug>`, or `auto_resolve`.
    #[serde(default)]
    pub surface: String,
}

fn default_answer_source() -> crate::ask::AnswerSource {
    crate::ask::AnswerSource::Human
}

/// A durable, provider-private context snapshot. Values are deliberately
/// opaque: projections must never interpret or render them.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProviderContext {
    pub conversation_id: String,
    pub provider: String,
    pub schema: u32,
    pub items: Vec<serde_json::Value>,
    /// Stable fingerprints of the framework history incorporated into `items`.
    /// Absent on schema-v1 snapshots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_fingerprints: Vec<String>,
}

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
    /// The agent's display name, and the actor id it renders.
    ///
    /// Both, because the name is only a rendering: recording the pair lets a
    /// reader resolve a name in a transcript back to the lineage that produced
    /// it without consulting the roster, which by then may have released the
    /// name to someone else. Equal values mean the roster was exhausted and the
    /// agent is going by its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Durable routing profile. Absent in logs written before workspace hosts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// A Bash process became an addressable session object.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TaskStarted {
    /// Durable tool-call id, unique within the lineage.
    pub task: String,
    pub command: String,
    #[serde(default)]
    pub persistent: bool,
}

/// Incremental output or metadata for a running Bash task.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TaskUpdated {
    pub task: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
}

/// Terminal state of a Bash task.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TaskFinished {
    pub task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub interrupted: bool,
}

/// A file mutation causally associated with one tool call.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChangeRecorded {
    pub tool_call_id: String,
    pub path: String,
    pub before_digest: String,
    pub after_digest: String,
    /// Inline for ordinary changes; large diffs live in the attachment store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_attachment: Option<String>,
}

/// What one completion call cost, and how much of it the provider served from
/// its prompt cache.
///
/// Recorded per call rather than per run because the cache answer changes
/// within a run: a fallback to another candidate lands on a different model,
/// and caches are model-scoped, so that attempt starts cold.
///
/// `cached_input_tokens` is the only signal either provider gives that caching
/// is working at all. Both the minimum cacheable prefix and its TTL are
/// undocumented at runtime — no API reports them — so the question worth asking
/// is not "what is the threshold" but "did this prefix cache", which this
/// answers on every call for free. A sustained zero across requests that share
/// a prefix means either the prefix is under the provider's minimum or
/// something is invalidating it; both have the same fix, so the number is more
/// useful than the threshold would be.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RunUsage {
    /// The account that served this call, which is not necessarily the one the
    /// run started on — fallback moves between candidates.
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    /// Input tokens the provider served from cache. Zero when the provider
    /// reports no cache detail, which is indistinguishable from a genuine miss
    /// — read it as "no evidence of a hit", not as a measured zero.
    #[serde(default)]
    pub cached_input_tokens: u64,
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

/// Image content from a tool result. rig's capture hook only exposes the
/// result *text*, so a tool that returns images (e.g. a screenshot MCP tool)
/// records them here from the streaming display path, keyed by
/// `internal_call_id`, and history replay reattaches them to the tool result.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolResultImagesEvent {
    pub internal_call_id: String,
    /// `ContentBlock::Image` references (bytes live in the attachment store).
    pub images: Vec<ContentBlock>,
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

/// Rig-native conversation messages committed after a successful agent run.
/// `reset` replaces prior conversation batches; otherwise this batch appends.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConversationMessages {
    pub messages: Vec<rig_core::completion::Message>,
    #[serde(default)]
    pub reset: bool,
    /// Prefix retained for model continuity but already represented by legacy
    /// replay events, so resume/transcript projections should skip it.
    #[serde(default)]
    pub display_from: usize,
}

/// Audit metadata for a successful model-context compaction. The adjacent
/// reset snapshot is authoritative; this event is operational and never enters
/// the model conversation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConversationCompacted {
    pub summary: String,
    pub tokens_before: u64,
    pub kept_messages: usize,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modified_files: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoOpenStatus {
    Idle,
    Active,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoClosedStatus {
    Done,
    Failed,
    Cancelled,
    Inherited,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum TodoStatus {
    Open { open: TodoOpenStatus },
    Closed { closed: TodoClosedStatus },
}

impl Default for TodoStatus {
    fn default() -> Self {
        Self::Open {
            open: TodoOpenStatus::Idle,
        }
    }
}

impl TodoStatus {
    pub fn is_open(self) -> bool {
        matches!(self, Self::Open { .. })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoItem {
    pub text: String,
    #[serde(default)]
    pub status: TodoStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TodoItem>,
}

/// The complete todo tree after an atomic tool update. Replay is
/// last-writer-wins per artist identity.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TodoUpdated {
    pub owner: String,
    pub items: Vec<TodoItem>,
}

/// A profile handed the session to another profile. Audit metadata: the
/// adjacent conversation reset carries the authoritative replacement context,
/// exactly as with compaction.
///
/// Resume reads the most recent one of these to decide which profile the
/// session is running as; rewinding past it restores the previous profile
/// because the event is masked along with everything after it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HandoffPerformed {
    pub from: String,
    pub to: String,
    #[serde(alias = "summary")]
    pub brief: String,
    /// Every profile this session has passed through, oldest first. Same-profile
    /// handoff is allowed; the chain is retained only for durable history.
    #[serde(default)]
    pub chain: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modified_files: Vec<String>,
    /// Background subagents still running at the moment of the handoff, so the
    /// incoming profile inherits them rather than orphaning their results.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<String>,
    /// The harness-owned todo list, carried verbatim. This is the one thing
    /// crossing the boundary that is not lossy prose.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub todos: Vec<TodoItem>,
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

    /// The cache figure is the reason this event exists, and a zero is a
    /// meaningful reading rather than a default to be skipped — so it has to
    /// survive the round trip even when it is zero.
    #[test]
    fn usage_round_trips_including_a_zero_cache_reading() {
        for cached in [0, 4096] {
            let event = SessionEvent::from(RunUsage {
                provider: "openai".into(),
                model: "gpt-test".into(),
                input_tokens: 8192,
                output_tokens: 256,
                total_tokens: 8448,
                cached_input_tokens: cached,
            });
            let stored = envelope(event.kind(), event.payload());
            let parsed: Envelope = serde_json::from_str(&serde_json::to_string(&stored).unwrap())
                .expect("usage envelopes parse");
            assert_eq!(parsed.event(), event, "cached_input_tokens={cached}");
        }
    }

    /// A log written before `run.usage` existed replays as a usage record with
    /// no cache evidence, rather than as an unknown kind.
    #[test]
    fn usage_tolerates_a_payload_written_before_the_cache_field() {
        let stored = envelope(
            "run.usage",
            serde_json::json!({"provider": "x", "model": "y"}),
        );
        assert_eq!(
            stored.event(),
            SessionEvent::from(RunUsage {
                provider: "x".into(),
                model: "y".into(),
                input_tokens: 0,
                output_tokens: 0,
                total_tokens: 0,
                cached_input_tokens: 0,
            })
        );
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
