//! Protocol-neutral structured tool-event normalization and deterministic
//! conversion into the shared Muse occurrence/proposition representation.
//!
//! Adapters for concrete harnesses map their native logs into [`ToolInvocationRecord`].
//! This crate then emits the same semantic dialect used by prose labels. A
//! returned success is represented as a reported outcome only; requested and
//! independently observed effects remain distinct inputs and distinct statements.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use muse_core::{
    ConceptId, ContentDigest, OccurrenceDocumentId, OccurrenceId, PropositionId, ReferentId,
    RelationId, SourceSpanId, StatementId,
};
use muse_occurrence::{
    Derivation, DiscourseRole, Literal, Occurrence, OccurrenceDocument, PresentationMode,
    Proposition, PropositionExpr, Referent, ReportedOutcomeStatus, SCHEMA_VERSION, SourceSpan,
    Statement, StatementBasis, TemporalAnchor, TemporalRelation, Term,
};
use muse_registry::RegistrySnapshot;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Version of the protocol-neutral structured-tool normalization schema.
pub const TOOL_SCHEMA_VERSION: &str = "muse-tool-record-3";

/// Lossless deterministic value form for structured tool arguments/results.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum StructuredValue {
    Null,
    Boolean(bool),
    Integer(String),
    Decimal(String),
    String(String),
    Array(Vec<StructuredValue>),
    Object(BTreeMap<String, StructuredValue>),
}

impl StructuredValue {
    /// Converts JSON without losing integer/decimal source lexemes.
    pub fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(value) => Self::Boolean(*value),
            serde_json::Value::Number(value) => {
                let lexeme = value.to_string();
                if value.is_i64() || value.is_u64() {
                    Self::Integer(lexeme)
                } else {
                    Self::Decimal(lexeme)
                }
            }
            serde_json::Value::String(value) => Self::String(value.clone()),
            serde_json::Value::Array(values) => {
                Self::Array(values.iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(values) => Self::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_json(value)))
                    .collect(),
            ),
        }
    }

    /// Converts back to ordinary JSON. Tagged [`StructuredValue`] serde is not
    /// used here because `Literal::Json` must contain the original JSON shape.
    pub fn to_json(&self) -> Result<serde_json::Value, ToolFormalizationError> {
        Ok(match self {
            Self::Null => serde_json::Value::Null,
            Self::Boolean(value) => serde_json::Value::Bool(*value),
            Self::Integer(value) | Self::Decimal(value) => serde_json::from_str(value)?,
            Self::String(value) => serde_json::Value::String(value.clone()),
            Self::Array(values) => serde_json::Value::Array(
                values
                    .iter()
                    .map(Self::to_json)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Self::Object(values) => {
                serde_json::Value::Object(
                    values
                        .iter()
                        .map(|(key, value)| Ok((key.clone(), value.to_json()?)))
                        .collect::<Result<
                            serde_json::Map<String, serde_json::Value>,
                            ToolFormalizationError,
                        >>()?,
                )
            }
        })
    }

    /// Shared occurrence-literal representation.
    pub fn to_literal(&self) -> Result<Literal, ToolFormalizationError> {
        Ok(match self {
            Self::Null => Literal::Null,
            Self::Boolean(value) => Literal::Boolean(*value),
            Self::Integer(value) => Literal::Integer(value.clone()),
            Self::Decimal(value) => Literal::Decimal(value.clone()),
            Self::String(value) => Literal::String(value.clone()),
            Self::Array(_) | Self::Object(_) => {
                Literal::Json(serde_json::to_string(&self.to_json()?)?)
            }
        })
    }
}

/// Namespace in which an adapter promises an external identity is stable.
///
/// Scope is explicit because tools, agents, paths, and artifacts do not share
/// one correct lifetime. Muse never widens a native identity beyond the scope
/// asserted by the adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityScope {
    /// Stable across records and runs from the same structured source.
    Source,
    /// Stable only within one source run/session. Requires `source.run`.
    Run,
    /// Stable only within this source record.
    Record,
}

/// Stable principal identity supplied by an adapter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub id: String,
    pub scope: IdentityScope,
    /// Single narrowest ontology sort established by the adapter.
    pub sort: ConceptId,
    /// Source-envelope discourse role; this is provenance, not a second ontology.
    pub discourse_role: DiscourseRole,
    pub label: Option<String>,
}

/// Stable tool identity supplied by an adapter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub id: String,
    pub scope: IdentityScope,
    pub name: String,
    pub specification_id: Option<String>,
    /// Single narrowest ontology sort for the tool artifact.
    pub sort: ConceptId,
}

/// Stable source/log coordinates for one structured record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredSource {
    pub source: String,
    pub run: Option<String>,
    pub turn: Option<String>,
    pub record: String,
    pub sequence: Option<u64>,
    pub unix_millis: Option<i64>,
    /// The principal whose log/record asserts the structured fields.
    pub recorder: Option<Principal>,
}

/// Artifact reference observed or requested by a tool record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactDescriptor {
    pub id: String,
    pub scope: IdentityScope,
    /// Single narrowest ontology sort.
    pub sort: ConceptId,
    pub locator: Option<String>,
}

/// Source-established state of an artifact, kept distinct from artifact
/// identity so changing content digests never create or overwrite identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactStateDescriptor {
    pub id: String,
    pub evidence: EffectEvidence,
    /// Exact ontology relation from the owning effect to this state.
    pub relation: RelationId,
    pub artifact: ArtifactDescriptor,
    pub digest: Option<String>,
    /// Single narrowest ontology sort for the state occurrence.
    pub sort: ConceptId,
    pub source_field: String,
}

/// Whether an effect is merely requested or independently established by the
/// structured source. A tool's returned `success` does not upgrade Requested to
/// Observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectEvidence {
    Requested,
    Observed,
}

/// One requested or observed effect. Fields not applicable to a family remain
/// absent; adapters must not synthesize values they did not receive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolEffect {
    pub id: String,
    /// Source record that independently supplies this effect. `None` means the
    /// invocation record itself.
    pub source_record: Option<StructuredSource>,
    pub evidence: EffectEvidence,
    /// Single narrowest ontology sort for the requested/observed effect.
    pub sort: ConceptId,
    pub target: Option<ArtifactDescriptor>,
    pub source: Option<ArtifactDescriptor>,
    pub destination: Option<ArtifactDescriptor>,
    pub inputs: Vec<ArtifactDescriptor>,
    pub outputs: Vec<ArtifactDescriptor>,
    pub states: Vec<ArtifactStateDescriptor>,
    pub command: Option<String>,
    pub source_field: String,
}

/// Captured execution streams/diagnostics from a tool result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionDetails {
    pub exit_code: Option<i32>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub diagnostics: Vec<String>,
}

/// One source-attributed returned payload fragment. A protocol may record text,
/// images, attachments, or later supplemental result material in distinct
/// structured records; their provenance must not be collapsed into one record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultPayloadFragment {
    /// Source record that supplies this payload fragment. `None` means the
    /// result-status record (or invocation record when the result has none).
    pub source_record: Option<StructuredSource>,
    pub value: StructuredValue,
    pub source_field: String,
}

/// Structured returned result. Status is an attribution from the source
/// protocol and is not independent evidence that requested world effects hold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultRecord {
    /// Source record that supplies status/detail/execution metadata. `None` means
    /// the invocation record itself.
    pub source_record: Option<StructuredSource>,
    pub status: ReportedOutcomeStatus,
    pub detail: Option<String>,
    pub payloads: Vec<ToolResultPayloadFragment>,
    pub execution: ExecutionDetails,
    /// Exact field carrying the source-reported status/detail attribution.
    pub status_source_field: String,
    /// Exact field from which execution details were decoded, when present.
    pub execution_source_field: Option<String>,
    pub duration_ms: Option<u64>,
    /// Exact field carrying duration, when a duration is present.
    pub duration_source_field: Option<String>,
}

/// Protocol-neutral normalized record for one tool invocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInvocationRecord {
    pub schema_version: String,
    pub source: StructuredSource,
    pub invocation_id: String,
    /// Native identity scope for `invocation_id`, `retry_of`, and
    /// `parent_invocation`.
    pub invocation_scope: IdentityScope,
    pub invoker: Principal,
    pub tool: ToolDescriptor,
    /// Single narrowest ontology sort for the invocation event.
    pub invocation_sort: ConceptId,
    pub arguments: BTreeMap<String, StructuredValue>,
    pub result: Option<ToolResultRecord>,
    pub effects: Vec<ToolEffect>,
    pub retry_of: Option<String>,
    pub parent_invocation: Option<String>,
}

impl ToolInvocationRecord {
    pub fn validate(&self) -> Result<(), ToolFormalizationError> {
        if self.schema_version != TOOL_SCHEMA_VERSION {
            return Err(ToolFormalizationError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        validate_structured_source(&self.source)?;
        validate_text("invocation id", &self.invocation_id)?;
        validate_identity_scope(self.invocation_scope, &self.source)?;
        validate_principal(&self.invoker)?;
        validate_identity_scope(self.invoker.scope, &self.source)?;
        if let Some(recorder) = &self.source.recorder {
            validate_principal(recorder)?;
            validate_identity_scope(recorder.scope, &self.source)?;
            if recorder.scope == self.invoker.scope
                && recorder.id == self.invoker.id
                && recorder != &self.invoker
            {
                return Err(ToolFormalizationError::InconsistentPrincipal(
                    recorder.id.clone(),
                ));
            }
        }
        validate_text("tool id", &self.tool.id)?;
        validate_text("tool name", &self.tool.name)?;
        validate_identity_scope(self.tool.scope, &self.source)?;
        self.tool.sort.validate()?;
        if let Some(specification) = &self.tool.specification_id {
            validate_text("tool specification id", specification)?;
        }
        self.invocation_sort.validate()?;
        if let Some(run) = &self.source.run {
            validate_text("source run", run)?;
        }
        if let Some(turn) = &self.source.turn {
            validate_text("source turn", turn)?;
        }
        if let Some(retry) = &self.retry_of {
            validate_text("retry invocation id", retry)?;
        }
        if let Some(parent) = &self.parent_invocation {
            validate_text("parent invocation id", parent)?;
        }
        for (name, value) in &self.arguments {
            validate_text("argument name", name)?;
            validate_structured_value(value)?;
        }
        let mut artifacts = BTreeMap::<(String, String), ArtifactDescriptor>::new();
        let mut effect_ids = BTreeSet::new();
        let mut state_ids = BTreeSet::new();
        for effect in &self.effects {
            if !effect_ids.insert(&effect.id) {
                return Err(ToolFormalizationError::DuplicateEffect(effect.id.clone()));
            }
            validate_text("effect id", &effect.id)?;
            validate_text("effect source field", &effect.source_field)?;
            if let Some(source) = &effect.source_record {
                validate_structured_source(source)?;
                ensure_same_source_namespace(&self.source, source)?;
            }
            effect.sort.validate()?;
            let effect_source = effect.source_record.as_ref().unwrap_or(&self.source);
            for artifact in effect
                .target
                .iter()
                .chain(effect.source.iter())
                .chain(effect.destination.iter())
                .chain(effect.inputs.iter())
                .chain(effect.outputs.iter())
            {
                validate_artifact(artifact)?;
                validate_identity_scope(artifact.scope, effect_source)?;
                register_artifact_descriptor(&mut artifacts, artifact, effect_source)?;
            }
            for state in &effect.states {
                if effect.evidence == EffectEvidence::Observed
                    && state.evidence == EffectEvidence::Requested
                {
                    return Err(ToolFormalizationError::InconsistentEffectStateEvidence {
                        effect: effect.id.clone(),
                        state: state.id.clone(),
                    });
                }
                validate_text("artifact state id", &state.id)?;
                validate_text("artifact state source field", &state.source_field)?;
                validate_artifact(&state.artifact)?;
                validate_identity_scope(state.artifact.scope, effect_source)?;
                register_artifact_descriptor(&mut artifacts, &state.artifact, effect_source)?;
                state.relation.validate()?;
                state.sort.validate()?;
                if let Some(digest) = &state.digest {
                    validate_text("artifact state digest", digest)?;
                }
                if !state_ids.insert(&state.id) {
                    return Err(ToolFormalizationError::DuplicateEffectState(
                        state.id.clone(),
                    ));
                }
            }
        }
        if let Some(result) = &self.result {
            validate_text("result status source field", &result.status_source_field)?;
            let has_execution = result.execution.exit_code.is_some()
                || result.execution.stdout.is_some()
                || result.execution.stderr.is_some()
                || !result.execution.diagnostics.is_empty();
            match (&result.execution_source_field, has_execution) {
                (Some(field), true) => validate_text("execution source field", field)?,
                (None, false) => {}
                _ => return Err(ToolFormalizationError::InconsistentResultSourceFields),
            }
            match (&result.duration_source_field, result.duration_ms.is_some()) {
                (Some(field), true) => validate_text("duration source field", field)?,
                (None, false) => {}
                _ => return Err(ToolFormalizationError::InconsistentResultSourceFields),
            }
            if let Some(source) = &result.source_record {
                validate_structured_source(source)?;
                ensure_same_source_namespace(&self.source, source)?;
            }
            for payload in &result.payloads {
                validate_text("result payload source field", &payload.source_field)?;
                if let Some(source) = &payload.source_record {
                    validate_structured_source(source)?;
                    ensure_same_source_namespace(&self.source, source)?;
                }
                validate_structured_value(&payload.value)?;
            }
        }
        Ok(())
    }
}

/// Deterministic protocol-neutral formalizer. Tool-family adapters are
/// responsible only for normalizing native records and supplying effects that
/// are genuinely established by those native fields.
#[derive(Clone, Debug)]
pub struct ToolFormalizer {
    pub adapter: String,
    pub adapter_version: String,
    pub ontology: RegistrySnapshot,
}

impl ToolFormalizer {
    pub fn formalize(
        &self,
        record: &ToolInvocationRecord,
    ) -> Result<OccurrenceDocument, ToolFormalizationError> {
        record.validate()?;
        validate_text("adapter", &self.adapter)?;
        validate_text("adapter version", &self.adapter_version)?;

        let seed = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            self.adapter, record.source.source, record.source.record, record.invocation_id
        );
        let mut builder = DocumentBuilder::new(
            self.ontology.clone(),
            self.adapter.clone(),
            self.adapter_version.clone(),
            &seed,
            &record.source,
        );

        let invoker = builder.principal(&record.invoker, "invoker")?;
        let recorder = match &record.source.recorder {
            Some(principal) => Some(builder.principal(principal, "recorder")?),
            None => None,
        };
        let tool = builder.tool(&record.tool)?;
        let run = builder.run_referent(&record.source)?;

        let invocation_id = builder.id_external_occurrence(
            record.invocation_scope,
            "tool-invocation",
            &record.invocation_id,
        )?;
        let invocation_types = BTreeSet::from([record.invocation_sort.clone()]);

        let root_span = builder.span("")?;
        builder.document.occurrences.insert(
            invocation_id.clone(),
            Occurrence {
                id: invocation_id.clone(),
                types: invocation_types,
                lexical_anchor: None,
                participants: Vec::new(),
                attributes: BTreeMap::new(),
                tense_aspect: None,
                grammatical_tense: None,
                grammatical_voice: None,
                grammatical_spans: BTreeSet::new(),
                reported_outcome: None,
                source_spans: BTreeSet::from([root_span.clone()]),
                evidence: BTreeSet::new(),
            },
        );
        builder.statement_for_occurrence(
            "invocation",
            recorder.clone(),
            PresentationMode::Assertion,
            invocation_id.clone(),
            root_span,
        )?;

        builder.relation_statement(
            "invocation-principal",
            recorder.clone(),
            RelationId::from("agent:toolInvocationPrincipal"),
            vec![
                Term::Occurrence(invocation_id.clone()),
                Term::Referent(invoker.clone()),
            ],
            "",
        )?;
        builder.relation_statement(
            "invokes-tool",
            recorder.clone(),
            RelationId::from("agent:invokesTool"),
            vec![
                Term::Occurrence(invocation_id.clone()),
                Term::Referent(tool),
            ],
            "",
        )?;
        if let Some(run) = run {
            builder.relation_statement(
                "invocation-run",
                recorder.clone(),
                RelationId::from("agent:toolInvocationInRun"),
                vec![Term::Occurrence(invocation_id.clone()), Term::Referent(run)],
                "",
            )?;
        }

        if let Some(sequence) = record.source.sequence {
            builder.relation_statement(
                "source-sequence",
                recorder.clone(),
                RelationId::from("harness:eventSequence"),
                vec![
                    Term::Occurrence(invocation_id.clone()),
                    Term::Literal(Literal::Integer(sequence.to_string())),
                ],
                "",
            )?;
        }

        for (name, value) in &record.arguments {
            builder.argument(
                &invocation_id,
                recorder.clone(),
                name,
                value,
                &format!("/arguments/{}", json_pointer_escape(name)),
            )?;
        }

        if let Some(retry_of) = &record.retry_of {
            let prior =
                builder.external_invocation(record.invocation_scope, retry_of, "retry-of")?;
            builder.relation_statement(
                "retry-of",
                recorder.clone(),
                RelationId::from("agent:toolInvocationRetryOf"),
                vec![
                    Term::Occurrence(invocation_id.clone()),
                    Term::Occurrence(prior),
                ],
                "/retry_of",
            )?;
        }
        if let Some(parent) = &record.parent_invocation {
            let parent = builder.external_invocation(record.invocation_scope, parent, "parent")?;
            builder.relation_statement(
                "parent-invocation",
                recorder.clone(),
                RelationId::from("agent:toolInvocationParent"),
                vec![
                    Term::Occurrence(invocation_id.clone()),
                    Term::Occurrence(parent),
                ],
                "/parent_invocation",
            )?;
        }

        if let Some(result) = &record.result {
            builder.result(&invocation_id, result)?;
        }
        if let Some(timestamp) = record.source.unix_millis {
            builder.temporal_statement(
                "invocation-time",
                recorder.clone(),
                &invocation_id,
                timestamp,
                "",
            )?;
        }
        for effect in &record.effects {
            builder.effect(&invocation_id, invoker.clone(), effect)?;
        }

        builder.document.validate()?;
        Ok(builder.document)
    }
}

struct DocumentBuilder {
    document: OccurrenceDocument,
    /// Record-local seed for statement/proposition/span and anonymous identities.
    seed: String,
    source: StructuredSource,
}

impl DocumentBuilder {
    fn new(
        ontology: RegistrySnapshot,
        adapter: String,
        adapter_version: String,
        seed: &str,
        source: &StructuredSource,
    ) -> Self {
        let id = OccurrenceDocumentId::from(stable_id("occurrence-document", &[seed]));
        Self {
            document: OccurrenceDocument {
                schema_version: SCHEMA_VERSION.into(),
                id,
                ontology,
                derivation: Derivation::DeterministicStructured {
                    adapter,
                    version: adapter_version,
                },
                source_spans: BTreeMap::new(),
                referents: BTreeMap::new(),
                occurrences: BTreeMap::new(),
                variables: BTreeMap::new(),
                propositions: BTreeMap::new(),
                statements: BTreeMap::new(),
                ambiguities: BTreeMap::new(),
                statement_order: Vec::new(),
            },
            seed: seed.to_owned(),
            source: source.clone(),
        }
    }

    fn id_referent(&self, role: &str, value: &str) -> ReferentId {
        ReferentId::from(stable_id("referent", &[&self.seed, role, value]))
    }

    fn identity_namespace_for(
        &self,
        scope: IdentityScope,
        source: &StructuredSource,
    ) -> Result<String, ToolFormalizationError> {
        Ok(match scope {
            IdentityScope::Source => format!("{}\u{1f}{}", self.adapter_name(), source.source),
            IdentityScope::Run => {
                let run = source.run.as_deref().ok_or(
                    ToolFormalizationError::MissingIdentityScopeContext {
                        scope: IdentityScope::Run,
                        field: "source.run",
                    },
                )?;
                format!("{}\u{1f}{}\u{1f}{run}", self.adapter_name(), source.source)
            }
            IdentityScope::Record => format!(
                "{}\u{1f}{}\u{1f}{}",
                self.adapter_name(),
                source.source,
                source.record
            ),
        })
    }

    fn identity_namespace(&self, scope: IdentityScope) -> Result<String, ToolFormalizationError> {
        self.identity_namespace_for(scope, &self.source)
    }

    fn adapter_name(&self) -> &str {
        match &self.document.derivation {
            Derivation::DeterministicStructured { adapter, .. } => adapter,
            _ => unreachable!("tool formalizer always builds deterministic structured documents"),
        }
    }

    fn id_external_referent_for(
        &self,
        scope: IdentityScope,
        role: &str,
        value: &str,
        source: &StructuredSource,
    ) -> Result<ReferentId, ToolFormalizationError> {
        let namespace = self.identity_namespace_for(scope, source)?;
        Ok(ReferentId::from(stable_id(
            "referent",
            &[&namespace, role, value],
        )))
    }

    fn id_external_referent(
        &self,
        scope: IdentityScope,
        role: &str,
        value: &str,
    ) -> Result<ReferentId, ToolFormalizationError> {
        self.id_external_referent_for(scope, role, value, &self.source)
    }

    fn id_occurrence(&self, role: &str, value: &str) -> OccurrenceId {
        OccurrenceId::from(stable_id("occurrence", &[&self.seed, role, value]))
    }

    fn id_external_occurrence(
        &self,
        scope: IdentityScope,
        role: &str,
        value: &str,
    ) -> Result<OccurrenceId, ToolFormalizationError> {
        let namespace = self.identity_namespace(scope)?;
        Ok(OccurrenceId::from(stable_id(
            "occurrence",
            &[&namespace, role, value],
        )))
    }

    fn id_proposition(&self, role: &str) -> PropositionId {
        PropositionId::from(stable_id("proposition", &[&self.seed, role]))
    }

    fn id_statement(&self, role: &str) -> StatementId {
        StatementId::from(stable_id("statement", &[&self.seed, role]))
    }

    fn span_id_for(&self, source: &StructuredSource, field_path: &str) -> SourceSpanId {
        SourceSpanId::from(stable_id(
            "source-span",
            &[
                self.adapter_name(),
                &source.source,
                source.run.as_deref().unwrap_or(""),
                source.turn.as_deref().unwrap_or(""),
                &source.record,
                field_path,
            ],
        ))
    }

    fn span_from(
        &mut self,
        source: &StructuredSource,
        field_path: &str,
    ) -> Result<SourceSpanId, ToolFormalizationError> {
        // RFC-6901 represents the document root with the empty pointer. It is
        // a real structured-source coordinate, not missing metadata.
        if !field_path.is_empty() {
            validate_text("source field path", field_path)?;
            if !field_path.starts_with('/') && !field_path.starts_with('@') {
                return Err(ToolFormalizationError::InvalidText {
                    kind: "source field path",
                    value: field_path.to_owned(),
                });
            }
        }
        let id = self.span_id_for(source, field_path);
        self.document
            .source_spans
            .entry(id.clone())
            .or_insert_with(|| SourceSpan {
                id: id.clone(),
                source: source.source.clone(),
                run: source.run.clone(),
                turn: source.turn.clone(),
                message: Some(source.record.clone()),
                block: None,
                bytes: None,
                field_path: Some(field_path.to_owned()),
            });
        Ok(id)
    }

    fn span(&mut self, field_path: &str) -> Result<SourceSpanId, ToolFormalizationError> {
        let source = self.source.clone();
        self.span_from(&source, field_path)
    }

    fn principal_from(
        &mut self,
        principal: &Principal,
        source: &StructuredSource,
        _role: &str,
    ) -> Result<ReferentId, ToolFormalizationError> {
        let id =
            self.id_external_referent_for(principal.scope, "principal", &principal.id, source)?;
        self.document
            .referents
            .entry(id.clone())
            .or_insert_with(|| Referent {
                id: id.clone(),
                types: BTreeSet::from([principal.sort.clone()]),
                lexical_anchor: None,
                labels: principal.label.iter().cloned().collect(),
                external_ids: BTreeMap::from([(
                    "structured_principal".into(),
                    principal.id.clone(),
                )]),
                discourse_roles: BTreeSet::from([principal.discourse_role]),
                source_spans: BTreeSet::new(),
            });
        Ok(id)
    }

    fn principal(
        &mut self,
        principal: &Principal,
        role: &str,
    ) -> Result<ReferentId, ToolFormalizationError> {
        let source = self.source.clone();
        self.principal_from(principal, &source, role)
    }

    fn tool(&mut self, tool: &ToolDescriptor) -> Result<ReferentId, ToolFormalizationError> {
        let id = self.id_external_referent(tool.scope, "tool", &tool.id)?;
        let mut external_ids = BTreeMap::from([("tool".into(), tool.id.clone())]);
        if let Some(specification) = &tool.specification_id {
            external_ids.insert("tool_specification".into(), specification.clone());
        }
        self.document.referents.insert(
            id.clone(),
            Referent {
                id: id.clone(),
                types: BTreeSet::from([tool.sort.clone()]),
                lexical_anchor: None,
                labels: BTreeSet::from([tool.name.clone()]),
                external_ids,
                discourse_roles: BTreeSet::from([DiscourseRole::Tool]),
                source_spans: BTreeSet::new(),
            },
        );
        Ok(id)
    }

    fn run_referent(
        &mut self,
        source: &StructuredSource,
    ) -> Result<Option<ReferentId>, ToolFormalizationError> {
        let Some(run) = &source.run else {
            return Ok(None);
        };
        let id = self.id_external_referent_for(IdentityScope::Run, "run", run, source)?;
        self.document.referents.insert(
            id.clone(),
            Referent {
                id: id.clone(),
                types: BTreeSet::from([ConceptId::from("agent:AgentRun")]),
                lexical_anchor: None,
                labels: BTreeSet::new(),
                external_ids: BTreeMap::from([("run".into(), run.clone())]),
                discourse_roles: BTreeSet::new(),
                source_spans: BTreeSet::new(),
            },
        );
        Ok(Some(id))
    }

    fn external_invocation(
        &mut self,
        scope: IdentityScope,
        external_id: &str,
        _role: &str,
    ) -> Result<OccurrenceId, ToolFormalizationError> {
        validate_text("external invocation id", external_id)?;
        let id = self.id_external_occurrence(scope, "tool-invocation", external_id)?;
        self.document
            .occurrences
            .entry(id.clone())
            .or_insert_with(|| Occurrence {
                id: id.clone(),
                types: BTreeSet::from([ConceptId::from("agent:ToolInvocation")]),
                lexical_anchor: None,
                participants: Vec::new(),
                attributes: BTreeMap::new(),
                tense_aspect: None,
                grammatical_tense: None,
                grammatical_voice: None,
                grammatical_spans: BTreeSet::new(),
                reported_outcome: None,
                source_spans: BTreeSet::new(),
                evidence: BTreeSet::new(),
            });
        Ok(id)
    }

    fn proposition_only(
        &mut self,
        role: &str,
        expression: PropositionExpr,
        field_path: &str,
    ) -> Result<PropositionId, ToolFormalizationError> {
        let span = self.span(field_path)?;
        let proposition_id = self.id_proposition(role);
        self.document.propositions.insert(
            proposition_id.clone(),
            Proposition {
                id: proposition_id.clone(),
                expression,
                operator_spans: BTreeSet::new(),
                operator_tense_aspect: None,
                operator_voice: None,
                operator_grammatical_spans: BTreeSet::new(),
                source_spans: BTreeSet::from([span]),
                evidence: BTreeSet::new(),
            },
        );
        Ok(proposition_id)
    }

    fn statement_for_proposition(
        &mut self,
        role: &str,
        presenter: Option<ReferentId>,
        mode: PresentationMode,
        proposition_id: PropositionId,
        field_path: &str,
    ) -> Result<(), ToolFormalizationError> {
        let span = self.span(field_path)?;
        let statement_id = self.id_statement(role);
        self.document.statements.insert(
            statement_id.clone(),
            Statement {
                id: statement_id.clone(),
                presenter,
                addressees: BTreeSet::new(),
                mode,
                basis: StatementBasis::Expressed,
                content: proposition_id,
                source_spans: BTreeSet::from([span]),
                evidence: BTreeSet::new(),
            },
        );
        self.document.statement_order.push(statement_id);
        Ok(())
    }

    fn proposition_statement(
        &mut self,
        role: &str,
        presenter: Option<ReferentId>,
        mode: PresentationMode,
        expression: PropositionExpr,
        field_path: &str,
    ) -> Result<PropositionId, ToolFormalizationError> {
        let proposition_id = self.proposition_only(role, expression, field_path)?;
        self.statement_for_proposition(role, presenter, mode, proposition_id.clone(), field_path)?;
        Ok(proposition_id)
    }

    fn statement_for_occurrence(
        &mut self,
        role: &str,
        presenter: Option<ReferentId>,
        mode: PresentationMode,
        occurrence: OccurrenceId,
        span: SourceSpanId,
    ) -> Result<(), ToolFormalizationError> {
        let proposition_id = self.id_proposition(role);
        self.document.propositions.insert(
            proposition_id.clone(),
            Proposition {
                id: proposition_id.clone(),
                expression: PropositionExpr::Occurrence { occurrence },
                operator_spans: BTreeSet::new(),
                operator_tense_aspect: None,
                operator_voice: None,
                operator_grammatical_spans: BTreeSet::new(),
                source_spans: BTreeSet::from([span.clone()]),
                evidence: BTreeSet::new(),
            },
        );
        let statement_id = self.id_statement(role);
        self.document.statements.insert(
            statement_id.clone(),
            Statement {
                id: statement_id.clone(),
                presenter,
                addressees: BTreeSet::new(),
                mode,
                basis: StatementBasis::Expressed,
                content: proposition_id,
                source_spans: BTreeSet::from([span]),
                evidence: BTreeSet::new(),
            },
        );
        self.document.statement_order.push(statement_id);
        Ok(())
    }

    fn relation_statement(
        &mut self,
        role: &str,
        presenter: Option<ReferentId>,
        relation: RelationId,
        arguments: Vec<Term>,
        field_path: &str,
    ) -> Result<(), ToolFormalizationError> {
        self.proposition_statement(
            role,
            presenter,
            PresentationMode::Assertion,
            PropositionExpr::Relation {
                relation,
                arguments,
            },
            field_path,
        )?;
        Ok(())
    }

    fn argument(
        &mut self,
        invocation: &OccurrenceId,
        presenter: Option<ReferentId>,
        name: &str,
        value: &StructuredValue,
        field_path: &str,
    ) -> Result<(), ToolFormalizationError> {
        let argument = self.id_referent("argument", name);
        let span = self.span(field_path)?;
        self.document.referents.insert(
            argument.clone(),
            Referent {
                id: argument.clone(),
                types: BTreeSet::from([ConceptId::from("comp:Argument")]),
                lexical_anchor: None,
                labels: BTreeSet::new(),
                external_ids: BTreeMap::from([("argument_name".into(), name.to_owned())]),
                discourse_roles: BTreeSet::new(),
                source_spans: BTreeSet::from([span]),
            },
        );
        let token = stable_token(name);
        self.relation_statement(
            &format!("argument-link-{token}"),
            presenter.clone(),
            RelationId::from("agent:toolInvocationHasArgument"),
            vec![
                Term::Occurrence(invocation.clone()),
                Term::Referent(argument.clone()),
            ],
            field_path,
        )?;
        self.relation_statement(
            &format!("argument-name-{token}"),
            presenter.clone(),
            RelationId::from("comp:argumentName"),
            vec![
                Term::Referent(argument.clone()),
                Term::Literal(Literal::String(name.to_owned())),
            ],
            field_path,
        )?;
        self.relation_statement(
            &format!("argument-value-{token}"),
            presenter,
            RelationId::from("comp:argumentValue"),
            vec![Term::Referent(argument), Term::Literal(value.to_literal()?)],
            field_path,
        )?;
        Ok(())
    }

    fn result(
        &mut self,
        invocation: &OccurrenceId,
        result: &ToolResultRecord,
    ) -> Result<(), ToolFormalizationError> {
        let source = result
            .source_record
            .as_ref()
            .unwrap_or(&self.source)
            .clone();
        let previous_source = std::mem::replace(&mut self.source, source.clone());
        let outcome = (|| {
            let presenter = match &source.recorder {
                Some(principal) => {
                    Some(self.principal_from(principal, &source, "result-recorder")?)
                }
                None => None,
            };
            let result_ref = self.id_referent("tool-result", &result.status_source_field);
            let span = self.span(&result.status_source_field)?;
            self.document.referents.insert(
                result_ref.clone(),
                Referent {
                    id: result_ref.clone(),
                    types: BTreeSet::from([ConceptId::from("agent:ToolResult")]),
                    lexical_anchor: None,
                    labels: BTreeSet::new(),
                    external_ids: BTreeMap::new(),
                    discourse_roles: BTreeSet::new(),
                    source_spans: BTreeSet::from([span]),
                },
            );
            self.relation_statement(
                "result-link",
                presenter.clone(),
                RelationId::from("agent:toolInvocationResult"),
                vec![
                    Term::Occurrence(invocation.clone()),
                    Term::Referent(result_ref.clone()),
                ],
                &result.status_source_field,
            )?;
            let outcome_ref =
                self.id_referent("tool-result-outcome", &format!("{:?}", result.status));
            let outcome_span = self.span(&result.status_source_field)?;
            self.document.referents.insert(
                outcome_ref.clone(),
                Referent {
                    id: outcome_ref.clone(),
                    types: BTreeSet::from([outcome_sort(result.status)]),
                    lexical_anchor: None,
                    labels: result.detail.iter().cloned().collect(),
                    external_ids: BTreeMap::new(),
                    discourse_roles: BTreeSet::new(),
                    source_spans: BTreeSet::from([outcome_span]),
                },
            );
            self.relation_statement(
                "result-outcome",
                presenter.clone(),
                RelationId::from("agent:toolResultOutcome"),
                vec![
                    Term::Referent(result_ref.clone()),
                    Term::Referent(outcome_ref),
                ],
                &result.status_source_field,
            )?;
            if let Some(duration_ms) = result.duration_ms {
                let field = result
                    .duration_source_field
                    .as_deref()
                    .ok_or(ToolFormalizationError::InconsistentResultSourceFields)?;
                self.relation_statement(
                    "invocation-duration",
                    presenter.clone(),
                    RelationId::from("agent:toolInvocationDurationMillis"),
                    vec![
                        Term::Occurrence(invocation.clone()),
                        Term::Literal(Literal::Integer(duration_ms.to_string())),
                    ],
                    field,
                )?;
            }
            for (payload_index, payload) in result.payloads.iter().enumerate() {
                let payload_source = payload.source_record.as_ref().unwrap_or(&source).clone();
                let previous_payload_source =
                    std::mem::replace(&mut self.source, payload_source.clone());
                let payload_presenter = match &payload_source.recorder {
                    Some(principal) => Some(self.principal_from(
                        principal,
                        &payload_source,
                        "result-payload-recorder",
                    )?),
                    None => presenter.clone(),
                };
                let payload_result = self.relation_statement(
                    &format!("result-payload-{payload_index}"),
                    payload_presenter,
                    RelationId::from("agent:toolResultPayload"),
                    vec![
                        Term::Referent(result_ref.clone()),
                        Term::Literal(payload.value.to_literal()?),
                    ],
                    &payload.source_field,
                );
                self.source = previous_payload_source;
                payload_result?;
            }
            let execution_field = result.execution_source_field.as_deref();
            if let Some(code) = result.execution.exit_code {
                let field = execution_field
                    .ok_or(ToolFormalizationError::InconsistentResultSourceFields)?;
                let exit = self.id_referent("exit-status", &code.to_string());
                let exit_span = self.span(field)?;
                self.document.referents.insert(
                    exit.clone(),
                    Referent {
                        id: exit.clone(),
                        types: BTreeSet::from([ConceptId::from("comp:ExitStatus")]),
                        lexical_anchor: None,
                        labels: BTreeSet::new(),
                        external_ids: BTreeMap::new(),
                        discourse_roles: BTreeSet::new(),
                        source_spans: BTreeSet::from([exit_span]),
                    },
                );
                self.relation_statement(
                    "exit-status",
                    presenter.clone(),
                    RelationId::from("comp:hasExitStatus"),
                    vec![
                        Term::Occurrence(invocation.clone()),
                        Term::Referent(exit.clone()),
                    ],
                    field,
                )?;
                self.relation_statement(
                    "exit-code",
                    presenter.clone(),
                    RelationId::from("comp:exitCodeValue"),
                    vec![
                        Term::Referent(exit),
                        Term::Literal(Literal::Integer(code.to_string())),
                    ],
                    field,
                )?;
            }
            if let Some(stdout) = &result.execution.stdout {
                let field = execution_field
                    .ok_or(ToolFormalizationError::InconsistentResultSourceFields)?;
                self.stream(
                    invocation,
                    presenter.clone(),
                    "stdout",
                    "comp:StandardOutput",
                    "comp:producesStandardOutput",
                    stdout,
                    field,
                )?;
            }
            if let Some(stderr) = &result.execution.stderr {
                let field = execution_field
                    .ok_or(ToolFormalizationError::InconsistentResultSourceFields)?;
                self.stream(
                    invocation,
                    presenter.clone(),
                    "stderr",
                    "comp:StandardError",
                    "comp:producesStandardError",
                    stderr,
                    field,
                )?;
            }
            for (index, diagnostic) in result.execution.diagnostics.iter().enumerate() {
                let field = execution_field
                    .ok_or(ToolFormalizationError::InconsistentResultSourceFields)?;
                let diagnostic_ref = self.id_referent("diagnostic", &index.to_string());
                let diagnostic_span = self.span(field)?;
                self.document.referents.insert(
                    diagnostic_ref.clone(),
                    Referent {
                        id: diagnostic_ref.clone(),
                        types: BTreeSet::from([ConceptId::from("se:Diagnostic")]),
                        lexical_anchor: None,
                        labels: BTreeSet::from([diagnostic.clone()]),
                        external_ids: BTreeMap::new(),
                        discourse_roles: BTreeSet::new(),
                        source_spans: BTreeSet::from([diagnostic_span]),
                    },
                );
                self.relation_statement(
                    &format!("result-diagnostic-{index}"),
                    presenter.clone(),
                    RelationId::from("agent:toolResultDiagnostic"),
                    vec![
                        Term::Referent(result_ref.clone()),
                        Term::Referent(diagnostic_ref),
                    ],
                    field,
                )?;
            }
            Ok(())
        })();
        self.source = previous_source;
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    fn stream(
        &mut self,
        invocation: &OccurrenceId,
        presenter: Option<ReferentId>,
        role: &str,
        stream_type: &str,
        relation: &str,
        content: &str,
        field_path: &str,
    ) -> Result<(), ToolFormalizationError> {
        let stream = self.id_referent(role, field_path);
        self.document.referents.insert(
            stream.clone(),
            Referent {
                id: stream.clone(),
                types: BTreeSet::from([ConceptId::from(stream_type)]),
                lexical_anchor: None,
                labels: BTreeSet::new(),
                external_ids: BTreeMap::new(),
                discourse_roles: BTreeSet::new(),
                source_spans: BTreeSet::new(),
            },
        );
        self.relation_statement(
            &format!("{role}-link"),
            presenter.clone(),
            RelationId::from(relation),
            vec![
                Term::Occurrence(invocation.clone()),
                Term::Referent(stream.clone()),
            ],
            field_path,
        )?;
        self.relation_statement(
            &format!("{role}-content"),
            presenter,
            RelationId::from("comp:streamContent"),
            vec![
                Term::Referent(stream),
                Term::Literal(Literal::String(content.to_owned())),
            ],
            field_path,
        )?;
        Ok(())
    }

    fn temporal_statement(
        &mut self,
        role: &str,
        presenter: Option<ReferentId>,
        occurrence: &OccurrenceId,
        unix_millis: i64,
        field_path: &str,
    ) -> Result<(), ToolFormalizationError> {
        self.proposition_statement(
            role,
            presenter,
            PresentationMode::Assertion,
            PropositionExpr::Temporal {
                subject: muse_occurrence::SemanticTarget::Occurrence(occurrence.clone()),
                relation: TemporalRelation::At,
                object: TemporalAnchor::UnixMillis { value: unix_millis },
            },
            field_path,
        )?;
        Ok(())
    }

    fn effect(
        &mut self,
        invocation: &OccurrenceId,
        _invoker: ReferentId,
        effect: &ToolEffect,
    ) -> Result<(), ToolFormalizationError> {
        let source = effect
            .source_record
            .as_ref()
            .unwrap_or(&self.source)
            .clone();
        let previous_source = std::mem::replace(&mut self.source, source.clone());
        let outcome = (|| {
            let recorder = match &source.recorder {
                Some(principal) => {
                    Some(self.principal_from(principal, &source, "effect-recorder")?)
                }
                None => None,
            };
            let id = self.id_occurrence("effect", &effect.id);
            let span = self.span(&effect.source_field)?;
            self.document.occurrences.insert(
                id.clone(),
                Occurrence {
                    id: id.clone(),
                    types: BTreeSet::from([effect.sort.clone()]),
                    lexical_anchor: None,
                    participants: Vec::new(),
                    attributes: BTreeMap::new(),
                    tense_aspect: None,
                    grammatical_tense: None,
                    grammatical_voice: None,
                    grammatical_spans: BTreeSet::new(),
                    reported_outcome: None,
                    source_spans: BTreeSet::from([span]),
                    evidence: BTreeSet::new(),
                },
            );

            let mut members = BTreeSet::new();
            members.insert(self.proposition_only(
                &format!("effect-occurrence-{}", stable_token(&effect.id)),
                PropositionExpr::Occurrence {
                    occurrence: id.clone(),
                },
                &effect.source_field,
            )?);

            let mut links: Vec<(RelationId, Term)> = Vec::new();
            match effect.sort.as_str() {
                "comp:FileRead" => self.filesystem_endpoint(
                    &mut links,
                    effect.target.as_ref(),
                    "comp:readsObject",
                    "comp:readPath",
                    "target",
                )?,
                "comp:FileWrite" => self.filesystem_endpoint(
                    &mut links,
                    effect.target.as_ref(),
                    "comp:writesObject",
                    "comp:writePath",
                    "target",
                )?,
                "comp:FileCreate" => self.filesystem_endpoint(
                    &mut links,
                    effect.target.as_ref(),
                    "comp:createsObject",
                    "comp:createPath",
                    "target",
                )?,
                "comp:FileDelete" => self.filesystem_endpoint(
                    &mut links,
                    effect.target.as_ref(),
                    "comp:deletesObject",
                    "comp:deletePath",
                    "target",
                )?,
                "comp:FileMove" => {
                    self.filesystem_endpoint(
                        &mut links,
                        effect.source.as_ref(),
                        "comp:moveSource",
                        "comp:moveSourcePath",
                        "source",
                    )?;
                    self.filesystem_endpoint(
                        &mut links,
                        effect.destination.as_ref(),
                        "comp:moveDestinationObject",
                        "comp:moveDestinationPath",
                        "destination",
                    )?;
                }
                "comp:FileCopy" => {
                    self.filesystem_endpoint(
                        &mut links,
                        effect.source.as_ref(),
                        "comp:copySource",
                        "comp:copySourcePath",
                        "source",
                    )?;
                    self.filesystem_endpoint(
                        &mut links,
                        effect.destination.as_ref(),
                        "comp:copyDestinationObject",
                        "comp:copyDestinationPath",
                        "destination",
                    )?;
                }
                _ => {}
            }
            if matches!(
                effect.sort.as_str(),
                "harness:PatchApplication"
                    | "harness:BuildExecution"
                    | "harness:CompilationExecution"
                    | "harness:TestExecution"
            ) {
                for artifact in &effect.inputs {
                    if let Some(target) = self.artifact(Some(artifact), "input")? {
                        links.push((RelationId::from("se:usesArtifact"), Term::Referent(target)));
                    }
                }
                for artifact in &effect.outputs {
                    if let Some(target) = self.artifact(Some(artifact), "output")? {
                        links.push((
                            RelationId::from("se:createsArtifact"),
                            Term::Referent(target),
                        ));
                    }
                }
            }
            for state in &effect.states {
                let (state_id, state_semantics) = self.artifact_state(state)?;
                links.push((state.relation.clone(), Term::Occurrence(state_id.clone())));
                if state.evidence == effect.evidence {
                    members.insert(state_semantics);
                } else {
                    // Validation permits only an independently observed state inside
                    // requested effect content. A requested state inside an observed
                    // effect would otherwise be asserted by the observed conjunction.
                    debug_assert_eq!(effect.evidence, EffectEvidence::Requested);
                    debug_assert_eq!(state.evidence, EffectEvidence::Observed);
                    self.statement_for_proposition(
                        &format!("artifact-state-{}", stable_token(&state.id)),
                        recorder.clone(),
                        PresentationMode::Assertion,
                        state_semantics,
                        &state.source_field,
                    )?;
                }
            }
            if let Some(command) = &effect.command {
                let command_ref = self.id_referent("command", &format!("{}:{command}", effect.id));
                self.document.referents.insert(
                    command_ref.clone(),
                    Referent {
                        id: command_ref.clone(),
                        types: BTreeSet::from([ConceptId::from("comp:Command")]),
                        lexical_anchor: None,
                        labels: BTreeSet::from([command.clone()]),
                        external_ids: BTreeMap::new(),
                        discourse_roles: BTreeSet::new(),
                        source_spans: BTreeSet::new(),
                    },
                );
                links.push((
                    RelationId::from("comp:invokesCommand"),
                    Term::Referent(command_ref),
                ));
            }
            for (ordinal, (relation, value)) in links.into_iter().enumerate() {
                members.insert(self.proposition_only(
                    &format!("effect-relation-{}-{ordinal}", stable_token(&effect.id)),
                    PropositionExpr::Relation {
                        relation,
                        arguments: vec![Term::Occurrence(id.clone()), value],
                    },
                    &effect.source_field,
                )?);
            }
            let effect_content = if members.len() == 1 {
                members
                    .into_iter()
                    .next()
                    .ok_or(ToolFormalizationError::InternalInvariant(
                        "effect proposition missing",
                    ))?
            } else {
                self.proposition_only(
                    &format!("effect-content-{}", stable_token(&effect.id)),
                    PropositionExpr::Conjunction { members },
                    &effect.source_field,
                )?
            };

            match effect.evidence {
                EffectEvidence::Requested => {
                    // The effect proposition is intentionally NOT rooted. The only
                    // asserted fact is that this invocation requests that content.
                    self.relation_statement(
                        &format!("requested-effect-{}", stable_token(&effect.id)),
                        recorder,
                        RelationId::from("agent:toolInvocationRequestedEffect"),
                        vec![
                            Term::Occurrence(invocation.clone()),
                            Term::Proposition(effect_content),
                        ],
                        &effect.source_field,
                    )?;
                }
                EffectEvidence::Observed => {
                    let observed_link = self.proposition_only(
                        &format!("observed-effect-link-{}", stable_token(&effect.id)),
                        PropositionExpr::Relation {
                            relation: RelationId::from("agent:toolInvocationObservedEffect"),
                            arguments: vec![
                                Term::Occurrence(invocation.clone()),
                                Term::Occurrence(id),
                            ],
                        },
                        &effect.source_field,
                    )?;
                    let combined = self.proposition_only(
                        &format!("observed-effect-content-{}", stable_token(&effect.id)),
                        PropositionExpr::Conjunction {
                            members: BTreeSet::from([observed_link, effect_content]),
                        },
                        &effect.source_field,
                    )?;
                    self.statement_for_proposition(
                        &format!("observed-effect-{}", stable_token(&effect.id)),
                        recorder,
                        PresentationMode::Assertion,
                        combined,
                        &effect.source_field,
                    )?;
                }
            }
            Ok(())
        })();
        self.source = previous_source;
        outcome
    }

    fn artifact_state(
        &mut self,
        state: &ArtifactStateDescriptor,
    ) -> Result<(OccurrenceId, PropositionId), ToolFormalizationError> {
        let artifact = self
            .artifact(Some(&state.artifact), "state")?
            .ok_or_else(|| ToolFormalizationError::MissingEffectField {
                effect: state.id.clone(),
                field: "artifact",
            })?;
        let id = self.id_occurrence("artifact-state", &state.id);
        let span = self.span(&state.source_field)?;
        self.document.occurrences.insert(
            id.clone(),
            Occurrence {
                id: id.clone(),
                types: BTreeSet::from([state.sort.clone()]),
                lexical_anchor: None,
                participants: Vec::new(),
                attributes: BTreeMap::new(),
                tense_aspect: None,
                grammatical_tense: None,
                grammatical_voice: None,
                grammatical_spans: BTreeSet::new(),
                reported_outcome: None,
                source_spans: BTreeSet::from([span]),
                evidence: BTreeSet::new(),
            },
        );
        let mut members = BTreeSet::from([
            self.proposition_only(
                &format!("artifact-state-occurrence-{}", stable_token(&state.id)),
                PropositionExpr::Occurrence {
                    occurrence: id.clone(),
                },
                &state.source_field,
            )?,
            self.proposition_only(
                &format!("artifact-state-artifact-{}", stable_token(&state.id)),
                PropositionExpr::Relation {
                    relation: RelationId::from("comp:stateOfArtifact"),
                    arguments: vec![Term::Occurrence(id.clone()), Term::Referent(artifact)],
                },
                &state.source_field,
            )?,
        ]);
        if let Some(digest) = &state.digest {
            members.insert(self.proposition_only(
                &format!("artifact-state-digest-{}", stable_token(&state.id)),
                PropositionExpr::Relation {
                    relation: RelationId::from("comp:stateDigest"),
                    arguments: vec![
                        Term::Occurrence(id.clone()),
                        Term::Literal(Literal::String(digest.clone())),
                    ],
                },
                &state.source_field,
            )?);
        }
        let semantics = self.proposition_only(
            &format!("artifact-state-content-{}", stable_token(&state.id)),
            PropositionExpr::Conjunction { members },
            &state.source_field,
        )?;
        Ok((id, semantics))
    }

    fn filesystem_endpoint(
        &mut self,
        links: &mut Vec<(RelationId, Term)>,
        descriptor: Option<&ArtifactDescriptor>,
        object_relation: &str,
        path_relation: &str,
        role: &str,
    ) -> Result<(), ToolFormalizationError> {
        let Some(descriptor) = descriptor else {
            return Ok(());
        };
        let referent = self.artifact(Some(descriptor), role)?.ok_or(
            ToolFormalizationError::InternalInvariant(
                "present artifact descriptor did not produce a referent",
            ),
        )?;
        if descriptor.sort.as_str() == "comp:Path" {
            links.push((RelationId::from(path_relation), Term::Referent(referent)));
        } else {
            links.push((RelationId::from(object_relation), Term::Referent(referent)));
            if let Some(locator) = &descriptor.locator {
                let path =
                    self.id_referent("filesystem-locator-path", &format!("{role}:{locator}"));
                self.document
                    .referents
                    .entry(path.clone())
                    .or_insert_with(|| Referent {
                        id: path.clone(),
                        types: BTreeSet::from([ConceptId::from("comp:Path")]),
                        lexical_anchor: None,
                        labels: BTreeSet::from([locator.clone()]),
                        external_ids: BTreeMap::from([("path_text".into(), locator.clone())]),
                        discourse_roles: BTreeSet::new(),
                        source_spans: BTreeSet::new(),
                    });
                links.push((RelationId::from(path_relation), Term::Referent(path)));
            }
        }
        Ok(())
    }

    fn artifact(
        &mut self,
        artifact: Option<&ArtifactDescriptor>,
        _role: &str,
    ) -> Result<Option<ReferentId>, ToolFormalizationError> {
        let Some(artifact) = artifact else {
            return Ok(None);
        };
        validate_artifact(artifact)?;
        let id = self.id_external_referent(artifact.scope, "artifact", &artifact.id)?;
        let types = BTreeSet::from([artifact.sort.clone()]);
        let mut external_ids = BTreeMap::from([("artifact".into(), artifact.id.clone())]);
        if let Some(locator) = &artifact.locator {
            external_ids.insert("locator".into(), locator.clone());
        }
        self.document.referents.insert(
            id.clone(),
            Referent {
                id: id.clone(),
                types,
                lexical_anchor: None,
                labels: artifact.locator.iter().cloned().collect(),
                external_ids,
                discourse_roles: BTreeSet::new(),
                source_spans: BTreeSet::new(),
            },
        );
        Ok(Some(id))
    }
}

fn outcome_sort(status: ReportedOutcomeStatus) -> ConceptId {
    ConceptId::from(match status {
        ReportedOutcomeStatus::Success => "comp:SuccessOutcome",
        ReportedOutcomeStatus::Failure => "comp:FailureOutcome",
        ReportedOutcomeStatus::Partial => "comp:PartialOutcome",
        ReportedOutcomeStatus::Skipped => "comp:SkippedOutcome",
        ReportedOutcomeStatus::Denied => "comp:DeniedOutcome",
        ReportedOutcomeStatus::Cancelled => "comp:CancelledOutcome",
        ReportedOutcomeStatus::Interrupted => "comp:InterruptedOutcome",
        ReportedOutcomeStatus::Unknown => "comp:UnknownOutcome",
    })
}

fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut bytes = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            bytes.push(0x1f);
        }
        bytes.extend_from_slice(part.as_bytes());
    }
    let digest = ContentDigest::sha256_bytes(&bytes);
    format!("{prefix}:sha256:{}", digest.value)
}

fn stable_token(value: &str) -> String {
    ContentDigest::sha256_bytes(value.as_bytes()).value
}

fn json_pointer_escape(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn validate_identity_scope(
    scope: IdentityScope,
    source: &StructuredSource,
) -> Result<(), ToolFormalizationError> {
    if scope == IdentityScope::Run && source.run.is_none() {
        return Err(ToolFormalizationError::MissingIdentityScopeContext {
            scope,
            field: "source.run",
        });
    }
    Ok(())
}

fn validate_structured_value(value: &StructuredValue) -> Result<(), ToolFormalizationError> {
    match value {
        StructuredValue::Integer(lexeme) => {
            let value: serde_json::Value = serde_json::from_str(lexeme)?;
            match value {
                serde_json::Value::Number(number) if number.is_i64() || number.is_u64() => Ok(()),
                _ => Err(ToolFormalizationError::MalformedNumericValue(
                    lexeme.clone(),
                )),
            }
        }
        StructuredValue::Decimal(lexeme) => {
            let value: serde_json::Value = serde_json::from_str(lexeme)?;
            match value {
                serde_json::Value::Number(number) if !number.is_i64() && !number.is_u64() => Ok(()),
                _ => Err(ToolFormalizationError::MalformedNumericValue(
                    lexeme.clone(),
                )),
            }
        }
        StructuredValue::Array(values) => {
            for value in values {
                validate_structured_value(value)?;
            }
            Ok(())
        }
        StructuredValue::Object(values) => {
            for value in values.values() {
                validate_structured_value(value)?;
            }
            Ok(())
        }
        StructuredValue::Null | StructuredValue::Boolean(_) | StructuredValue::String(_) => Ok(()),
    }
}

fn validate_structured_source(source: &StructuredSource) -> Result<(), ToolFormalizationError> {
    validate_text("source", &source.source)?;
    validate_text("record", &source.record)?;
    if let Some(run) = &source.run {
        validate_text("source run", run)?;
    }
    if let Some(turn) = &source.turn {
        validate_text("source turn", turn)?;
    }
    if let Some(recorder) = &source.recorder {
        validate_principal(recorder)?;
        validate_identity_scope(recorder.scope, source)?;
    }
    Ok(())
}

fn ensure_same_source_namespace(
    invocation: &StructuredSource,
    evidence: &StructuredSource,
) -> Result<(), ToolFormalizationError> {
    if invocation.source != evidence.source {
        return Err(ToolFormalizationError::CrossSourceEvidence {
            invocation_source: invocation.source.clone(),
            evidence_source: evidence.source.clone(),
        });
    }
    Ok(())
}

fn validate_principal(principal: &Principal) -> Result<(), ToolFormalizationError> {
    validate_text("principal id", &principal.id)?;
    if let Some(label) = &principal.label {
        validate_text("principal label", label)?;
    }
    principal.sort.validate()?;
    Ok(())
}

fn register_artifact_descriptor(
    seen: &mut BTreeMap<(String, String), ArtifactDescriptor>,
    artifact: &ArtifactDescriptor,
    source: &StructuredSource,
) -> Result<(), ToolFormalizationError> {
    let namespace = match artifact.scope {
        IdentityScope::Source => source.source.clone(),
        IdentityScope::Run => format!(
            "{}\u{1f}{}",
            source.source,
            source
                .run
                .as_deref()
                .ok_or(ToolFormalizationError::MissingIdentityScopeContext {
                    scope: IdentityScope::Run,
                    field: "source.run",
                },)?
        ),
        IdentityScope::Record => format!("{}\u{1f}{}", source.source, source.record),
    };
    let key = (namespace, artifact.id.clone());
    if let Some(previous) = seen.get(&key) {
        if previous != artifact {
            return Err(ToolFormalizationError::InconsistentArtifact(
                artifact.id.clone(),
            ));
        }
    } else {
        seen.insert(key, artifact.clone());
    }
    Ok(())
}

fn validate_artifact(artifact: &ArtifactDescriptor) -> Result<(), ToolFormalizationError> {
    validate_text("artifact id", &artifact.id)?;
    artifact.sort.validate()?;
    if let Some(locator) = &artifact.locator {
        validate_text("artifact locator", locator)?;
    }
    Ok(())
}

fn validate_text(kind: &'static str, value: &str) -> Result<(), ToolFormalizationError> {
    if value.is_empty() {
        return Err(ToolFormalizationError::InvalidText {
            kind,
            value: value.to_owned(),
        });
    }
    if value.trim() != value || value.chars().any(char::is_control) {
        return Err(ToolFormalizationError::InvalidText {
            kind,
            value: value.to_owned(),
        });
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ToolFormalizationError {
    #[error(
        "cannot attach evidence from source {evidence_source:?} to invocation source {invocation_source:?}"
    )]
    CrossSourceEvidence {
        invocation_source: String,
        evidence_source: String,
    },
    #[error("unsupported structured-tool schema {0}")]
    UnsupportedSchema(String),
    #[error("invalid {kind}: {value:?}")]
    InvalidText { kind: &'static str, value: String },
    #[error("same principal id has inconsistent descriptors: {0}")]
    InconsistentPrincipal(String),
    #[error("identity scope {scope:?} requires {field}")]
    MissingIdentityScopeContext {
        scope: IdentityScope,
        field: &'static str,
    },
    #[error("same artifact id has inconsistent descriptors: {0}")]
    InconsistentArtifact(String),
    #[error("malformed or misclassified structured numeric value {0:?}")]
    MalformedNumericValue(String),
    #[error("duplicate tool effect id {0}")]
    DuplicateEffect(String),
    #[error("duplicate artifact-state id {0}")]
    DuplicateEffectState(String),
    #[error("result values and source-field coordinates are inconsistent")]
    InconsistentResultSourceFields,
    #[error("effect/state {effect} is missing required field {field}")]
    MissingEffectField { effect: String, field: &'static str },
    #[error(
        "observed effect {effect} contains merely requested state {state}; split these into separate effects"
    )]
    InconsistentEffectStateEvidence { effect: String, state: String },
    #[error("internal deterministic tool-formalizer invariant failed: {0}")]
    InternalInvariant(&'static str),
    #[error(transparent)]
    Identity(#[from] muse_core::IdentityError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Occurrence(#[from] muse_occurrence::OccurrenceValidationError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use muse_core::canonical_digest;

    fn snapshot() -> RegistrySnapshot {
        let packages = BTreeSet::new();
        RegistrySnapshot {
            identity: canonical_digest(&packages).unwrap(),
            packages,
        }
    }

    fn fixture_record() -> ToolInvocationRecord {
        ToolInvocationRecord {
            schema_version: TOOL_SCHEMA_VERSION.into(),
            source: StructuredSource {
                source: "fixture".into(),
                run: Some("run-1".into()),
                turn: Some("turn-1".into()),
                record: "record-1".into(),
                sequence: Some(1),
                unix_millis: Some(1_700_000_000_000),
                recorder: Some(Principal {
                    id: "harness".into(),
                    scope: IdentityScope::Source,
                    sort: ConceptId::from("harness:CodingHarness"),
                    discourse_role: DiscourseRole::Harness,
                    label: None,
                }),
            },
            invocation_id: "call-1".into(),
            invocation_scope: IdentityScope::Run,
            invoker: Principal {
                id: "agent-1".into(),
                scope: IdentityScope::Source,
                sort: ConceptId::from("agent:SoftwareAgent"),
                discourse_role: DiscourseRole::Agent,
                label: None,
            },
            tool: ToolDescriptor {
                id: "write".into(),
                scope: IdentityScope::Source,
                name: "write".into(),
                specification_id: None,
                sort: ConceptId::from("agent:Tool"),
            },
            invocation_sort: ConceptId::from("harness:FilesystemToolInvocation"),
            arguments: BTreeMap::from([(
                "path".into(),
                StructuredValue::String("src/lib.rs".into()),
            )]),
            result: Some(ToolResultRecord {
                source_record: None,
                status: ReportedOutcomeStatus::Success,
                detail: None,
                payloads: vec![ToolResultPayloadFragment {
                    source_record: None,
                    value: StructuredValue::String("ok".into()),
                    source_field: "/result".into(),
                }],
                execution: ExecutionDetails::default(),
                status_source_field: "/outcome".into(),
                execution_source_field: None,
                duration_ms: Some(4),
                duration_source_field: Some("/duration_ms".into()),
            }),
            effects: vec![ToolEffect {
                id: "requested-write".into(),
                source_record: None,
                evidence: EffectEvidence::Requested,
                sort: ConceptId::from("comp:FileWrite"),
                target: Some(ArtifactDescriptor {
                    id: "src/lib.rs".into(),
                    scope: IdentityScope::Run,
                    sort: ConceptId::from("comp:Path"),
                    locator: Some("src/lib.rs".into()),
                }),
                source: None,
                destination: None,
                inputs: Vec::new(),
                outputs: Vec::new(),
                states: Vec::new(),
                command: None,
                source_field: "/arguments/path".into(),
            }],
            retry_of: None,
            parent_invocation: None,
        }
    }

    fn evidence_source(record: &ToolInvocationRecord, record_id: &str) -> StructuredSource {
        StructuredSource {
            source: record.source.source.clone(),
            run: record.source.run.clone(),
            turn: record.source.turn.clone(),
            record: record_id.into(),
            sequence: None,
            unix_millis: None,
            recorder: record.source.recorder.clone(),
        }
    }

    #[test]
    fn success_does_not_create_an_observed_effect() {
        let record = fixture_record();
        let document = ToolFormalizer {
            adapter: "fixture".into(),
            adapter_version: "1".into(),
            ontology: snapshot(),
        }
        .formalize(&record)
        .unwrap();

        let mut requested = 0usize;
        let mut observed = 0usize;
        for proposition in document.propositions.values() {
            if let PropositionExpr::Relation { relation, .. } = &proposition.expression {
                match relation.as_str() {
                    "agent:toolInvocationRequestedEffect" => requested += 1,
                    "agent:toolInvocationObservedEffect" => observed += 1,
                    _ => {}
                }
            }
        }
        assert_eq!(requested, 1);
        assert_eq!(observed, 0);
        assert!(
            document
                .statements
                .values()
                .all(|statement| statement.mode == PresentationMode::Assertion)
        );
    }

    #[test]
    fn observed_effect_rejects_merely_requested_nested_state() {
        let mut record = fixture_record();
        let state_artifact = ArtifactDescriptor {
            id: "src/lib.rs".into(),
            scope: IdentityScope::Run,
            sort: ConceptId::from("comp:Path"),
            locator: Some("src/lib.rs".into()),
        };
        record.effects = vec![ToolEffect {
            id: "observed-write".into(),
            source_record: None,
            evidence: EffectEvidence::Observed,
            sort: ConceptId::from("comp:FileWrite"),
            target: Some(state_artifact.clone()),
            source: None,
            destination: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            states: vec![ArtifactStateDescriptor {
                id: "requested-after".into(),
                evidence: EffectEvidence::Requested,
                relation: RelationId::from("comp:afterArtifactState"),
                artifact: state_artifact,
                digest: None,
                sort: ConceptId::from("comp:FileState"),
                source_field: "/arguments/path".into(),
            }],
            command: None,
            source_field: "/arguments/path".into(),
        }];

        assert!(matches!(
            record.validate(),
            Err(ToolFormalizationError::InconsistentEffectStateEvidence { .. })
        ));
    }

    #[test]
    fn source_scoped_identity_survives_run_boundaries_but_run_scoped_identity_does_not() {
        let formalizer = ToolFormalizer {
            adapter: "fixture".into(),
            adapter_version: "1".into(),
            ontology: snapshot(),
        };
        let first = fixture_record();
        let mut second = fixture_record();
        second.source.run = Some("run-2".into());
        second.source.record = "record-2".into();
        second.invocation_id = "call-2".into();

        let first_doc = formalizer.formalize(&first).unwrap();
        let second_doc = formalizer.formalize(&second).unwrap();
        let referent_id = |document: &OccurrenceDocument, key: &str, value: &str| {
            document
                .referents
                .values()
                .find(|referent| referent.external_ids.get(key).map(String::as_str) == Some(value))
                .map(|referent| referent.id.clone())
                .unwrap()
        };

        assert_eq!(
            referent_id(&first_doc, "structured_principal", "agent-1"),
            referent_id(&second_doc, "structured_principal", "agent-1")
        );
        assert_ne!(
            referent_id(&first_doc, "artifact", "src/lib.rs"),
            referent_id(&second_doc, "artifact", "src/lib.rs")
        );
    }

    #[test]
    fn run_scope_requires_a_run_context() {
        let mut record = fixture_record();
        record.source.run = None;
        assert!(matches!(
            record.validate(),
            Err(ToolFormalizationError::MissingIdentityScopeContext {
                scope: IdentityScope::Run,
                field: "source.run"
            })
        ));
    }

    #[test]
    fn equal_external_artifact_ids_in_distinct_scopes_are_distinct_identities() {
        let mut record = fixture_record();
        let mut second = record.effects[0].clone();
        second.id = "requested-write-source-scope".into();
        second.target.as_mut().unwrap().scope = IdentityScope::Source;
        record.effects.push(second);
        assert!(record.validate().is_ok());
    }

    #[test]
    fn artifact_state_ids_are_unique_across_the_entire_record() {
        let mut record = fixture_record();
        let artifact = record.effects[0].target.clone().unwrap();
        let state = ArtifactStateDescriptor {
            id: "state-1".into(),
            evidence: EffectEvidence::Observed,
            relation: RelationId::from("comp:observedArtifactState"),
            artifact,
            digest: Some("sha256:abc".into()),
            sort: ConceptId::from("comp:ArtifactState"),
            source_field: "/state".into(),
        };
        record.effects[0].states.push(state.clone());
        let mut second = record.effects[0].clone();
        second.id = "second-effect".into();
        second.states = vec![state];
        record.effects.push(second);
        assert!(matches!(
            record.validate(),
            Err(ToolFormalizationError::DuplicateEffectState(id)) if id == "state-1"
        ));
    }

    #[test]
    fn result_spans_use_the_result_record_coordinates() {
        let mut record = fixture_record();
        let result_source = evidence_source(&record, "result-record");
        record.result.as_mut().unwrap().source_record = Some(result_source.clone());

        let document = ToolFormalizer {
            adapter: "fixture".into(),
            adapter_version: "1".into(),
            ontology: snapshot(),
        }
        .formalize(&record)
        .unwrap();

        let result_span = document
            .source_spans
            .values()
            .find(|span| span.field_path.as_deref() == Some("/result"))
            .expect("result span exists");
        assert_eq!(result_span.source, result_source.source);
        assert_eq!(result_span.run, result_source.run);
        assert_eq!(result_span.turn, result_source.turn);
        assert_eq!(result_span.message.as_deref(), Some("result-record"));
    }

    #[test]
    fn observed_effect_spans_use_the_evidence_record_coordinates() {
        let mut record = fixture_record();
        let change_source = evidence_source(&record, "change-record");
        let artifact = ArtifactDescriptor {
            id: "file:src/lib.rs".into(),
            scope: IdentityScope::Run,
            sort: ConceptId::from("comp:File"),
            locator: Some("src/lib.rs".into()),
        };
        record.effects.push(ToolEffect {
            id: "observed-change".into(),
            source_record: Some(change_source.clone()),
            evidence: EffectEvidence::Observed,
            sort: ConceptId::from("comp:FileWrite"),
            target: Some(artifact.clone()),
            source: None,
            destination: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            states: vec![ArtifactStateDescriptor {
                id: "after-change".into(),
                evidence: EffectEvidence::Observed,
                relation: RelationId::from("comp:afterArtifactState"),
                artifact,
                digest: Some("sha256:def".into()),
                sort: ConceptId::from("comp:FileState"),
                source_field: "/after_digest".into(),
            }],
            command: None,
            source_field: "/path".into(),
        });

        let document = ToolFormalizer {
            adapter: "fixture".into(),
            adapter_version: "1".into(),
            ontology: snapshot(),
        }
        .formalize(&record)
        .unwrap();

        for field in ["/path", "/after_digest"] {
            let span = document
                .source_spans
                .values()
                .find(|span| span.field_path.as_deref() == Some(field))
                .unwrap_or_else(|| panic!("missing source span for {field}"));
            assert_eq!(span.message.as_deref(), Some("change-record"));
            assert_eq!(span.source, change_source.source);
        }
    }

    #[test]
    fn evidence_from_a_different_source_namespace_is_rejected() {
        let mut record = fixture_record();
        let mut source = evidence_source(&record, "foreign-result");
        source.source = "foreign-session".into();
        record.result.as_mut().unwrap().source_record = Some(source);
        assert!(matches!(
            record.validate(),
            Err(ToolFormalizationError::CrossSourceEvidence {
                invocation_source,
                evidence_source,
            }) if invocation_source == "fixture" && evidence_source == "foreign-session"
        ));
    }

    #[test]
    fn record_scoped_artifact_ids_are_namespaced_by_evidence_record() {
        let mut record = fixture_record();
        record.effects.clear();
        for (index, record_id) in ["change-a", "change-b"].into_iter().enumerate() {
            let source = evidence_source(&record, record_id);
            record.effects.push(ToolEffect {
                id: format!("effect-{index}"),
                source_record: Some(source),
                evidence: EffectEvidence::Observed,
                sort: ConceptId::from("comp:FileWrite"),
                target: Some(ArtifactDescriptor {
                    id: "same-native-id".into(),
                    scope: IdentityScope::Record,
                    sort: ConceptId::from("comp:File"),
                    locator: Some(format!("file-{index}.rs")),
                }),
                source: None,
                destination: None,
                inputs: Vec::new(),
                outputs: Vec::new(),
                states: Vec::new(),
                command: None,
                source_field: "/path".into(),
            });
        }
        assert!(record.validate().is_ok());

        let document = ToolFormalizer {
            adapter: "fixture".into(),
            adapter_version: "1".into(),
            ontology: snapshot(),
        }
        .formalize(&record)
        .unwrap();
        let matching = document
            .referents
            .values()
            .filter(|referent| {
                referent.external_ids.get("artifact").map(String::as_str) == Some("same-native-id")
            })
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 2);
        assert_ne!(matching[0].id, matching[1].id);
    }

    #[test]
    fn observed_effect_is_not_derived_from_result_status() {
        let requested = ToolEffect {
            id: "write".into(),
            source_record: None,
            evidence: EffectEvidence::Requested,
            sort: ConceptId::from("comp:FileWrite"),
            target: None,
            source: None,
            destination: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            states: Vec::new(),
            command: None,
            source_field: "/request".into(),
        };
        let observed = ToolEffect {
            evidence: EffectEvidence::Observed,
            source_field: "/observed_change".into(),
            ..requested.clone()
        };
        assert_ne!(requested.evidence, observed.evidence);
    }
}
