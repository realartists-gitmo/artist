//! Pre-label corpus, annotation, conformance, quarantine, and leakage-safe split
//! contracts for Muse's generic source-window→formal SLM.
//!
//! Learned windows include ordinary prose and normalized structured tool calls/results.
//! Deterministic tool extraction remains an independent safety/baseline path; it does
//! not exclude structured records from learned semantic interpretation.

#![forbid(unsafe_code)]

pub mod semantic_v6;
pub use semantic_v6::*;
mod v6_compile;

use std::collections::{BTreeMap, BTreeSet};

use muse_core::ContentDigest;
use muse_occurrence::{
    Derivation, DiscourseRole, SCHEMA_VERSION as OCCURRENCE_SCHEMA_VERSION,
    TRAINING_CANONICALIZATION_VERSION,
};
use muse_registry::{PackageRegistry, RegistrySnapshot};
use muse_superstrate::{
    BASE_THEORY_NAMESPACE, BASE_THEORY_VERSION, LOWERING_VERSION, LoweringError,
    lower_and_kernel_check,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Human/model annotation protocol whose output is the canonical occurrence IR.
pub const LABEL_PROTOCOL_VERSION: &str = "muse-semantic-label-6";
/// Corpus record/window schema.
pub const CORPUS_SCHEMA_VERSION: &str = "muse-corpus-5";
/// Label conformance receipt schema.
pub const CONFORMANCE_RECEIPT_VERSION: &str = "muse-label-conformance-6";
/// External-id key identifying the exact discourse speaker for prose windows.
pub const WINDOW_SPEAKER_EXTERNAL_ID: &str = "muse_window_speaker";
/// External-id key identifying an exact source-supplied prose addressee.
pub const WINDOW_ADDRESSEE_EXTERNAL_ID: &str = "muse_window_addressee";
/// External-id key identifying the source-supplied actor/invoker for structured windows.
pub const WINDOW_ACTOR_EXTERNAL_ID: &str = "muse_window_actor";
/// External-id key identifying the source recorder/presenter for structured windows when known.
pub const WINDOW_RECORDER_EXTERNAL_ID: &str = "muse_window_recorder";
/// Reserved structured-source metadata span containing the normalized tool identity.
pub const STRUCTURED_TOOL_NAME_PATH: &str = "@tool_name";
/// Reserved structured-source metadata span containing an explicit provider invocation identity.
pub const STRUCTURED_INVOCATION_ID_PATH: &str = "@invocation_id";
/// Reserved structured-source metadata span identifying the exact normalized source record.
pub const STRUCTURED_RECORD_PATH: &str = "@record";
const MAX_STRUCTURED_CONTEXT_VALUE_BYTES: usize = 2_048;
const MAX_STRUCTURED_CONTEXT_BYTES: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerRole {
    User,
    Agent,
    System,
    Harness,
    Tool,
    Unknown,
}

impl SpeakerRole {
    fn discourse_role(self) -> DiscourseRole {
        match self {
            Self::User => DiscourseRole::User,
            Self::Agent => DiscourseRole::Agent,
            Self::System => DiscourseRole::System,
            Self::Harness => DiscourseRole::Harness,
            Self::Tool => DiscourseRole::Tool,
            Self::Unknown => DiscourseRole::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProseChannel {
    UserText,
    AgentText,
    AgentReasoning,
    SystemText,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpaqueEncoding {
    Base64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearnedVisibility {
    ModelVisible,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpaqueFieldRule {
    /// RFC-6901 path pattern. `*` is permitted only as a complete path segment.
    pub path_pattern: String,
    pub encoding: OpaqueEncoding,
    pub media_type: Option<String>,
}

/// Compatibility name retained by the public Muse facade. It denotes the same
/// deterministic opaque-field rule; no separate hint semantics exist.
pub type OpaqueFieldHint = OpaqueFieldRule;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldAliasRule {
    /// Alias and canonical patterns must contain the same number of `*` segments.
    pub alias_pattern: String,
    pub canonical_pattern: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextDependencyRule {
    /// Primary field pattern whose fragments require additional exact source context.
    pub primary_pattern: String,
    /// Source field patterns repeated read-only into each matching part.
    pub context_patterns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizationContract {
    pub id: String,
    pub version: String,
    /// Explicit source-adapter assertion that this normalized surface was delivered to the model.
    /// Unknown/private/telemetry surfaces must not have a learned normalization contract.
    pub visibility: LearnedVisibility,
    pub source_adapter: String,
    pub source_adapter_version: String,
    #[serde(default)]
    pub aliases: Vec<FieldAliasRule>,
    #[serde(default)]
    pub opaque_fields: Vec<OpaqueFieldRule>,
    #[serde(default)]
    pub context_dependencies: Vec<ContextDependencyRule>,
}
impl NormalizationContract {
    pub fn digest(&self) -> Result<ContentDigest, TrainingError> {
        self.validate()?;
        Ok(ContentDigest::sha256_bytes(&serde_json::to_vec(self)?))
    }
    fn validate(&self) -> Result<(), TrainingError> {
        nonempty("normalization contract id", &self.id)?;
        nonempty("normalization contract version", &self.version)?;
        nonempty("normalization source adapter", &self.source_adapter)?;
        nonempty(
            "normalization source adapter version",
            &self.source_adapter_version,
        )?;
        if !matches!(self.visibility, LearnedVisibility::ModelVisible) {
            return Err(TrainingError::InvalidNormalizationContract(self.id.clone()));
        }
        let mut aliases = BTreeSet::new();
        for rule in &self.aliases {
            validate_path_pattern(&rule.alias_pattern)?;
            validate_path_pattern(&rule.canonical_pattern)?;
            if wildcard_count(&rule.alias_pattern) != wildcard_count(&rule.canonical_pattern)
                || rule.alias_pattern == rule.canonical_pattern
                || !aliases.insert(rule.alias_pattern.clone())
            {
                return Err(TrainingError::InvalidNormalizationContract(self.id.clone()));
            }
        }
        let mut opaque = BTreeSet::new();
        for rule in &self.opaque_fields {
            validate_path_pattern(&rule.path_pattern)?;
            if !opaque.insert(rule.path_pattern.clone()) {
                return Err(TrainingError::InvalidNormalizationContract(self.id.clone()));
            }
        }
        let mut context = BTreeSet::new();
        for rule in &self.context_dependencies {
            validate_path_pattern(&rule.primary_pattern)?;
            if !context.insert(rule.primary_pattern.clone()) {
                return Err(TrainingError::InvalidNormalizationContract(self.id.clone()));
            }
            for pattern in &rule.context_patterns {
                validate_path_pattern(pattern)?;
                if wildcard_count(pattern) != wildcard_count(&rule.primary_pattern) {
                    return Err(TrainingError::InvalidNormalizationContract(self.id.clone()));
                }
            }
        }
        Ok(())
    }
}

fn parse_json_without_duplicate_keys(raw: &str) -> Result<serde_json::Value, TrainingError> {
    use serde::de::{Error as _, MapAccess, SeqAccess, Visitor};
    struct ExactValue;
    impl<'de> Visitor<'de> for ExactValue {
        type Value = serde_json::Value;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("JSON value without duplicate object keys")
        }
        fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
            Ok(value.into())
        }
        fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
            Ok(value.into())
        }
        fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
            Ok(value.into())
        }
        fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
            serde_json::Number::from_f64(value)
                .map(serde_json::Value::Number)
                .ok_or_else(|| E::custom("non-finite JSON number"))
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
            Ok(value.into())
        }
        fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
            Ok(value.into())
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(serde_json::Value::Null)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(serde_json::Value::Null)
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(value) = seq.next_element_seed(ExactValue)? {
                out.push(value);
            }
            Ok(serde_json::Value::Array(out))
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut out = serde_json::Map::new();
            while let Some(key) = map.next_key::<String>()? {
                if out.contains_key(&key) {
                    return Err(A::Error::custom(format!(
                        "duplicate JSON object key: {key}"
                    )));
                }
                let value = map.next_value_seed(ExactValue)?;
                out.insert(key, value);
            }
            Ok(serde_json::Value::Object(out))
        }
    }
    impl<'de> serde::de::DeserializeSeed<'de> for ExactValue {
        type Value = serde_json::Value;
        fn deserialize<D: serde::Deserializer<'de>>(
            self,
            deserializer: D,
        ) -> Result<Self::Value, D::Error> {
            deserializer.deserialize_any(self)
        }
    }
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    let value = serde::de::DeserializeSeed::deserialize(ExactValue, &mut deserializer)
        .map_err(TrainingError::Json)?;
    deserializer.end().map_err(TrainingError::Json)?;
    Ok(value)
}

/// Exact source JSON retained as raw UTF-8. Window construction reparses it with
/// duplicate-key rejection; the raw bytes, not `serde_json::Value`, are the source identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExactJsonPayload {
    pub raw: String,
}
impl ExactJsonPayload {
    #[must_use]
    pub fn from_value(value: &serde_json::Value) -> Self {
        Self {
            raw: serde_json::to_string(value).expect("serde_json::Value is serializable"),
        }
    }
    fn parse(&self) -> Result<serde_json::Value, TrainingError> {
        // First pass rejects duplicate keys. The second uses serde_json arbitrary-precision
        // numbers so exact numeric lexemes are not rounded through f64.
        let _ = parse_json_without_duplicate_keys(&self.raw)?;
        Ok(serde_json::from_str(&self.raw)?)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "representation", rename_all = "snake_case")]
pub enum StructuredFieldValue {
    Json {
        value: serde_json::Value,
    },
    /// Exact source value is retained out-of-band by the corpus adapter and
    /// represented model-side only by deterministic identity/size metadata.
    Opaque {
        encoding: OpaqueEncoding,
        media_type: Option<String>,
        decoded_digest: ContentDigest,
        decoded_bytes: u64,
    },
}
impl StructuredFieldValue {
    #[must_use]
    pub fn json(&self) -> Option<&serde_json::Value> {
        match self {
            Self::Json { value } => Some(value),
            Self::Opaque { .. } => None,
        }
    }
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        self.json().and_then(serde_json::Value::as_str)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranscriptRecordKind {
    Prose {
        channel: ProseChannel,
        text: String,
    },
    /// Lossless normalized source-visible tool-call envelope. Dataset adapters may
    /// normalize harness envelope syntax but must retain every call argument/value.
    ToolCall {
        /// Canonical source-visible callable identity repeated into every structured part.
        tool_name: String,
        /// Explicit provider/harness invocation identity when present. Never invented.
        invocation_id: Option<String>,
        payload: ExactJsonPayload,
        normalization_contract: String,
    },
    /// A source-visible lifecycle/update record about an invocation. This is
    /// deliberately not another ToolCall: observing an update must never create
    /// a second invocation event for the same provider call identity.
    ToolCallUpdate {
        /// Tool identity when the source or an exact invocation-id join establishes it.
        tool_name: Option<String>,
        /// Explicit provider/harness invocation identity when present. Never invented.
        invocation_id: Option<String>,
        payload: ExactJsonPayload,
        normalization_contract: String,
    },
    /// Lossless normalized source-visible tool-result envelope. Result payload
    /// content is retained even when it is not deterministically interpretable.
    ToolResult {
        /// Tool identity when source-provided or losslessly joined from the matching
        /// call by explicit invocation identity. It may be absent for orphan results.
        tool_name: Option<String>,
        /// Explicit provider/harness invocation identity when present. Never invented.
        invocation_id: Option<String>,
        payload: ExactJsonPayload,
        normalization_contract: String,
    },
    Metadata,
}

/// Source-normalized transcript record. Dataset-specific adapters must populate
/// these fields without changing source text or crossing conversation identities.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptRecord {
    pub source: String,
    pub conversation: String,
    pub run: Option<String>,
    pub message: String,
    pub sequence: u64,
    pub speaker_id: String,
    pub speaker_role: SpeakerRole,
    /// Optional exact prose addressee supplied by the source protocol. Never inferred.
    #[serde(default)]
    pub addressee_id: Option<String>,
    #[serde(default)]
    pub addressee_role: Option<SpeakerRole>,
    /// Optional recorder identity when distinct from the actor/speaker. Structured
    /// source records use this for record attribution; prose normally leaves it absent.
    pub recorder_id: Option<String>,
    pub recorder_role: Option<SpeakerRole>,
    pub record: String,
    pub kind: TranscriptRecordKind,
}

/// Exact reversible mapping from window bytes to original source-record bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowBlock {
    pub record: String,
    pub sequence: u64,
    pub message: String,
    pub source_start_byte: u64,
    pub source_end_byte: u64,
    pub start_byte: u64,
    pub end_byte: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProseWindow {
    pub schema_version: String,
    pub id: String,
    pub source: String,
    pub conversation: String,
    pub run: Option<String>,
    pub speaker_id: String,
    pub speaker_role: SpeakerRole,
    /// Exact source-supplied addressee when the protocol establishes one.
    #[serde(default)]
    pub addressee_id: Option<String>,
    #[serde(default)]
    pub addressee_role: Option<SpeakerRole>,
    pub channel: ProseChannel,
    pub text: String,
    pub blocks: Vec<WindowBlock>,
}

impl ProseWindow {
    #[must_use]
    pub fn semantic_source(&self) -> String {
        format!("muse-window:{}", self.id)
    }

    #[must_use]
    pub fn leakage_group(&self) -> String {
        match &self.run {
            Some(run) => format!("{}\u{1f}{}\u{1f}{run}", self.source, self.conversation),
            None => format!("{}\u{1f}{}", self.source, self.conversation),
        }
    }

    pub fn validate(&self) -> Result<(), TrainingError> {
        if self.schema_version != CORPUS_SCHEMA_VERSION {
            return Err(TrainingError::CorpusVersion(self.schema_version.clone()));
        }
        nonempty("window id", &self.id)?;
        nonempty("window source", &self.source)?;
        nonempty("window conversation", &self.conversation)?;
        nonempty("speaker id", &self.speaker_id)?;
        if self.text.is_empty() || self.blocks.is_empty() {
            return Err(TrainingError::EmptyWindow);
        }
        let mut expected = 0u64;
        for (index, block) in self.blocks.iter().enumerate() {
            if block.start_byte != expected
                || block.end_byte < block.start_byte
                || block.source_end_byte < block.source_start_byte
            {
                return Err(TrainingError::InvalidWindowBlock(index));
            }
            let start = usize::try_from(block.start_byte)
                .map_err(|_| TrainingError::InvalidWindowBlock(index))?;
            let end = usize::try_from(block.end_byte)
                .map_err(|_| TrainingError::InvalidWindowBlock(index))?;
            if end > self.text.len()
                || !self.text.is_char_boundary(start)
                || !self.text.is_char_boundary(end)
            {
                return Err(TrainingError::InvalidWindowBlock(index));
            }
            if block.end_byte - block.start_byte != block.source_end_byte - block.source_start_byte
            {
                return Err(TrainingError::InvalidWindowBlock(index));
            }
            expected = block.end_byte;
            if index + 1 < self.blocks.len() {
                let sep = self.text.as_bytes().get(end).copied();
                if sep != Some(b'\n') {
                    return Err(TrainingError::InvalidWindowSeparator(index));
                }
                expected = expected.saturating_add(1);
            }
        }
        if expected != self.text.len() as u64 {
            return Err(TrainingError::InvalidWindowCoverage);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredChannel {
    ToolCall,
    ToolCallUpdate,
    ToolResult,
}

/// One source-visible leaf (or empty container) from a normalized structured
/// record. Oversized strings are split at UTF-8/discourse-safe boundaries; the
/// source byte range is relative to the complete decoded string at `field_path`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredContextField {
    /// Exact RFC-6901 path of a source scalar carried read-only to preserve semantic
    /// context after partitioning. It remains a legal source anchor because it is exact
    /// model-visible source context, but it never counts toward primary-field coverage.
    pub field_path: String,
    pub value: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredFieldBlock {
    pub field_path: String,
    /// Exact source-schema-declared aliases of `field_path`. Alias values are
    /// verified equal and omitted from model payload, but remain legal span targets.
    #[serde(default)]
    pub alias_paths: Vec<String>,
    /// Exact RFC-6901 paths of enclosing source containers. These are legal
    /// whole-object source anchors so a semantic object can retain identity when
    /// its member fields are partitioned across multiple model windows.
    #[serde(default)]
    pub container_paths: Vec<String>,
    /// Exact source scalars repeated for interpretation. They are source-addressable but
    /// read-only and do not satisfy primary-field semantic-coverage requirements.
    #[serde(default)]
    pub context: Vec<StructuredContextField>,
    pub value: StructuredFieldValue,
    pub source_start_byte: Option<u64>,
    pub source_end_byte: Option<u64>,
}

impl StructuredFieldBlock {
    fn validate(&self) -> Result<(), TrainingError> {
        if !self.field_path.is_empty() && !self.field_path.starts_with('/') {
            return Err(TrainingError::InvalidStructuredFieldPath(
                self.field_path.clone(),
            ));
        }
        let mut alias_paths = BTreeSet::new();
        for alias in &self.alias_paths {
            if alias.is_empty()
                || !alias.starts_with('/')
                || alias == &self.field_path
                || !alias_paths.insert(alias.as_str())
            {
                return Err(TrainingError::InvalidStructuredAlias(alias.clone()));
            }
        }
        let mut container_paths = BTreeSet::new();
        for container in &self.container_paths {
            if (!container.is_empty() && !container.starts_with('/'))
                || container == &self.field_path
                || !container_paths.insert(container.as_str())
            {
                return Err(TrainingError::InvalidStructuredContainerPath(
                    container.clone(),
                ));
            }
        }
        let mut context_bytes = 0usize;
        let mut context_paths = BTreeSet::new();
        for context in &self.context {
            if context.field_path.is_empty()
                || !context.field_path.starts_with('/')
                || context.field_path == self.field_path
                || !context_paths.insert(context.field_path.as_str())
                || context.value.is_array()
                || context.value.is_object()
            {
                return Err(TrainingError::InvalidStructuredContext(
                    self.field_path.clone(),
                ));
            }
            let bytes = serde_json::to_vec(context)?.len();
            if bytes
                > MAX_STRUCTURED_CONTEXT_VALUE_BYTES
                    .saturating_add(context.field_path.len())
                    .saturating_add(256)
            {
                return Err(TrainingError::InvalidStructuredContext(
                    self.field_path.clone(),
                ));
            }
            context_bytes = context_bytes.saturating_add(bytes);
        }
        if context_bytes > MAX_STRUCTURED_CONTEXT_BYTES {
            return Err(TrainingError::InvalidStructuredContext(
                self.field_path.clone(),
            ));
        }
        match (&self.value, self.source_start_byte, self.source_end_byte) {
            (StructuredFieldValue::Json { value }, Some(start), Some(end)) if value.is_string() => {
                let text = value.as_str().ok_or_else(|| {
                    TrainingError::InvalidStructuredFieldRange(self.field_path.clone())
                })?;
                if start > end || end - start != text.len() as u64 {
                    return Err(TrainingError::InvalidStructuredFieldRange(
                        self.field_path.clone(),
                    ));
                }
            }
            (StructuredFieldValue::Json { value }, None, None) if !value.is_string() => {}
            (StructuredFieldValue::Opaque { .. }, None, None) => {}
            _ => {
                return Err(TrainingError::InvalidStructuredFieldRange(
                    self.field_path.clone(),
                ));
            }
        }
        Ok(())
    }
}

/// A bounded, lossless view of one normalized tool call/result record. Every
/// source leaf appears in exactly one part except an oversized string, whose
/// non-overlapping fragments collectively cover the complete decoded string.
/// `part_index/part_count` preserve one-record identity across model windows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredWindow {
    pub schema_version: String,
    pub id: String,
    pub source: String,
    pub conversation: String,
    pub run: Option<String>,
    pub message: String,
    pub record: String,
    pub sequence: u64,
    pub part_index: u32,
    pub part_count: u32,
    pub actor_id: String,
    pub actor_role: SpeakerRole,
    pub recorder_id: Option<String>,
    pub recorder_role: Option<SpeakerRole>,
    /// Repeated context so every split part retains callable identity.
    pub tool_name: Option<String>,
    /// Repeated only when an exact source/provider invocation identity exists.
    pub invocation_id: Option<String>,
    pub channel: StructuredChannel,
    /// Exact versioned normalization contract used to construct this model-visible part.
    pub normalization_contract: String,
    pub normalization_digest: ContentDigest,
    pub fields: Vec<StructuredFieldBlock>,
}

impl StructuredWindow {
    #[must_use]
    pub fn semantic_source(&self) -> String {
        format!("muse-window:{}", self.id)
    }
    #[must_use]
    pub fn leakage_group(&self) -> String {
        match &self.run {
            Some(run) => format!("{}\u{1f}{}\u{1f}{run}", self.source, self.conversation),
            None => format!("{}\u{1f}{}", self.source, self.conversation),
        }
    }
    pub fn validate(&self) -> Result<(), TrainingError> {
        if self.schema_version != CORPUS_SCHEMA_VERSION {
            return Err(TrainingError::CorpusVersion(self.schema_version.clone()));
        }
        nonempty("window id", &self.id)?;
        nonempty("window source", &self.source)?;
        nonempty("window conversation", &self.conversation)?;
        nonempty("message id", &self.message)?;
        nonempty("record id", &self.record)?;
        nonempty("structured actor id", &self.actor_id)?;
        if self.part_count == 0 || self.part_index >= self.part_count || self.fields.is_empty() {
            return Err(TrainingError::InvalidStructuredPart {
                index: self.part_index,
                count: self.part_count,
            });
        }
        match (&self.recorder_id, self.recorder_role) {
            (Some(id), Some(_)) => nonempty("structured recorder id", id)?,
            (None, None) => {}
            _ => return Err(TrainingError::InconsistentStructuredRecorder),
        }
        match self.channel {
            StructuredChannel::ToolCall => {
                let name = self
                    .tool_name
                    .as_deref()
                    .ok_or(TrainingError::MissingStructuredToolName)?;
                nonempty("structured tool name", name)?;
            }
            StructuredChannel::ToolCallUpdate | StructuredChannel::ToolResult => {
                if let Some(name) = self.tool_name.as_deref() {
                    nonempty("structured tool name", name)?;
                }
            }
        }
        if let Some(id) = self.invocation_id.as_deref() {
            nonempty("structured invocation id", id)?;
        }
        nonempty(
            "structured normalization contract",
            &self.normalization_contract,
        )?;
        nonempty(
            "structured normalization digest",
            &self.normalization_digest.value,
        )?;
        for field in &self.fields {
            field.validate()?;
        }
        Ok(())
    }
}

/// Training input window. Structured records remain distinct from prose so field
/// provenance and record attribution cannot be confused with discourse speech.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "window_kind", content = "window", rename_all = "snake_case")]
pub enum TrainingWindow {
    Prose(ProseWindow),
    Structured(StructuredWindow),
}

impl TrainingWindow {
    pub fn validate(&self) -> Result<(), TrainingError> {
        match self {
            Self::Prose(w) => w.validate(),
            Self::Structured(w) => w.validate(),
        }
    }
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Prose(w) => &w.id,
            Self::Structured(w) => &w.id,
        }
    }
    #[must_use]
    pub fn source(&self) -> &str {
        match self {
            Self::Prose(w) => &w.source,
            Self::Structured(w) => &w.source,
        }
    }
    #[must_use]
    pub fn conversation(&self) -> &str {
        match self {
            Self::Prose(w) => &w.conversation,
            Self::Structured(w) => &w.conversation,
        }
    }
    #[must_use]
    pub fn run(&self) -> Option<&str> {
        match self {
            Self::Prose(w) => w.run.as_deref(),
            Self::Structured(w) => w.run.as_deref(),
        }
    }
    #[must_use]
    pub fn leakage_group(&self) -> String {
        match self {
            Self::Prose(w) => w.leakage_group(),
            Self::Structured(w) => w.leakage_group(),
        }
    }
    #[must_use]
    pub fn first_sequence(&self) -> u64 {
        match self {
            Self::Prose(w) => w.blocks.first().map_or(0, |b| b.sequence),
            Self::Structured(w) => w.sequence,
        }
    }
    #[must_use]
    pub fn part_index(&self) -> u32 {
        match self {
            Self::Prose(_) => 0,
            Self::Structured(w) => w.part_index,
        }
    }
}

/// Build exact contiguous prose windows. Tool calls/results and metadata are hard
/// boundaries. Different messages, speakers, channels, runs, or conversations
/// are also hard boundaries so first-person perspective is never ambiguous.
pub fn build_prose_windows(
    records: &[TranscriptRecord],
    max_window_bytes: Option<usize>,
) -> Result<Vec<ProseWindow>, TrainingError> {
    if max_window_bytes == Some(0) {
        return Err(TrainingError::ZeroWindowLimit);
    }
    let mut output = Vec::new();
    let mut current: Vec<Fragment> = Vec::new();
    let mut prior_sequences: BTreeMap<(String, String), u64> = BTreeMap::new();

    for record in records {
        nonempty("record source", &record.source)?;
        nonempty("record conversation", &record.conversation)?;
        nonempty("record message", &record.message)?;
        nonempty("record id", &record.record)?;
        nonempty("speaker id", &record.speaker_id)?;
        let sequence_key = (record.source.clone(), record.conversation.clone());
        if let Some(previous) = prior_sequences.get(&sequence_key).copied() {
            if record.sequence <= previous {
                return Err(TrainingError::NonMonotonicSequence {
                    source_id: record.source.clone(),
                    conversation: record.conversation.clone(),
                    previous,
                    current: record.sequence,
                });
            }
        }
        prior_sequences.insert(sequence_key, record.sequence);
        let TranscriptRecordKind::Prose { channel, text } = &record.kind else {
            flush(&mut output, &mut current)?;
            continue;
        };
        if text.is_empty() {
            flush(&mut output, &mut current)?;
            continue;
        }
        let fragments = split_text(text, max_window_bytes)?;
        for (fragment_index, (start, end)) in fragments.iter().copied().enumerate() {
            let fragment = Fragment {
                record,
                channel: *channel,
                source_start: start,
                source_end: end,
            };
            let incompatible = current
                .first()
                .is_some_and(|first| !compatible(first.record, first.channel, record, *channel));
            let separator = usize::from(!current.is_empty());
            let current_bytes =
                current.iter().map(Fragment::len).sum::<usize>() + current.len().saturating_sub(1);
            let would_overflow = max_window_bytes
                .is_some_and(|limit| current_bytes + separator + fragment.len() > limit);
            if incompatible || would_overflow {
                flush(&mut output, &mut current)?;
            }
            current.push(fragment);
            // A split fragment ends its window. This prevents synthetic separator
            // bytes from masquerading as contiguous bytes of one source record.
            if fragments.len() > 1 || fragment_index + 1 < fragments.len() {
                flush(&mut output, &mut current)?;
            }
        }
    }
    flush(&mut output, &mut current)?;
    Ok(output)
}

/// Build the complete learned corpus surface. Prose retains discourse-boundary
/// windowing. Structured records are flattened into exact RFC-6901-addressed
/// source fields and packed into bounded parts under a serialized payload budget;
/// fixed window-envelope overhead is additional. Oversized textual fields are
/// split without dropping bytes. Tool calls/results therefore remain learned
/// inputs even when a single native result is many megabytes.
pub fn build_training_windows(
    records: &[TranscriptRecord],
    normalization_contracts: &BTreeMap<String, NormalizationContract>,
    max_prose_window_bytes: Option<usize>,
    max_structured_payload_bytes: Option<usize>,
) -> Result<Vec<TrainingWindow>, TrainingError> {
    if max_structured_payload_bytes == Some(0) {
        return Err(TrainingError::ZeroStructuredWindowLimit);
    }
    for (id, contract) in normalization_contracts {
        if id != &contract.id {
            return Err(TrainingError::InvalidNormalizationContract(id.clone()));
        }
        contract.validate()?;
    }
    let prose = build_prose_windows(records, max_prose_window_bytes)?;
    let mut output = prose
        .into_iter()
        .map(TrainingWindow::Prose)
        .collect::<Vec<_>>();
    for record in records {
        let (channel, tool_name, invocation_id, payload, contract_id) = match &record.kind {
            TranscriptRecordKind::ToolCall {
                tool_name,
                invocation_id,
                payload,
                normalization_contract,
            } => (
                StructuredChannel::ToolCall,
                Some(tool_name.clone()),
                invocation_id.clone(),
                payload,
                normalization_contract,
            ),
            TranscriptRecordKind::ToolCallUpdate {
                tool_name,
                invocation_id,
                payload,
                normalization_contract,
            } => (
                StructuredChannel::ToolCallUpdate,
                tool_name.clone(),
                invocation_id.clone(),
                payload,
                normalization_contract,
            ),
            TranscriptRecordKind::ToolResult {
                tool_name,
                invocation_id,
                payload,
                normalization_contract,
            } => (
                StructuredChannel::ToolResult,
                tool_name.clone(),
                invocation_id.clone(),
                payload,
                normalization_contract,
            ),
            _ => continue,
        };
        let contract = normalization_contracts
            .get(contract_id)
            .ok_or_else(|| TrainingError::UnknownNormalizationContract(contract_id.clone()))?;
        let contract_digest = contract.digest()?;
        let parsed = payload.parse()?;
        let resolved = resolve_normalization_contract(&parsed, contract)?;
        let fields = flatten_structured_payload(&parsed, &resolved, max_structured_payload_bytes)?;
        let parts = pack_structured_fields(fields, max_structured_payload_bytes)?;
        let part_count =
            u32::try_from(parts.len()).map_err(|_| TrainingError::TooManyStructuredParts)?;
        for (part_index, fields) in parts.into_iter().enumerate() {
            let part_index =
                u32::try_from(part_index).map_err(|_| TrainingError::TooManyStructuredParts)?;
            let identity = serde_json::to_vec(&serde_json::json!({
                "schemaVersion": CORPUS_SCHEMA_VERSION,
                "source": record.source,
                "conversation": record.conversation,
                "run": record.run,
                "message": record.message,
                "record": record.record,
                "sequence": record.sequence,
                "partIndex": part_index,
                "partCount": part_count,
                "speakerId": record.speaker_id,
                "speakerRole": record.speaker_role,
                "recorderId": record.recorder_id,
                "recorderRole": record.recorder_role,
                "toolName": tool_name,
                "invocationId": invocation_id,
                "channel": channel,
                "contractId": contract_id,
                "contractDigest": contract_digest,
                "fields": fields,
            }))?;
            let id = ContentDigest::sha256_bytes(&identity).value;
            let window = StructuredWindow {
                schema_version: CORPUS_SCHEMA_VERSION.into(),
                id,
                source: record.source.clone(),
                conversation: record.conversation.clone(),
                run: record.run.clone(),
                message: record.message.clone(),
                record: record.record.clone(),
                sequence: record.sequence,
                part_index,
                part_count,
                actor_id: record.speaker_id.clone(),
                actor_role: record.speaker_role,
                recorder_id: record.recorder_id.clone(),
                recorder_role: record.recorder_role,
                tool_name: tool_name.clone(),
                invocation_id: invocation_id.clone(),
                channel,
                normalization_contract: contract_id.clone(),
                normalization_digest: contract_digest.clone(),
                fields,
            };
            window.validate()?;
            output.push(TrainingWindow::Structured(window));
        }
    }
    output.sort_by(|left, right| {
        (
            left.source(),
            left.conversation(),
            left.run(),
            left.first_sequence(),
            left.part_index(),
            left.id(),
        )
            .cmp(&(
                right.source(),
                right.conversation(),
                right.run(),
                right.first_sequence(),
                right.part_index(),
                right.id(),
            ))
    });
    Ok(output)
}

fn escape_json_pointer_segment(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn validate_path_pattern(pattern: &str) -> Result<(), TrainingError> {
    if pattern.is_empty() {
        return Ok(());
    }
    if !pattern.starts_with('/') {
        return Err(TrainingError::InvalidNormalizationPathPattern(
            pattern.into(),
        ));
    }
    for segment in pattern[1..].split('/') {
        if segment.contains('*') && segment != "*" {
            return Err(TrainingError::InvalidNormalizationPathPattern(
                pattern.into(),
            ));
        }
        let bytes = segment.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'~' {
                if i + 1 >= bytes.len() || !matches!(bytes[i + 1], b'0' | b'1') {
                    return Err(TrainingError::InvalidNormalizationPathPattern(
                        pattern.into(),
                    ));
                }
                i += 2;
            } else {
                i += 1;
            }
        }
    }
    Ok(())
}

fn wildcard_count(pattern: &str) -> usize {
    if pattern.is_empty() {
        0
    } else {
        pattern[1..]
            .split('/')
            .filter(|segment| *segment == "*")
            .count()
    }
}

fn path_pattern_match(pattern: &str, path: &str) -> Option<Vec<String>> {
    if pattern.is_empty() {
        return (path.is_empty()).then(Vec::new);
    }
    if path.is_empty() {
        return None;
    }
    let ps = pattern[1..].split('/').collect::<Vec<_>>();
    let xs = path[1..].split('/').collect::<Vec<_>>();
    if ps.len() != xs.len() {
        return None;
    }
    let mut bindings = Vec::new();
    for (p, x) in ps.into_iter().zip(xs) {
        if p == "*" {
            bindings.push(x.to_owned());
        } else if p != x {
            return None;
        }
    }
    Some(bindings)
}

fn instantiate_pattern(pattern: &str, bindings: &[String]) -> Result<String, TrainingError> {
    if pattern.is_empty() {
        return Ok(String::new());
    }
    let mut index = 0usize;
    let mut out = String::new();
    for segment in pattern[1..].split('/') {
        out.push('/');
        if segment == "*" {
            let binding = bindings
                .get(index)
                .ok_or_else(|| TrainingError::InvalidNormalizationPathPattern(pattern.into()))?;
            out.push_str(binding);
            index += 1;
        } else {
            out.push_str(segment);
        }
    }
    if index != bindings.len() {
        return Err(TrainingError::InvalidNormalizationPathPattern(
            pattern.into(),
        ));
    }
    Ok(out)
}

fn collect_json_paths(value: &serde_json::Value, path: &str, out: &mut Vec<String>) {
    out.push(path.to_owned());
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                collect_json_paths(
                    child,
                    &format!("{path}/{}", escape_json_pointer_segment(key)),
                    out,
                );
            }
        }
        serde_json::Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                collect_json_paths(child, &format!("{path}/{index}"), out);
            }
        }
        _ => {}
    }
}

fn strict_base64_decode(text: &str) -> Result<Vec<u8>, TrainingError> {
    if text.len() % 4 != 0 {
        return Err(TrainingError::InvalidOpaqueEncoding("base64".into()));
    }
    fn val(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (group_index, chunk) in bytes.chunks_exact(4).enumerate() {
        let last = group_index + 1 == bytes.len() / 4;
        let pad = usize::from(chunk[3] == b'=') + usize::from(chunk[2] == b'=');
        if (chunk[0] == b'=' || chunk[1] == b'=')
            || (!last && pad > 0)
            || (chunk[2] == b'=' && chunk[3] != b'=')
        {
            return Err(TrainingError::InvalidOpaqueEncoding("base64".into()));
        }
        let a = u32::from(
            val(chunk[0]).ok_or_else(|| TrainingError::InvalidOpaqueEncoding("base64".into()))?,
        );
        let b = u32::from(
            val(chunk[1]).ok_or_else(|| TrainingError::InvalidOpaqueEncoding("base64".into()))?,
        );
        let c = if chunk[2] == b'=' {
            0
        } else {
            u32::from(
                val(chunk[2])
                    .ok_or_else(|| TrainingError::InvalidOpaqueEncoding("base64".into()))?,
            )
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            u32::from(
                val(chunk[3])
                    .ok_or_else(|| TrainingError::InvalidOpaqueEncoding("base64".into()))?,
            )
        };
        if pad == 2 && (b & 0x0f) != 0 {
            return Err(TrainingError::InvalidOpaqueEncoding("base64".into()));
        }
        if pad == 1 && (c & 0x03) != 0 {
            return Err(TrainingError::InvalidOpaqueEncoding("base64".into()));
        }
        let n = (a << 18) | (b << 12) | (c << 6) | d;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

#[derive(Clone, Debug)]
struct ResolvedNormalization {
    aliases: BTreeMap<String, String>,
    canonical_aliases: BTreeMap<String, Vec<String>>,
    opaque: BTreeMap<String, OpaqueFieldRule>,
    context: BTreeMap<String, Vec<StructuredContextField>>,
}

fn resolve_normalization_contract(
    payload: &serde_json::Value,
    contract: &NormalizationContract,
) -> Result<ResolvedNormalization, TrainingError> {
    contract.validate()?;
    let mut paths = Vec::new();
    collect_json_paths(payload, "", &mut paths);
    let mut aliases = BTreeMap::new();
    let mut canonical_aliases: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for rule in &contract.aliases {
        for alias in &paths {
            let Some(bindings) = path_pattern_match(&rule.alias_pattern, alias) else {
                continue;
            };
            let canonical = instantiate_pattern(&rule.canonical_pattern, &bindings)?;
            let av = payload
                .pointer(alias)
                .ok_or_else(|| TrainingError::InvalidStructuredAlias(alias.clone()))?;
            let cv = payload
                .pointer(&canonical)
                .ok_or_else(|| TrainingError::InvalidStructuredAlias(alias.clone()))?;
            if av.is_array() || av.is_object() || cv.is_array() || cv.is_object() || av != cv {
                return Err(TrainingError::InvalidStructuredAlias(alias.clone()));
            }
            if aliases.insert(alias.clone(), canonical.clone()).is_some() {
                return Err(TrainingError::OverlappingNormalizationRule(alias.clone()));
            }
            canonical_aliases
                .entry(canonical)
                .or_default()
                .push(alias.clone());
        }
    }
    for canonical in canonical_aliases.keys() {
        if aliases.contains_key(canonical) {
            return Err(TrainingError::InvalidStructuredAlias(canonical.clone()));
        }
    }
    for values in canonical_aliases.values_mut() {
        values.sort();
        values.dedup();
    }

    let mut opaque = BTreeMap::new();
    for rule in &contract.opaque_fields {
        for path in &paths {
            if path_pattern_match(&rule.path_pattern, path).is_none() {
                continue;
            }
            if opaque.insert(path.clone(), rule.clone()).is_some() {
                return Err(TrainingError::OverlappingNormalizationRule(path.clone()));
            }
            let value = payload
                .pointer(path)
                .ok_or_else(|| TrainingError::UnknownOpaqueField(path.clone()))?;
            match rule.encoding {
                OpaqueEncoding::Base64 => {
                    let text = value
                        .as_str()
                        .ok_or_else(|| TrainingError::InvalidOpaqueEncoding(path.clone()))?;
                    strict_base64_decode(text)?;
                }
            }
        }
    }

    let mut context: BTreeMap<String, Vec<StructuredContextField>> = BTreeMap::new();
    for rule in &contract.context_dependencies {
        for primary in &paths {
            let Some(bindings) = path_pattern_match(&rule.primary_pattern, primary) else {
                continue;
            };
            let mut fields = Vec::new();
            let mut used = 0usize;
            for pattern in &rule.context_patterns {
                let path = instantiate_pattern(pattern, &bindings)?;
                if aliases.contains_key(&path) {
                    return Err(TrainingError::InvalidStructuredContext(primary.clone()));
                }
                let value = payload.pointer(&path).ok_or_else(|| {
                    TrainingError::MissingStructuredContext {
                        primary: primary.clone(),
                        context: path.clone(),
                    }
                })?;
                if value.is_array() || value.is_object() {
                    return Err(TrainingError::InvalidStructuredContext(primary.clone()));
                }
                let field = StructuredContextField {
                    field_path: path,
                    value: value.clone(),
                };
                let bytes = serde_json::to_vec(&field)?.len();
                if bytes
                    > MAX_STRUCTURED_CONTEXT_VALUE_BYTES
                        .saturating_add(field.field_path.len())
                        .saturating_add(256)
                    || used.saturating_add(bytes) > MAX_STRUCTURED_CONTEXT_BYTES
                {
                    return Err(TrainingError::InvalidStructuredContext(primary.clone()));
                }
                used = used.saturating_add(bytes);
                fields.push(field);
            }
            fields.sort_by(|a, b| a.field_path.cmp(&b.field_path));
            fields.dedup_by(|a, b| a.field_path == b.field_path);
            if context.insert(primary.clone(), fields).is_some() {
                return Err(TrainingError::OverlappingNormalizationRule(primary.clone()));
            }
        }
    }
    Ok(ResolvedNormalization {
        aliases,
        canonical_aliases,
        opaque,
        context,
    })
}

fn flatten_structured_payload(
    payload: &serde_json::Value,
    normalization: &ResolvedNormalization,
    max_window_bytes: Option<usize>,
) -> Result<Vec<StructuredFieldBlock>, TrainingError> {
    fn visit(
        value: &serde_json::Value,
        path: &str,
        containers: &[String],
        limit: Option<usize>,
        normalization: &ResolvedNormalization,
        out: &mut Vec<StructuredFieldBlock>,
    ) -> Result<(), TrainingError> {
        if normalization.aliases.contains_key(path) {
            return Ok(());
        }
        let alias_paths = normalization
            .canonical_aliases
            .get(path)
            .cloned()
            .unwrap_or_default();
        let context = normalization.context.get(path).cloned().unwrap_or_default();
        if let Some(rule) = normalization.opaque.get(path) {
            let text = value
                .as_str()
                .ok_or_else(|| TrainingError::InvalidOpaqueEncoding(path.into()))?;
            let decoded = match rule.encoding {
                OpaqueEncoding::Base64 => strict_base64_decode(text)?,
            };
            out.push(StructuredFieldBlock {
                field_path: path.into(),
                alias_paths,
                container_paths: containers.to_vec(),
                context,
                value: StructuredFieldValue::Opaque {
                    encoding: rule.encoding,
                    media_type: rule.media_type.clone(),
                    decoded_digest: ContentDigest::sha256_bytes(&decoded),
                    decoded_bytes: decoded.len() as u64,
                },
                source_start_byte: None,
                source_end_byte: None,
            });
            return Ok(());
        }
        match value {
            serde_json::Value::Object(map) if !map.is_empty() => {
                let mut child_containers = containers.to_vec();
                child_containers.push(path.into());
                for (key, child) in map {
                    let child_path = format!("{path}/{}", escape_json_pointer_segment(key));
                    visit(
                        child,
                        &child_path,
                        &child_containers,
                        limit,
                        normalization,
                        out,
                    )?;
                }
            }
            serde_json::Value::Array(values) if !values.is_empty() => {
                let mut child_containers = containers.to_vec();
                child_containers.push(path.into());
                for (index, child) in values.iter().enumerate() {
                    visit(
                        child,
                        &format!("{path}/{index}"),
                        &child_containers,
                        limit,
                        normalization,
                        out,
                    )?;
                }
            }
            serde_json::Value::String(text) => {
                let context_bytes = serde_json::to_vec(&context)?.len();
                let container_bytes = serde_json::to_vec(containers)?.len();
                let fragment_limit = limit.map(|value| {
                    value.saturating_sub(
                        path.len()
                            .saturating_add(context_bytes)
                            .saturating_add(container_bytes)
                            .saturating_add(384),
                    )
                });
                if fragment_limit == Some(0) {
                    return Err(TrainingError::StructuredPathExceedsWindow(path.into()));
                }
                for (start, end) in split_text(text, fragment_limit)? {
                    out.push(StructuredFieldBlock {
                        field_path: path.into(),
                        alias_paths: alias_paths.clone(),
                        container_paths: containers.to_vec(),
                        context: context.clone(),
                        value: StructuredFieldValue::Json {
                            value: serde_json::Value::String(text[start..end].into()),
                        },
                        source_start_byte: Some(start as u64),
                        source_end_byte: Some(end as u64),
                    });
                }
                if text.is_empty() {
                    out.push(StructuredFieldBlock {
                        field_path: path.into(),
                        alias_paths,
                        container_paths: containers.to_vec(),
                        context,
                        value: StructuredFieldValue::Json {
                            value: serde_json::Value::String(String::new()),
                        },
                        source_start_byte: Some(0),
                        source_end_byte: Some(0),
                    });
                }
            }
            _ => out.push(StructuredFieldBlock {
                field_path: path.into(),
                alias_paths,
                container_paths: containers.to_vec(),
                context,
                value: StructuredFieldValue::Json {
                    value: value.clone(),
                },
                source_start_byte: None,
                source_end_byte: None,
            }),
        }
        Ok(())
    }
    let mut out = Vec::new();
    visit(payload, "", &[], max_window_bytes, normalization, &mut out)?;
    if out.is_empty() {
        out.push(StructuredFieldBlock {
            field_path: String::new(),
            alias_paths: Vec::new(),
            container_paths: Vec::new(),
            context: Vec::new(),
            value: StructuredFieldValue::Json {
                value: payload.clone(),
            },
            source_start_byte: None,
            source_end_byte: None,
        });
    }
    Ok(out)
}

fn pack_structured_fields(
    fields: Vec<StructuredFieldBlock>,
    limit: Option<usize>,
) -> Result<Vec<Vec<StructuredFieldBlock>>, TrainingError> {
    let Some(limit) = limit else {
        return Ok(vec![fields]);
    };
    let mut parts = Vec::new();
    let mut current = Vec::new();
    let mut current_bytes = 0usize;
    for field in fields {
        let field_bytes = serde_json::to_vec(&field)?.len();
        if field_bytes > limit {
            return Err(TrainingError::StructuredFieldExceedsWindow {
                field: field.field_path.clone(),
                size: field_bytes,
                limit,
            });
        }
        if !current.is_empty() && current_bytes.saturating_add(field_bytes) > limit {
            parts.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current_bytes = current_bytes.saturating_add(field_bytes);
        current.push(field);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    if parts.is_empty() {
        return Err(TrainingError::EmptyStructuredWindow);
    }
    Ok(parts)
}

#[derive(Clone, Copy)]
struct Fragment<'a> {
    record: &'a TranscriptRecord,
    channel: ProseChannel,
    source_start: usize,
    source_end: usize,
}
impl Fragment<'_> {
    fn len(&self) -> usize {
        self.source_end - self.source_start
    }
}

fn compatible(
    left: &TranscriptRecord,
    left_channel: ProseChannel,
    right: &TranscriptRecord,
    right_channel: ProseChannel,
) -> bool {
    left.source == right.source
        && left.conversation == right.conversation
        && left.run == right.run
        && left.message == right.message
        && left.speaker_id == right.speaker_id
        && left.speaker_role == right.speaker_role
        && left.addressee_id == right.addressee_id
        && left.addressee_role == right.addressee_role
        && left_channel == right_channel
}

fn split_text(text: &str, limit: Option<usize>) -> Result<Vec<(usize, usize)>, TrainingError> {
    let Some(limit) = limit else {
        return Ok(vec![(0, text.len())]);
    };
    if text.len() <= limit {
        return Ok(vec![(0, text.len())]);
    }
    let mut out = Vec::new();
    let mut start = 0;
    while text.len() - start > limit {
        let hard = start + limit;
        let mut end = hard;
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            let required = text[start..].chars().next().map_or(1, char::len_utf8);
            return Err(TrainingError::WindowLimitTooSmall { limit, required });
        }
        let floor = start + (end - start) / 2;
        let slice = &text[start..end];
        let mut chosen = None;
        for marker in ["\n\n", "\n", ". ", "? ", "! ", "; ", ", ", " "] {
            if let Some(pos) = slice.rfind(marker) {
                let candidate = start + pos + marker.len();
                if candidate >= floor && candidate > start {
                    chosen = Some(candidate);
                    break;
                }
            }
        }
        let end = chosen.unwrap_or(end);
        out.push((start, end));
        start = end;
    }
    if start < text.len() {
        out.push((start, text.len()));
    }
    Ok(out)
}

fn flush(
    output: &mut Vec<ProseWindow>,
    current: &mut Vec<Fragment<'_>>,
) -> Result<(), TrainingError> {
    if current.is_empty() {
        return Ok(());
    }
    let first = current[0];
    let mut text = String::new();
    let mut blocks = Vec::new();
    for (index, fragment) in current.iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        let start = text.len();
        let source_text = match &fragment.record.kind {
            TranscriptRecordKind::Prose { text, .. } => text,
            _ => return Err(TrainingError::InternalNonProseFragment),
        };
        text.push_str(&source_text[fragment.source_start..fragment.source_end]);
        let end = text.len();
        blocks.push(WindowBlock {
            record: fragment.record.record.clone(),
            sequence: fragment.record.sequence,
            message: fragment.record.message.clone(),
            source_start_byte: fragment.source_start as u64,
            source_end_byte: fragment.source_end as u64,
            start_byte: start as u64,
            end_byte: end as u64,
        });
    }
    let identity_blocks = blocks
        .iter()
        .map(|block| {
            (
                block.record.as_str(),
                block.sequence,
                block.message.as_str(),
                block.source_start_byte,
                block.source_end_byte,
            )
        })
        .collect::<Vec<_>>();
    let identity = serde_json::to_vec(&(
        CORPUS_SCHEMA_VERSION,
        first.record.source.as_str(),
        first.record.conversation.as_str(),
        first.record.run.as_deref(),
        first.record.speaker_id.as_str(),
        first.record.speaker_role,
        first.record.addressee_id.as_deref(),
        first.record.addressee_role,
        first.channel,
        &identity_blocks,
        text.as_str(),
    ))?;
    let digest = ContentDigest::sha256_bytes(&identity);
    let window = ProseWindow {
        schema_version: CORPUS_SCHEMA_VERSION.into(),
        id: digest.value,
        source: first.record.source.clone(),
        conversation: first.record.conversation.clone(),
        run: first.record.run.clone(),
        speaker_id: first.record.speaker_id.clone(),
        speaker_role: first.record.speaker_role,
        addressee_id: first.record.addressee_id.clone(),
        addressee_role: first.record.addressee_role,
        channel: first.channel,
        text,
        blocks,
    };
    window.validate()?;
    output.push(window);
    current.clear();
    Ok(())
}

/// Everything that fixes the meaning of one training target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainingContract {
    pub label_protocol: String,
    pub corpus_schema: String,
    pub occurrence_schema: String,
    pub canonicalization: String,
    pub lowering: String,
    pub base_theory_namespace: String,
    pub base_theory_version: String,
    pub ontology: RegistrySnapshot,
    /// Exact normalization rules permitted for structured learned windows.
    #[serde(default)]
    pub normalization_contracts: BTreeMap<String, NormalizationContract>,
}

impl TrainingContract {
    #[must_use]
    pub fn for_ontology(ontology: RegistrySnapshot) -> Self {
        Self {
            label_protocol: LABEL_PROTOCOL_VERSION.into(),
            corpus_schema: CORPUS_SCHEMA_VERSION.into(),
            occurrence_schema: OCCURRENCE_SCHEMA_VERSION.into(),
            canonicalization: TRAINING_CANONICALIZATION_VERSION.into(),
            lowering: LOWERING_VERSION.into(),
            base_theory_namespace: BASE_THEORY_NAMESPACE.into(),
            base_theory_version: BASE_THEORY_VERSION.into(),
            ontology,
            normalization_contracts: BTreeMap::new(),
        }
    }
    #[must_use]
    pub fn with_normalization_contracts(
        mut self,
        contracts: impl IntoIterator<Item = NormalizationContract>,
    ) -> Self {
        self.normalization_contracts = contracts
            .into_iter()
            .map(|contract| (contract.id.clone(), contract))
            .collect();
        self
    }
    fn validate(&self) -> Result<(), TrainingError> {
        if self.label_protocol != LABEL_PROTOCOL_VERSION
            || self.corpus_schema != CORPUS_SCHEMA_VERSION
            || self.occurrence_schema != OCCURRENCE_SCHEMA_VERSION
            || self.canonicalization != TRAINING_CANONICALIZATION_VERSION
            || self.lowering != LOWERING_VERSION
            || self.base_theory_namespace != BASE_THEORY_NAMESPACE
            || self.base_theory_version != BASE_THEORY_VERSION
        {
            return Err(TrainingError::ContractVersionMismatch);
        }
        for (id, contract) in &self.normalization_contracts {
            if id != &contract.id {
                return Err(TrainingError::InvalidNormalizationContract(id.clone()));
            }
            contract.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelParent {
    pub ordinal: u32,
    pub canonical_target: ContentDigest,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelRevision {
    pub ordinal: u32,
    pub parent: Option<LabelParent>,
}
impl LabelRevision {
    fn validate(&self) -> Result<(), TrainingError> {
        match (self.ordinal, &self.parent) {
            (0, _) => Err(TrainingError::InvalidRevision),
            (1, None) => Ok(()),
            (ordinal, Some(parent)) if parent.ordinal.checked_add(1) == Some(ordinal) => Ok(()),
            _ => Err(TrainingError::InvalidRevision),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabeledExample {
    pub contract: TrainingContract,
    pub window: TrainingWindow,
    pub revision: LabelRevision,
    pub annotation_derivation: Derivation,
    pub label: LearnedSemanticTarget,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConformanceReceipt {
    pub version: String,
    pub contract_digest: ContentDigest,
    pub window_id: String,
    pub revision: u32,
    pub annotation_derivation: Derivation,
    pub canonical_target: ContentDigest,
    pub formal_submission: String,
    pub formal_ontology: String,
    pub formal_theory: String,
    pub statement_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BounceReason {
    Source,
    Windowing,
    Ontology,
    Formalism,
}

/// Explicit human/model refusal to fabricate a lossy label when the source or
/// frozen contract cannot represent the window faithfully. Ambiguity is not a bounce.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelBounce {
    pub protocol: String,
    pub window_id: String,
    pub reason: BounceReason,
    pub detail: String,
}
impl LabelBounce {
    pub fn validate(&self, window: &TrainingWindow) -> Result<(), TrainingError> {
        if self.protocol != LABEL_PROTOCOL_VERSION {
            return Err(TrainingError::WrongLabelProtocol);
        }
        if self.window_id != window.id() {
            return Err(TrainingError::BounceWindowMismatch);
        }
        nonempty("bounce detail", &self.detail)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineRecord {
    pub window_id: String,
    pub revision: u32,
    pub error: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConformanceBatch {
    pub accepted: Vec<ConformanceReceipt>,
    pub quarantine: Vec<QuarantineRecord>,
}

pub fn conform_label(
    registry: &PackageRegistry,
    example: &LabeledExample,
) -> Result<ConformanceReceipt, TrainingError> {
    example.contract.validate()?;
    example.window.validate()?;
    example.revision.validate()?;
    validate_derivation(&example.annotation_derivation)?;
    if let TrainingWindow::Structured(window) = &example.window {
        let contract = example
            .contract
            .normalization_contracts
            .get(&window.normalization_contract)
            .ok_or_else(|| {
                TrainingError::UnknownNormalizationContract(window.normalization_contract.clone())
            })?;
        let digest = contract.digest()?;
        if digest != window.normalization_digest {
            return Err(TrainingError::NormalizationContractMismatch(
                window.normalization_contract.clone(),
            ));
        }
    }
    let ontology_index = registry.ontology_index(&example.contract.ontology)?;
    let compiled = v6_compile::validate_and_compile_v6(
        &example.window,
        &example.label,
        example.contract.ontology.clone(),
        example.annotation_derivation.clone(),
        &ontology_index,
    )?;
    let canonical_target = compiled.canonical_training_digest()?;
    if let Some(parent) = &example.revision.parent {
        if parent.canonical_target == canonical_target {
            return Err(TrainingError::RevisionDidNotChange);
        }
    }
    let checked = lower_and_kernel_check(registry, &compiled)?;
    let contract_digest = ContentDigest::sha256_bytes(&serde_json::to_vec(&example.contract)?);
    Ok(ConformanceReceipt {
        version: CONFORMANCE_RECEIPT_VERSION.into(),
        contract_digest,
        window_id: example.window.id().to_owned(),
        revision: example.revision.ordinal,
        annotation_derivation: example.annotation_derivation.clone(),
        canonical_target,
        formal_submission: checked.lowered.submission.canonical_hash().to_string(),
        formal_ontology: checked
            .lowered
            .submission
            .ontology
            .canonical_hash()
            .to_string(),
        formal_theory: checked.compiled.theory.id().to_string(),
        statement_count: checked.statements.len() as u64,
    })
}

pub fn conform_label_batch(
    registry: &PackageRegistry,
    examples: &[LabeledExample],
) -> ConformanceBatch {
    let mut batch = ConformanceBatch::default();
    for example in examples {
        match conform_label(registry, example) {
            Ok(receipt) => batch.accepted.push(receipt),
            Err(error) => batch.quarantine.push(QuarantineRecord {
                window_id: example.window.id().to_owned(),
                revision: example.revision.ordinal,
                error: error.to_string(),
            }),
        }
    }
    batch
}

fn validate_derivation(derivation: &Derivation) -> Result<(), TrainingError> {
    match derivation {
        Derivation::HumanAnnotation { protocol, .. }
        | Derivation::ModelGenerated { protocol, .. }
            if protocol == LABEL_PROTOCOL_VERSION =>
        {
            Ok(())
        }
        Derivation::DeterministicStructured { .. } => {
            Err(TrainingError::DeterministicDerivationForLearnedLabel)
        }
        Derivation::Imported { .. } => Err(TrainingError::ImportedDerivationForLearnedLabel),
        _ => Err(TrainingError::WrongLabelProtocol),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetSplit {
    Train,
    Validation,
    Test,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitPolicy {
    pub train_basis_points: u16,
    pub validation_basis_points: u16,
    pub test_basis_points: u16,
    pub salt: String,
}
impl SplitPolicy {
    pub fn validate(&self) -> Result<(), TrainingError> {
        nonempty("split salt", &self.salt)?;
        if u32::from(self.train_basis_points)
            + u32::from(self.validation_basis_points)
            + u32::from(self.test_basis_points)
            == 10_000
        {
            Ok(())
        } else {
            Err(TrainingError::InvalidSplitPolicy)
        }
    }
    pub fn assign(&self, window: &TrainingWindow) -> Result<DatasetSplit, TrainingError> {
        self.validate()?;
        let digest = ContentDigest::sha256_bytes(
            format!("{}\u{1f}{}", self.salt, window.leakage_group()).as_bytes(),
        );
        let prefix = digest
            .value
            .get(..16)
            .ok_or(TrainingError::InvalidSplitPolicy)?;
        let bucket = (u64::from_str_radix(prefix, 16)
            .map_err(|_| TrainingError::InvalidSplitPolicy)?
            % 10_000) as u16;
        Ok(if bucket < self.train_basis_points {
            DatasetSplit::Train
        } else if bucket < self.train_basis_points + self.validation_basis_points {
            DatasetSplit::Validation
        } else {
            DatasetSplit::Test
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetEntry {
    pub window: TrainingWindow,
    pub revision: LabelRevision,
    pub annotation_derivation: Derivation,
    pub canonical_target: ContentDigest,
    pub receipt: ConformanceReceipt,
    pub split: DatasetSplit,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetManifest {
    pub contract: TrainingContract,
    pub split_policy: SplitPolicy,
    pub entries: Vec<DatasetEntry>,
    pub identity: ContentDigest,
}

pub fn build_dataset_manifest(
    contract: TrainingContract,
    split_policy: SplitPolicy,
    mut entries: Vec<DatasetEntry>,
) -> Result<DatasetManifest, TrainingError> {
    contract.validate()?;
    split_policy.validate()?;
    let mut seen = BTreeSet::new();
    let mut latest: BTreeMap<&str, u32> = BTreeMap::new();
    for entry in &entries {
        entry.window.validate()?;
        entry.revision.validate()?;
        if !seen.insert((entry.window.id(), entry.revision.ordinal)) {
            return Err(TrainingError::DuplicateDatasetRevision);
        }
        latest
            .entry(entry.window.id())
            .and_modify(|value| *value = (*value).max(entry.revision.ordinal))
            .or_insert(entry.revision.ordinal);
        if entry.receipt.window_id != entry.window.id()
            || entry.receipt.revision != entry.revision.ordinal
            || entry.receipt.canonical_target != entry.canonical_target
            || entry.receipt.annotation_derivation != entry.annotation_derivation
        {
            return Err(TrainingError::StaleReceipt);
        }
        let expected_split = split_policy.assign(&entry.window)?;
        if entry.split != expected_split {
            return Err(TrainingError::SplitAssignmentMismatch {
                window_id: entry.window.id().into(),
                expected: expected_split,
                actual: entry.split,
            });
        }
    }
    for entry in &entries {
        if latest.get(entry.window.id()) != Some(&entry.revision.ordinal) {
            return Err(TrainingError::StaleDatasetRevision);
        }
    }
    entries.sort_by(|left, right| {
        (
            left.split,
            left.window.source(),
            left.window.conversation(),
            left.window.run(),
            left.window.id(),
            left.revision.ordinal,
        )
            .cmp(&(
                right.split,
                right.window.source(),
                right.window.conversation(),
                right.window.run(),
                right.window.id(),
                right.revision.ordinal,
            ))
    });
    let identity = ContentDigest::sha256_bytes(&serde_json::to_vec(&(
        contract.clone(),
        split_policy.clone(),
        &entries,
    ))?);
    Ok(DatasetManifest {
        contract,
        split_policy,
        entries,
        identity,
    })
}

fn nonempty(field: &'static str, value: &str) -> Result<(), TrainingError> {
    if value.trim().is_empty() {
        Err(TrainingError::EmptyField(field))
    } else {
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum TrainingError {
    #[error("semantic-v6 conformance error: {0}")]
    SemanticV6(String),
    #[error("empty {0}")]
    EmptyField(&'static str),
    #[error("corpus schema version mismatch: {0}")]
    CorpusVersion(String),
    #[error("prose window is empty")]
    EmptyWindow,
    #[error("structured window is empty")]
    EmptyStructuredWindow,
    #[error("invalid prose window block {0}")]
    InvalidWindowBlock(usize),
    #[error("invalid synthetic separator after prose window block {0}")]
    InvalidWindowSeparator(usize),
    #[error("prose window blocks do not cover the complete text")]
    InvalidWindowCoverage,
    #[error("maximum prose window size cannot be zero")]
    ZeroWindowLimit,
    #[error("maximum structured window size cannot be zero")]
    ZeroStructuredWindowLimit,
    #[error("tool-call structured window is missing its tool name")]
    MissingStructuredToolName,
    #[error(
        "structured tool-call label must contain exactly one ToolInvocation root grounded by @tool_name; found {0}"
    )]
    StructuredToolInvocationRootCount(usize),
    #[error(
        "structured tool-call label must contain exactly one Tool referent grounded by @tool_name; found {0}"
    )]
    StructuredToolReferentCount(usize),
    #[error("structured tool-call root is missing toolInvocationPrincipal to the window actor")]
    MissingStructuredToolPrincipal,
    #[error("structured tool-call root is missing invokesTool to the window tool referent")]
    MissingStructuredInvokesTool,
    #[error(
        "structured tool-call-update label must contain exactly one ToolInvocationUpdate root grounded by @record; found {0}"
    )]
    StructuredToolInvocationUpdateRootCount(usize),
    #[error(
        "structured tool-result label must contain exactly one ToolResult root grounded by @record; found {0}"
    )]
    StructuredToolResultRootCount(usize),
    #[error(
        "maximum prose/text-field window size {limit} cannot fit the next UTF-8 scalar ({required} bytes)"
    )]
    WindowLimitTooSmall { limit: usize, required: usize },
    #[error("structured field path exceeds the configured window budget: {0}")]
    StructuredPathExceedsWindow(String),
    #[error("structured field {field} serializes to {size} bytes, exceeding window budget {limit}")]
    StructuredFieldExceedsWindow {
        field: String,
        size: usize,
        limit: usize,
    },
    #[error("structured record requires more parts than the corpus schema can represent")]
    TooManyStructuredParts,
    #[error("invalid structured part index/count: {index}/{count}")]
    InvalidStructuredPart { index: u32, count: u32 },
    #[error("invalid structured field path {0}")]
    InvalidStructuredFieldPath(String),
    #[error("invalid structured container source path {0}")]
    InvalidStructuredContainerPath(String),
    #[error("invalid structured field source range for {0}")]
    InvalidStructuredFieldRange(String),
    #[error("invalid or over-budget structured context carried for field {0}")]
    InvalidStructuredContext(String),
    #[error("invalid structured source alias declaration: {0}")]
    InvalidStructuredAlias(String),
    #[error("invalid normalization contract: {0}")]
    InvalidNormalizationContract(String),
    #[error("unknown normalization contract: {0}")]
    UnknownNormalizationContract(String),
    #[error("structured window normalization contract digest mismatch: {0}")]
    NormalizationContractMismatch(String),
    #[error("invalid normalization RFC-6901 path pattern: {0}")]
    InvalidNormalizationPathPattern(String),
    #[error("normalization rules overlap on concrete source path: {0}")]
    OverlappingNormalizationRule(String),
    #[error("opaque field rule does not resolve to a source payload path: {0}")]
    UnknownOpaqueField(String),
    #[error("invalid opaque-field encoding or value: {0}")]
    InvalidOpaqueEncoding(String),
    #[error("structured field {primary} requires missing semantic context field {context}")]
    MissingStructuredContext { primary: String, context: String },
    #[error("internal corpus builder received a non-prose fragment")]
    InternalNonProseFragment,
    #[error(
        "non-monotonic source sequence for {source_id}/{conversation}: previous {previous}, current {current}"
    )]
    NonMonotonicSequence {
        source_id: String,
        conversation: String,
        previous: u64,
        current: u64,
    },
    #[error("training contract version mismatch")]
    ContractVersionMismatch,
    #[error("label ontology snapshot differs from frozen training contract")]
    OntologyPinMismatch,
    #[error("invalid label revision ancestry")]
    InvalidRevision,
    #[error("label revision canonical target did not change from parent")]
    RevisionDidNotChange,
    #[error("deterministic structured derivation cannot be submitted as a learned label")]
    DeterministicDerivationForLearnedLabel,
    #[error(
        "imported derivation cannot be submitted as a learned label; labels must name the frozen human/model protocol"
    )]
    ImportedDerivationForLearnedLabel,
    #[error("label derivation does not use the frozen semantic-label protocol")]
    WrongLabelProtocol,
    #[error("structured recorder id/role must either both be present or both absent")]
    InconsistentStructuredRecorder,
    #[error("expected exactly one window-speaker referent, found {0}")]
    SpeakerReferentCount(usize),
    #[error("window speaker referent has wrong discourse role")]
    SpeakerRoleMismatch,
    #[error("statement {0} presenter is not the window speaker")]
    PresenterMismatch(String),
    #[error("expected exactly one structured-window actor referent, found {0}")]
    StructuredActorReferentCount(usize),
    #[error("structured-window actor referent has wrong discourse role")]
    StructuredActorRoleMismatch,
    #[error("expected exactly one structured-window recorder referent, found {0}")]
    StructuredRecorderReferentCount(usize),
    #[error("structured-window recorder referent has wrong discourse role")]
    StructuredRecorderRoleMismatch,
    #[error("structured statement {0} presenter does not match recorder metadata")]
    StructuredPresenterMismatch(String),
    #[error("prose statement {0} may not use record presentation mode")]
    RecordModeInProse(String),
    #[error("structured statement {0} must use record presentation mode")]
    NonRecordModeInStructuredWindow(String),
    #[error("statement {0} has no source span")]
    MissingStatementSpan(String),
    #[error("proposition {0} has no source span")]
    MissingPropositionSpan(String),
    #[error("compositional entailment has no explicit source premises")]
    EmptyCompositionalPremises,
    #[error("invalid prose source alignment for span {0}")]
    InvalidProseSpan(String),
    #[error("invalid structured source alignment for span {0}")]
    InvalidStructuredSpan(String),
    #[error("structured byte span {0} targets a non-string field")]
    StructuredByteSpanOnNonString(String),
    #[error(
        "learned ontology concept must use the canonical namespaceless symbol, not qualified ID {0}"
    )]
    QualifiedOntologyConceptInLearnedLabel(String),
    #[error(
        "learned ontology relation must use the canonical namespaceless symbol, not qualified ID {0}"
    )]
    QualifiedOntologyRelationInLearnedLabel(String),
    #[error(
        "referent {0} has no ontology type; learned labels require universal ontological typing"
    )]
    MissingReferentOntologyType(String),
    #[error(
        "referent {0} has multiple ontology types; learned labels emit exactly one narrowest justified concept and derive ancestors downstream"
    )]
    NonMinimalReferentOntologyTyping(String),
    #[error("referent {0} carries an external identity not reserved by its source-window contract")]
    UnexpectedExternalIdentityInLearnedLabel(String),
    #[error(
        "referent {0} has no exact source grounding and is not a reserved source-window identity"
    )]
    UngroundedReferent(String),
    #[error(
        "occurrence {0} has no ontology type; learned labels require universal ontological typing"
    )]
    MissingOccurrenceOntologyType(String),
    #[error(
        "occurrence {0} has multiple ontology types; learned labels emit exactly one narrowest justified concept and derive ancestors downstream"
    )]
    NonMinimalOccurrenceOntologyTyping(String),
    #[error("occurrence {0} has no source span")]
    MissingOccurrenceSpan(String),
    #[error("prose occurrence {0} has no source-anchored predicate")]
    MissingOccurrenceLexicalAnchor(String),
    #[error("structured occurrence {0} interprets textual source bytes but has no lexical anchor")]
    MissingStructuredTextLexicalAnchor(String),

    #[error("occurrence {0} has grammatical profile without exact grammatical evidence")]
    MissingGrammaticalEvidence(String),
    #[error("occurrence {0} has grammatical evidence but no grammatical profile")]
    UnexpectedGrammaticalEvidence(String),
    #[error("occurrence {0} has an empty tense/aspect annotation")]
    EmptyTenseAspect(String),

    #[error(
        "occurrence {0} reuses lexical predicate material as a source-anchored participant role"
    )]
    RoleAnchorOverlapsLexicalPredicate(String),
    #[error("proposition {0} has a semantic predicate/operator without an exact source cue")]
    MissingOperatorCue(String),
    #[error(
        "atomic occurrence proposition {0} carries an operator cue; predicate grounding belongs on the occurrence"
    )]
    UnexpectedOperatorCue(String),
    #[error(
        "structural operator {0} carries grammatical profile but this operator kind cannot bear grammar"
    )]
    OperatorGrammarUnsupported(String),
    #[error("structural operator {0} has grammatical profile without exact grammatical cue spans")]
    MissingOperatorGrammaticalEvidence(String),
    #[error("structural operator {0} has grammatical cue spans without a grammatical profile")]
    UnexpectedOperatorGrammaticalEvidence(String),
    #[error("structural operator {0} has an empty tense/aspect annotation")]
    EmptyOperatorTenseAspect(String),
    #[error("source-anchored structural fallback is not contained in the operator cue")]
    SourceAnchorOutsideOperatorCue,
    #[error("{0} source evidence is not contained in the semantic object's source span")]
    SemanticSpanNotContained(&'static str),
    #[error("operator {0} requires embedded eventuality content")]
    OperatorRequiresEventuality(String),
    #[error("question statement {0} does not contain interrogative abstraction")]
    QuestionWithoutInterrogative(String),
    #[error("statement {0} duplicates top-level communicative force inside SpeechAct")]
    DuplicatedTopLevelForce(String),
    #[error(
        "statement {0} leaks knowledge/recollection content as a fact without independent source assertion"
    )]
    FactiveLeak(String),
    #[error("statement {0} source spans do not cover its semantic content")]
    StatementContentCoverage(String),
    #[error(
        "bound variable in proposition {0} has no ontology domain; use the narrowest justified concept, falling back to Entity when necessary"
    )]
    MissingVariableOntologyDomain(String),
    #[error(
        "free-text unresolved temporal expressions are forbidden in learned labels; use exact SourceTime grounding"
    )]
    FreeTextTemporalExpression,
    #[error("ambiguity {0} has no exact source grounding")]
    UngroundedAmbiguity(String),
    #[error(
        "ambiguity {0} uses a free-text kind; learned labels permit only the finite ambiguity inventory"
    )]
    FreeTextAmbiguityKind(String),
    #[error(
        "learned labels may not carry auxiliary EvidenceId metadata; exact source spans are the evidence contract"
    )]
    UnexpectedLearnedEvidence,
    #[error("label contains a proposition unreachable from statements or ambiguity structure")]
    OrphanProposition,
    #[error("label contains an occurrence unreachable from statements or ambiguity structure")]
    OrphanOccurrence,
    #[error("label bounce does not target its supplied training window")]
    BounceWindowMismatch,
    #[error("split basis points must sum to 10,000 and split salt must be non-empty")]
    InvalidSplitPolicy,
    #[error("dataset split mismatch for {window_id}: expected {expected:?}, found {actual:?}")]
    SplitAssignmentMismatch {
        window_id: String,
        expected: DatasetSplit,
        actual: DatasetSplit,
    },
    #[error("duplicate dataset window revision")]
    DuplicateDatasetRevision,
    #[error("dataset contains a stale non-latest label revision")]
    StaleDatasetRevision,
    #[error("conformance receipt does not match dataset entry")]
    StaleReceipt,
    #[error(transparent)]
    Occurrence(#[from] muse_occurrence::OccurrenceValidationError),
    #[error(transparent)]
    Canonical(#[from] muse_occurrence::TrainingCanonicalizationError),
    #[error(transparent)]
    Registry(#[from] muse_registry::RegistryError),
    #[error(transparent)]
    Lowering(#[from] LoweringError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> NormalizationContract {
        NormalizationContract {
            id: "generic-json-v1".into(),
            version: "1".into(),
            visibility: LearnedVisibility::ModelVisible,
            source_adapter: "test".into(),
            source_adapter_version: "1".into(),
            aliases: Vec::new(),
            opaque_fields: Vec::new(),
            context_dependencies: Vec::new(),
        }
    }
    fn contracts(contract: NormalizationContract) -> BTreeMap<String, NormalizationContract> {
        BTreeMap::from([(contract.id.clone(), contract)])
    }
    fn json(value: serde_json::Value) -> ExactJsonPayload {
        ExactJsonPayload::from_value(&value)
    }
    fn base_record(kind: TranscriptRecordKind) -> TranscriptRecord {
        TranscriptRecord {
            source: "s".into(),
            conversation: "c".into(),
            run: Some("r".into()),
            message: "m".into(),
            sequence: 1,
            speaker_id: "agent".into(),
            speaker_role: SpeakerRole::Agent,
            addressee_id: None,
            addressee_role: None,
            recorder_id: Some("h".into()),
            recorder_role: Some(SpeakerRole::Harness),
            record: "rec".into(),
            kind,
        }
    }

    #[test]
    fn long_utf8_text_is_split_without_loss() {
        let text = "α beta. γ delta. ".repeat(40);
        let records = vec![TranscriptRecord {
            source: "src".into(),
            conversation: "c".into(),
            run: Some("r".into()),
            message: "m".into(),
            sequence: 1,
            speaker_id: "u".into(),
            speaker_role: SpeakerRole::User,
            addressee_id: None,
            addressee_role: None,
            recorder_id: None,
            recorder_role: None,
            record: "rec".into(),
            kind: TranscriptRecordKind::Prose {
                channel: ProseChannel::UserText,
                text: text.clone(),
            },
        }];
        let windows = build_prose_windows(&records, Some(80)).unwrap();
        let joined = windows
            .iter()
            .flat_map(|w| {
                w.blocks.iter().map(|b| {
                    text[b.source_start_byte as usize..b.source_end_byte as usize].to_owned()
                })
            })
            .collect::<String>();
        assert_eq!(joined, text);
        assert!(windows.iter().all(|window| window.text.len() <= 80));
    }

    #[test]
    fn tool_records_are_hard_prose_boundaries() {
        let prose = |seq: u64, record: &str, text: &str| TranscriptRecord {
            source: "s".into(),
            conversation: "c".into(),
            run: Some("r".into()),
            message: "m".into(),
            sequence: seq,
            speaker_id: "a".into(),
            speaker_role: SpeakerRole::Agent,
            addressee_id: None,
            addressee_role: None,
            recorder_id: None,
            recorder_role: None,
            record: record.into(),
            kind: TranscriptRecordKind::Prose {
                channel: ProseChannel::AgentText,
                text: text.into(),
            },
        };
        let records = vec![
            prose(1, "a", "one"),
            TranscriptRecord {
                sequence: 2,
                ..base_record(TranscriptRecordKind::ToolCall {
                    tool_name: "x".into(),
                    invocation_id: Some("call-1".into()),
                    payload: json(serde_json::json!({"tool":"x"})),
                    normalization_contract: "generic-json-v1".into(),
                })
            },
            prose(3, "b", "two"),
        ];
        assert_eq!(build_prose_windows(&records, None).unwrap().len(), 2);
    }

    #[test]
    fn structured_call_is_learned_and_contract_pinned() {
        let c = contract();
        let cs = contracts(c.clone());
        let records = vec![base_record(TranscriptRecordKind::ToolCall {
            tool_name: "SendMessage".into(),
            invocation_id: Some("call-send".into()),
            payload: json(
                serde_json::json!({"input":{"recipient":"worker","message":"Inspect parser."}}),
            ),
            normalization_contract: c.id.clone(),
        })];
        let windows = build_training_windows(&records, &cs, None, Some(4096)).unwrap();
        let TrainingWindow::Structured(window) = &windows[0] else {
            panic!("structured")
        };
        assert_eq!(window.normalization_digest, c.digest().unwrap());
        assert!(
            window
                .fields
                .iter()
                .any(|field| field.field_path == "/input/message")
        );
    }

    #[test]
    fn wildcard_alias_and_context_rules_are_exact() {
        let c = NormalizationContract {
            id: "message-v1".into(),
            version: "1".into(),
            visibility: LearnedVisibility::ModelVisible,
            source_adapter: "test".into(),
            source_adapter_version: "1".into(),
            aliases: vec![FieldAliasRule {
                alias_pattern: "/messages/*/content".into(),
                canonical_pattern: "/messages/*/message".into(),
            }],
            opaque_fields: Vec::new(),
            context_dependencies: vec![ContextDependencyRule {
                primary_pattern: "/messages/*/message".into(),
                context_patterns: vec!["/messages/*/recipient".into()],
            }],
        };
        let payload = serde_json::json!({"messages":[{"recipient":"worker","message":"Inspect parser","content":"Inspect parser"}]});
        let parsed = ExactJsonPayload::from_value(&payload).parse().unwrap();
        let resolved = resolve_normalization_contract(&parsed, &c).unwrap();
        assert_eq!(
            resolved
                .aliases
                .get("/messages/0/content")
                .map(String::as_str),
            Some("/messages/0/message")
        );
        let fields = flatten_structured_payload(&parsed, &resolved, Some(4096)).unwrap();
        let message = fields
            .iter()
            .find(|field| field.field_path == "/messages/0/message")
            .unwrap();
        assert!(
            message
                .alias_paths
                .iter()
                .any(|path| path == "/messages/0/content")
        );
        assert!(
            message
                .context
                .iter()
                .any(|field| field.field_path == "/messages/0/recipient")
        );
    }

    #[test]
    fn duplicate_json_keys_are_rejected() {
        let payload = ExactJsonPayload {
            raw: r#"{"x":1,"x":2}"#.into(),
        };
        assert!(payload.parse().is_err());
    }

    #[test]
    fn exact_json_numbers_do_not_round_through_f64() {
        let payload = ExactJsonPayload {
            raw: r#"{"big":123456789012345678901234567890,"exp":1.2300e+40}"#.into(),
        };
        let parsed = payload.parse().unwrap();
        let object = parsed.as_object().unwrap();
        assert_eq!(
            object["big"].as_number().unwrap().to_string(),
            "123456789012345678901234567890"
        );
        assert_eq!(object["exp"].as_number().unwrap().to_string(), "1.2300e+40");
    }

    #[test]
    fn opaque_base64_must_be_valid_and_is_decoded_before_digesting() {
        let c = NormalizationContract {
            id: "image-v1".into(),
            version: "1".into(),
            visibility: LearnedVisibility::ModelVisible,
            source_adapter: "test".into(),
            source_adapter_version: "1".into(),
            aliases: Vec::new(),
            opaque_fields: vec![OpaqueFieldRule {
                path_pattern: "/data".into(),
                encoding: OpaqueEncoding::Base64,
                media_type: Some("application/octet-stream".into()),
            }],
            context_dependencies: Vec::new(),
        };
        let parsed = json(serde_json::json!({"data":"YWJj"})).parse().unwrap();
        let resolved = resolve_normalization_contract(&parsed, &c).unwrap();
        let fields = flatten_structured_payload(&parsed, &resolved, Some(4096)).unwrap();
        match &fields[0].value {
            StructuredFieldValue::Opaque {
                decoded_bytes,
                decoded_digest,
                ..
            } => {
                assert_eq!(*decoded_bytes, 3);
                assert_eq!(*decoded_digest, ContentDigest::sha256_bytes(b"abc"));
            }
            _ => panic!("opaque"),
        }
        let invalid = json(serde_json::json!({"data":"not base64"}))
            .parse()
            .unwrap();
        assert!(resolve_normalization_contract(&invalid, &c).is_err());
    }

    #[test]
    fn large_field_uses_only_declared_context() {
        let c = NormalizationContract {
            id: "patch-v1".into(),
            version: "1".into(),
            visibility: LearnedVisibility::ModelVisible,
            source_adapter: "test".into(),
            source_adapter_version: "1".into(),
            aliases: Vec::new(),
            opaque_fields: Vec::new(),
            context_dependencies: vec![ContextDependencyRule {
                primary_pattern: "/structuredPatch/*/content".into(),
                context_patterns: vec!["/structuredPatch/*/path".into()],
            }],
        };
        let text = "α beta. γ delta. ".repeat(200);
        let records = vec![base_record(TranscriptRecordKind::ToolResult {
            tool_name: Some("apply_patch".into()),
            invocation_id: Some("call-patch".into()),
            payload: json(serde_json::json!({"structuredPatch":[{"path":"x","content":text}]})),
            normalization_contract: c.id.clone(),
        })];
        let windows = build_training_windows(&records, &contracts(c), None, Some(512)).unwrap();
        assert!(windows.len() > 1);
        for window in windows {
            let TrainingWindow::Structured(window) = window else {
                continue;
            };
            for field in window.fields {
                if field.field_path == "/structuredPatch/0/content" {
                    assert!(
                        field
                            .context
                            .iter()
                            .any(|x| x.field_path == "/structuredPatch/0/path")
                    );
                }
            }
        }
    }
}
