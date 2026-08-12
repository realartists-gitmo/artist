//! Deterministic normalization of Artist session logs into Muse's protocol-neutral
//! structured tool representation.
//!
//! The adapter is pinned to one exact Artist source revision. It parses the frozen
//! session envelope independently rather than depending on Artist crates, so the
//! training/runtime contract is reviewable and content-addressable on its own.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use muse_core::ConceptId;
use muse_occurrence::DiscourseRole;
use muse_tooling::{
    ArtifactDescriptor, ArtifactStateDescriptor, EffectEvidence, ExecutionDetails, IdentityScope,
    Principal, StructuredSource, StructuredValue, TOOL_SCHEMA_VERSION, ToolDescriptor, ToolEffect,
    ToolInvocationRecord, ToolResultPayloadFragment, ToolResultRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

mod event_formalizer;
pub use event_formalizer::ArtistEventFormalizer;

/// Repository whose log contract this adapter implements.
pub const ARTIST_REPOSITORY: &str = "realartists-gitmo/artist";
/// Mutable branch audited to select the pinned commit.
pub const ARTIST_BRANCH: &str = "Gortnite";
/// Exact Artist commit defining the audited session/tool schema.
pub const ARTIST_GORTNITE_COMMIT: &str = "656383b4906a796727b09a249ef5da7e60f51b81";
/// Exact Git tree for [`ARTIST_GORTNITE_COMMIT`].
pub const ARTIST_GORTNITE_TREE: &str = "d37315bb8d9345d51d37699867f6b1e8c1b0631d";
/// Latest payload schema understood by this intermediate adapter.
pub const ARTIST_SESSION_SCHEMA_VERSION: u32 = 7;
/// Historical schemas are deliberately decoded by their own
/// pinned surface inventories; new schemas must add another explicit branch.
pub const ARTIST_SUPPORTED_SESSION_SCHEMA_VERSIONS: [u32; 7] = [1, 2, 3, 4, 5, 6, 7];
/// Version string recorded in Muse deterministic derivations.
pub const ARTIST_ADAPTER_VERSION: &str = "muse-artist-adapter-3+artist.session-schema-2";
pub const ARTIST_V3_ADAPTER_VERSION: &str = "muse-artist-adapter-4+artist.session-schema-3";
pub const ARTIST_V4_ADAPTER_VERSION: &str = "muse-artist-adapter-5+artist.session-schema-4";
pub const ARTIST_V1_ADAPTER_VERSION: &str =
    "muse-artist-adapter-2+artist.656383b4906a796727b09a249ef5da7e60f51b81";
/// Version of the total known-event formalizer at the pinned Artist commit.
pub const ARTIST_EVENT_FORMALIZER_VERSION: &str =
    "muse-artist-event-formalizer-2+artist.session-schema-2";
pub const ARTIST_V3_EVENT_FORMALIZER_VERSION: &str =
    "muse-artist-event-formalizer-3+artist.session-schema-3";
pub const ARTIST_V4_EVENT_FORMALIZER_VERSION: &str =
    "muse-artist-event-formalizer-4+artist.session-schema-4";
pub const ARTIST_V5_ADAPTER_VERSION: &str = "muse-artist-adapter-6+artist.session-schema-5";
pub const ARTIST_V5_EVENT_FORMALIZER_VERSION: &str =
    "muse-artist-event-formalizer-5+artist.session-schema-5";
pub const ARTIST_V6_ADAPTER_VERSION: &str = "muse-artist-adapter-7+artist.session-schema-6";
pub const ARTIST_V6_EVENT_FORMALIZER_VERSION: &str =
    "muse-artist-event-formalizer-6+artist.session-schema-6";
pub const ARTIST_V7_ADAPTER_VERSION: &str = "muse-artist-adapter-8+artist.session-schema-7";
pub const ARTIST_V7_EVENT_FORMALIZER_VERSION: &str =
    "muse-artist-event-formalizer-7+artist.session-schema-7";
pub const ARTIST_V1_EVENT_FORMALIZER_VERSION: &str =
    "muse-artist-event-formalizer-1+artist.656383b4906a796727b09a249ef5da7e60f51b81";

/// Built-in tool names from the exhaustive `Tool::ALL` registry at the pinned commit.
pub const ARTIST_V1_BUILTIN_TOOLS: [&str; 31] = [
    "bash",
    "read",
    "find",
    "grep",
    "edit",
    "write",
    "skill",
    "todo",
    "memory",
    "code_map",
    "code_show",
    "code_surface",
    "code_implements",
    "code_deps",
    "code_cycles",
    "code_calls",
    "code_trace",
    "code_impact",
    "code_search",
    "code_related",
    "ast_query",
    "ast_rewrite",
    "computer",
    "canvas",
    "handoff",
    "subagent",
    "poll",
    "abort",
    "send",
    "list",
    "ask",
];
pub const ARTIST_BUILTIN_TOOLS: [&str; 41] = [
    "bash",
    "run",
    "read",
    "find",
    "grep",
    "edit",
    "write",
    "skill",
    "todo",
    "memory",
    "code_map",
    "code_show",
    "code_surface",
    "code_implements",
    "code_deps",
    "code_cycles",
    "code_calls",
    "code_trace",
    "code_impact",
    "code_search",
    "code_related",
    "ast_query",
    "ast_rewrite",
    "computer",
    "canvas",
    "handoff",
    "agent",
    "subagent",
    "poll",
    "stop",
    "abort",
    "delete",
    "send",
    "list",
    "page",
    "ask",
    "relationship",
    "eval",
    "debug",
    "lsp",
    "forge",
];

/// Every event kind emitted by the pinned schema. This list is deliberately
/// exhaustive at the commit pin; a future kind remains explicit as unknown.
pub const ARTIST_KNOWN_EVENT_KINDS: [&str; 37] = [
    "session.created",
    "run.started",
    "run.usage",
    "run.finished",
    "task.started",
    "task.updated",
    "task.finished",
    "change.recorded",
    "turn.user",
    "model.turn",
    "tool.result",
    "tool.result.images",
    "steering.delivered",
    "delegate.started",
    "delegate.finished",
    "conversation.messages",
    "conversation.compacted",
    "history.rewind",
    "legacy.turn",
    "rule.fired",
    "rule.injection",
    "rule.retro_findings",
    "handoff.performed",
    "todo.updated",
    "provider.context.v1",
    "canvas.created",
    "canvas.opened",
    "canvas.state",
    "ask.posted",
    "ask.answered",
    "memory.written",
    "computer.stage_opened",
    "computer.stage_closed",
    "computer.launched",
    "computer.observed",
    "computer.acted",
    "computer.elided",
];
pub const ARTIST_V2_KNOWN_EVENT_KINDS: [&str; 38] = [
    "session.created",
    "run.started",
    "run.usage",
    "run.finished",
    "task.started",
    "task.updated",
    "task.finished",
    "change.recorded",
    "turn.user",
    "model.turn",
    "tool.result",
    "tool.result.images",
    "steering.delivered",
    "delegate.started",
    "delegate.finished",
    "conversation.messages",
    "conversation.compacted",
    "history.rewind",
    "legacy.turn",
    "rule.fired",
    "rule.injection",
    "rule.retro_findings",
    "handoff.performed",
    "todo.updated",
    "provider.context.v1",
    "tool.context.v1",
    "canvas.created",
    "canvas.opened",
    "canvas.state",
    "ask.posted",
    "ask.answered",
    "memory.written",
    "computer.stage_opened",
    "computer.stage_closed",
    "computer.launched",
    "computer.observed",
    "computer.acted",
    "computer.elided",
];
/// Schema v3 only expands payload fields of the existing context event.
pub const ARTIST_V3_KNOWN_EVENT_KINDS: [&str; 38] = ARTIST_V2_KNOWN_EVENT_KINDS;
pub const ARTIST_V4_KNOWN_EVENT_KINDS: [&str; 39] = [
    "session.created",
    "run.started",
    "run.usage",
    "run.finished",
    "task.started",
    "task.updated",
    "task.finished",
    "change.recorded",
    "turn.user",
    "model.turn",
    "tool.result",
    "tool.result.images",
    "steering.delivered",
    "delegate.started",
    "delegate.finished",
    "work_unit.yield.v1",
    "conversation.messages",
    "conversation.compacted",
    "history.rewind",
    "legacy.turn",
    "rule.fired",
    "rule.injection",
    "rule.retro_findings",
    "handoff.performed",
    "todo.updated",
    "provider.context.v1",
    "tool.context.v1",
    "canvas.created",
    "canvas.opened",
    "canvas.state",
    "ask.posted",
    "ask.answered",
    "memory.written",
    "computer.stage_opened",
    "computer.stage_closed",
    "computer.launched",
    "computer.observed",
    "computer.acted",
    "computer.elided",
];
pub const ARTIST_V5_KNOWN_EVENT_KINDS: [&str; 39] = ARTIST_V4_KNOWN_EVENT_KINDS;

/// Lossless normalized view of every known Artist envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistStructuredEvent {
    pub schema_version: u32,
    pub adapter_version: String,
    pub source: StructuredSource,
    pub session: String,
    pub lineage: String,
    pub run: Option<String>,
    pub kind: String,
    pub sequence: u64,
    pub timestamp_millis: u64,
    pub payload: Value,
}

/// Frozen one-line Artist session envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistEnvelope {
    pub v: u32,
    pub seq: u64,
    pub ts: u64,
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    pub lineage: String,
    pub kind: String,
    pub payload: Value,
}

/// Prose channel exposed by the Artist log. Structured tool calls/results remain
/// available through `tool_invocations`; this channel exists for prose→formal use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtistProseChannel {
    UserText,
    AgentText,
    AgentReasoningSummary,
    AgentReasoningText,
    ToolResultText,
    LegacyUserText,
    LegacyAgentText,
}

/// One exact prose span and the discourse participant who produced/presented it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistProseBlock {
    pub source: StructuredSource,
    pub speaker: Principal,
    pub channel: ArtistProseChannel,
    pub text: String,
    pub source_field: String,
    /// `false` for tool-result prose because default SLM corpus construction
    /// intentionally trains on non-tool-call transcript prose; runtime may still
    /// choose to formalize this text separately.
    pub include_in_default_training: bool,
}

/// Why a source event/fragment was preserved outside the current deterministic
/// specialization. Residual data is never silently discarded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidualReason {
    UnsupportedPayloadVersion,
    NotYetSpecialized,
    UnknownContentBlock,
    MalformedKnownPayload,
    UnmatchedFileChange,
    PartiallySpecialized,
}

/// Losslessly retained Artist source material not consumed by a specialization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistResidual {
    pub source: StructuredSource,
    pub kind: String,
    pub reason: ResidualReason,
    pub value: Value,
}

/// Complete normalization result for one Artist session log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistNormalization {
    pub structured_events: Vec<ArtistStructuredEvent>,
    pub tool_invocations: Vec<ToolInvocationRecord>,
    pub prose: Vec<ArtistProseBlock>,
    pub residual: Vec<ArtistResidual>,
}

#[derive(Clone, Debug)]
struct PendingChange {
    envelope: ArtistEnvelope,
    tool_call_id: String,
    path: String,
    before_digest: String,
    after_digest: String,
    diff: Option<String>,
    diff_attachment: Option<String>,
}

#[derive(Clone, Debug)]
struct PendingImages {
    envelope: ArtistEnvelope,
    internal_call_id: String,
}

/// Strict stateful normalizer for one ordered `events.jsonl` session.
#[derive(Default)]
pub struct ArtistNormalizer {
    session: Option<String>,
    previous_seq: Option<u64>,
    structured_events: Vec<ArtistStructuredEvent>,
    records: Vec<ToolInvocationRecord>,
    invocation_index: BTreeMap<String, usize>,
    aliases: BTreeMap<String, usize>,
    run_identities: BTreeMap<String, Principal>,
    prose: Vec<ArtistProseBlock>,
    residual: Vec<ArtistResidual>,
    deferred_changes: Vec<PendingChange>,
    deferred_images: Vec<PendingImages>,
}

impl ArtistNormalizer {
    /// Normalize newline-delimited Artist envelopes.
    pub fn normalize_jsonl(input: &str) -> Result<ArtistNormalization, ArtistAdapterError> {
        let mut normalizer = Self::default();
        for (line_index, line) in input.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let envelope = serde_json::from_str::<ArtistEnvelope>(line).map_err(|source| {
                ArtistAdapterError::JsonLine {
                    line: line_index + 1,
                    source,
                }
            })?;
            normalizer.push(envelope)?;
        }
        normalizer.finish()
    }

    /// Consume one source-order envelope.
    pub fn push(&mut self, envelope: ArtistEnvelope) -> Result<(), ArtistAdapterError> {
        self.validate_envelope_order(&envelope)?;
        if !is_supported_schema(envelope.v) {
            let source = source_for(&envelope, None)?;
            self.residual.push(ArtistResidual {
                source,
                kind: envelope.kind.clone(),
                reason: ResidualReason::UnsupportedPayloadVersion,
                value: envelope.payload,
            });
            return Ok(());
        }

        if !is_known_event_kind(envelope.v, &envelope.kind) {
            let source = source_for(&envelope, None)?;
            self.residual.push(ArtistResidual {
                source,
                kind: envelope.kind.clone(),
                reason: ResidualReason::NotYetSpecialized,
                value: envelope.payload.clone(),
            });
            return Ok(());
        }
        self.structured_events.push(ArtistStructuredEvent {
            schema_version: envelope.v,
            adapter_version: adapter_version_for_schema(envelope.v).into(),
            source: source_for(&envelope, None)?,
            session: envelope.session.clone(),
            lineage: envelope.lineage.clone(),
            run: envelope.run.clone(),
            kind: envelope.kind.clone(),
            sequence: envelope.seq,
            timestamp_millis: envelope.ts,
            payload: envelope.payload.clone(),
        });

        match envelope.kind.as_str() {
            "run.started" => self.run_started(&envelope),
            "turn.user" => self.turn_user(&envelope),
            "model.turn" => self.model_turn(&envelope),
            "tool.result" => self.tool_result(&envelope),
            "tool.result.images" => self.tool_result_images(envelope),
            "change.recorded" => self.change_recorded(envelope),
            "legacy.turn" => self.legacy_turn(&envelope),
            _ => {
                let source = source_for(&envelope, None)?;
                self.residual.push(ArtistResidual {
                    source,
                    kind: envelope.kind.clone(),
                    reason: ResidualReason::NotYetSpecialized,
                    value: envelope.payload.clone(),
                });
                Ok(())
            }
        }
    }

    /// Resolve deferred correlations and return source-order normalized records.
    pub fn finish(mut self) -> Result<ArtistNormalization, ArtistAdapterError> {
        for pending in std::mem::take(&mut self.deferred_images) {
            if !self.attach_tool_result_images(&pending.envelope, &pending.internal_call_id)? {
                let source = source_for(&pending.envelope, None)?;
                self.residual.push(ArtistResidual {
                    source,
                    kind: pending.envelope.kind.clone(),
                    reason: ResidualReason::NotYetSpecialized,
                    value: pending.envelope.payload,
                });
            }
        }
        for change in std::mem::take(&mut self.deferred_changes) {
            if !self.attach_change(&change)? {
                let source = source_for(&change.envelope, None)?;
                self.residual.push(ArtistResidual {
                    source,
                    kind: change.envelope.kind.clone(),
                    reason: ResidualReason::UnmatchedFileChange,
                    value: change.envelope.payload,
                });
            }
        }
        self.records.sort_by_key(|record| {
            (
                record.source.sequence.unwrap_or(u64::MAX),
                record.source.record.clone(),
                record.invocation_id.clone(),
            )
        });
        self.prose.sort_by_key(|block| {
            (
                block.source.sequence.unwrap_or(u64::MAX),
                block.source.record.clone(),
                block.source_field.clone(),
            )
        });
        self.structured_events.sort_by_key(|event| event.sequence);
        Ok(ArtistNormalization {
            structured_events: self.structured_events,
            tool_invocations: self.records,
            prose: self.prose,
            residual: self.residual,
        })
    }

    fn validate_envelope_order(
        &mut self,
        envelope: &ArtistEnvelope,
    ) -> Result<(), ArtistAdapterError> {
        if envelope.session.trim().is_empty() {
            return Err(ArtistAdapterError::InvalidEnvelope(
                "empty session id".into(),
            ));
        }
        if envelope.lineage.trim().is_empty() {
            return Err(ArtistAdapterError::InvalidEnvelope("empty lineage".into()));
        }
        if envelope.kind.trim().is_empty() {
            return Err(ArtistAdapterError::InvalidEnvelope(
                "empty event kind".into(),
            ));
        }
        if let Some(session) = &self.session {
            if session != &envelope.session {
                return Err(ArtistAdapterError::MixedSessions {
                    expected: session.clone(),
                    actual: envelope.session.clone(),
                });
            }
        } else {
            self.session = Some(envelope.session.clone());
        }
        if let Some(previous) = self.previous_seq {
            if envelope.seq <= previous {
                return Err(ArtistAdapterError::NonMonotonicSequence {
                    previous,
                    current: envelope.seq,
                });
            }
        }
        self.previous_seq = Some(envelope.seq);
        Ok(())
    }

    fn run_started(&mut self, envelope: &ArtistEnvelope) -> Result<(), ArtistAdapterError> {
        let Some(run) = &envelope.run else {
            return self.malformed(envelope, "run.started without envelope run id");
        };
        let actor = optional_string(&envelope.payload, "actor")?;
        let agent = optional_string(&envelope.payload, "agent")?;
        let principal = Principal {
            id: actor.unwrap_or_else(|| format!("lineage:{}", envelope.lineage)),
            scope: IdentityScope::Source,
            sort: ConceptId::from("agent:SoftwareAgent"),
            discourse_role: DiscourseRole::Agent,
            label: agent,
        };
        self.run_identities
            .insert(run_identity_key(envelope, run), principal);
        let source = source_for(envelope, None)?;
        self.residual.push(ArtistResidual {
            source,
            kind: envelope.kind.clone(),
            reason: ResidualReason::PartiallySpecialized,
            value: envelope.payload.clone(),
        });
        Ok(())
    }

    fn turn_user(&mut self, envelope: &ArtistEnvelope) -> Result<(), ArtistAdapterError> {
        let Some(content) = envelope.payload.get("content").and_then(Value::as_array) else {
            return self.malformed(envelope, "turn.user content is not an array");
        };
        let source = source_for(envelope, None)?;
        for (index, block) in content.iter().enumerate() {
            match block_type(block) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        self.prose.push(ArtistProseBlock {
                            source: source.clone(),
                            speaker: user_principal(),
                            channel: ArtistProseChannel::UserText,
                            text: text.to_owned(),
                            source_field: format!("$.payload.content.{index}.text"),
                            include_in_default_training: true,
                        });
                    } else {
                        self.residual_fragment(
                            &source,
                            envelope,
                            block,
                            ResidualReason::MalformedKnownPayload,
                            index,
                        );
                    }
                }
                _ => self.residual_fragment(
                    &source,
                    envelope,
                    block,
                    ResidualReason::UnknownContentBlock,
                    index,
                ),
            }
        }
        Ok(())
    }

    fn model_turn(&mut self, envelope: &ArtistEnvelope) -> Result<(), ArtistAdapterError> {
        let Some(content) = envelope.payload.get("content").and_then(Value::as_array) else {
            return self.malformed(envelope, "model.turn content is not an array");
        };
        let turn = envelope
            .payload
            .get("turn")
            .and_then(Value::as_u64)
            .map(|value| value.to_string());
        let source = source_for(envelope, turn)?;
        let invoker = self.agent_principal(envelope);

        for (index, block) in content.iter().enumerate() {
            match block_type(block) {
                Some("text") => self.push_agent_prose(
                    &source,
                    &invoker,
                    ArtistProseChannel::AgentText,
                    block,
                    index,
                    true,
                ),
                Some("reasoning_summary") => self.push_agent_prose(
                    &source,
                    &invoker,
                    ArtistProseChannel::AgentReasoningSummary,
                    block,
                    index,
                    true,
                ),
                Some("reasoning_text") => self.push_agent_prose(
                    &source,
                    &invoker,
                    ArtistProseChannel::AgentReasoningText,
                    block,
                    index,
                    true,
                ),
                Some("tool_call") => {
                    self.model_tool_call(envelope, &source, &invoker, block, index)?;
                }
                Some("reasoning_encrypted" | "reasoning_redacted" | "image") => {
                    self.residual_fragment(
                        &source,
                        envelope,
                        block,
                        ResidualReason::NotYetSpecialized,
                        index,
                    );
                }
                _ => self.residual_fragment(
                    &source,
                    envelope,
                    block,
                    ResidualReason::UnknownContentBlock,
                    index,
                ),
            }
        }
        Ok(())
    }

    fn model_tool_call(
        &mut self,
        envelope: &ArtistEnvelope,
        source: &StructuredSource,
        invoker: &Principal,
        block: &Value,
        index: usize,
    ) -> Result<(), ArtistAdapterError> {
        let id = required_string(block, "id")?;
        let name = required_string(block, "name")?;
        let arguments = block.get("arguments").ok_or_else(|| {
            ArtistAdapterError::MalformedPayload("tool_call missing arguments".into())
        })?;
        let call_id = optional_string(block, "call_id")?;
        let key = invocation_key(envelope, &id);
        if self.invocation_index.contains_key(&key) {
            return Err(ArtistAdapterError::DuplicateInvocation(id));
        }

        let record = ToolInvocationRecord {
            schema_version: TOOL_SCHEMA_VERSION.into(),
            source: source.clone(),
            invocation_id: id.clone(),
            invocation_scope: invocation_scope(source),
            invoker: invoker.clone(),
            tool: tool_descriptor(&name, envelope.v),
            invocation_sort: tool_invocation_sort(&name),
            arguments: object_to_structured(arguments)?,
            result: None,
            effects: requested_effects(
                &name,
                arguments,
                source,
                &format!("$.payload.content.{index}.arguments"),
                index,
            ),
            retry_of: None,
            parent_invocation: None,
        };
        record.validate()?;
        let position = self.records.len();
        self.records.push(record);
        self.invocation_index.insert(key, position);
        self.register_alias(envelope, &id, position)?;
        if let Some(call_id) = call_id {
            self.register_alias(envelope, &call_id, position)?;
        }
        if block.get("signature").is_some_and(|value| !value.is_null()) {
            self.residual_fragment(
                source,
                envelope,
                block,
                ResidualReason::PartiallySpecialized,
                index,
            );
        }
        Ok(())
    }

    fn tool_result(&mut self, envelope: &ArtistEnvelope) -> Result<(), ArtistAdapterError> {
        let internal_call_id = required_string(&envelope.payload, "internal_call_id")?;
        let name = required_string(&envelope.payload, "name")?;
        let arguments = envelope.payload.get("arguments").ok_or_else(|| {
            ArtistAdapterError::MalformedPayload("tool.result missing arguments".into())
        })?;
        let result_text = required_string(&envelope.payload, "result")?;
        // Schema v7 records the actual model-facing form separately from its
        // canonical decoding. Older events remain literal by definition.
        let (visible_result, canonical_result, result_source_field) = envelope
            .payload
            .get("presentation")
            .and_then(Value::as_object)
            .and_then(|presentation| {
                Some((
                    presentation.get("visible")?.as_str()?.to_owned(),
                    presentation.get("canonical")?.as_str()?.to_owned(),
                    "$.payload.presentation.canonical".to_owned(),
                ))
            })
            .unwrap_or_else(|| {
                (
                    result_text.clone(),
                    result_text.clone(),
                    "$.payload.result".to_owned(),
                )
            });
        let provider_call_id = optional_string(&envelope.payload, "tool_call_id")?;
        let source = source_for(envelope, None)?;
        let outcome = parse_outcome(envelope.payload.get("outcome").ok_or_else(|| {
            ArtistAdapterError::MalformedPayload("tool.result missing outcome".into())
        })?)?;
        let duration_ms = envelope.payload.get("duration_ms").and_then(Value::as_u64);

        let position = if let Some(position) = self.lookup_alias(envelope, &internal_call_id) {
            position
        } else {
            let invoker = self.agent_principal(envelope);
            let record = ToolInvocationRecord {
                schema_version: TOOL_SCHEMA_VERSION.into(),
                source: source.clone(),
                invocation_id: internal_call_id.clone(),
                invocation_scope: invocation_scope(&source),
                invoker,
                tool: tool_descriptor(&name, envelope.v),
                invocation_sort: tool_invocation_sort(&name),
                arguments: object_to_structured(arguments)?,
                result: None,
                effects: requested_effects(&name, arguments, &source, "$.payload.arguments", 0),
                retry_of: None,
                parent_invocation: None,
            };
            let position = self.records.len();
            self.records.push(record);
            self.invocation_index
                .insert(invocation_key(envelope, &internal_call_id), position);
            self.register_alias(envelope, &internal_call_id, position)?;
            position
        };

        let expected_arguments = object_to_structured(arguments)?;
        {
            let record = self
                .records
                .get(position)
                .ok_or_else(|| ArtistAdapterError::Internal("tool index out of bounds".into()))?;
            if record.tool.name != name || record.arguments != expected_arguments {
                return Err(ArtistAdapterError::InconsistentToolResult {
                    invocation: internal_call_id,
                });
            }
            if record.result.is_some() {
                return Err(ArtistAdapterError::DuplicateToolResult(
                    record.invocation_id.clone(),
                ));
            }
        }
        if let Some(call_id) = provider_call_id {
            self.register_alias(envelope, &call_id, position)?;
        }
        let execution = if name == "bash" {
            parse_bash_execution(&canonical_result)
        } else {
            ExecutionDetails::default()
        };
        let execution_source_field =
            execution_has_data(&execution).then(|| result_source_field.clone());
        let record = self
            .records
            .get_mut(position)
            .ok_or_else(|| ArtistAdapterError::Internal("tool index out of bounds".into()))?;
        record.result = Some(ToolResultRecord {
            source_record: if record.source.record == source.record {
                None
            } else {
                Some(source.clone())
            },
            status: outcome.status,
            detail: outcome.detail,
            payloads: vec![ToolResultPayloadFragment {
                source_record: None,
                value: StructuredValue::String(canonical_result),
                source_field: result_source_field.clone(),
            }],
            execution,
            status_source_field: "$.payload.outcome".into(),
            execution_source_field,
            duration_ms,
            duration_source_field: duration_ms.map(|_| "$.payload.duration_ms".into()),
        });

        self.prose.push(ArtistProseBlock {
            source,
            speaker: Principal {
                id: format!("tool:{name}"),
                scope: IdentityScope::Source,
                sort: ConceptId::from("agent:Tool"),
                discourse_role: DiscourseRole::Tool,
                label: Some(name),
            },
            channel: ArtistProseChannel::ToolResultText,
            text: visible_result,
            source_field: envelope.payload.get("presentation").map_or_else(
                || "$.payload.result".into(),
                || "$.payload.presentation.visible".into(),
            ),
            include_in_default_training: false,
        });
        Ok(())
    }

    fn tool_result_images(&mut self, envelope: ArtistEnvelope) -> Result<(), ArtistAdapterError> {
        let internal_call_id = required_string(&envelope.payload, "internal_call_id")?;
        if !self.attach_tool_result_images(&envelope, &internal_call_id)? {
            self.deferred_images.push(PendingImages {
                envelope,
                internal_call_id,
            });
        }
        Ok(())
    }

    fn attach_tool_result_images(
        &mut self,
        envelope: &ArtistEnvelope,
        internal_call_id: &str,
    ) -> Result<bool, ArtistAdapterError> {
        let images = envelope
            .payload
            .get("images")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ArtistAdapterError::MalformedPayload(
                    "tool.result.images missing images array".into(),
                )
            })?;
        let Some(position) = self.lookup_alias(envelope, internal_call_id) else {
            return Ok(false);
        };
        let source = source_for(envelope, None)?;
        let record = self
            .records
            .get_mut(position)
            .ok_or_else(|| ArtistAdapterError::Internal("tool index out of bounds".into()))?;
        let Some(result) = record.result.as_mut() else {
            return Ok(false);
        };
        for (index, image) in images.iter().enumerate() {
            result.payloads.push(ToolResultPayloadFragment {
                source_record: Some(source.clone()),
                value: StructuredValue::from_json(image),
                source_field: format!("$.payload.images.{index}"),
            });
        }
        Ok(true)
    }

    fn change_recorded(&mut self, envelope: ArtistEnvelope) -> Result<(), ArtistAdapterError> {
        let change = PendingChange {
            tool_call_id: required_string(&envelope.payload, "tool_call_id")?,
            path: required_string(&envelope.payload, "path")?,
            before_digest: required_string(&envelope.payload, "before_digest")?,
            after_digest: required_string(&envelope.payload, "after_digest")?,
            diff: optional_string(&envelope.payload, "diff")?,
            diff_attachment: optional_string(&envelope.payload, "diff_attachment")?,
            envelope,
        };
        if !self.attach_change(&change)? {
            self.deferred_changes.push(change);
        }
        Ok(())
    }

    fn attach_change(&mut self, change: &PendingChange) -> Result<bool, ArtistAdapterError> {
        let Some(position) = self.lookup_alias(&change.envelope, &change.tool_call_id) else {
            return Ok(false);
        };
        let source = source_for(&change.envelope, None)?;
        let artifact = ArtifactDescriptor {
            id: format!("file-change:{}:{}", change.envelope.seq, change.path),
            scope: IdentityScope::Record,
            sort: ConceptId::from("comp:File"),
            locator: Some(change.path.clone()),
        };
        let mut states = vec![
            ArtifactStateDescriptor {
                id: format!("change:{}:before", change.envelope.seq),
                evidence: EffectEvidence::Observed,
                relation: muse_core::RelationId::from("comp:beforeArtifactState"),
                artifact: artifact.clone(),
                digest: Some(change.before_digest.clone()),
                sort: ConceptId::from("comp:FileState"),
                source_field: "$.payload.before_digest".into(),
            },
            ArtifactStateDescriptor {
                id: format!("change:{}:after", change.envelope.seq),
                evidence: EffectEvidence::Observed,
                relation: muse_core::RelationId::from("comp:afterArtifactState"),
                artifact: artifact.clone(),
                digest: Some(change.after_digest.clone()),
                sort: ConceptId::from("comp:FileState"),
                source_field: "$.payload.after_digest".into(),
            },
        ];
        states.sort_by(|left, right| left.id.cmp(&right.id));
        let mut outputs = Vec::new();
        if let Some(diff_attachment) = &change.diff_attachment {
            outputs.push(ArtifactDescriptor {
                id: format!("attachment:{diff_attachment}"),
                scope: IdentityScope::Source,
                sort: ConceptId::from("se:InformationArtifact"),
                locator: Some(diff_attachment.clone()),
            });
        }
        let effect = ToolEffect {
            id: format!("artist-change:{}", change.envelope.seq),
            source_record: Some(source),
            evidence: EffectEvidence::Observed,
            sort: ConceptId::from("comp:FileWrite"),
            target: Some(artifact),
            source: None,
            destination: None,
            inputs: Vec::new(),
            outputs,
            states,
            command: None,
            source_field: "$.payload.path".into(),
        };
        let record = self
            .records
            .get_mut(position)
            .ok_or_else(|| ArtistAdapterError::Internal("tool index out of bounds".into()))?;
        record.effects.push(effect);
        if change.diff.is_some() || change.diff_attachment.is_some() {
            self.residual.push(ArtistResidual {
                source: source_for(&change.envelope, None)?,
                kind: change.envelope.kind.clone(),
                reason: ResidualReason::PartiallySpecialized,
                value: change.envelope.payload.clone(),
            });
        }
        Ok(true)
    }

    fn legacy_turn(&mut self, envelope: &ArtistEnvelope) -> Result<(), ArtistAdapterError> {
        let role = required_string(&envelope.payload, "role")?;
        let content = required_string(&envelope.payload, "content")?;
        let source = source_for(envelope, None)?;
        let (speaker, channel) = match role.as_str() {
            "user" => (user_principal(), ArtistProseChannel::LegacyUserText),
            "assistant" => (
                self.agent_principal(envelope),
                ArtistProseChannel::LegacyAgentText,
            ),
            _ => return self.malformed(envelope, "legacy.turn has unknown role"),
        };
        self.prose.push(ArtistProseBlock {
            source,
            speaker,
            channel,
            text: content,
            source_field: "$.payload.content".into(),
            include_in_default_training: true,
        });
        Ok(())
    }

    fn push_agent_prose(
        &mut self,
        source: &StructuredSource,
        speaker: &Principal,
        channel: ArtistProseChannel,
        block: &Value,
        index: usize,
        include_in_default_training: bool,
    ) {
        if let Some(text) = block.get("text").and_then(Value::as_str) {
            self.prose.push(ArtistProseBlock {
                source: source.clone(),
                speaker: speaker.clone(),
                channel,
                text: text.to_owned(),
                source_field: format!("$.payload.content.{index}.text"),
                include_in_default_training,
            });
        }
    }

    fn agent_principal(&self, envelope: &ArtistEnvelope) -> Principal {
        if let Some(run) = &envelope.run {
            if let Some(principal) = self.run_identities.get(&run_identity_key(envelope, run)) {
                return principal.clone();
            }
        }
        Principal {
            id: format!("lineage:{}", envelope.lineage),
            scope: IdentityScope::Source,
            sort: ConceptId::from("agent:SoftwareAgent"),
            discourse_role: DiscourseRole::Agent,
            label: None,
        }
    }

    fn register_alias(
        &mut self,
        envelope: &ArtistEnvelope,
        alias: &str,
        position: usize,
    ) -> Result<(), ArtistAdapterError> {
        let key = alias_key(envelope, alias);
        if let Some(previous) = self.aliases.insert(key, position) {
            if previous != position {
                return Err(ArtistAdapterError::AliasCollision(alias.to_owned()));
            }
        }
        Ok(())
    }

    fn lookup_alias(&self, envelope: &ArtistEnvelope, alias: &str) -> Option<usize> {
        self.aliases.get(&alias_key(envelope, alias)).copied()
    }

    fn malformed(
        &mut self,
        envelope: &ArtistEnvelope,
        _message: &str,
    ) -> Result<(), ArtistAdapterError> {
        let source = source_for(envelope, None)?;
        self.residual.push(ArtistResidual {
            source,
            kind: envelope.kind.clone(),
            reason: ResidualReason::MalformedKnownPayload,
            value: envelope.payload.clone(),
        });
        Ok(())
    }

    fn residual_fragment(
        &mut self,
        source: &StructuredSource,
        envelope: &ArtistEnvelope,
        block: &Value,
        reason: ResidualReason,
        index: usize,
    ) {
        let mut fragment_source = source.clone();
        fragment_source.record = format!("{}:block:{index}", source.record);
        self.residual.push(ArtistResidual {
            source: fragment_source,
            kind: format!("{}:content", envelope.kind),
            reason,
            value: block.clone(),
        });
    }
}

#[derive(Clone, Debug)]
struct ParsedOutcome {
    status: muse_occurrence::ReportedOutcomeStatus,
    detail: Option<String>,
}

fn parse_outcome(value: &Value) -> Result<ParsedOutcome, ArtistAdapterError> {
    use muse_occurrence::ReportedOutcomeStatus;

    let status = required_string(value, "status")?;
    Ok(match status.as_str() {
        "success" => ParsedOutcome {
            status: ReportedOutcomeStatus::Success,
            detail: None,
        },
        "error" => ParsedOutcome {
            status: ReportedOutcomeStatus::Failure,
            detail: optional_string(value, "message")?,
        },
        "skipped" => ParsedOutcome {
            status: ReportedOutcomeStatus::Skipped,
            detail: optional_string(value, "reason")?,
        },
        "denied" => ParsedOutcome {
            status: ReportedOutcomeStatus::Denied,
            detail: optional_string(value, "reason")?,
        },
        other => {
            return Err(ArtistAdapterError::MalformedPayload(format!(
                "unknown tool outcome status {other:?}"
            )));
        }
    })
}

fn requested_effects(
    name: &str,
    arguments: &Value,
    source: &StructuredSource,
    arguments_path: &str,
    effect_ordinal: usize,
) -> Vec<ToolEffect> {
    let path = arguments.get("path").and_then(Value::as_str);
    let path_effect = |sort: ConceptId, path: &str| ToolEffect {
        id: format!("requested:{effect_ordinal}:{name}:path"),
        source_record: None,
        evidence: EffectEvidence::Requested,
        sort,
        target: Some(ArtifactDescriptor {
            id: path.to_owned(),
            scope: if source.run.is_some() {
                IdentityScope::Run
            } else {
                IdentityScope::Record
            },
            sort: ConceptId::from("comp:Path"),
            locator: Some(path.to_owned()),
        }),
        source: None,
        destination: None,
        inputs: Vec::new(),
        outputs: Vec::new(),
        states: Vec::new(),
        command: None,
        source_field: format!("{arguments_path}.path"),
    };

    match name {
        "read" => path
            .map(|path| vec![path_effect(ConceptId::from("comp:FileRead"), path)])
            .unwrap_or_default(),
        "write" | "edit" => path
            .map(|path| vec![path_effect(ConceptId::from("comp:FileWrite"), path)])
            .unwrap_or_default(),
        "bash" => {
            let mode = arguments.get("mode").and_then(Value::as_str);
            let command = arguments.get("command").and_then(Value::as_str);
            let requests_execution = matches!(mode, None | Some("exec" | "start"));
            if requests_execution {
                command
                    .map(|command| {
                        vec![ToolEffect {
                            id: format!("requested:{effect_ordinal}:bash:process"),
                            source_record: None,
                            evidence: EffectEvidence::Requested,
                            sort: ConceptId::from("comp:ProcessExecution"),
                            target: None,
                            source: None,
                            destination: None,
                            inputs: Vec::new(),
                            outputs: Vec::new(),
                            states: Vec::new(),
                            command: Some(command.to_owned()),
                            source_field: format!("{arguments_path}.command"),
                        }]
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

fn parse_bash_execution(text: &str) -> ExecutionDetails {
    let mut exit_code = None;
    let mut output_marker = None;
    let lines = text.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if let Some(value) = line.strip_prefix("exitCode: ") {
            exit_code = value.trim().parse::<i32>().ok();
        }
        if *line == "--- output ---" || *line == "--- stdout ---" {
            output_marker = Some(index + 1);
            break;
        }
    }
    let Some(start) = output_marker else {
        return ExecutionDetails {
            exit_code,
            ..ExecutionDetails::default()
        };
    };
    let body = lines[start..].join("\n");
    if let Some((stdout, stderr)) = body.split_once("\n--- stderr ---\n") {
        ExecutionDetails {
            exit_code,
            stdout: Some(stdout.to_owned()).filter(|value| !value.is_empty()),
            stderr: Some(stderr.to_owned()).filter(|value| !value.is_empty()),
            diagnostics: Vec::new(),
        }
    } else {
        ExecutionDetails {
            exit_code,
            ..ExecutionDetails::default()
        }
    }
}

fn execution_has_data(value: &ExecutionDetails) -> bool {
    value.exit_code.is_some()
        || value.stdout.is_some()
        || value.stderr.is_some()
        || !value.diagnostics.is_empty()
}

fn tool_descriptor(name: &str, schema_version: u32) -> ToolDescriptor {
    let built_in = builtins_for_schema(schema_version).contains(&name);
    ToolDescriptor {
        id: name.to_owned(),
        scope: IdentityScope::Source,
        name: name.to_owned(),
        specification_id: built_in.then(|| {
            format!(
                "artist:{}:built-in-tool:{name}",
                adapter_version_for_schema(schema_version)
            )
        }),
        sort: ConceptId::from("agent:Tool"),
    }
}

fn tool_invocation_sort(name: &str) -> ConceptId {
    match name {
        "bash" => ConceptId::from("harness:ShellToolInvocation"),
        "run" | "eval" | "debug" => ConceptId::from("harness:ExecutionToolInvocation"),
        "read" | "find" | "grep" | "edit" | "write" => {
            ConceptId::from("harness:FilesystemToolInvocation")
        }
        "skill" | "page" => ConceptId::from("harness:ResourceLookupToolInvocation"),
        "todo" => ConceptId::from("harness:TaskManagementToolInvocation"),
        "memory" => ConceptId::from("harness:MemoryToolInvocation"),
        "code_map" | "code_show" | "code_surface" | "code_implements" | "code_deps"
        | "code_cycles" | "code_calls" | "code_trace" | "code_impact" | "code_search"
        | "code_related" | "ast_query" => ConceptId::from("harness:CodeInspectionToolInvocation"),
        "ast_rewrite" => ConceptId::from("harness:CodeTransformationToolInvocation"),
        "computer" => ConceptId::from("harness:ComputerToolInvocation"),
        "lsp" | "relationship" => ConceptId::from("harness:CodeInspectionToolInvocation"),
        "forge" => ConceptId::from("harness:ExternalServiceToolInvocation"),
        "canvas" => ConceptId::from("harness:ApplicationToolInvocation"),
        "handoff" | "agent" | "subagent" | "poll" | "stop" | "abort" | "delete" | "send"
        | "list" => ConceptId::from("harness:AgentCoordinationToolInvocation"),
        "ask" => ConceptId::from("harness:UserInteractionToolInvocation"),
        _ => ConceptId::from("agent:ToolInvocation"),
    }
}

fn is_supported_schema(version: u32) -> bool {
    ARTIST_SUPPORTED_SESSION_SCHEMA_VERSIONS.contains(&version)
}

fn adapter_version_for_schema(version: u32) -> &'static str {
    match version {
        1 => ARTIST_V1_ADAPTER_VERSION,
        2 => ARTIST_ADAPTER_VERSION,
        3 => ARTIST_V3_ADAPTER_VERSION,
        4 => ARTIST_V4_ADAPTER_VERSION,
        5 => ARTIST_V5_ADAPTER_VERSION,
        6 => ARTIST_V6_ADAPTER_VERSION,
        7 => ARTIST_V7_ADAPTER_VERSION,
        _ => "unsupported",
    }
}

pub(crate) fn event_formalizer_version_for_schema(version: u32) -> &'static str {
    match version {
        1 => ARTIST_V1_EVENT_FORMALIZER_VERSION,
        2 => ARTIST_EVENT_FORMALIZER_VERSION,
        3 => ARTIST_V3_EVENT_FORMALIZER_VERSION,
        4 => ARTIST_V4_EVENT_FORMALIZER_VERSION,
        5 => ARTIST_V5_EVENT_FORMALIZER_VERSION,
        6 => ARTIST_V6_EVENT_FORMALIZER_VERSION,
        7 => ARTIST_V7_EVENT_FORMALIZER_VERSION,
        _ => "unsupported",
    }
}

fn builtins_for_schema(version: u32) -> &'static [&'static str] {
    match version {
        1 => &ARTIST_V1_BUILTIN_TOOLS,
        2 | 3 | 4 | 5 | 6 | 7 => &ARTIST_BUILTIN_TOOLS,
        _ => &[],
    }
}

fn is_known_event_kind(version: u32, kind: &str) -> bool {
    match version {
        1 => ARTIST_KNOWN_EVENT_KINDS.contains(&kind),
        2 => ARTIST_V2_KNOWN_EVENT_KINDS.contains(&kind),
        3 => ARTIST_V3_KNOWN_EVENT_KINDS.contains(&kind),
        4 => ARTIST_V4_KNOWN_EVENT_KINDS.contains(&kind),
        5 => ARTIST_V5_KNOWN_EVENT_KINDS.contains(&kind),
        6 => ARTIST_V5_KNOWN_EVENT_KINDS.contains(&kind),
        7 => ARTIST_V5_KNOWN_EVENT_KINDS.contains(&kind),
        _ => false,
    }
}

fn source_for(
    envelope: &ArtistEnvelope,
    turn: Option<String>,
) -> Result<StructuredSource, ArtistAdapterError> {
    let unix_millis = i64::try_from(envelope.ts)
        .map_err(|_| ArtistAdapterError::TimestampOutOfRange(envelope.ts))?;
    Ok(StructuredSource {
        source: format!("artist-session:{}", envelope.session),
        run: envelope.run.clone(),
        turn,
        record: format!("seq:{}", envelope.seq),
        sequence: Some(envelope.seq),
        unix_millis: Some(unix_millis),
        recorder: Some(Principal {
            id: "artist-session-recorder".into(),
            scope: IdentityScope::Source,
            sort: ConceptId::from("harness:CodingHarness"),
            discourse_role: DiscourseRole::Harness,
            label: Some("Artist session recorder".into()),
        }),
    })
}

fn invocation_scope(source: &StructuredSource) -> IdentityScope {
    if source.run.is_some() {
        IdentityScope::Run
    } else {
        IdentityScope::Record
    }
}

fn invocation_key(envelope: &ArtistEnvelope, id: &str) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        envelope.session,
        envelope.lineage,
        envelope.run.as_deref().unwrap_or(""),
        id
    )
}

fn alias_key(envelope: &ArtistEnvelope, alias: &str) -> String {
    invocation_key(envelope, alias)
}

fn run_identity_key(envelope: &ArtistEnvelope, run: &str) -> String {
    format!("{}\u{1f}{}\u{1f}{run}", envelope.session, envelope.lineage)
}

fn user_principal() -> Principal {
    Principal {
        id: "user".into(),
        scope: IdentityScope::Source,
        sort: ConceptId::from("ufo:Agent"),
        discourse_role: DiscourseRole::User,
        label: None,
    }
}

fn object_to_structured(
    value: &Value,
) -> Result<BTreeMap<String, StructuredValue>, ArtistAdapterError> {
    let object = value.as_object().ok_or_else(|| {
        ArtistAdapterError::MalformedPayload("tool arguments are not an object".into())
    })?;
    Ok(object
        .iter()
        .map(|(key, value)| (key.clone(), StructuredValue::from_json(value)))
        .collect())
}

fn block_type(value: &Value) -> Option<&str> {
    value.get("type").and_then(Value::as_str)
}

fn required_string(value: &Value, field: &'static str) -> Result<String, ArtistAdapterError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            ArtistAdapterError::MalformedPayload(format!("missing string field {field}"))
        })
}

fn optional_string(
    value: &Value,
    field: &'static str,
) -> Result<Option<String>, ArtistAdapterError> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(ArtistAdapterError::MalformedPayload(format!(
            "field {field} is not a string/null"
        ))),
    }
}

/// Adapter failures that indicate malformed/corrupt input or violated Artist
/// identity/correlation invariants. Forward-compatible unknown events are residuals,
/// not errors.
#[derive(Debug, Error)]
pub enum ArtistAdapterError {
    #[error("invalid Artist JSONL at line {line}: {source}")]
    JsonLine {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("mixed Artist sessions: expected {expected:?}, got {actual:?}")]
    MixedSessions { expected: String, actual: String },
    #[error("non-monotonic Artist event sequence: previous {previous}, current {current}")]
    NonMonotonicSequence { previous: u64, current: u64 },
    #[error("Artist timestamp is outside Muse i64 millisecond range: {0}")]
    TimestampOutOfRange(u64),
    #[error("invalid Artist envelope: {0}")]
    InvalidEnvelope(String),
    #[error("malformed known Artist payload: {0}")]
    MalformedPayload(String),
    #[error("duplicate Artist internal invocation id: {0}")]
    DuplicateInvocation(String),
    #[error("Artist invocation alias resolves to multiple calls: {0}")]
    AliasCollision(String),
    #[error("Artist tool.result disagrees with its model.tool_call: {invocation}")]
    InconsistentToolResult { invocation: String },
    #[error("duplicate Artist tool.result for invocation: {0}")]
    DuplicateToolResult(String),
    #[error("internal adapter invariant failed: {0}")]
    Internal(String),
    #[error(transparent)]
    Tooling(#[from] muse_tooling::ToolFormalizationError),
    #[error(transparent)]
    Occurrence(#[from] muse_occurrence::OccurrenceValidationError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(seq: u64, kind: &str, payload: Value) -> ArtistEnvelope {
        ArtistEnvelope {
            v: ARTIST_SESSION_SCHEMA_VERSION,
            seq,
            ts: 1_700_000_000_000 + seq,
            session: "session-1".into(),
            run: Some("run-1".into()),
            lineage: "main".into(),
            kind: kind.into(),
            payload,
        }
    }

    #[test]
    fn joins_model_call_result_and_independent_change_record() {
        let mut normalizer = ArtistNormalizer::default();
        normalizer
            .push(envelope(
                1,
                "run.started",
                serde_json::json!({
                    "provider": "openai",
                    "model": "model",
                    "agent": "Coder",
                    "actor": "actor-1"
                }),
            ))
            .unwrap();
        normalizer
            .push(envelope(
                2,
                "model.turn",
                serde_json::json!({
                    "turn": 1,
                    "content": [{
                        "type": "tool_call",
                        "id": "internal-1",
                        "call_id": "provider-1",
                        "name": "write",
                        "arguments": {"path": "src/lib.rs", "content": "pub fn x() {}"},
                        "signature": null
                    }],
                    "total_tokens": 1,
                    "partial": false
                }),
            ))
            .unwrap();
        normalizer
            .push(envelope(
                3,
                "change.recorded",
                serde_json::json!({
                    "tool_call_id": "provider-1",
                    "path": "src/lib.rs",
                    "before_digest": "sha256:before",
                    "after_digest": "sha256:after",
                    "diff": "@@",
                    "diff_attachment": null
                }),
            ))
            .unwrap();
        normalizer
            .push(envelope(
                4,
                "tool.result",
                serde_json::json!({
                    "internal_call_id": "internal-1",
                    "tool_call_id": "provider-1",
                    "name": "write",
                    "arguments": {"path": "src/lib.rs", "content": "pub fn x() {}"},
                    "result": "Written src/lib.rs",
                    "outcome": {"status": "success"},
                    "duration_ms": 8
                }),
            ))
            .unwrap();

        let normalized = normalizer.finish().unwrap();
        assert_eq!(normalized.tool_invocations.len(), 1);
        let record = &normalized.tool_invocations[0];
        assert_eq!(record.invoker.id, "actor-1");
        assert_eq!(record.effects.len(), 2);
        assert_eq!(record.effects[0].evidence, EffectEvidence::Requested);
        assert_eq!(record.effects[1].evidence, EffectEvidence::Observed);
        assert_eq!(
            record.effects[1]
                .source_record
                .as_ref()
                .map(|source| source.record.as_str()),
            Some("seq:3")
        );
        assert_eq!(
            record
                .result
                .as_ref()
                .and_then(|result| result.source_record.as_ref())
                .map(|source| source.record.as_str()),
            Some("seq:4")
        );
        assert!(record.validate().is_ok());
    }

    #[test]
    fn result_does_not_turn_requested_write_into_observed_write() {
        let mut normalizer = ArtistNormalizer::default();
        normalizer
            .push(envelope(
                1,
                "model.turn",
                serde_json::json!({
                    "turn": 1,
                    "content": [{
                        "type": "tool_call",
                        "id": "internal-1",
                        "name": "write",
                        "arguments": {"path": "x", "content": "y"}
                    }]
                }),
            ))
            .unwrap();
        normalizer
            .push(envelope(
                2,
                "tool.result",
                serde_json::json!({
                    "internal_call_id": "internal-1",
                    "name": "write",
                    "arguments": {"path": "x", "content": "y"},
                    "result": "ok",
                    "outcome": {"status": "success"}
                }),
            ))
            .unwrap();
        let normalized = normalizer.finish().unwrap();
        let record = &normalized.tool_invocations[0];
        assert_eq!(record.effects.len(), 1);
        assert_eq!(record.effects[0].evidence, EffectEvidence::Requested);
    }

    #[test]
    fn prose_preserves_user_and_agent_identity() {
        let mut normalizer = ArtistNormalizer::default();
        normalizer
            .push(envelope(
                1,
                "turn.user",
                serde_json::json!({
                    "content": [{"type": "text", "text": "I thought X"}],
                    "source": "prompt"
                }),
            ))
            .unwrap();
        normalizer
            .push(envelope(
                2,
                "model.turn",
                serde_json::json!({
                    "turn": 1,
                    "content": [{"type": "text", "text": "I thought Y"}]
                }),
            ))
            .unwrap();
        let normalized = normalizer.finish().unwrap();
        assert_eq!(normalized.prose.len(), 2);
        assert_eq!(
            normalized.prose[0].speaker.discourse_role,
            DiscourseRole::User
        );
        assert_eq!(
            normalized.prose[1].speaker.discourse_role,
            DiscourseRole::Agent
        );
    }

    #[test]
    fn unknown_future_event_is_retained_not_dropped() {
        let mut normalizer = ArtistNormalizer::default();
        normalizer
            .push(envelope(1, "future.event", serde_json::json!({"x": 1})))
            .unwrap();
        let normalized = normalizer.finish().unwrap();
        assert_eq!(normalized.residual.len(), 1);
        assert_eq!(normalized.residual[0].kind, "future.event");
    }

    #[test]
    fn v4_work_unit_yield_is_retained_as_report_evidence() {
        let normalized = ArtistNormalizer::normalize_jsonl(
            r#"{"v":4,"seq":1,"ts":2,"session":"s","lineage":"main/a","kind":"work_unit.yield.v1","payload":{"agent":"ada","sequence":1,"profile":"reviewer","yield_schema":{"type":"object"},"value":{"answer":"done"}}}"#,
        )
        .unwrap();
        assert_eq!(normalized.structured_events.len(), 1);
        let event = &normalized.structured_events[0];
        assert_eq!(event.kind, "work_unit.yield.v1");
        assert_eq!(event.payload["value"]["answer"], "done");
        assert_eq!(event.adapter_version, ARTIST_V4_ADAPTER_VERSION);
    }

    #[test]
    fn v5_rule_events_preserve_the_exact_rule_snapshot_provenance() {
        let normalized = ArtistNormalizer::normalize_jsonl(
            r#"{"v":5,"seq":1,"ts":2,"session":"s","lineage":"main","kind":"rule.fired","payload":{"rule":"no-leak","target":"assistant-text","matched":"Box::leak","turn":1,"provenance":{"ruleId":"no-leak","digest":"sha256:abc","source":".artist/rules/no-leak.md"}}}"#,
        )
        .unwrap();
        let event = &normalized.structured_events[0];
        assert_eq!(event.adapter_version, ARTIST_V5_ADAPTER_VERSION);
        assert_eq!(event.payload["provenance"]["digest"], "sha256:abc");
    }

    #[test]
    fn v5_still_uses_the_active_builtin_tool_catalog() {
        let descriptor = tool_descriptor("read", 5);
        assert_eq!(
            descriptor.specification_id,
            Some(format!(
                "artist:{ARTIST_V5_ADAPTER_VERSION}:built-in-tool:read"
            ))
        );
    }

    #[test]
    fn v6_rule_actions_preserve_control_effects_separately_from_evidence() {
        let normalized = ArtistNormalizer::normalize_jsonl(
            r#"{"v":6,"seq":1,"ts":2,"session":"s","lineage":"main","kind":"rule.fired","payload":{"rule":"no-leak","target":"assistant-text","matched":"Box::leak","turn":1,"action":"abort_and_retry"}}"#,
        )
        .unwrap();
        let event = &normalized.structured_events[0];
        assert_eq!(event.adapter_version, ARTIST_V6_ADAPTER_VERSION);
        assert_eq!(event.payload["action"], "abort_and_retry");
    }

    #[test]
    fn auto_resolved_ask_retains_its_non_human_provenance() {
        let normalized = ArtistNormalizer::normalize_jsonl(
            r#"{"v":4,"seq":1,"ts":2,"session":"s","lineage":"main","kind":"ask.answered","payload":{"answer":{"selections":["option"]},"source":"AutoResolve","surface":"auto_resolve"}}"#,
        )
        .unwrap();
        let event = &normalized.structured_events[0];
        assert_eq!(event.kind, "ask.answered");
        assert_eq!(event.payload["source"], "AutoResolve");
        assert_ne!(event.payload["source"], "Human");
    }

    #[test]
    fn every_pinned_builtin_has_a_non_generic_family() {
        for name in ARTIST_BUILTIN_TOOLS {
            assert_ne!(
                tool_invocation_sort(name),
                ConceptId::from("agent:ToolInvocation"),
                "{name}"
            );
        }
    }

    #[test]
    fn dynamic_tool_names_normalize_without_a_built_in_assumption() {
        let descriptor = tool_descriptor("mcp:github/search", ARTIST_SESSION_SCHEMA_VERSION);
        assert!(descriptor.specification_id.is_none());
        assert_eq!(
            tool_invocation_sort(&descriptor.name),
            ConceptId::from("agent:ToolInvocation")
        );
    }
}
