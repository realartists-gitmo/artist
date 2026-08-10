//! Minimal model-facing semantic target for semantic-label v6.
//!
//! This is intentionally not the rich `OccurrenceDocument`.  The model predicts
//! only independent semantic facts grounded in the current window.  Provenance,
//! ontology snapshots, source identities, discourse actors, deterministic tool
//! shells, derivation metadata, and formal closure are compiler-owned.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use muse_core::{
    AmbiguityId, ConceptId, ContentId, OccurrenceId, PropositionId, ReferentId, RelationId,
    VariableId,
};
use muse_occurrence::{ComparisonOperator, GrammaticalTense};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "source_kind", rename_all = "snake_case")]
pub enum LearnedSpan {
    Prose {
        start_byte: u64,
        end_byte: u64,
    },
    Structured {
        /// RFC-6901 source path. The empty string is the JSON document root.
        field_path: String,
        /// Required for strings/fragments, forbidden for non-string/container anchors.
        start_byte: Option<u64>,
        end_byte: Option<u64>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LearnedAnchor {
    /// Exact source coordinates inline. The compiler interns them deterministically;
    /// the SLM never predicts meaningless source-span IDs.
    pub spans: BTreeSet<LearnedSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedReferent {
    pub id: ReferentId,
    /// Exactly one narrowest canonical namespaceless ontology sort.
    pub sort: ConceptId,
    /// Exact source material denoting the referent; also preserves open noun/name content.
    pub anchor: LearnedAnchor,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedOccurrence {
    pub id: OccurrenceId,
    /// Exactly one narrowest canonical namespaceless ontology sort.
    pub sort: ConceptId,
    /// Exact predicate/eventuality source material; preserves open verbs/predicates.
    pub anchor: LearnedAnchor,
    /// Only source-expressed temporal tense. Voice and "simple" aspect are not targets.
    pub tense: Option<GrammaticalTense>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedVariable {
    pub id: VariableId,
    /// Ontology domain is predicted once, here, rather than repeated by each binder.
    pub sort: ConceptId,
    pub anchor: LearnedAnchor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextTerm {
    Speaker,
    Addressee,
    Actor,
    Recorder,
    Tool,
    Invocation,
    Result,
    Update,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearnedArithmeticOperator {
    Add,
    Subtract,
    Multiply,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LearnedLiteral {
    String {
        value: String,
    },
    Integer {
        value: String,
    },
    Decimal {
        value: String,
    },
    Boolean {
        value: bool,
    },
    Null,
    Ratio {
        numerator: Box<LearnedLiteral>,
        denominator: Box<LearnedLiteral>,
    },
    Percentage {
        magnitude: Box<LearnedLiteral>,
    },
    Approximate {
        value: Box<LearnedLiteral>,
    },
    Interval {
        lower: Box<LearnedLiteral>,
        upper: Box<LearnedLiteral>,
        lower_inclusive: bool,
        upper_inclusive: bool,
    },
    PluralScale {
        base: Box<LearnedLiteral>,
    },
    Arithmetic {
        operator: LearnedArithmeticOperator,
        left: Box<LearnedLiteral>,
        right: Box<LearnedLiteral>,
    },
    Measurement {
        magnitude: Box<LearnedLiteral>,
        unit: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GroundedLiteral {
    pub value: LearnedLiteral,
    pub anchor: LearnedAnchor,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum LearnedTerm {
    Referent(ReferentId),
    Occurrence(OccurrenceId),
    Proposition(PropositionId),
    Variable(VariableId),
    Literal(GroundedLiteral),
    Context(ContextTerm),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardinalityComparator {
    Exactly,
    AtLeast,
    AtMost,
    MoreThan,
    FewerThan,
    Approximately,
}

/// Truth-valued semantic structure only. Questions/directives/quotations live in
/// `LearnedContent` and therefore cannot accidentally be asserted as propositions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LearnedPropositionExpr {
    TypeAssertion {
        subject: LearnedTerm,
        r#type: ConceptId,
    },
    Relation {
        relation: RelationId,
        arguments: Vec<LearnedTerm>,
    },
    Occurrence {
        occurrence: OccurrenceId,
    },
    Equality {
        left: LearnedTerm,
        right: LearnedTerm,
    },
    Comparison {
        operator: ComparisonOperator,
        left: LearnedTerm,
        right: LearnedTerm,
        dimension: Option<LearnedTerm>,
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
    Counterfactual {
        antecedent: PropositionId,
        consequent: PropositionId,
    },
    Unless {
        condition: PropositionId,
        consequent: PropositionId,
    },
    Exists {
        variable: VariableId,
        body: PropositionId,
    },
    ForAll {
        variable: VariableId,
        body: PropositionId,
    },
    Cardinality {
        comparator: CardinalityComparator,
        count: GroundedLiteral,
        variable: VariableId,
        body: PropositionId,
    },
    /// Vague count magnitude such as "1000s"/"thousands". This is not
    /// collapsed to Approximately(1000), which would falsely create a point estimate.
    PluralScaleCardinality {
        scale: GroundedLiteral,
        variable: VariableId,
        body: PropositionId,
    },
    /// Open generalized quantifier represented by an ontology-typed lexical operator.
    GeneralizedQuantified {
        quantifier: ReferentId,
        variable: VariableId,
        body: PropositionId,
    },
    /// Source-grounded lexical propositional operator. `operator` must be a narrowest
    /// subtype of `PropositionalOperator`; its ontology sort carries the force.
    ScopedOperator {
        operator: ReferentId,
        content: PropositionId,
    },
    /// Focus/exclusive/additive force. `operator` is ontology typed; `focus` is the
    /// exact focal constituent and does not duplicate its denotation.
    Focus {
        operator: ReferentId,
        focus: LearnedAnchor,
        content: PropositionId,
    },
    /// Asserted and presupposed content remain distinct; this is not conjunction.
    Presuppositional {
        asserted: PropositionId,
        presupposed: PropositionId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedProposition {
    pub id: PropositionId,
    pub expression: LearnedPropositionExpr,
    /// Exact cue licensing the structural operator/relation when materially expressed.
    /// Atomic denotations can derive their footprint from their terms/occurrence.
    pub operator_anchor: Option<LearnedAnchor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LearnedContentBody {
    /// Directive/commissive/other propositional content. `sort` determines the
    /// ontology category (CommandContent, RequestContent, PromiseContent, ...).
    Proposition { proposition: PropositionId },
    /// Interrogative content is not a proposition. The proposition is its open
    /// condition; answer variables and explicit alternatives belong to the question.
    Question {
        condition: PropositionId,
        answer_variables: BTreeSet<VariableId>,
        alternatives: BTreeSet<PropositionId>,
    },
    /// Exact mention/quotation of source material; no truth commitment is introduced.
    Quotation { quoted: LearnedAnchor },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedContent {
    pub id: ContentId,
    /// Narrowest semantic-content ontology sort. Its source footprint is derived from `body`;
    /// there is no container/owner span the model can use to fake coverage.
    pub sort: ConceptId,
    pub body: LearnedContentBody,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum LearnedReading {
    Proposition(PropositionId),
    Content(ContentId),
}

/// An unresolved ambiguity is represented by complete competing readings. None
/// of the alternatives is asserted merely because it appears here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedAmbiguity {
    pub id: AmbiguityId,
    pub alternatives: BTreeSet<LearnedReading>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum LearnedRoot {
    Proposition(PropositionId),
    Content(ContentId),
    Ambiguity(AmbiguityId),
}

/// The entire SLM output. There is deliberately no schema/version/provenance or
/// source-envelope metadata here; those are fixed by the dataset/compiler contract.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedSemanticTarget {
    pub referents: BTreeMap<ReferentId, LearnedReferent>,
    pub occurrences: BTreeMap<OccurrenceId, LearnedOccurrence>,
    pub variables: BTreeMap<VariableId, LearnedVariable>,
    pub propositions: BTreeMap<PropositionId, LearnedProposition>,
    pub contents: BTreeMap<ContentId, LearnedContent>,
    pub ambiguities: BTreeMap<AmbiguityId, LearnedAmbiguity>,
    pub roots: Vec<LearnedRoot>,
}
