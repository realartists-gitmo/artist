//! Canonical perspective-neutral occurrence and proposition representation.
//!
//! The representation deliberately separates proposition content from the
//! source occurrence or stance that presents that content. An actor asserting,
//! hypothesizing, believing, desiring, questioning, or commanding `P` never
//! turns `P` into an unqualified global fact merely by being represented here.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use muse_core::{
    AmbiguityId, CanonicalHashError, ConceptId, ContentDigest, EvidenceId, IdentityError,
    OccurrenceDocumentId, OccurrenceId, PropositionId, ReferentId, RelationId, SourceSpanId,
    StatementId, VariableId, canonical_digest,
};
use muse_registry::RegistrySnapshot;
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod canonical;
pub use canonical::{TRAINING_CANONICALIZATION_VERSION, TrainingCanonicalizationError};

/// Version of the shared Muse occurrence/proposition schema.
pub const SCHEMA_VERSION: &str = "muse-occurrence-7";

/// Half-open byte range within a source text unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    fn validate(&self) -> Result<(), OccurrenceValidationError> {
        if self.start > self.end {
            return Err(OccurrenceValidationError::InvalidByteRange {
                start: self.start,
                end: self.end,
            });
        }
        Ok(())
    }
}

/// Exact alignment back to prose or a structured field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub id: SourceSpanId,
    /// Stable source dataset/session/log identity.
    pub source: String,
    pub run: Option<String>,
    pub turn: Option<String>,
    pub message: Option<String>,
    pub block: Option<u32>,
    pub bytes: Option<ByteRange>,
    /// JSON pointer, field path, or equivalent for structured records.
    pub field_path: Option<String>,
}

impl SourceSpan {
    fn validate(&self) -> Result<(), OccurrenceValidationError> {
        self.id.validate()?;
        validate_nonempty("source span source", &self.source)?;
        if let Some(bytes) = &self.bytes {
            bytes.validate()?;
        }
        // RFC-6901 uses the empty string for the document root; structured
        // source spans therefore permit `Some("")` as a real path.
        if let Some(field_path) = &self.field_path {
            if !field_path.is_empty()
                && !field_path.starts_with('/')
                && !field_path.starts_with('@')
            {
                return Err(OccurrenceValidationError::InvalidSourceFieldPath(
                    field_path.clone(),
                ));
            }
        }
        Ok(())
    }
}

/// How a referent participates in the source discourse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscourseRole {
    User,
    Agent,
    System,
    Harness,
    Tool,
    External,
    Unknown,
}

/// An entity or type referred to by the source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Referent {
    pub id: ReferentId,
    /// Narrowest ontology types justified by the source. Learned prose uses
    /// globally unique namespaceless canonical symbols; deterministic/internal
    /// producers may use authoritative qualified IDs. Lowering resolves both.
    pub types: BTreeSet<ConceptId>,
    /// Exact open-vocabulary noun/name material when ontology typing is broader than the lexical content.
    #[serde(default)]
    pub lexical_anchor: Option<LexicalAnchor>,
    /// Surface labels are evidence/debugging metadata, not identity.
    pub labels: BTreeSet<String>,
    /// Dataset/provider/tool identities that are explicitly supplied by source.
    pub external_ids: BTreeMap<String, String>,
    pub discourse_roles: BTreeSet<DiscourseRole>,
    pub source_spans: BTreeSet<SourceSpanId>,
}

/// Closed arithmetic operations preserved without forcing the labeler/model to evaluate them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArithmeticOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
}

/// Ordered comparison operators. Identity/value equality remains `PropositionExpr::Equality`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonOperator {
    LessThan,
    LessOrEqual,
    GreaterThan,
    GreaterOrEqual,
}

/// Lossless scalar payload used inside formal terms.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Literal {
    String(String),
    /// Canonical mathematical integer value, not a transport spelling.
    Integer(String),
    /// Canonical finite decimal value, not a transport spelling.
    Decimal(String),
    Boolean(bool),
    Null,
    /// Exact structured data retained as data rather than interpreted semantics.
    Json(String),
    /// Unevaluated source-explicit ratio. Denominator must be non-zero.
    Ratio {
        numerator: Box<Literal>,
        denominator: Box<Literal>,
    },
    /// Source-explicit percentage; the child preserves the numeric percentage magnitude.
    Percentage {
        magnitude: Box<Literal>,
    },
    /// Source-explicit approximation. This is intentionally not equal to its child.
    Approximate {
        value: Box<Literal>,
    },
    /// Source-explicit numeric interval/range.
    Interval {
        lower: Box<Literal>,
        upper: Box<Literal>,
        lower_inclusive: bool,
        upper_inclusive: bool,
    },
    /// Vague plural scale such as `1000s`/`thousands`; it does not assert one exact count.
    PluralScale {
        base: Box<Literal>,
    },
    /// Unevaluated arithmetic expression.
    Arithmetic {
        operator: ArithmeticOperator,
        left: Box<Literal>,
        right: Box<Literal>,
    },
    /// Measurement quale carrying a numeric magnitude and exact source unit symbol/name.
    Measurement {
        magnitude: Box<Literal>,
        unit: String,
    },
}

/// Term admitted as an argument of a proposition or occurrence relation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Term {
    Referent(ReferentId),
    Occurrence(OccurrenceId),
    Proposition(PropositionId),
    Variable(VariableId),
    Literal(Literal),
}

/// Status explicitly reported by a source record or source prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportedOutcomeStatus {
    Success,
    Failure,
    Partial,
    Skipped,
    Denied,
    Cancelled,
    Interrupted,
    Unknown,
}

/// An outcome attribution. This never means Muse independently observed every
/// external effect implied by the requested action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportedOutcome {
    pub status: ReportedOutcomeStatus,
    pub reporter: Option<ReferentId>,
    pub detail: Option<String>,
    pub source_spans: BTreeSet<SourceSpanId>,
}

/// Exact source material that supplies an open-vocabulary semantic item.
///
/// Identity is the ordered set of exact source spans, never a labeler-invented
/// lemma or paraphrase. Multiple spans permit phrasal/discontinuous predicates
/// while keeping every content-bearing piece source grounded.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LexicalAnchor {
    pub spans: BTreeSet<SourceSpanId>,
}

/// Generic participant roles used by open-world prose. `Ontology` preserves
/// an exact controlled-vocabulary relation whenever the source plus pinned
/// ontology/lexicon information licenses it, including learned prose.
/// `SourceAnchored` is reserved for a materially expressed role that cannot be
/// represented by the finite generic inventory without guessing.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ParticipantRole {
    /// Conservative syntax-preserving fallbacks for open vocabulary. Use these
    /// when a semantic role would require lexical/world knowledge not supplied
    /// by the source construction itself.
    Subject,
    DirectObject,
    IndirectObject,
    PredicateComplement,
    Agent,
    Patient,
    Theme,
    Experiencer,
    Content,
    Source,
    Goal,
    Recipient,
    Instrument,
    Location,
    Manner,
    Beneficiary,
    Stimulus,
    Topic,
    Possessor,
    Attribute,
    Value,
    /// Learned prose uses the canonical namespaceless relation symbol.
    Ontology(RelationId),
    SourceAnchored(LexicalAnchor),
}

/// One semantic-role participation in an occurrence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    pub role: ParticipantRole,
    pub value: Term,
}

/// Grammatical tense explicitly licensed by source grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrammaticalTense {
    Past,
    Present,
    Future,
}

/// Grammatical aspect explicitly licensed by source grammar. Bare morphology
/// alone is insufficient evidence for any non-simple value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrammaticalAspect {
    Simple,
    Progressive,
    Perfect,
    PerfectProgressive,
}

/// Grammatical voice explicitly licensed by source syntax. This is structural
/// evidence, not a lexical guess about semantic roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrammaticalVoice {
    Active,
    Passive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TenseAspect {
    pub tense: Option<GrammaticalTense>,
    pub aspect: GrammaticalAspect,
}

/// Event/state/situation/process record grounded in source evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Occurrence {
    pub id: OccurrenceId,
    /// Same ontology-symbol convention as `Referent::types`.
    pub types: BTreeSet<ConceptId>,
    /// Open-world predicate material. Deterministic ontology-backed occurrences
    /// may leave this absent; learned prose requires it for every open predication; structured field-licensed occurrences may omit it.
    pub lexical_anchor: Option<LexicalAnchor>,
    pub participants: Vec<Participant>,
    pub attributes: BTreeMap<RelationId, Vec<Term>>,
    pub tense_aspect: Option<TenseAspect>,
    /// Independently expressed grammatical tense. Learned semantic-v6 uses this without inventing a `Simple` aspect.
    pub grammatical_tense: Option<GrammaticalTense>,
    pub grammatical_voice: Option<GrammaticalVoice>,
    /// Exact grammatical cues that license `tense_aspect` and/or grammatical voice.
    pub grammatical_spans: BTreeSet<SourceSpanId>,
    pub reported_outcome: Option<ReportedOutcome>,
    pub source_spans: BTreeSet<SourceSpanId>,
    pub evidence: BTreeSet<EvidenceId>,
}

/// Target that can participate in temporal or causal semantic structure.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SemanticTarget {
    Occurrence(OccurrenceId),
    Proposition(PropositionId),
    /// Bound semantic target used by interrogatives/quantifiers, e.g. the
    /// unknown reason in “why P?”.
    Variable(VariableId),
}

/// Canonical temporal relation. The interval relations follow their ordinary
/// directional readings; `At` anchors a target to a temporal value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalRelation {
    At,
    Before,
    After,
    Simultaneous,
    Meets,
    MetBy,
    Overlaps,
    OverlappedBy,
    Starts,
    StartedBy,
    During,
    Contains,
    Finishes,
    FinishedBy,
}

/// Precision of an exact or normalized calendar value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalPrecision {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Millisecond,
    Nanosecond,
}

/// Structured temporal anchor. Source expressions that cannot be normalized
/// without guessing remain explicit rather than being silently resolved.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TemporalAnchor {
    UnixMillis {
        value: i64,
    },
    Calendar {
        value: String,
        precision: TemporalPrecision,
        timezone: Option<String>,
    },
    Target {
        target: SemanticTarget,
    },
    /// Bound temporal answer/value, e.g. the unknown time in “when P?”.
    Variable {
        variable: VariableId,
    },
    SourceTime {
        source_span: SourceSpanId,
    },
    UnresolvedExpression {
        text: String,
    },
}

/// Causal/enablement relation explicitly expressed by the source.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "type", rename_all = "snake_case")]
pub enum CausalRelation {
    Causes,
    Enables,
    Prevents,
    ContributesTo,
    Motivates,
    /// Teleological purpose: the cause-side target is undertaken toward the
    /// effect-side target. This does not assert that the purpose was achieved.
    Purpose,
    Explains,
    Other(RelationId),
}

/// What kind of semantic uncertainty remains after conservative parsing.
/// One candidate resolution. These candidates are metadata about competing
/// interpretations, not a source-level disjunction asserted by the speaker.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum AmbiguityAlternative {
    Referent(ReferentId),
    Occurrence(OccurrenceId),
    Proposition(PropositionId),
    Concept(ConceptId),
    Relation(RelationId),
    Temporal(TemporalAnchor),
    Literal(Literal),
}

/// Explicit unresolved semantic choice among complete competing readings.
/// No candidate is privileged and no exhaustiveness claim is predicted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ambiguity {
    pub id: AmbiguityId,
    pub alternatives: BTreeSet<AmbiguityAlternative>,
    pub source_spans: BTreeSet<SourceSpanId>,
}

/// Quantifier explicitly present in proposition content.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Quantifier {
    Exists,
    ForAll,
    /// Exact numeric cardinality operators. The payload must be a non-negative
    /// integer literal; generalized non-numeric quantifiers remain source anchored.
    Exactly(Literal),
    AtLeast(Literal),
    AtMost(Literal),
    MoreThan(Literal),
    FewerThan(Literal),
    Approximately(Literal),
    /// Vague plural numeric scale (e.g. "1000s"), not a point estimate.
    PluralScale(Literal),
    /// Source-expressed generalized quantifier whose force is not one of the
    /// standardized logical/cardinality operators above.
    SourceAnchored(LexicalAnchor),
}

/// Source-expressed modality. Closed structural meanings have dedicated
/// variants. `SourceAnchored` preserves an unstandardized modal construction
/// without weakening it to a different modal force.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Modality {
    Possible,
    Necessary,
    Permitted,
    Obligatory,
    Prohibited,
    Ontology(ConceptId),
    SourceAnchored(LexicalAnchor),
}

/// Attitude whose holder is explicit. Knowledge and recollection are structural
/// because they take a holder and proposition/interrogative content; their
/// presence does not independently assert that embedded content as fact.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum AttitudeKind {
    Thought,
    Belief,
    Knowledge,
    Recollection,
    Desire,
    Intention,
    Assumption,
    Expectation,
    Suspicion,
    Preference,
    Ontology(ConceptId),
    SourceAnchored(LexicalAnchor),
}

/// Speech/communicative act embedded inside proposition content.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SpeechActKind {
    Assertion,
    Report,
    Question,
    Command,
    Request,
    Suggestion,
    Promise,
    Ontology(ConceptId),
    SourceAnchored(LexicalAnchor),
}

/// Interrogative content is distinct from top-level question force. WH/reason
/// forms bind an answer variable that must occur in the body.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum InterrogativeKind {
    Polar,
    Wh,
    Reason,
    Alternative,
    Tag,
    SourceAnchored(LexicalAnchor),
}

/// Phase of an embedded eventuality. This is separate from grammatical aspect.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PhaseKind {
    Begin,
    Continue,
    End,
    SourceAnchored(LexicalAnchor),
}

/// Structural proposition language shared by deterministic and learned paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PropositionExpr {
    TypeAssertion {
        subject: Term,
        r#type: ConceptId,
    },
    Relation {
        relation: RelationId,
        arguments: Vec<Term>,
    },
    Occurrence {
        occurrence: OccurrenceId,
    },
    Equality {
        left: Term,
        right: Term,
    },
    /// Ordered/dimensional comparison; equality is intentionally excluded to avoid duplicate encodings.
    Comparison {
        operator: ComparisonOperator,
        left: Term,
        right: Term,
        dimension: Option<Term>,
    },
    Negation {
        content: PropositionId,
    },
    Conjunction {
        members: BTreeSet<PropositionId>,
    },
    Disjunction {
        members: BTreeSet<PropositionId>,
    },
    Implication {
        antecedent: PropositionId,
        consequent: PropositionId,
    },
    /// Counterfactual conditional. This is intentionally distinct from material
    /// implication because its truth conditions depend on counterfactual alternatives.
    Counterfactual {
        antecedent: PropositionId,
        consequent: PropositionId,
    },
    /// Source-explicit `unless`; retained separately rather than silently reduced to material implication.
    Unless {
        condition: PropositionId,
        consequent: PropositionId,
    },
    Quantified {
        quantifier: Quantifier,
        variable: VariableId,
        domain: Option<ConceptId>,
        body: PropositionId,
    },
    /// Open lexical generalized quantifier represented as a typed semantic-operator referent.
    GeneralizedQuantified {
        quantifier: ReferentId,
        variable: VariableId,
        domain: Option<ConceptId>,
        body: PropositionId,
    },
    /// Ontology-backed scoped propositional operator. The referent's narrowest
    /// ontology sort determines the force; learned-v6 uses this for modal operators.
    ScopedOperator {
        operator: ReferentId,
        content: PropositionId,
    },
    /// Legacy modality channel retained only for compatibility with older deterministic inputs.
    Modal {
        modality: Modality,
        content: PropositionId,
    },
    /// Generic/habitual generalization, distinct from universal or majority quantification.
    Generic {
        content: PropositionId,
    },
    Perfect {
        content: PropositionId,
    },
    Progressive {
        content: PropositionId,
    },
    /// Source-explicit focus operator. The operator is a typed lexical referent and focus is exact source material.
    Focus {
        operator: ReferentId,
        focus: LexicalAnchor,
        content: PropositionId,
    },
    /// Explicitly separates asserted and presupposed content instead of conjoining them.
    Presuppositional {
        asserted: PropositionId,
        presupposed: PropositionId,
    },
    Capability {
        bearer: Term,
        content: PropositionId,
    },
    Interrogative {
        interrogative: InterrogativeKind,
        variable: Option<VariableId>,
        domain: Option<ConceptId>,
        body: PropositionId,
    },
    Phase {
        phase: PhaseKind,
        content: PropositionId,
    },
    Attitude {
        holder: Term,
        attitude: AttitudeKind,
        content: PropositionId,
    },
    SpeechAct {
        speaker: Term,
        act: SpeechActKind,
        addressees: BTreeSet<Term>,
        content: PropositionId,
    },
    /// Temporal relation explicitly present in source content.
    Temporal {
        subject: SemanticTarget,
        relation: TemporalRelation,
        object: TemporalAnchor,
    },
    /// Causal/enablement relation explicitly present in source content.
    Causal {
        relation: CausalRelation,
        cause: SemanticTarget,
        effect: SemanticTarget,
    },
    /// Exact mention/quotation of proposition content without asserting it.
    Quotation {
        content: PropositionId,
    },
}

/// One named proposition and its direct source alignment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposition {
    pub id: PropositionId,
    pub expression: PropositionExpr,
    /// Exact source cues that license this node's structural operator. Empty is
    /// permitted for atomic content; learned linguistic structure requires it for source-cued structural nodes.
    pub operator_spans: BTreeSet<SourceSpanId>,
    /// Grammatical profile carried by a structural operator such as an attitude,
    /// speech act, phase, capability, or modal construction.
    pub operator_tense_aspect: Option<TenseAspect>,
    pub operator_voice: Option<GrammaticalVoice>,
    pub operator_grammatical_spans: BTreeSet<SourceSpanId>,
    pub source_spans: BTreeSet<SourceSpanId>,
    pub evidence: BTreeSet<EvidenceId>,
}

/// Communicative force of a top-level source statement. Epistemic, evidential,
/// temporal, modal, and reporting semantics belong inside proposition structure.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PresentationMode {
    /// A structured source record establishes the content without itself being a linguistic assertion.
    Record,
    Assertion,
    Question,
    Command,
    Request,
    Suggestion,
    Promise,
    Quotation,
    Mention,
    SourceAnchored(LexicalAnchor),
}

/// Why a top-level formal statement is included in the semantic label.
///
/// `CompositionalEntailment` is limited to truth-conditional decomposition of
/// the represented source expression (for example, the members of an explicit
/// conjunction). It is not an invitation to emit downstream theorem/proof or
/// commonsense closure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositionalRule {
    /// From an explicitly represented conjunction to one of its members.
    ConjunctionProjection,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StatementBasis {
    Expressed,
    CompositionalEntailment {
        rule: CompositionalRule,
        premises: BTreeSet<PropositionId>,
    },
}

/// A materially expressed source statement. The presenter is independent from
/// actors/holders embedded in proposition content, which is essential for
/// resolving first-person prose correctly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    pub id: StatementId,
    pub presenter: Option<ReferentId>,
    pub addressees: BTreeSet<ReferentId>,
    pub mode: PresentationMode,
    pub basis: StatementBasis,
    pub content: PropositionId,
    pub source_spans: BTreeSet<SourceSpanId>,
    pub evidence: BTreeSet<EvidenceId>,
}

/// Provenance of the semantic conversion itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Derivation {
    DeterministicStructured { adapter: String, version: String },
    HumanAnnotation { annotator: String, protocol: String },
    ModelGenerated { model: String, protocol: String },
    Imported { source: String },
}

/// Ontology-typed semantic variable. Variables may be bound by logical operators or
/// remain open answer variables inside interrogative content; their ontology domain
/// and source denotation are stored once here rather than repeated by every use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticVariable {
    pub id: VariableId,
    pub sort: ConceptId,
    pub lexical_anchor: Option<LexicalAnchor>,
    pub source_spans: BTreeSet<SourceSpanId>,
}

/// Complete shared semantic document consumed by formal lowering and training.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OccurrenceDocument {
    pub schema_version: String,
    pub id: OccurrenceDocumentId,
    pub ontology: RegistrySnapshot,
    pub derivation: Derivation,
    pub source_spans: BTreeMap<SourceSpanId, SourceSpan>,
    pub referents: BTreeMap<ReferentId, Referent>,
    pub occurrences: BTreeMap<OccurrenceId, Occurrence>,
    pub variables: BTreeMap<VariableId, SemanticVariable>,
    pub propositions: BTreeMap<PropositionId, Proposition>,
    pub statements: BTreeMap<StatementId, Statement>,
    pub ambiguities: BTreeMap<AmbiguityId, Ambiguity>,
    /// Stable source order of materially expressed top-level statements.
    pub statement_order: Vec<StatementId>,
}

impl OccurrenceDocument {
    /// Validates structural references and the exact ontology snapshot identity.
    pub fn validate(&self) -> Result<(), OccurrenceValidationError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(OccurrenceValidationError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        self.id.validate()?;
        let expected_snapshot = canonical_digest(&self.ontology.packages)?;
        if expected_snapshot != self.ontology.identity {
            return Err(OccurrenceValidationError::OntologySnapshotDigestMismatch {
                expected: expected_snapshot,
                actual: self.ontology.identity.clone(),
            });
        }

        for (key, span) in &self.source_spans {
            if key != &span.id {
                return Err(OccurrenceValidationError::SourceSpanIdentityMismatch {
                    key: key.clone(),
                    declared: span.id.clone(),
                });
            }
            span.validate()?;
        }
        for (key, referent) in &self.referents {
            if key != &referent.id {
                return Err(OccurrenceValidationError::ReferentIdentityMismatch {
                    key: key.clone(),
                    declared: referent.id.clone(),
                });
            }
            key.validate()?;
            for ty in &referent.types {
                ty.validate()?;
            }
            self.validate_spans(&referent.source_spans)?;
        }
        for (key, occurrence) in &self.occurrences {
            if key != &occurrence.id {
                return Err(OccurrenceValidationError::OccurrenceIdentityMismatch {
                    key: key.clone(),
                    declared: occurrence.id.clone(),
                });
            }
            key.validate()?;
            for ty in &occurrence.types {
                ty.validate()?;
            }
            if let Some(anchor) = &occurrence.lexical_anchor {
                self.validate_lexical_anchor(anchor)?;
            }
            self.validate_spans(&occurrence.grammatical_spans)?;
            for participant in &occurrence.participants {
                self.validate_participant_role(&participant.role)?;
                self.validate_term(&participant.value)?;
            }
            for (relation, values) in &occurrence.attributes {
                relation.validate()?;
                for value in values {
                    self.validate_term(value)?;
                }
            }
            if let Some(outcome) = &occurrence.reported_outcome {
                if let Some(reporter) = &outcome.reporter {
                    self.require_referent(reporter)?;
                }
                self.validate_spans(&outcome.source_spans)?;
            }
            self.validate_spans(&occurrence.source_spans)?;
        }
        for (key, variable) in &self.variables {
            if key != &variable.id {
                return Err(OccurrenceValidationError::VariableIdentityMismatch {
                    key: key.clone(),
                    declared: variable.id.clone(),
                });
            }
            key.validate()?;
            variable.sort.validate()?;
            if let Some(anchor) = &variable.lexical_anchor {
                self.validate_lexical_anchor(anchor)?;
            }
            self.validate_spans(&variable.source_spans)?;
        }
        for (key, proposition) in &self.propositions {
            if key != &proposition.id {
                return Err(OccurrenceValidationError::PropositionIdentityMismatch {
                    key: key.clone(),
                    declared: proposition.id.clone(),
                });
            }
            key.validate()?;
            self.validate_proposition_expr(&proposition.expression)?;
            self.validate_spans(&proposition.operator_spans)?;
            self.validate_spans(&proposition.operator_grammatical_spans)?;
            self.validate_spans(&proposition.source_spans)?;
        }
        self.validate_proposition_cycles()?;

        for (key, statement) in &self.statements {
            if key != &statement.id {
                return Err(OccurrenceValidationError::StatementIdentityMismatch {
                    key: key.clone(),
                    declared: statement.id.clone(),
                });
            }
            key.validate()?;
            if let Some(presenter) = &statement.presenter {
                self.require_referent(presenter)?;
            }
            for addressee in &statement.addressees {
                self.require_referent(addressee)?;
            }
            self.require_proposition(&statement.content)?;
            if let StatementBasis::CompositionalEntailment { rule, premises } = &statement.basis {
                if premises.is_empty() {
                    return Err(OccurrenceValidationError::EmptyCompositionalPremises(
                        key.clone(),
                    ));
                }
                for premise in premises {
                    self.require_proposition(premise)?;
                }
                match rule {
                    CompositionalRule::ConjunctionProjection => {
                        if premises.len() != 1 {
                            return Err(OccurrenceValidationError::InvalidCompositionalEntailment(
                                key.clone(),
                            ));
                        }
                        let premise = premises.iter().next().ok_or_else(|| {
                            OccurrenceValidationError::InvalidCompositionalEntailment(key.clone())
                        })?;
                        let proposition = self.propositions.get(premise).ok_or_else(|| {
                            OccurrenceValidationError::InvalidCompositionalEntailment(key.clone())
                        })?;
                        match &proposition.expression {
                            PropositionExpr::Conjunction { members }
                                if members.contains(&statement.content) => {}
                            _ => {
                                return Err(
                                    OccurrenceValidationError::InvalidCompositionalEntailment(
                                        key.clone(),
                                    ),
                                );
                            }
                        }
                    }
                }
            }
            self.validate_spans(&statement.source_spans)?;
        }
        for (key, ambiguity) in &self.ambiguities {
            if key != &ambiguity.id {
                return Err(OccurrenceValidationError::AmbiguityIdentityMismatch {
                    key: key.clone(),
                    declared: ambiguity.id.clone(),
                });
            }
            key.validate()?;
            if ambiguity.alternatives.len() < 2 {
                return Err(OccurrenceValidationError::AmbiguityNeedsTwoAlternatives(
                    key.clone(),
                ));
            }
            for alternative in &ambiguity.alternatives {
                self.validate_ambiguity_alternative(alternative)?;
            }
            self.validate_spans(&ambiguity.source_spans)?;
        }

        let mut ordered = BTreeSet::new();
        for statement in &self.statement_order {
            if !self.statements.contains_key(statement) {
                return Err(OccurrenceValidationError::UnknownStatement(
                    statement.clone(),
                ));
            }
            if !ordered.insert(statement.clone()) {
                return Err(OccurrenceValidationError::DuplicateStatementOrder(
                    statement.clone(),
                ));
            }
        }
        if ordered.len() != self.statements.len() {
            return Err(OccurrenceValidationError::IncompleteStatementOrder);
        }
        Ok(())
    }

    /// Stable digest of the validated document. Maps/sets use sorted containers,
    /// so formatting and insertion order do not affect this identity.
    pub fn canonical_digest(&self) -> Result<ContentDigest, OccurrenceValidationError> {
        self.validate()?;
        Ok(canonical_digest(self)?)
    }

    fn validate_spans(
        &self,
        spans: &BTreeSet<SourceSpanId>,
    ) -> Result<(), OccurrenceValidationError> {
        for span in spans {
            if !self.source_spans.contains_key(span) {
                return Err(OccurrenceValidationError::UnknownSourceSpan(span.clone()));
            }
        }
        Ok(())
    }

    fn require_referent(&self, id: &ReferentId) -> Result<(), OccurrenceValidationError> {
        if self.referents.contains_key(id) {
            Ok(())
        } else {
            Err(OccurrenceValidationError::UnknownReferent(id.clone()))
        }
    }

    fn require_occurrence(&self, id: &OccurrenceId) -> Result<(), OccurrenceValidationError> {
        if self.occurrences.contains_key(id) {
            Ok(())
        } else {
            Err(OccurrenceValidationError::UnknownOccurrence(id.clone()))
        }
    }

    fn require_variable(&self, id: &VariableId) -> Result<(), OccurrenceValidationError> {
        if self.variables.contains_key(id) {
            Ok(())
        } else {
            Err(OccurrenceValidationError::UnknownVariable(id.clone()))
        }
    }

    fn require_proposition(&self, id: &PropositionId) -> Result<(), OccurrenceValidationError> {
        if self.propositions.contains_key(id) {
            Ok(())
        } else {
            Err(OccurrenceValidationError::UnknownProposition(id.clone()))
        }
    }

    fn validate_term(&self, term: &Term) -> Result<(), OccurrenceValidationError> {
        match term {
            Term::Referent(id) => self.require_referent(id),
            Term::Occurrence(id) => self.require_occurrence(id),
            Term::Proposition(id) => self.require_proposition(id),
            Term::Variable(id) => self.require_variable(id),
            Term::Literal(value) => validate_literal(value),
        }
    }

    fn validate_lexical_anchor(
        &self,
        anchor: &LexicalAnchor,
    ) -> Result<(), OccurrenceValidationError> {
        if anchor.spans.is_empty() {
            return Err(OccurrenceValidationError::EmptyLexicalAnchor);
        }
        self.validate_spans(&anchor.spans)
    }

    fn validate_participant_role(
        &self,
        role: &ParticipantRole,
    ) -> Result<(), OccurrenceValidationError> {
        match role {
            ParticipantRole::Ontology(relation) => relation.validate().map_err(Into::into),
            ParticipantRole::SourceAnchored(anchor) => self.validate_lexical_anchor(anchor),
            _ => Ok(()),
        }
    }

    fn validate_modality(&self, modality: &Modality) -> Result<(), OccurrenceValidationError> {
        match modality {
            Modality::Ontology(concept) => concept.validate().map_err(Into::into),
            Modality::SourceAnchored(anchor) => self.validate_lexical_anchor(anchor),
            _ => Ok(()),
        }
    }

    fn validate_attitude(&self, attitude: &AttitudeKind) -> Result<(), OccurrenceValidationError> {
        match attitude {
            AttitudeKind::Ontology(concept) => concept.validate().map_err(Into::into),
            AttitudeKind::SourceAnchored(anchor) => self.validate_lexical_anchor(anchor),
            _ => Ok(()),
        }
    }

    fn validate_speech_act(&self, act: &SpeechActKind) -> Result<(), OccurrenceValidationError> {
        match act {
            SpeechActKind::Ontology(concept) => concept.validate().map_err(Into::into),
            SpeechActKind::SourceAnchored(anchor) => self.validate_lexical_anchor(anchor),
            _ => Ok(()),
        }
    }

    fn validate_phase(&self, phase: &PhaseKind) -> Result<(), OccurrenceValidationError> {
        match phase {
            PhaseKind::SourceAnchored(anchor) => self.validate_lexical_anchor(anchor),
            _ => Ok(()),
        }
    }

    fn validate_interrogative(
        &self,
        interrogative: &InterrogativeKind,
    ) -> Result<(), OccurrenceValidationError> {
        match interrogative {
            InterrogativeKind::SourceAnchored(anchor) => self.validate_lexical_anchor(anchor),
            _ => Ok(()),
        }
    }

    fn validate_proposition_expr(
        &self,
        expression: &PropositionExpr,
    ) -> Result<(), OccurrenceValidationError> {
        match expression {
            PropositionExpr::TypeAssertion { subject, r#type } => {
                self.validate_term(subject)?;
                r#type.validate()?;
            }
            PropositionExpr::Relation {
                relation,
                arguments,
            } => {
                relation.validate()?;
                if arguments.is_empty() {
                    return Err(OccurrenceValidationError::EmptyRelationArguments(
                        relation.clone(),
                    ));
                }
                for argument in arguments {
                    self.validate_term(argument)?;
                }
            }
            PropositionExpr::Occurrence { occurrence } => self.require_occurrence(occurrence)?,
            PropositionExpr::Equality { left, right } => {
                self.validate_term(left)?;
                self.validate_term(right)?;
            }
            PropositionExpr::Comparison {
                left,
                right,
                dimension,
                ..
            } => {
                self.validate_term(left)?;
                self.validate_term(right)?;
                if let Some(dimension) = dimension {
                    self.validate_term(dimension)?;
                }
            }
            PropositionExpr::Negation { content }
            | PropositionExpr::Phase { content, .. }
            | PropositionExpr::Modal { content, .. }
            | PropositionExpr::Generic { content }
            | PropositionExpr::Perfect { content }
            | PropositionExpr::Progressive { content }
            | PropositionExpr::Attitude { content, .. }
            | PropositionExpr::SpeechAct { content, .. }
            | PropositionExpr::Quotation { content } => self.require_proposition(content)?,
            PropositionExpr::Capability { bearer, content } => {
                self.validate_term(bearer)?;
                self.require_proposition(content)?;
            }
            PropositionExpr::Interrogative {
                interrogative,
                variable,
                domain,
                body,
            } => {
                self.validate_interrogative(interrogative)?;
                if let Some(domain) = domain {
                    domain.validate()?;
                }
                match interrogative {
                    InterrogativeKind::Wh | InterrogativeKind::Reason => {
                        let variable = variable
                            .as_ref()
                            .ok_or(OccurrenceValidationError::MissingInterrogativeVariable)?;
                        self.require_variable(variable)?;
                    }
                    InterrogativeKind::Polar
                    | InterrogativeKind::Alternative
                    | InterrogativeKind::Tag => {
                        if variable.is_some() {
                            return Err(OccurrenceValidationError::UnexpectedInterrogativeVariable);
                        }
                    }
                    InterrogativeKind::SourceAnchored(_) => {
                        if let Some(variable) = variable {
                            self.require_variable(variable)?;
                        }
                    }
                }
                self.require_proposition(body)?;
            }
            PropositionExpr::Conjunction { members } | PropositionExpr::Disjunction { members } => {
                if members.len() < 2 {
                    return Err(OccurrenceValidationError::BooleanNeedsTwoMembers);
                }
                for member in members {
                    self.require_proposition(member)?;
                }
            }
            PropositionExpr::Implication {
                antecedent,
                consequent,
            }
            | PropositionExpr::Counterfactual {
                antecedent,
                consequent,
            } => {
                self.require_proposition(antecedent)?;
                self.require_proposition(consequent)?;
            }
            PropositionExpr::Unless {
                condition,
                consequent,
            } => {
                self.require_proposition(condition)?;
                self.require_proposition(consequent)?;
            }
            PropositionExpr::Presuppositional {
                asserted,
                presupposed,
            } => {
                self.require_proposition(asserted)?;
                self.require_proposition(presupposed)?;
            }
            PropositionExpr::Quantified {
                quantifier,
                variable,
                domain,
                body,
            } => {
                match quantifier {
                    Quantifier::SourceAnchored(anchor) => self.validate_lexical_anchor(anchor)?,
                    Quantifier::Exactly(value)
                    | Quantifier::AtLeast(value)
                    | Quantifier::AtMost(value)
                    | Quantifier::MoreThan(value)
                    | Quantifier::FewerThan(value)
                    | Quantifier::Approximately(value) => validate_cardinality_literal(value)?,
                    Quantifier::PluralScale(value) => validate_plural_scale_literal(value)?,
                    Quantifier::Exists | Quantifier::ForAll => {}
                }
                self.require_variable(variable)?;
                if let Some(domain) = domain {
                    domain.validate()?;
                    if self
                        .variables
                        .get(variable)
                        .is_some_and(|declared| &declared.sort != domain)
                    {
                        return Err(OccurrenceValidationError::VariableDomainMismatch(
                            variable.clone(),
                        ));
                    }
                }
                self.require_proposition(body)?;
            }
            PropositionExpr::GeneralizedQuantified {
                quantifier,
                variable,
                domain,
                body,
            } => {
                self.require_referent(quantifier)?;
                self.require_variable(variable)?;
                if let Some(domain) = domain {
                    domain.validate()?;
                    if self
                        .variables
                        .get(variable)
                        .is_some_and(|declared| &declared.sort != domain)
                    {
                        return Err(OccurrenceValidationError::VariableDomainMismatch(
                            variable.clone(),
                        ));
                    }
                }
                self.require_proposition(body)?;
            }
            PropositionExpr::ScopedOperator {
                operator, content, ..
            } => {
                self.require_referent(operator)?;
                self.require_proposition(content)?;
            }
            PropositionExpr::Focus {
                operator,
                focus,
                content,
            } => {
                self.require_referent(operator)?;
                self.validate_lexical_anchor(focus)?;
                self.require_proposition(content)?;
            }
            PropositionExpr::Temporal {
                subject, object, ..
            } => {
                self.validate_semantic_target(subject)?;
                self.validate_temporal_anchor(object)?;
            }
            PropositionExpr::Causal {
                relation,
                cause,
                effect,
            } => {
                if let CausalRelation::Other(relation) = relation {
                    relation.validate()?;
                }
                self.validate_semantic_target(cause)?;
                self.validate_semantic_target(effect)?;
            }
        }
        match expression {
            PropositionExpr::Modal { modality, .. } => self.validate_modality(modality)?,
            PropositionExpr::Phase { phase, .. } => self.validate_phase(phase)?,
            PropositionExpr::Attitude {
                holder, attitude, ..
            } => {
                self.validate_term(holder)?;
                self.validate_attitude(attitude)?;
            }
            PropositionExpr::SpeechAct {
                speaker,
                act,
                addressees,
                ..
            } => {
                self.validate_term(speaker)?;
                self.validate_speech_act(act)?;
                for addressee in addressees {
                    self.validate_term(addressee)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn validate_semantic_target(
        &self,
        target: &SemanticTarget,
    ) -> Result<(), OccurrenceValidationError> {
        match target {
            SemanticTarget::Occurrence(id) => self.require_occurrence(id),
            SemanticTarget::Proposition(id) => self.require_proposition(id),
            SemanticTarget::Variable(id) => id.validate().map_err(Into::into),
        }
    }

    fn validate_temporal_anchor(
        &self,
        anchor: &TemporalAnchor,
    ) -> Result<(), OccurrenceValidationError> {
        match anchor {
            TemporalAnchor::UnixMillis { .. } => Ok(()),
            TemporalAnchor::Calendar {
                value, timezone, ..
            } => {
                validate_nonempty("calendar time", value)?;
                if let Some(timezone) = timezone {
                    validate_nonempty("calendar timezone", timezone)?;
                }
                Ok(())
            }
            TemporalAnchor::Target { target } => self.validate_semantic_target(target),
            TemporalAnchor::Variable { variable } => variable.validate().map_err(Into::into),
            TemporalAnchor::SourceTime { source_span } => {
                if self.source_spans.contains_key(source_span) {
                    Ok(())
                } else {
                    Err(OccurrenceValidationError::UnknownSourceSpan(
                        source_span.clone(),
                    ))
                }
            }
            TemporalAnchor::UnresolvedExpression { text } => {
                validate_nonempty("temporal expression", text)?;
                Ok(())
            }
        }
    }

    fn validate_ambiguity_alternative(
        &self,
        alternative: &AmbiguityAlternative,
    ) -> Result<(), OccurrenceValidationError> {
        match alternative {
            AmbiguityAlternative::Referent(id) => self.require_referent(id),
            AmbiguityAlternative::Occurrence(id) => self.require_occurrence(id),
            AmbiguityAlternative::Proposition(id) => self.require_proposition(id),
            AmbiguityAlternative::Concept(id) => id.validate().map_err(Into::into),
            AmbiguityAlternative::Relation(id) => id.validate().map_err(Into::into),
            AmbiguityAlternative::Temporal(anchor) => self.validate_temporal_anchor(anchor),
            AmbiguityAlternative::Literal(_) => Ok(()),
        }
    }

    fn proposition_dependencies(expression: &PropositionExpr) -> Vec<&PropositionId> {
        match expression {
            PropositionExpr::TypeAssertion { subject, .. } => match subject {
                Term::Proposition(id) => vec![id],
                _ => Vec::new(),
            },
            PropositionExpr::Relation { arguments, .. } => arguments
                .iter()
                .filter_map(|term| match term {
                    Term::Proposition(id) => Some(id),
                    _ => None,
                })
                .collect(),
            PropositionExpr::Equality { left, right } => [left, right]
                .into_iter()
                .filter_map(|term| match term {
                    Term::Proposition(id) => Some(id),
                    _ => None,
                })
                .collect(),
            PropositionExpr::Comparison {
                left,
                right,
                dimension,
                ..
            } => {
                let mut dependencies = [left, right]
                    .into_iter()
                    .filter_map(|term| match term {
                        Term::Proposition(id) => Some(id),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if let Some(Term::Proposition(id)) = dimension {
                    dependencies.push(id);
                }
                dependencies
            }
            PropositionExpr::Negation { content }
            | PropositionExpr::ScopedOperator { content, .. }
            | PropositionExpr::Modal { content, .. }
            | PropositionExpr::Generic { content }
            | PropositionExpr::Perfect { content }
            | PropositionExpr::Progressive { content }
            | PropositionExpr::Focus { content, .. }
            | PropositionExpr::Capability { content, .. }
            | PropositionExpr::Phase { content, .. }
            | PropositionExpr::Attitude { content, .. }
            | PropositionExpr::SpeechAct { content, .. }
            | PropositionExpr::Quotation { content } => vec![content],
            PropositionExpr::Interrogative { body, .. } => vec![body],
            PropositionExpr::Conjunction { members } | PropositionExpr::Disjunction { members } => {
                members.iter().collect()
            }
            PropositionExpr::Implication {
                antecedent,
                consequent,
            }
            | PropositionExpr::Counterfactual {
                antecedent,
                consequent,
            } => vec![antecedent, consequent],
            PropositionExpr::Unless {
                condition,
                consequent,
            } => vec![condition, consequent],
            PropositionExpr::Presuppositional {
                asserted,
                presupposed,
            } => vec![asserted, presupposed],
            PropositionExpr::Quantified { body, .. }
            | PropositionExpr::GeneralizedQuantified { body, .. } => vec![body],
            PropositionExpr::Temporal {
                subject, object, ..
            } => {
                let mut dependencies = Vec::new();
                if let SemanticTarget::Proposition(id) = subject {
                    dependencies.push(id);
                }
                if let TemporalAnchor::Target {
                    target: SemanticTarget::Proposition(id),
                } = object
                {
                    dependencies.push(id)
                }
                dependencies
            }
            PropositionExpr::Causal { cause, effect, .. } => {
                let mut dependencies = Vec::new();
                if let SemanticTarget::Proposition(id) = cause {
                    dependencies.push(id);
                }
                if let SemanticTarget::Proposition(id) = effect {
                    dependencies.push(id);
                }
                dependencies
            }
            _ => Vec::new(),
        }
    }

    fn validate_proposition_cycles(&self) -> Result<(), OccurrenceValidationError> {
        fn visit(
            document: &OccurrenceDocument,
            id: &PropositionId,
            temporary: &mut BTreeSet<PropositionId>,
            permanent: &mut BTreeSet<PropositionId>,
        ) -> Result<(), OccurrenceValidationError> {
            if permanent.contains(id) {
                return Ok(());
            }
            if !temporary.insert(id.clone()) {
                return Err(OccurrenceValidationError::CyclicProposition(id.clone()));
            }
            let proposition = document
                .propositions
                .get(id)
                .ok_or_else(|| OccurrenceValidationError::UnknownProposition(id.clone()))?;
            for dependency in OccurrenceDocument::proposition_dependencies(&proposition.expression)
            {
                visit(document, dependency, temporary, permanent)?;
            }
            temporary.remove(id);
            permanent.insert(id.clone());
            Ok(())
        }

        let mut permanent = BTreeSet::new();
        for id in self.propositions.keys() {
            let mut temporary = BTreeSet::new();
            visit(self, id, &mut temporary, &mut permanent)?;
        }
        Ok(())
    }
}

fn validate_nonempty(kind: &'static str, value: &str) -> Result<(), IdentityError> {
    if value.is_empty() {
        return Err(IdentityError::Empty { kind });
    }
    if value.trim() != value {
        return Err(IdentityError::SurroundingWhitespace {
            kind,
            value: value.to_owned(),
        });
    }
    Ok(())
}

fn validate_literal(value: &Literal) -> Result<(), OccurrenceValidationError> {
    match value {
        Literal::Ratio {
            numerator,
            denominator,
        } => {
            validate_literal(numerator)?;
            validate_literal(denominator)?;
            if !is_numeric_literal(numerator) || !is_numeric_literal(denominator) {
                return Err(OccurrenceValidationError::InvalidNumericLiteral);
            }
            if literal_is_zero(denominator) {
                return Err(OccurrenceValidationError::ZeroRatioDenominator);
            }
        }
        Literal::Percentage { magnitude } => {
            validate_literal(magnitude)?;
            let exact = match &**magnitude {
                Literal::Integer(_) | Literal::Decimal(_) | Literal::Ratio { .. } => true,
                Literal::Arithmetic { operator, .. } => {
                    !matches!(operator, ArithmeticOperator::Divide)
                }
                _ => false,
            };
            if !exact {
                return Err(OccurrenceValidationError::InvalidNumericLiteral);
            }
        }
        Literal::Approximate { value: magnitude } => {
            validate_literal(magnitude)?;
            if !is_numeric_literal(magnitude) {
                return Err(OccurrenceValidationError::InvalidNumericLiteral);
            }
        }
        Literal::PluralScale { base } => {
            validate_literal(base)?;
            validate_cardinality_literal(base)?;
        }
        Literal::Measurement { magnitude, unit } => {
            validate_literal(magnitude)?;
            if !is_numeric_literal(magnitude) {
                return Err(OccurrenceValidationError::InvalidNumericLiteral);
            }
            if unit.trim().is_empty() || unit.trim() != unit {
                return Err(OccurrenceValidationError::InvalidMeasurementUnit);
            }
        }
        Literal::Interval { lower, upper, .. } => {
            validate_literal(lower)?;
            validate_literal(upper)?;
            if !is_numeric_point_literal(lower) || !is_numeric_point_literal(upper) {
                return Err(OccurrenceValidationError::InvalidNumericLiteral);
            }
        }
        Literal::Arithmetic { left, right, .. } => {
            validate_literal(left)?;
            validate_literal(right)?;
            if !is_numeric_literal(left) || !is_numeric_literal(right) {
                return Err(OccurrenceValidationError::InvalidNumericLiteral);
            }
        }
        Literal::String(_)
        | Literal::Integer(_)
        | Literal::Decimal(_)
        | Literal::Boolean(_)
        | Literal::Null
        | Literal::Json(_) => {}
    }
    Ok(())
}

fn is_numeric_literal(value: &Literal) -> bool {
    matches!(
        value,
        Literal::Integer(_)
            | Literal::Decimal(_)
            | Literal::Ratio { .. }
            | Literal::Percentage { .. }
            | Literal::Approximate { .. }
            | Literal::Interval { .. }
            | Literal::PluralScale { .. }
            | Literal::Arithmetic { .. }
    )
}

fn is_numeric_point_literal(value: &Literal) -> bool {
    matches!(
        value,
        Literal::Integer(_)
            | Literal::Decimal(_)
            | Literal::Ratio { .. }
            | Literal::Percentage { .. }
            | Literal::Approximate { .. }
            | Literal::Arithmetic { .. }
    )
}

fn literal_is_zero(value: &Literal) -> bool {
    match value {
        Literal::Integer(value) | Literal::Decimal(value) => {
            let trimmed = value.trim_start_matches(['+', '-']);
            let mantissa = trimmed.split(['e', 'E']).next().unwrap_or(trimmed);
            mantissa.chars().all(|ch| ch == '0' || ch == '.')
                && mantissa.chars().any(|ch| ch == '0')
        }
        _ => false,
    }
}

fn validate_cardinality_literal(value: &Literal) -> Result<(), OccurrenceValidationError> {
    match value {
        Literal::Integer(text)
            if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            Ok(())
        }
        _ => Err(OccurrenceValidationError::InvalidCardinalityLiteral),
    }
}

fn validate_plural_scale_literal(value: &Literal) -> Result<(), OccurrenceValidationError> {
    match value {
        Literal::PluralScale { base } => validate_cardinality_literal(base),
        _ => Err(OccurrenceValidationError::InvalidCardinalityLiteral),
    }
}

#[derive(Debug, Error)]
pub enum OccurrenceValidationError {
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    Canonical(#[from] CanonicalHashError),
    #[error("unsupported occurrence schema {0}")]
    UnsupportedSchema(String),
    #[error("invalid source byte range {start}..{end}")]
    InvalidByteRange { start: u64, end: u64 },
    #[error("invalid structured source field path {0}")]
    InvalidSourceFieldPath(String),
    #[error("ontology snapshot digest mismatch: expected {expected}, found {actual}")]
    OntologySnapshotDigestMismatch {
        expected: ContentDigest,
        actual: ContentDigest,
    },
    #[error("source-span map key {key} contains declaration {declared}")]
    SourceSpanIdentityMismatch {
        key: SourceSpanId,
        declared: SourceSpanId,
    },
    #[error("referent map key {key} contains declaration {declared}")]
    ReferentIdentityMismatch {
        key: ReferentId,
        declared: ReferentId,
    },
    #[error("occurrence map key {key} contains declaration {declared}")]
    OccurrenceIdentityMismatch {
        key: OccurrenceId,
        declared: OccurrenceId,
    },
    #[error("proposition map key {key} contains declaration {declared}")]
    PropositionIdentityMismatch {
        key: PropositionId,
        declared: PropositionId,
    },
    #[error("statement map key {key} contains declaration {declared}")]
    StatementIdentityMismatch {
        key: StatementId,
        declared: StatementId,
    },
    #[error("ambiguity map key {key} contains declaration {declared}")]
    AmbiguityIdentityMismatch {
        key: AmbiguityId,
        declared: AmbiguityId,
    },
    #[error("unknown source span {0}")]
    UnknownSourceSpan(SourceSpanId),
    #[error("unknown referent {0}")]
    UnknownReferent(ReferentId),
    #[error("unknown occurrence {0}")]
    UnknownOccurrence(OccurrenceId),
    #[error("unknown proposition {0}")]
    UnknownProposition(PropositionId),
    #[error("unknown semantic variable {0}")]
    UnknownVariable(VariableId),
    #[error("semantic variable map key {key} disagrees with declared id {declared}")]
    VariableIdentityMismatch {
        key: VariableId,
        declared: VariableId,
    },
    #[error("quantifier/interrogative domain disagrees with declared semantic variable {0}")]
    VariableDomainMismatch(VariableId),
    #[error("unknown statement {0}")]
    UnknownStatement(StatementId),
    #[error("numeric cardinality quantifiers require a non-negative integer literal")]
    InvalidCardinalityLiteral,
    #[error("numeric literal position contains a non-numeric literal")]
    InvalidNumericLiteral,
    #[error("measurement unit must be non-empty and free of surrounding whitespace")]
    InvalidMeasurementUnit,
    #[error("ratio denominator must be non-zero")]
    ZeroRatioDenominator,
    #[error("relation {0} has no arguments")]
    EmptyRelationArguments(RelationId),
    #[error("source-anchored lexical item has no source spans")]
    EmptyLexicalAnchor,
    #[error("WH/reason interrogative requires a bound answer variable")]
    MissingInterrogativeVariable,
    #[error("this interrogative kind does not admit an answer variable")]
    UnexpectedInterrogativeVariable,
    #[error("conjunction/disjunction requires at least two members")]
    BooleanNeedsTwoMembers,
    #[error("ambiguity {0} requires at least two alternatives")]
    AmbiguityNeedsTwoAlternatives(AmbiguityId),
    #[error("compositional statement {0} requires at least one premise")]
    EmptyCompositionalPremises(StatementId),
    #[error("statement {0} is not licensed by its declared compositional rule")]
    InvalidCompositionalEntailment(StatementId),
    #[error("cyclic proposition dependency at {0}")]
    CyclicProposition(PropositionId),
    #[error("statement order contains duplicate {0}")]
    DuplicateStatementOrder(StatementId),
    #[error("statement order does not contain every statement exactly once")]
    IncompleteStatementOrder,
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

    #[test]
    fn structured_source_span_accepts_rfc6901_root_and_reserved_metadata_paths() {
        let span = |path: &str| SourceSpan {
            id: SourceSpanId::from("span:test"),
            source: "s".into(),
            run: None,
            turn: None,
            message: None,
            block: None,
            bytes: None,
            field_path: Some(path.into()),
        };
        assert!(span("").validate().is_ok());
        assert!(span("/input/message").validate().is_ok());
        assert!(span("@tool_name").validate().is_ok());
        assert!(matches!(
            span("not/a/pointer").validate(),
            Err(OccurrenceValidationError::InvalidSourceFieldPath(_))
        ));
    }

    #[test]
    fn numeric_cardinality_requires_non_negative_integer_literal() {
        assert!(validate_cardinality_literal(&Literal::Integer("3".into())).is_ok());
        assert!(matches!(
            validate_cardinality_literal(&Literal::Decimal("3.0".into())),
            Err(OccurrenceValidationError::InvalidCardinalityLiteral)
        ));
        assert!(matches!(
            validate_cardinality_literal(&Literal::Integer("-1".into())),
            Err(OccurrenceValidationError::InvalidCardinalityLiteral)
        ));
    }

    #[test]
    fn counterfactual_tracks_both_proposition_dependencies() {
        let antecedent = PropositionId::from("proposition:antecedent");
        let consequent = PropositionId::from("proposition:consequent");
        let expression = PropositionExpr::Counterfactual {
            antecedent: antecedent.clone(),
            consequent: consequent.clone(),
        };
        assert_eq!(
            OccurrenceDocument::proposition_dependencies(&expression),
            vec![&antecedent, &consequent]
        );
    }

    #[test]
    fn first_person_holder_is_not_the_global_perspective() {
        let user = ReferentId::from("referent:user");
        let agent = ReferentId::from("referent:agent");
        let p = PropositionId::from("proposition:x");
        let user_thought = PropositionId::from("proposition:user-thought-x");
        let agent_thought = PropositionId::from("proposition:agent-thought-x");

        let referent = |id: ReferentId, role| Referent {
            id,
            types: BTreeSet::new(),
            lexical_anchor: None,
            labels: BTreeSet::new(),
            external_ids: BTreeMap::new(),
            discourse_roles: BTreeSet::from([role]),
            source_spans: BTreeSet::new(),
        };
        let propositions = BTreeMap::from([
            (
                p.clone(),
                Proposition {
                    id: p.clone(),
                    expression: PropositionExpr::Equality {
                        left: Term::Literal(Literal::String("x".into())),
                        right: Term::Literal(Literal::String("x".into())),
                    },
                    operator_spans: BTreeSet::new(),
                    operator_tense_aspect: None,
                    operator_voice: None,
                    operator_grammatical_spans: BTreeSet::new(),
                    source_spans: BTreeSet::new(),
                    evidence: BTreeSet::new(),
                },
            ),
            (
                user_thought.clone(),
                Proposition {
                    id: user_thought.clone(),
                    expression: PropositionExpr::Attitude {
                        holder: Term::Referent(user.clone()),
                        attitude: AttitudeKind::Thought,
                        content: p.clone(),
                    },
                    operator_spans: BTreeSet::new(),
                    operator_tense_aspect: None,
                    operator_voice: None,
                    operator_grammatical_spans: BTreeSet::new(),
                    source_spans: BTreeSet::new(),
                    evidence: BTreeSet::new(),
                },
            ),
            (
                agent_thought.clone(),
                Proposition {
                    id: agent_thought.clone(),
                    expression: PropositionExpr::Attitude {
                        holder: Term::Referent(agent.clone()),
                        attitude: AttitudeKind::Thought,
                        content: p,
                    },
                    operator_spans: BTreeSet::new(),
                    operator_tense_aspect: None,
                    operator_voice: None,
                    operator_grammatical_spans: BTreeSet::new(),
                    source_spans: BTreeSet::new(),
                    evidence: BTreeSet::new(),
                },
            ),
        ]);

        let make_statement = |id: &str, presenter: ReferentId, content: PropositionId| Statement {
            id: StatementId::from(id),
            presenter: Some(presenter),
            addressees: BTreeSet::new(),
            mode: PresentationMode::Assertion,
            basis: StatementBasis::Expressed,
            content,
            source_spans: BTreeSet::new(),
            evidence: BTreeSet::new(),
        };
        let s1 = make_statement("statement:user", user.clone(), user_thought);
        let s2 = make_statement("statement:agent", agent.clone(), agent_thought);
        let document = OccurrenceDocument {
            schema_version: SCHEMA_VERSION.into(),
            id: OccurrenceDocumentId::from("occurrence-document:test"),
            ontology: snapshot(),
            derivation: Derivation::HumanAnnotation {
                annotator: "test".into(),
                protocol: "test".into(),
            },
            source_spans: BTreeMap::new(),
            referents: BTreeMap::from([
                (user.clone(), referent(user, DiscourseRole::User)),
                (agent.clone(), referent(agent, DiscourseRole::Agent)),
            ]),
            occurrences: BTreeMap::new(),
            variables: BTreeMap::new(),
            propositions,
            statements: BTreeMap::from([(s1.id.clone(), s1.clone()), (s2.id.clone(), s2.clone())]),
            ambiguities: BTreeMap::new(),
            statement_order: vec![s1.id, s2.id],
        };
        document.validate().unwrap();
    }
}
