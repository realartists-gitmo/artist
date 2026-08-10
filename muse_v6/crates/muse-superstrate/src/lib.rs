//! Deterministic lowering from Muse occurrence semantics into Artist's exact
//! formal boundary and fixed DTT kernel.
//!
//! The bridge never invents source semantics. It checks ontology references,
//! canonicalizes producer-local identities, represents source presentation as
//! part of each formal proposition, and then asks the existing kernel to check
//! the mechanically generated term.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use artist_formal::{
    GraphBuilder, InterpretedGraph, Literal as FormalLiteral, ObjectId, ObjectInterpretation,
    ObjectMeaning, ObjectNode, Ontology, OntologyParameter, OntologySymbolDeclaration,
    OntologySymbolId, OntologyTypeDeclaration, OntologyTypeExpr, OntologyTypeId, Symbol,
};
use artist_kernel::{
    CompiledOntologySubmission, ElaborationRecord, ElaborationRole, OntologyCompileError,
    OntologyCompiler, Theory,
};
use muse_core::{
    ConceptId, OccurrenceId, PropositionId, ReferentId, RelationId, StatementId, VariableId,
};
use muse_occurrence::{
    AttitudeKind, CausalRelation, Literal, Modality, OccurrenceDocument, ParticipantRole,
    PropositionExpr, SemanticTarget, SpeechActKind, TemporalAnchor, Term,
    TrainingCanonicalizationError,
};
use muse_ontology::OntologyIndex;
use muse_registry::{PackageRegistry, RegistryError, RegistrySnapshot};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Deterministic semantic→formal lowering contract.
pub const LOWERING_VERSION: &str = "muse-superstrate-lowering-10";
/// Fixed base theory namespace used by the bridge.
pub const BASE_THEORY_NAMESPACE: &str = "muse.superstrate.base";
/// Fixed base theory version. Ontology snapshot identity is added to the formal ontology.
pub const BASE_THEORY_VERSION: &str = "1";
const FORMAL_NAMESPACE: &str = "muse.superstrate";
const PROPOSITION_TYPE: &str = "muse.formal.Proposition";
const AMBIGUITY_TYPE: &str = "muse.formal.Ambiguity";
const SOURCE_ANCHOR_TYPE: &str = "muse.formal.SourceAnchor";

/// Exact lowered object before kernel elaboration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoweredOccurrenceDocument {
    pub canonical: OccurrenceDocument,
    pub submission: InterpretedGraph,
    pub canonical_document_root: ObjectId,
    /// Exact deterministic map from authoritative qualified ontology concept IDs to formal ontology types.
    pub concept_types: BTreeMap<ConceptId, OntologyTypeId>,
    /// Exact deterministic map from authoritative qualified ontology relation IDs to formal ontology symbols used in this document.
    pub relation_symbols: BTreeMap<RelationId, OntologySymbolId>,
    /// Exact deterministic semantic-object mappings at the formal boundary.
    pub referent_objects: BTreeMap<ReferentId, ObjectId>,
    pub occurrence_objects: BTreeMap<OccurrenceId, ObjectId>,
    /// Structural formal proposition for each canonical occurrence identity.
    pub occurrence_roots: BTreeMap<OccurrenceId, ObjectId>,
    pub proposition_roots: BTreeMap<PropositionId, ObjectId>,
    pub statement_roots: BTreeMap<StatementId, ObjectId>,
    pub ambiguity_roots: BTreeMap<String, ObjectId>,
}

/// Kernel-checked lowering receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelCheckedOccurrenceDocument {
    pub lowered: LoweredOccurrenceDocument,
    pub compiled: CompiledOntologySubmission,
    pub statements: BTreeMap<StatementId, ElaborationRecord>,
}

/// Resolve the exact pinned ontology snapshot, lower, compile, verify, and
/// proposition-check every materially presented top-level statement.
pub fn lower_and_kernel_check(
    registry: &PackageRegistry,
    document: &OccurrenceDocument,
) -> Result<KernelCheckedOccurrenceDocument, LoweringError> {
    let index = registry.ontology_index(&document.ontology)?;
    let lowered = lower(document, &index)?;
    let base = Theory::empty(BASE_THEORY_NAMESPACE, BASE_THEORY_VERSION);
    let compiled = OntologyCompiler::compile(base, &lowered.submission)?;
    OntologyCompiler::verify_compiled(&lowered.submission, &compiled)?;
    let mut statements = BTreeMap::new();
    for (statement, root) in &lowered.statement_roots {
        let record = OntologyCompiler::elaborate(
            &lowered.submission,
            &compiled,
            root,
            ElaborationRole::Proposition,
        )?;
        OntologyCompiler::verify_record(&lowered.submission, &compiled, &record)?;
        statements.insert(statement.clone(), record);
    }
    Ok(KernelCheckedOccurrenceDocument {
        lowered,
        compiled,
        statements,
    })
}

/// Check an occurrence document against its pinned ontology snapshot and lower
/// it into the exact `artist-formal::InterpretedGraph` write boundary.
pub fn lower(
    document: &OccurrenceDocument,
    index: &OntologyIndex,
) -> Result<LoweredOccurrenceDocument, LoweringError> {
    document.validate()?;
    validate_snapshot(index, &document.ontology)?;
    let canonical = document.canonical_training_projection()?;
    let resolved = resolve_ontology_symbols(&canonical, index)?;
    validate_resolved_ontology_references(&resolved, index)?;
    let mut builder = FormalBuilder::new(&resolved, index)?;

    let canonical_json = serde_json::to_string(&canonical)?;
    let canonical_document_root = builder.quote("canonical-document", &canonical_json);
    builder
        .graph
        .root("muse/canonical-document", canonical_document_root.clone());

    let mut statement_roots = BTreeMap::new();
    for statement_id in &resolved.statement_order {
        let statement = resolved
            .statements
            .get(statement_id)
            .ok_or_else(|| LoweringError::MissingStatement(statement_id.clone()))?;
        // Presentation/provenance metadata is compiler-owned audit data, not a second semantic operator.
        // The formal statement root is the semantic proposition itself.
        let root = builder.proposition(&resolved, &statement.content)?;
        builder
            .graph
            .root(format!("statement/{}", statement_id.as_str()), root.clone());
        statement_roots.insert(statement_id.clone(), root);
    }

    let mut ambiguity_roots = BTreeMap::new();
    for (ambiguity_id, ambiguity) in &resolved.ambiguities {
        let mut values = Vec::new();
        for (index, alternative) in ambiguity.alternatives.iter().enumerate() {
            let (object, ty, _) = builder.ambiguity_alternative_typed(&resolved, alternative)?;
            values.push((format!("alternative{index}"), object, ty));
        }
        let operation = format!("muse.ambiguity/{}", values.len());
        let root = builder.apply_owned_typed_roles(
            &operation,
            values,
            OntologyTypeExpr::named(OntologyTypeId(AMBIGUITY_TYPE.into())),
        )?;
        builder
            .graph
            .root(format!("ambiguity/{}", ambiguity_id.as_str()), root.clone());
        ambiguity_roots.insert(ambiguity_id.as_str().to_owned(), root);
    }

    let graph = builder.graph.finish()?;
    let concept_types = builder.concept_types;
    let relation_symbols = builder.relation_symbols;
    let referent_objects = builder.referent_objects;
    let occurrence_objects = builder.occurrence_objects;
    let occurrence_roots = builder.occurrence_structures;
    let proposition_roots = builder.proposition_objects;
    let submission = InterpretedGraph {
        graph,
        ontology: builder.ontology,
        interpretations: builder.interpretations,
    };
    submission.validate()?;
    let diagnostics = submission.typing_diagnostics();
    if !diagnostics.is_empty() {
        return Err(LoweringError::FormalTyping(format!("{diagnostics:?}")));
    }
    Ok(LoweredOccurrenceDocument {
        canonical,
        submission,
        canonical_document_root,
        concept_types,
        relation_symbols,
        referent_objects,
        occurrence_objects,
        occurrence_roots,
        proposition_roots,
        statement_roots,
        ambiguity_roots,
    })
}

fn validate_snapshot(
    index: &OntologyIndex,
    snapshot: &RegistrySnapshot,
) -> Result<(), LoweringError> {
    if index.packages != snapshot.packages {
        return Err(LoweringError::OntologySnapshotMismatch);
    }
    Ok(())
}

fn resolve_ontology_symbols(
    document: &OccurrenceDocument,
    index: &OntologyIndex,
) -> Result<OccurrenceDocument, LoweringError> {
    fn concept(id: &ConceptId, index: &OntologyIndex) -> Result<ConceptId, LoweringError> {
        index
            .resolve_concept(id)
            .ok_or_else(|| LoweringError::UnknownConcept(id.clone()))
    }
    fn relation(id: &RelationId, index: &OntologyIndex) -> Result<RelationId, LoweringError> {
        index
            .resolve_relation(id)
            .ok_or_else(|| LoweringError::UnknownRelation(id.clone()))
    }
    fn concept_set(
        values: &BTreeSet<ConceptId>,
        index: &OntologyIndex,
    ) -> Result<BTreeSet<ConceptId>, LoweringError> {
        values.iter().map(|value| concept(value, index)).collect()
    }

    let mut resolved = document.clone();
    for referent in resolved.referents.values_mut() {
        referent.types = concept_set(&referent.types, index)?;
    }
    for variable in resolved.variables.values_mut() {
        variable.sort = concept(&variable.sort, index)?;
    }
    for occurrence in resolved.occurrences.values_mut() {
        occurrence.types = concept_set(&occurrence.types, index)?;
        for participant in &mut occurrence.participants {
            if let ParticipantRole::Ontology(id) = &mut participant.role {
                *id = relation(id, index)?;
            }
        }
        let attributes = std::mem::take(&mut occurrence.attributes);
        occurrence.attributes = attributes
            .into_iter()
            .map(|(id, values)| Ok((relation(&id, index)?, values)))
            .collect::<Result<_, LoweringError>>()?;
    }
    for proposition in resolved.propositions.values_mut() {
        match &mut proposition.expression {
            PropositionExpr::TypeAssertion { r#type, .. } => *r#type = concept(r#type, index)?,
            PropositionExpr::Relation { relation: id, .. } => *id = relation(id, index)?,
            PropositionExpr::Quantified {
                domain: Some(id), ..
            }
            | PropositionExpr::GeneralizedQuantified {
                domain: Some(id), ..
            }
            | PropositionExpr::Interrogative {
                domain: Some(id), ..
            } => *id = concept(id, index)?,
            PropositionExpr::Modal {
                modality: Modality::Ontology(id),
                ..
            } => *id = concept(id, index)?,
            PropositionExpr::Attitude {
                attitude: AttitudeKind::Ontology(id),
                ..
            } => *id = concept(id, index)?,
            PropositionExpr::SpeechAct {
                act: SpeechActKind::Ontology(id),
                ..
            } => *id = concept(id, index)?,
            PropositionExpr::Causal {
                relation: CausalRelation::Other(id),
                ..
            } => *id = relation(id, index)?,
            _ => {}
        }
    }
    for ambiguity in resolved.ambiguities.values_mut() {
        let alternatives = std::mem::take(&mut ambiguity.alternatives);
        ambiguity.alternatives = alternatives
            .into_iter()
            .map(|alternative| match alternative {
                muse_occurrence::AmbiguityAlternative::Concept(id) => Ok(
                    muse_occurrence::AmbiguityAlternative::Concept(concept(&id, index)?),
                ),
                muse_occurrence::AmbiguityAlternative::Relation(id) => Ok(
                    muse_occurrence::AmbiguityAlternative::Relation(relation(&id, index)?),
                ),
                other => Ok(other),
            })
            .collect::<Result<_, LoweringError>>()?;
    }
    Ok(resolved)
}

/// Ontology conformance enforced before formal lowering. This is deliberately
/// stricter than structural IR validation: unknown vocabulary and binary
/// relation arity errors cannot become training targets.
pub fn validate_ontology_references(
    document: &OccurrenceDocument,
    index: &OntologyIndex,
) -> Result<(), LoweringError> {
    let resolved = resolve_ontology_symbols(document, index)?;
    validate_resolved_ontology_references(&resolved, index)
}

fn validate_resolved_ontology_references(
    document: &OccurrenceDocument,
    index: &OntologyIndex,
) -> Result<(), LoweringError> {
    for referent in document.referents.values() {
        if referent.types.is_empty() {
            return Err(LoweringError::MissingOntologyType(format!(
                "referent {}",
                referent.id.as_str()
            )));
        }
        check_types(&referent.types, index)?;
        check_disjoint(&referent.types, index)?;
    }
    for variable in document.variables.values() {
        check_concept(&variable.sort, index)?;
    }
    for occurrence in document.occurrences.values() {
        if occurrence.types.is_empty() {
            return Err(LoweringError::MissingOntologyType(format!(
                "occurrence {}",
                occurrence.id.as_str()
            )));
        }
        check_types(&occurrence.types, index)?;
        check_disjoint(&occurrence.types, index)?;
        check_occurrence_type(&occurrence.types, index)?;
        for participant in &occurrence.participants {
            if let ParticipantRole::Ontology(relation) = &participant.role {
                let declaration = check_relation(relation, index)?;
                let arguments = [
                    Term::Occurrence(occurrence.id.clone()),
                    participant.value.clone(),
                ];
                check_relation_constraints(document, &arguments, declaration, index)?;
            }
            check_term(document, &participant.value, index)?;
        }
        for relation in occurrence.attributes.keys() {
            check_relation(relation, index)?;
        }
    }
    for proposition in document.propositions.values() {
        validate_expression(document, &proposition.expression, index)?;
    }
    for ambiguity in document.ambiguities.values() {
        for alternative in &ambiguity.alternatives {
            match alternative {
                muse_occurrence::AmbiguityAlternative::Referent(id) => {
                    check_term(document, &Term::Referent(id.clone()), index)?;
                }
                muse_occurrence::AmbiguityAlternative::Occurrence(id) => {
                    check_term(document, &Term::Occurrence(id.clone()), index)?;
                }
                muse_occurrence::AmbiguityAlternative::Proposition(id) => {
                    check_term(document, &Term::Proposition(id.clone()), index)?;
                }
                muse_occurrence::AmbiguityAlternative::Concept(id) => {
                    check_concept(id, index)?;
                }
                muse_occurrence::AmbiguityAlternative::Relation(id) => {
                    check_relation(id, index)?;
                }
                muse_occurrence::AmbiguityAlternative::Temporal(_)
                | muse_occurrence::AmbiguityAlternative::Literal(_) => {}
            }
        }
    }
    Ok(())
}

fn validate_expression(
    document: &OccurrenceDocument,
    expression: &PropositionExpr,
    index: &OntologyIndex,
) -> Result<(), LoweringError> {
    match expression {
        PropositionExpr::TypeAssertion { subject, r#type } => {
            check_concept(r#type, index)?;
            check_term(document, subject, index)?;
        }
        PropositionExpr::Relation {
            relation,
            arguments,
        } => {
            let declaration = check_relation(relation, index)?;
            if arguments.len() != 2 {
                return Err(LoweringError::RelationArity {
                    relation: relation.clone(),
                    actual: arguments.len(),
                });
            }
            for argument in arguments {
                check_term(document, argument, index)?;
            }
            check_relation_constraints(document, arguments, declaration, index)?;
        }
        PropositionExpr::Occurrence { occurrence } => {
            if !document.occurrences.contains_key(occurrence) {
                return Err(LoweringError::UnknownOccurrence(occurrence.clone()));
            }
        }
        PropositionExpr::Equality { left, right } => {
            check_term(document, left, index)?;
            check_term(document, right, index)?;
        }
        PropositionExpr::Comparison {
            left,
            right,
            dimension,
            ..
        } => {
            check_term(document, left, index)?;
            check_term(document, right, index)?;
            if let Some(dimension) = dimension {
                check_term(document, dimension, index)?;
            }
        }
        PropositionExpr::Quantified {
            variable, domain, ..
        } => {
            let declared = document
                .variables
                .get(variable)
                .ok_or_else(|| LoweringError::MissingVariableType(variable.clone()))?;
            if let Some(domain) = domain {
                check_concept(domain, index)?;
                if domain != &declared.sort {
                    return Err(LoweringError::VariableTypeConflict {
                        variable: variable.clone(),
                        left: declared.sort.clone(),
                        right: domain.clone(),
                    });
                }
            }
        }
        PropositionExpr::GeneralizedQuantified {
            quantifier,
            variable,
            domain,
            ..
        } => {
            check_term(document, &Term::Referent(quantifier.clone()), index)?;
            let declared = document
                .variables
                .get(variable)
                .ok_or_else(|| LoweringError::MissingVariableType(variable.clone()))?;
            if let Some(domain) = domain {
                check_concept(domain, index)?;
                if domain != &declared.sort {
                    return Err(LoweringError::VariableTypeConflict {
                        variable: variable.clone(),
                        left: declared.sort.clone(),
                        right: domain.clone(),
                    });
                }
            }
        }
        PropositionExpr::Interrogative {
            variable, domain, ..
        } => match (variable, domain) {
            (Some(_variable), Some(domain)) => {
                check_concept(domain, index)?;
            }
            (Some(variable), None) => {
                return Err(LoweringError::MissingVariableType(variable.clone()));
            }
            (None, Some(domain)) => {
                check_concept(domain, index)?;
            }
            (None, None) => {}
        },
        PropositionExpr::ScopedOperator { operator, .. } => {
            check_term(document, &Term::Referent(operator.clone()), index)?;
            let referent = document
                .referents
                .get(operator)
                .ok_or_else(|| LoweringError::UnknownReferent(operator.clone()))?;
            let required = index
                .resolve_concept(&ConceptId::from("PropositionalOperator"))
                .ok_or_else(|| {
                    LoweringError::UnknownConcept(ConceptId::from("PropositionalOperator"))
                })?;
            if !referent
                .types
                .iter()
                .any(|actual| is_subtype(actual, &required, index))
            {
                return Err(LoweringError::MissingOntologyType(format!(
                    "scoped operator {} is not a PropositionalOperator",
                    operator.as_str()
                )));
            }
        }
        PropositionExpr::Modal { modality, .. } => check_modality(modality, index)?,
        PropositionExpr::Focus { operator, .. } => {
            check_term(document, &Term::Referent(operator.clone()), index)?;
        }
        PropositionExpr::Capability { bearer, .. } => check_term(document, bearer, index)?,
        PropositionExpr::Attitude {
            holder, attitude, ..
        } => {
            check_term(document, holder, index)?;
            check_attitude(attitude, index)?;
        }
        PropositionExpr::SpeechAct {
            speaker,
            act,
            addressees,
            ..
        } => {
            check_term(document, speaker, index)?;
            for addressee in addressees {
                check_term(document, addressee, index)?;
            }
            check_speech(act, index)?;
        }
        PropositionExpr::Causal { relation, .. } => check_causal(relation, index)?,
        PropositionExpr::Negation { .. }
        | PropositionExpr::Conjunction { .. }
        | PropositionExpr::Disjunction { .. }
        | PropositionExpr::Implication { .. }
        | PropositionExpr::Counterfactual { .. }
        | PropositionExpr::Unless { .. }
        | PropositionExpr::Generic { .. }
        | PropositionExpr::Perfect { .. }
        | PropositionExpr::Progressive { .. }
        | PropositionExpr::Presuppositional { .. }
        | PropositionExpr::Phase { .. }
        | PropositionExpr::Temporal { .. }
        | PropositionExpr::Quotation { .. } => {}
    }
    Ok(())
}

fn check_term(
    document: &OccurrenceDocument,
    term: &Term,
    index: &OntologyIndex,
) -> Result<(), LoweringError> {
    match term {
        Term::Referent(id) => {
            let value = document
                .referents
                .get(id)
                .ok_or_else(|| LoweringError::UnknownReferent(id.clone()))?;
            check_types(&value.types, index)
        }
        Term::Occurrence(id) => {
            let value = document
                .occurrences
                .get(id)
                .ok_or_else(|| LoweringError::UnknownOccurrence(id.clone()))?;
            check_types(&value.types, index)
        }
        Term::Proposition(id) => {
            if document.propositions.contains_key(id) {
                Ok(())
            } else {
                Err(LoweringError::UnknownProposition(id.clone()))
            }
        }
        Term::Variable(id) => document
            .variables
            .contains_key(id)
            .then_some(())
            .ok_or_else(|| LoweringError::MissingVariableType(id.clone())),
        Term::Literal(_) => Ok(()),
    }
}

fn check_types(types: &BTreeSet<ConceptId>, index: &OntologyIndex) -> Result<(), LoweringError> {
    for ty in types {
        check_concept(ty, index)?;
    }
    Ok(())
}

fn check_concept<'a>(
    concept: &ConceptId,
    index: &'a OntologyIndex,
) -> Result<&'a muse_ontology::ConceptDeclaration, LoweringError> {
    index
        .concepts
        .get(concept)
        .ok_or_else(|| LoweringError::UnknownConcept(concept.clone()))
}

fn check_relation<'a>(
    relation: &RelationId,
    index: &'a OntologyIndex,
) -> Result<&'a muse_ontology::RelationDeclaration, LoweringError> {
    index
        .relations
        .get(relation)
        .ok_or_else(|| LoweringError::UnknownRelation(relation.clone()))
}

fn check_disjoint(types: &BTreeSet<ConceptId>, index: &OntologyIndex) -> Result<(), LoweringError> {
    let values = types.iter().collect::<Vec<_>>();
    for (position, left) in values.iter().enumerate() {
        for right in values.iter().skip(position + 1) {
            let pair = if left <= right {
                ((*left).clone(), (*right).clone())
            } else {
                ((*right).clone(), (*left).clone())
            };
            if index.disjoint_pairs.contains(&pair) {
                return Err(LoweringError::DisjointTypes {
                    left: pair.0,
                    right: pair.1,
                });
            }
        }
    }
    Ok(())
}

fn check_relation_constraints(
    document: &OccurrenceDocument,
    arguments: &[Term],
    declaration: &muse_ontology::RelationDeclaration,
    index: &OntologyIndex,
) -> Result<(), LoweringError> {
    // Multiple declared domains/ranges are alternatives (a union of licensed
    // classes), not an accidental intersection requirement. This is the only
    // interpretation that permits a relation such as Action-or-Intention -> X.
    if let Some(subject) = term_types(document, &arguments[0]) {
        if !declaration.domains.is_empty()
            && !declaration.domains.iter().any(|required| {
                subject
                    .iter()
                    .any(|actual| is_subtype(actual, required, index))
            })
        {
            return Err(LoweringError::RelationDomain {
                relation: declaration.id.clone(),
                allowed: declaration.domains.clone(),
            });
        }
    }
    if let Some(object) = term_types(document, &arguments[1]) {
        if !declaration.ranges.is_empty()
            && !declaration.ranges.iter().any(|required| {
                object
                    .iter()
                    .any(|actual| is_subtype(actual, required, index))
            })
        {
            return Err(LoweringError::RelationRange {
                relation: declaration.id.clone(),
                allowed: declaration.ranges.clone(),
            });
        }
    }
    Ok(())
}

fn literal_type(value: &Literal) -> ConceptId {
    ConceptId::from(match value {
        Literal::String(_) => "comp:StringValue",
        Literal::Integer(_) => "comp:IntegerValue",
        Literal::Decimal(_) => "comp:DecimalValue",
        Literal::Boolean(_) => "comp:BooleanValue",
        Literal::Null => "comp:DataValue",
        Literal::Json(_) => "comp:StructuredDataValue",
        Literal::Ratio { .. } => "comp:RatioValue",
        Literal::Percentage { .. } => "comp:PercentageValue",
        Literal::Approximate { .. } => "comp:ApproximateNumericValue",
        Literal::Interval { .. } => "comp:NumericInterval",
        Literal::PluralScale { .. } => "comp:PluralScaleValue",
        Literal::Arithmetic { operator, .. } => match operator {
            muse_occurrence::ArithmeticOperator::Add => "comp:AdditionExpressionValue",
            muse_occurrence::ArithmeticOperator::Subtract => "comp:SubtractionExpressionValue",
            muse_occurrence::ArithmeticOperator::Multiply => "comp:MultiplicationExpressionValue",
            muse_occurrence::ArithmeticOperator::Divide => "comp:DivisionExpressionValue",
        },
        Literal::Measurement { .. } => "comp:MeasurementValue",
    })
}

fn term_types(document: &OccurrenceDocument, term: &Term) -> Option<BTreeSet<ConceptId>> {
    match term {
        Term::Referent(id) => document
            .referents
            .get(id)
            .map(|value| value.types.clone())
            .filter(|v| !v.is_empty()),
        Term::Occurrence(id) => document
            .occurrences
            .get(id)
            .map(|value| value.types.clone())
            .filter(|v| !v.is_empty()),
        Term::Literal(value) => Some(BTreeSet::from([literal_type(value)])),
        Term::Variable(id) => document
            .variables
            .get(id)
            .map(|variable| BTreeSet::from([variable.sort.clone()])),
        Term::Proposition(id) => document
            .propositions
            .contains_key(id)
            .then(|| BTreeSet::from([ConceptId::from("ufo:Proposition")])),
    }
}

fn is_subtype(actual: &ConceptId, required: &ConceptId, index: &OntologyIndex) -> bool {
    actual == required
        || index
            .ancestors
            .get(actual)
            .is_some_and(|ancestors| ancestors.contains(required))
}

fn check_occurrence_type(
    types: &BTreeSet<ConceptId>,
    index: &OntologyIndex,
) -> Result<(), LoweringError> {
    let event = ConceptId::from("ufo:Event");
    let situation = ConceptId::from("ufo:Situation");
    if !index.concepts.contains_key(&event) {
        return Err(LoweringError::UnknownConcept(event));
    }
    if !index.concepts.contains_key(&situation) {
        return Err(LoweringError::UnknownConcept(situation));
    }
    if types
        .iter()
        .any(|actual| is_subtype(actual, &event, index) || is_subtype(actual, &situation, index))
    {
        Ok(())
    } else {
        Err(LoweringError::InvalidOccurrenceOntologyType(
            types.iter().cloned().collect(),
        ))
    }
}

fn check_modality(value: &Modality, index: &OntologyIndex) -> Result<(), LoweringError> {
    if let Modality::Ontology(id) = value {
        check_concept(id, index)?;
    }
    Ok(())
}
fn check_attitude(value: &AttitudeKind, index: &OntologyIndex) -> Result<(), LoweringError> {
    if let AttitudeKind::Ontology(id) = value {
        check_concept(id, index)?;
    }
    Ok(())
}
fn check_speech(value: &SpeechActKind, index: &OntologyIndex) -> Result<(), LoweringError> {
    if let SpeechActKind::Ontology(id) = value {
        check_concept(id, index)?;
    }
    Ok(())
}
fn check_causal(value: &CausalRelation, index: &OntologyIndex) -> Result<(), LoweringError> {
    if let CausalRelation::Other(id) = value {
        check_relation(id, index)?;
    }
    Ok(())
}

struct FormalBuilder {
    graph: GraphBuilder,
    ontology: Ontology,
    interpretations: BTreeMap<ObjectId, ObjectInterpretation>,
    symbols: BTreeMap<String, ObjectId>,
    concept_types: BTreeMap<ConceptId, OntologyTypeId>,
    relation_symbols: BTreeMap<RelationId, OntologySymbolId>,
    referent_objects: BTreeMap<ReferentId, ObjectId>,
    occurrence_objects: BTreeMap<OccurrenceId, ObjectId>,
    occurrence_structures: BTreeMap<OccurrenceId, ObjectId>,
    proposition_objects: BTreeMap<PropositionId, ObjectId>,
    variable_types: BTreeMap<VariableId, ConceptId>,
    variable_objects: BTreeMap<VariableId, ObjectId>,
    next: u64,
}

impl FormalBuilder {
    fn new(document: &OccurrenceDocument, index: &OntologyIndex) -> Result<Self, LoweringError> {
        let mut types = BTreeMap::new();
        types.insert(
            OntologyTypeId(PROPOSITION_TYPE.into()),
            OntologyTypeDeclaration {
                id: OntologyTypeId(PROPOSITION_TYPE.into()),
                universe: 1,
                description: Some("Muse source-presented proposition".into()),
            },
        );
        types.insert(
            OntologyTypeId(AMBIGUITY_TYPE.into()),
            OntologyTypeDeclaration {
                id: OntologyTypeId(AMBIGUITY_TYPE.into()),
                universe: 0,
                description: Some("Explicit unresolved source ambiguity record".into()),
            },
        );
        types.insert(
            OntologyTypeId(SOURCE_ANCHOR_TYPE.into()),
            OntologyTypeDeclaration {
                id: OntologyTypeId(SOURCE_ANCHOR_TYPE.into()),
                universe: 0,
                description: Some(
                    "Exact source constituent selector used by semantic operators such as focus"
                        .into(),
                ),
            },
        );
        let mut concept_types = BTreeMap::new();
        for concept in index.concepts.keys() {
            let id = OntologyTypeId(format!("muse.concept/{}", concept.as_str()));
            concept_types.insert(concept.clone(), id.clone());
            types.insert(
                id.clone(),
                OntologyTypeDeclaration {
                    id,
                    universe: 0,
                    description: Some(concept.as_str().to_owned()),
                },
            );
        }
        let variable_types = document
            .variables
            .iter()
            .map(|(id, variable)| (id.clone(), variable.sort.clone()))
            .collect::<BTreeMap<_, _>>();
        for proposition in document.propositions.values() {
            match &proposition.expression {
                PropositionExpr::Quantified {
                    variable,
                    domain: Some(domain),
                    ..
                }
                | PropositionExpr::Interrogative {
                    variable: Some(variable),
                    domain: Some(domain),
                    ..
                } => {
                    let previous = variable_types
                        .get(variable)
                        .ok_or_else(|| LoweringError::MissingVariableType(variable.clone()))?;
                    if previous != domain {
                        return Err(LoweringError::VariableTypeConflict {
                            variable: variable.clone(),
                            left: previous.clone(),
                            right: domain.clone(),
                        });
                    }
                }
                _ => {}
            }
        }
        let ontology = Ontology {
            namespace: FORMAL_NAMESPACE.into(),
            version: format!("{LOWERING_VERSION}+snapshot.{}", document.ontology.identity),
            proposition_type: OntologyTypeId(PROPOSITION_TYPE.into()),
            types,
            symbols: BTreeMap::new(),
        };
        let mut builder = Self {
            graph: GraphBuilder::new(),
            ontology,
            interpretations: BTreeMap::new(),
            symbols: BTreeMap::new(),
            concept_types,
            relation_symbols: BTreeMap::new(),
            referent_objects: BTreeMap::new(),
            occurrence_objects: BTreeMap::new(),
            occurrence_structures: BTreeMap::new(),
            proposition_objects: BTreeMap::new(),
            variable_types,
            variable_objects: BTreeMap::new(),
            next: 0,
        };
        for (id, referent) in &document.referents {
            let object = builder.typed_instance(
                &format!("referent/{}", id.as_str()),
                id.as_str(),
                &referent.types,
            )?;
            builder.referent_objects.insert(id.clone(), object);
        }
        for (id, occurrence) in &document.occurrences {
            let object = builder.typed_instance(
                &format!("occurrence/{}", id.as_str()),
                id.as_str(),
                &occurrence.types,
            )?;
            builder.occurrence_objects.insert(id.clone(), object);
        }
        Ok(builder)
    }

    fn quote(&mut self, label: &str, exact: &str) -> ObjectId {
        let id = ObjectId::new(format!("q/{}/{}", self.next, sanitize(label)));
        self.next += 1;
        self.graph.insert(
            id.clone(),
            ObjectNode::new().with_property("json", FormalLiteral::Text(exact.to_owned())),
        );
        self.interpretations.insert(
            id.clone(),
            ObjectInterpretation {
                ty: OntologyTypeExpr::QuotedObject,
                meaning: ObjectMeaning::Quoted,
            },
        );
        id
    }

    fn source_anchor(&mut self, anchor: &muse_occurrence::LexicalAnchor) -> ObjectId {
        let id = ObjectId::new(format!("source-anchor/{}", self.next));
        self.next += 1;
        let mut node = ObjectNode::new();
        for (index, span) in anchor.spans.iter().enumerate() {
            node = node.with_property(
                format!("span{index}"),
                FormalLiteral::Text(span.as_str().to_owned()),
            );
        }
        self.graph.insert(id.clone(), node);
        self.interpretations.insert(
            id.clone(),
            ObjectInterpretation {
                ty: OntologyTypeExpr::named(OntologyTypeId(SOURCE_ANCHOR_TYPE.into())),
                meaning: ObjectMeaning::Quoted,
            },
        );
        id
    }

    fn exact_type_for_concepts(
        &mut self,
        concepts: &BTreeSet<ConceptId>,
    ) -> Result<(OntologyTypeExpr, String), LoweringError> {
        if concepts.is_empty() {
            return Err(LoweringError::MissingOntologyType("formal instance".into()));
        }
        if concepts.len() == 1 {
            let concept = concepts
                .iter()
                .next()
                .ok_or_else(|| LoweringError::MissingOntologyType("formal instance".into()))?;
            let id = self
                .concept_types
                .get(concept)
                .cloned()
                .ok_or_else(|| LoweringError::UnknownConcept(concept.clone()))?;
            let class = sanitize(&id.0);
            return Ok((OntologyTypeExpr::named(id), class));
        }
        let joined = concepts
            .iter()
            .map(|value| sanitize(value.as_str()))
            .collect::<Vec<_>>()
            .join("__");
        let id = OntologyTypeId(format!("muse.intersection/{joined}"));
        self.ontology
            .types
            .entry(id.clone())
            .or_insert_with(|| OntologyTypeDeclaration {
                id: id.clone(),
                universe: 0,
                description: Some(format!(
                    "Intersection of {}",
                    concepts
                        .iter()
                        .map(|value| value.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            });
        let class = sanitize(&id.0);
        Ok((OntologyTypeExpr::named(id), class))
    }

    fn typed_instance(
        &mut self,
        label: &str,
        exact: &str,
        concepts: &BTreeSet<ConceptId>,
    ) -> Result<ObjectId, LoweringError> {
        let source = self.quote(&format!("{label}/source"), exact);
        let (ty, class) = self.exact_type_for_concepts(concepts)?;
        let operation = format!("muse.instance/{class}");
        self.apply_typed(
            &operation,
            vec![("source", source, OntologyTypeExpr::QuotedObject)],
            ty,
        )
    }

    fn object_type_and_class(
        &self,
        object: &ObjectId,
    ) -> Result<(OntologyTypeExpr, String), LoweringError> {
        let ty = self
            .interpretations
            .get(object)
            .ok_or_else(|| LoweringError::Internal(format!("missing interpretation for {object}")))?
            .ty
            .clone();
        let class = match &ty {
            OntologyTypeExpr::Named { id } => sanitize(&id.0),
            OntologyTypeExpr::QuotedObject => "quoted".into(),
            OntologyTypeExpr::Universe { level } => format!("universe_{level}"),
            OntologyTypeExpr::Function { .. } => "function".into(),
        };
        Ok((ty, class))
    }

    fn variable_object(
        &mut self,
        variable: &VariableId,
    ) -> Result<(ObjectId, OntologyTypeExpr, String), LoweringError> {
        if let Some(object) = self.variable_objects.get(variable).cloned() {
            let (ty, class) = self.object_type_and_class(&object)?;
            return Ok((object, ty, class));
        }
        let concept = self
            .variable_types
            .get(variable)
            .cloned()
            .ok_or_else(|| LoweringError::MissingVariableType(variable.clone()))?;
        let concepts = BTreeSet::from([concept]);
        let object = self.typed_instance(
            &format!("variable/{}", variable.as_str()),
            variable.as_str(),
            &concepts,
        )?;
        self.variable_objects
            .insert(variable.clone(), object.clone());
        let (ty, class) = self.object_type_and_class(&object)?;
        Ok((object, ty, class))
    }

    fn declare(
        &mut self,
        id: &str,
        parameters: Vec<(&str, OntologyTypeExpr)>,
        result: OntologyTypeExpr,
        description: &str,
    ) -> Result<ObjectId, LoweringError> {
        if let Some(existing) = self.symbols.get(id) {
            return Ok(existing.clone());
        }
        let symbol_id = OntologySymbolId(id.to_owned());
        let declaration = OntologySymbolDeclaration {
            id: symbol_id.clone(),
            parameters: parameters
                .into_iter()
                .map(|(role, ty)| OntologyParameter {
                    role: Symbol::new(role),
                    ty,
                })
                .collect(),
            result,
            description: Some(description.to_owned()),
        };
        let full_type = declaration.full_type();
        self.ontology.symbols.insert(symbol_id.clone(), declaration);
        let object = ObjectId::new(format!("symbol/{}", sanitize(id)));
        self.graph.insert(object.clone(), ObjectNode::new());
        self.interpretations.insert(
            object.clone(),
            ObjectInterpretation {
                ty: full_type,
                meaning: ObjectMeaning::Symbol { id: symbol_id },
            },
        );
        self.symbols.insert(id.to_owned(), object.clone());
        Ok(object)
    }

    fn apply(
        &mut self,
        operation: &str,
        arguments: Vec<(&str, ObjectId)>,
        result: OntologyTypeExpr,
    ) -> Result<ObjectId, LoweringError> {
        let symbol_id = OntologySymbolId(operation.to_owned());
        let declaration = self.ontology.symbols.get(&symbol_id).cloned();
        let operator = if let Some(declaration) = declaration {
            if declaration.parameters.len() != arguments.len() {
                return Err(LoweringError::Internal(format!(
                    "operation arity changed: {operation}"
                )));
            }
            self.symbols[operation].clone()
        } else {
            let params = arguments
                .iter()
                .map(|(role, _)| (*role, OntologyTypeExpr::QuotedObject))
                .collect();
            self.declare(
                operation,
                params,
                result.clone(),
                "Generated Muse structural operator",
            )?
        };
        let mut node = ObjectNode::new().with_operator(operator);
        let roles = arguments
            .iter()
            .map(|(role, _)| Symbol::new(*role))
            .collect::<Vec<_>>();
        for (role, argument) in arguments {
            node = node.with_edge(role, argument);
        }
        let id = ObjectId::new(format!("a/{}", self.next));
        self.next += 1;
        self.graph.insert(id.clone(), node);
        self.interpretations.insert(
            id.clone(),
            ObjectInterpretation {
                ty: result,
                meaning: ObjectMeaning::Application { arguments: roles },
            },
        );
        Ok(id)
    }

    fn apply_typed(
        &mut self,
        operation: &str,
        arguments: Vec<(&str, ObjectId, OntologyTypeExpr)>,
        result: OntologyTypeExpr,
    ) -> Result<ObjectId, LoweringError> {
        if !self
            .ontology
            .symbols
            .contains_key(&OntologySymbolId(operation.to_owned()))
        {
            self.declare(
                operation,
                arguments.iter().map(|(r, _, t)| (*r, t.clone())).collect(),
                result.clone(),
                "Muse structural semantic operator",
            )?;
        }
        self.apply(
            operation,
            arguments.into_iter().map(|(r, o, _)| (r, o)).collect(),
            result,
        )
    }

    fn proposition(
        &mut self,
        document: &OccurrenceDocument,
        id: &PropositionId,
    ) -> Result<ObjectId, LoweringError> {
        if let Some(existing) = self.proposition_objects.get(id) {
            return Ok(existing.clone());
        }
        let proposition_value = document
            .propositions
            .get(id)
            .ok_or_else(|| LoweringError::UnknownProposition(id.clone()))?;
        let expr = &proposition_value.expression;
        let object = match expr {
            PropositionExpr::TypeAssertion { subject, r#type } => {
                let (term, term_type, term_class) = self.term_typed(document, subject)?;
                let op = format!(
                    "muse.type_assertion/{}/{}",
                    sanitize(r#type.as_str()),
                    term_class
                );
                self.apply_typed(&op, vec![("subject", term, term_type)], proposition())
            }
            PropositionExpr::Relation {
                relation,
                arguments,
            } => {
                let mut values = Vec::new();
                let mut classes = Vec::new();
                for (position, term) in arguments.iter().enumerate() {
                    let (object, ty, class) = self.term_typed(document, term)?;
                    classes.push(class);
                    values.push((format!("arg{position}"), object, ty));
                }
                let op = format!(
                    "muse.relation/{}/{}",
                    sanitize(relation.as_str()),
                    classes.join("_")
                );
                let object = self.apply_owned_typed_roles(&op, values, proposition())?;
                self.relation_symbols
                    .insert(relation.clone(), OntologySymbolId(op));
                Ok(object)
            }
            PropositionExpr::Occurrence { occurrence } => {
                self.occurrence_proposition(document, occurrence)
            }
            PropositionExpr::Equality { left, right } => {
                let (left, left_type, left_class) = self.term_typed(document, left)?;
                let (right, right_type, right_class) = self.term_typed(document, right)?;
                let operation = format!("muse.equality/{left_class}/{right_class}");
                self.apply_typed(
                    &operation,
                    vec![("left", left, left_type), ("right", right, right_type)],
                    proposition(),
                )
            }
            PropositionExpr::Comparison {
                operator,
                left,
                right,
                dimension,
            } => {
                let (left, left_type, left_class) = self.term_typed(document, left)?;
                let (right, right_type, right_class) = self.term_typed(document, right)?;
                let op_name = match operator {
                    muse_occurrence::ComparisonOperator::LessThan => "less_than",
                    muse_occurrence::ComparisonOperator::LessOrEqual => "less_or_equal",
                    muse_occurrence::ComparisonOperator::GreaterThan => "greater_than",
                    muse_occurrence::ComparisonOperator::GreaterOrEqual => "greater_or_equal",
                };
                let mut values = vec![
                    ("left".to_owned(), left, left_type),
                    ("right".to_owned(), right, right_type),
                ];
                let mut classes = vec![left_class, right_class];
                if let Some(dimension) = dimension {
                    let (object, ty, class) = self.term_typed(document, dimension)?;
                    values.push(("dimension".to_owned(), object, ty));
                    classes.push(class);
                }
                self.apply_owned_typed_roles(
                    &format!("muse.comparison/{op_name}/{}", classes.join("_")),
                    values,
                    proposition(),
                )
            }
            PropositionExpr::Negation { content } => {
                self.unary_prop(document, "muse.negation", content)
            }
            PropositionExpr::Conjunction { members } => self.nary_props(
                document,
                &format!("muse.conjunction/{}", members.len()),
                members,
            ),
            PropositionExpr::Disjunction { members } => self.nary_props(
                document,
                &format!("muse.disjunction/{}", members.len()),
                members,
            ),
            PropositionExpr::Implication {
                antecedent,
                consequent,
            } => {
                let a = self.proposition(document, antecedent)?;
                let c = self.proposition(document, consequent)?;
                self.apply_typed(
                    "muse.implication",
                    vec![
                        ("antecedent", a, proposition()),
                        ("consequent", c, proposition()),
                    ],
                    proposition(),
                )
            }
            PropositionExpr::Counterfactual {
                antecedent,
                consequent,
            } => {
                let a = self.proposition(document, antecedent)?;
                let c = self.proposition(document, consequent)?;
                self.apply_typed(
                    "muse.counterfactual",
                    vec![
                        ("antecedent", a, proposition()),
                        ("consequent", c, proposition()),
                    ],
                    proposition(),
                )
            }
            PropositionExpr::Unless {
                condition,
                consequent,
            } => {
                let condition = self.proposition(document, condition)?;
                let consequent = self.proposition(document, consequent)?;
                self.apply_typed(
                    "muse.unless",
                    vec![
                        ("condition", condition, proposition()),
                        ("consequent", consequent, proposition()),
                    ],
                    proposition(),
                )
            }
            PropositionExpr::Quantified {
                quantifier,
                variable,
                domain: _,
                body,
            } => {
                let (variable, variable_type, variable_class) = self.variable_object(variable)?;
                let body = self.proposition(document, body)?;
                match quantifier {
                    muse_occurrence::Quantifier::Exists => self.apply_typed(
                        &format!("muse.quantifier/exists/{variable_class}"),
                        vec![
                            ("variable", variable, variable_type),
                            ("body", body, proposition()),
                        ],
                        proposition(),
                    ),
                    muse_occurrence::Quantifier::ForAll => self.apply_typed(
                        &format!("muse.quantifier/forall/{variable_class}"),
                        vec![
                            ("variable", variable, variable_type),
                            ("body", body, proposition()),
                        ],
                        proposition(),
                    ),
                    muse_occurrence::Quantifier::Exactly(value)
                    | muse_occurrence::Quantifier::AtLeast(value)
                    | muse_occurrence::Quantifier::AtMost(value)
                    | muse_occurrence::Quantifier::MoreThan(value)
                    | muse_occurrence::Quantifier::FewerThan(value)
                    | muse_occurrence::Quantifier::Approximately(value)
                    | muse_occurrence::Quantifier::PluralScale(value) => {
                        let kind = match quantifier {
                            muse_occurrence::Quantifier::Exactly(_) => "exactly",
                            muse_occurrence::Quantifier::AtLeast(_) => "at_least",
                            muse_occurrence::Quantifier::AtMost(_) => "at_most",
                            muse_occurrence::Quantifier::MoreThan(_) => "more_than",
                            muse_occurrence::Quantifier::FewerThan(_) => "fewer_than",
                            muse_occurrence::Quantifier::Approximately(_) => "approximately",
                            muse_occurrence::Quantifier::PluralScale(_) => "plural_scale",
                            _ => unreachable!(),
                        };
                        let (count, count_type, count_class) = self.literal_typed(value)?;
                        self.apply_typed(
                            &format!("muse.quantifier/{kind}/{variable_class}/{count_class}"),
                            vec![
                                ("count", count, count_type),
                                ("variable", variable, variable_type),
                                ("body", body, proposition()),
                            ],
                            proposition(),
                        )
                    }
                    muse_occurrence::Quantifier::SourceAnchored(anchor) => {
                        let anchor =
                            self.quote("legacy-quantifier-anchor", &serde_json::to_string(anchor)?);
                        self.apply_typed(
                            &format!("muse.quantifier/legacy_lexical/{variable_class}"),
                            vec![
                                ("operator", anchor, OntologyTypeExpr::QuotedObject),
                                ("variable", variable, variable_type),
                                ("body", body, proposition()),
                            ],
                            proposition(),
                        )
                    }
                }
            }
            PropositionExpr::GeneralizedQuantified {
                quantifier,
                variable,
                domain: _,
                body,
            } => {
                let (operator, operator_type, operator_class) =
                    self.term_typed(document, &Term::Referent(quantifier.clone()))?;
                let (variable, variable_type, variable_class) = self.variable_object(variable)?;
                let body = self.proposition(document, body)?;
                self.apply_typed(
                    &format!("muse.generalized_quantifier/{operator_class}/{variable_class}"),
                    vec![
                        ("operator", operator, operator_type),
                        ("variable", variable, variable_type),
                        ("body", body, proposition()),
                    ],
                    proposition(),
                )
            }
            PropositionExpr::ScopedOperator { operator, content } => {
                let (operator, operator_type, operator_class) =
                    self.term_typed(document, &Term::Referent(operator.clone()))?;
                let content = self.proposition(document, content)?;
                self.apply_typed(
                    &format!("muse.scoped_operator/{operator_class}"),
                    vec![
                        ("operator", operator, operator_type),
                        ("content", content, proposition()),
                    ],
                    proposition(),
                )
            }
            PropositionExpr::Modal { modality, content } => {
                let operation = match modality {
                    Modality::Possible => "muse.modal/possible".to_owned(),
                    Modality::Necessary => "muse.modal/necessary".to_owned(),
                    Modality::Permitted => "muse.modal/permitted".to_owned(),
                    Modality::Obligatory => "muse.modal/obligatory".to_owned(),
                    Modality::Prohibited => "muse.modal/prohibited".to_owned(),
                    Modality::Ontology(concept) => {
                        format!("muse.modal/ontology/{}", sanitize(concept.as_str()))
                    }
                    Modality::SourceAnchored(_) => "muse.modal/legacy_lexical".to_owned(),
                };
                self.unary_prop(document, &operation, content)
            }
            PropositionExpr::Generic { content } => {
                self.unary_prop(document, "muse.generic", content)
            }
            PropositionExpr::Perfect { content } => {
                self.unary_prop(document, "muse.aspect/perfect", content)
            }
            PropositionExpr::Progressive { content } => {
                self.unary_prop(document, "muse.aspect/progressive", content)
            }
            PropositionExpr::Focus {
                operator,
                focus,
                content,
            } => {
                let (operator, operator_type, operator_class) =
                    self.term_typed(document, &Term::Referent(operator.clone()))?;
                let focus = self.source_anchor(focus);
                let content = self.proposition(document, content)?;
                self.apply_typed(
                    &format!("muse.focus/{operator_class}"),
                    vec![
                        ("operator", operator, operator_type),
                        (
                            "focused_source",
                            focus,
                            OntologyTypeExpr::named(OntologyTypeId(SOURCE_ANCHOR_TYPE.into())),
                        ),
                        ("content", content, proposition()),
                    ],
                    proposition(),
                )
            }
            PropositionExpr::Presuppositional {
                asserted,
                presupposed,
            } => {
                let asserted = self.proposition(document, asserted)?;
                let presupposed = self.proposition(document, presupposed)?;
                self.apply_typed(
                    "muse.presuppositional",
                    vec![
                        ("asserted", asserted, proposition()),
                        ("presupposed", presupposed, proposition()),
                    ],
                    proposition(),
                )
            }
            PropositionExpr::Capability { bearer, content } => {
                let (bearer, bearer_type, bearer_class) = self.term_typed(document, bearer)?;
                let content = self.proposition(document, content)?;
                let operation = format!("muse.capability/{bearer_class}");
                self.apply_typed(
                    &operation,
                    vec![
                        ("bearer", bearer, bearer_type),
                        ("content", content, proposition()),
                    ],
                    proposition(),
                )
            }
            PropositionExpr::Interrogative {
                interrogative,
                variable,
                domain: _,
                body,
            } => {
                let metadata = self.quote("interrogative", &serde_json::to_string(interrogative)?);
                let body = self.proposition(document, body)?;
                if let Some(variable) = variable {
                    let (variable, variable_type, variable_class) =
                        self.variable_object(variable)?;
                    let operation = format!("muse.interrogative/{variable_class}");
                    self.apply_typed(
                        &operation,
                        vec![
                            ("metadata", metadata, OntologyTypeExpr::QuotedObject),
                            ("variable", variable, variable_type),
                            ("body", body, proposition()),
                        ],
                        proposition(),
                    )
                } else {
                    self.apply_typed(
                        "muse.interrogative/closed",
                        vec![
                            ("metadata", metadata, OntologyTypeExpr::QuotedObject),
                            ("body", body, proposition()),
                        ],
                        proposition(),
                    )
                }
            }
            PropositionExpr::Phase { phase, content } => {
                self.metadata_prop(document, "muse.phase", phase, content)
            }
            PropositionExpr::Attitude {
                holder,
                attitude,
                content,
            } => {
                let (holder, holder_type, holder_class) = self.term_typed(document, holder)?;
                let meta = self.quote("attitude", &serde_json::to_string(attitude)?);
                let content = self.proposition(document, content)?;
                let operation = format!("muse.attitude/{holder_class}");
                self.apply_typed(
                    &operation,
                    vec![
                        ("holder", holder, holder_type),
                        ("attitude", meta, OntologyTypeExpr::QuotedObject),
                        ("content", content, proposition()),
                    ],
                    proposition(),
                )
            }
            PropositionExpr::SpeechAct {
                speaker,
                act,
                addressees,
                content,
            } => {
                let (speaker, speaker_type, speaker_class) = self.term_typed(document, speaker)?;
                let mut values = vec![
                    ("speaker".to_owned(), speaker, speaker_type),
                    (
                        "act".to_owned(),
                        self.quote("speech-act", &serde_json::to_string(act)?),
                        OntologyTypeExpr::QuotedObject,
                    ),
                ];
                let mut classes = vec![speaker_class];
                for (index, addressee) in addressees.iter().enumerate() {
                    let (object, ty, class) = self.term_typed(document, addressee)?;
                    classes.push(class);
                    values.push((format!("addressee{index}"), object, ty));
                }
                values.push((
                    "content".to_owned(),
                    self.proposition(document, content)?,
                    proposition(),
                ));
                self.apply_owned_typed_roles(
                    &format!("muse.speech_act/{}/{}", addressees.len(), classes.join("_")),
                    values,
                    proposition(),
                )
            }
            PropositionExpr::Temporal {
                subject,
                relation,
                object,
            } => {
                let relation_object =
                    self.quote("temporal-relation", &serde_json::to_string(relation)?);
                let (subject_object, subject_type, subject_class) =
                    self.semantic_target_typed(document, subject)?;
                let (anchor_object, anchor_type, anchor_class) =
                    self.temporal_anchor_typed(document, object)?;
                let operation = format!("muse.temporal/{subject_class}/{anchor_class}");
                self.apply_typed(
                    &operation,
                    vec![
                        ("relation", relation_object, OntologyTypeExpr::QuotedObject),
                        ("subject", subject_object, subject_type),
                        ("object", anchor_object, anchor_type),
                    ],
                    proposition(),
                )
            }
            PropositionExpr::Causal {
                relation,
                cause,
                effect,
            } => {
                let relation_object =
                    self.quote("causal-relation", &serde_json::to_string(relation)?);
                let (cause_object, cause_type, cause_class) =
                    self.semantic_target_typed(document, cause)?;
                let (effect_object, effect_type, effect_class) =
                    self.semantic_target_typed(document, effect)?;
                let operation = format!("muse.causal/{cause_class}/{effect_class}");
                self.apply_typed(
                    &operation,
                    vec![
                        ("relation", relation_object, OntologyTypeExpr::QuotedObject),
                        ("cause", cause_object, cause_type),
                        ("effect", effect_object, effect_type),
                    ],
                    proposition(),
                )
            }
            PropositionExpr::Quotation { content } => {
                let content = self.proposition(document, content)?;
                self.apply_typed(
                    "muse.quotation",
                    vec![("content", content, proposition())],
                    proposition(),
                )
            }
        }?;
        self.proposition_objects.insert(id.clone(), object.clone());
        Ok(object)
    }

    fn occurrence_proposition(
        &mut self,
        document: &OccurrenceDocument,
        id: &OccurrenceId,
    ) -> Result<ObjectId, LoweringError> {
        if let Some(existing) = self.occurrence_structures.get(id) {
            return Ok(existing.clone());
        }
        let occurrence = document
            .occurrences
            .get(id)
            .ok_or_else(|| LoweringError::UnknownOccurrence(id.clone()))?;
        let identity = self
            .occurrence_objects
            .get(id)
            .cloned()
            .ok_or_else(|| LoweringError::UnknownOccurrence(id.clone()))?;
        let (identity_type, identity_class) = self.object_type_and_class(&identity)?;
        let mut facts = vec![self.apply_typed(
            &format!("muse.occurrence.instance/{identity_class}"),
            vec![("occurrence", identity.clone(), identity_type.clone())],
            proposition(),
        )?];

        // Lexical/source alignment is canonical provenance, not a proposition. The
        // typed object already carries its ontology sort, so no duplicate type fact
        // is emitted either.
        if let Some(tense) = occurrence.grammatical_tense {
            let tense_name = match tense {
                muse_occurrence::GrammaticalTense::Past => "past",
                muse_occurrence::GrammaticalTense::Present => "present",
                muse_occurrence::GrammaticalTense::Future => "future",
            };
            facts.push(self.apply_typed(
                &format!("muse.tense/{tense_name}/{identity_class}"),
                vec![("occurrence", identity.clone(), identity_type.clone())],
                proposition(),
            )?);
        }

        // Compatibility-only channels. semantic-v6 compilation rejects every one
        // of these before lowering; they remain solely for historical deterministic
        // adapters while those adapters migrate to ontology relations.
        if occurrence.tense_aspect.is_some() || occurrence.grammatical_voice.is_some() {
            let grammar = self.quote(
                "legacy-occurrence-grammar",
                &serde_json::to_string(&(occurrence.tense_aspect, occurrence.grammatical_voice))?,
            );
            facts.push(self.apply_typed(
                &format!("muse.legacy.occurrence.grammar/{identity_class}"),
                vec![
                    ("occurrence", identity.clone(), identity_type.clone()),
                    ("grammar", grammar, OntologyTypeExpr::QuotedObject),
                ],
                proposition(),
            )?);
        }
        for participant in &occurrence.participants {
            let role = self.quote(
                "legacy-occurrence-role",
                &serde_json::to_string(&participant.role)?,
            );
            let role_class = participant_role_class(&participant.role);
            let (value, value_type, value_class) = self.term_typed(document, &participant.value)?;
            facts.push(self.apply_typed(
                &format!(
                    "muse.legacy.occurrence.participant/{role_class}/{identity_class}/{value_class}"
                ),
                vec![
                    ("occurrence", identity.clone(), identity_type.clone()),
                    ("role", role, OntologyTypeExpr::QuotedObject),
                    ("value", value, value_type),
                ],
                proposition(),
            )?);
        }
        for (relation, values) in &occurrence.attributes {
            for value in values {
                let relation_object =
                    self.quote("legacy-occurrence-attribute-relation", relation.as_str());
                let relation_class = sanitize(relation.as_str());
                let (value, value_type, value_class) = self.term_typed(document, value)?;
                facts.push(self.apply_typed(
                    &format!("muse.legacy.occurrence.attribute/{relation_class}/{identity_class}/{value_class}"),
                    vec![("occurrence", identity.clone(), identity_type.clone()), ("relation", relation_object, OntologyTypeExpr::QuotedObject), ("value", value, value_type)],
                    proposition(),
                )?);
            }
        }
        if let Some(outcome) = &occurrence.reported_outcome {
            let outcome = self.quote(
                "legacy-occurrence-reported-outcome",
                &serde_json::to_string(outcome)?,
            );
            facts.push(self.apply_typed(
                &format!("muse.legacy.occurrence.reported_outcome/{identity_class}"),
                vec![
                    ("occurrence", identity.clone(), identity_type),
                    ("outcome", outcome, OntologyTypeExpr::QuotedObject),
                ],
                proposition(),
            )?);
        }

        let root = self.conjoin_formal("muse.occurrence.structure", facts)?;
        self.occurrence_structures.insert(id.clone(), root.clone());
        Ok(root)
    }

    fn term_typed(
        &mut self,
        document: &OccurrenceDocument,
        term: &Term,
    ) -> Result<(ObjectId, OntologyTypeExpr, String), LoweringError> {
        match term {
            Term::Referent(id) => {
                let object = self
                    .referent_objects
                    .get(id)
                    .cloned()
                    .ok_or_else(|| LoweringError::UnknownReferent(id.clone()))?;
                let (ty, class) = self.object_type_and_class(&object)?;
                Ok((object, ty, class))
            }
            Term::Occurrence(id) => {
                let object = self
                    .occurrence_objects
                    .get(id)
                    .cloned()
                    .ok_or_else(|| LoweringError::UnknownOccurrence(id.clone()))?;
                let (ty, class) = self.object_type_and_class(&object)?;
                Ok((object, ty, class))
            }
            Term::Proposition(id) => Ok((
                self.proposition(document, id)?,
                proposition(),
                "proposition".into(),
            )),
            Term::Variable(id) => self.variable_object(id),
            Term::Literal(value) => self.literal_typed(value),
        }
    }

    fn literal_typed(
        &mut self,
        value: &Literal,
    ) -> Result<(ObjectId, OntologyTypeExpr, String), LoweringError> {
        fn atomic_concept(value: &Literal) -> Option<&'static str> {
            Some(match value {
                Literal::String(_) => "comp:StringValue",
                Literal::Integer(_) => "comp:IntegerValue",
                Literal::Decimal(_) => "comp:DecimalValue",
                Literal::Boolean(_) => "comp:BooleanValue",
                Literal::Null => "comp:DataValue",
                Literal::Json(_) => "comp:StructuredDataValue",
                _ => return None,
            })
        }
        if let Some(concept) = atomic_concept(value) {
            let concepts = BTreeSet::from([ConceptId::from(concept)]);
            let exact = match value {
                Literal::String(v)
                | Literal::Integer(v)
                | Literal::Decimal(v)
                | Literal::Json(v) => v.clone(),
                Literal::Boolean(v) => v.to_string(),
                Literal::Null => "null".into(),
                _ => unreachable!(),
            };
            let object = self.typed_instance("literal", &exact, &concepts)?;
            let (ty, class) = self.object_type_and_class(&object)?;
            return Ok((object, ty, class));
        }
        let (concept, operation, children): (
            &str,
            String,
            Vec<(String, ObjectId, OntologyTypeExpr)>,
        ) = match value {
            Literal::Ratio {
                numerator,
                denominator,
            } => {
                let (n, nt, nc) = self.literal_typed(numerator)?;
                let (d, dt, dc) = self.literal_typed(denominator)?;
                (
                    "comp:RatioValue",
                    format!("muse.literal/ratio/{nc}/{dc}"),
                    vec![("numerator".into(), n, nt), ("denominator".into(), d, dt)],
                )
            }
            Literal::Percentage { magnitude } => {
                let (m, mt, mc) = self.literal_typed(magnitude)?;
                (
                    "comp:PercentageValue",
                    format!("muse.literal/percentage/{mc}"),
                    vec![("magnitude".into(), m, mt)],
                )
            }
            Literal::Approximate { value } => {
                let (v, vt, vc) = self.literal_typed(value)?;
                (
                    "comp:ApproximateNumericValue",
                    format!("muse.literal/approximate/{vc}"),
                    vec![("value".into(), v, vt)],
                )
            }
            Literal::Interval {
                lower,
                upper,
                lower_inclusive,
                upper_inclusive,
            } => {
                let (l, lt, lc) = self.literal_typed(lower)?;
                let (u, ut, uc) = self.literal_typed(upper)?;
                let closure = format!(
                    "{}{}",
                    if *lower_inclusive { "closed" } else { "open" },
                    if *upper_inclusive { "_closed" } else { "_open" }
                );
                (
                    "comp:NumericInterval",
                    format!("muse.literal/interval/{closure}/{lc}/{uc}"),
                    vec![("lower".into(), l, lt), ("upper".into(), u, ut)],
                )
            }
            Literal::PluralScale { base } => {
                let (b, bt, bc) = self.literal_typed(base)?;
                (
                    "comp:PluralScaleValue",
                    format!("muse.literal/plural_scale/{bc}"),
                    vec![("base".into(), b, bt)],
                )
            }
            Literal::Arithmetic {
                operator,
                left,
                right,
            } => {
                let (l, lt, lc) = self.literal_typed(left)?;
                let (r, rt, rc) = self.literal_typed(right)?;
                let (concept, op) = match operator {
                    muse_occurrence::ArithmeticOperator::Add => {
                        ("comp:AdditionExpressionValue", "add")
                    }
                    muse_occurrence::ArithmeticOperator::Subtract => {
                        ("comp:SubtractionExpressionValue", "subtract")
                    }
                    muse_occurrence::ArithmeticOperator::Multiply => {
                        ("comp:MultiplicationExpressionValue", "multiply")
                    }
                    muse_occurrence::ArithmeticOperator::Divide => {
                        ("comp:DivisionExpressionValue", "divide")
                    }
                };
                (
                    concept,
                    format!("muse.literal/arithmetic/{op}/{lc}/{rc}"),
                    vec![("left".into(), l, lt), ("right".into(), r, rt)],
                )
            }
            Literal::Measurement { magnitude, unit } => {
                let (m, mt, mc) = self.literal_typed(magnitude)?;
                let units = BTreeSet::from([ConceptId::from("comp:UnitOfMeasure")]);
                let u = self.typed_instance("unit", unit, &units)?;
                let (ut, uc) = self.object_type_and_class(&u)?;
                (
                    "comp:MeasurementValue",
                    format!("muse.literal/measurement/{mc}/{uc}"),
                    vec![("magnitude".into(), m, mt), ("unit".into(), u, ut)],
                )
            }
            _ => unreachable!(),
        };
        let result = self
            .exact_type_for_concepts(&BTreeSet::from([ConceptId::from(concept)]))?
            .0;
        let object = self.apply_owned_typed_roles(&operation, children, result)?;
        let (ty, class) = self.object_type_and_class(&object)?;
        Ok((object, ty, class))
    }

    fn ambiguity_alternative_typed(
        &mut self,
        document: &OccurrenceDocument,
        alternative: &muse_occurrence::AmbiguityAlternative,
    ) -> Result<(ObjectId, OntologyTypeExpr, String), LoweringError> {
        use muse_occurrence::AmbiguityAlternative;
        match alternative {
            AmbiguityAlternative::Referent(id) => {
                self.term_typed(document, &Term::Referent(id.clone()))
            }
            AmbiguityAlternative::Occurrence(id) => {
                self.term_typed(document, &Term::Occurrence(id.clone()))
            }
            AmbiguityAlternative::Proposition(id) => Ok((
                self.proposition(document, id)?,
                proposition(),
                "proposition".into(),
            )),
            AmbiguityAlternative::Literal(value) => {
                self.term_typed(document, &Term::Literal(value.clone()))
            }
            // Legacy rich-IR ambiguity can mention ontology symbols or temporal anchors directly.
            // Semantic-v6 ambiguity branches are complete propositions/content and never use these cases.
            AmbiguityAlternative::Concept(id) => Ok((
                self.quote("ambiguity-concept", id.as_str()),
                OntologyTypeExpr::QuotedObject,
                "concept_symbol".into(),
            )),
            AmbiguityAlternative::Relation(id) => Ok((
                self.quote("ambiguity-relation", id.as_str()),
                OntologyTypeExpr::QuotedObject,
                "relation_symbol".into(),
            )),
            AmbiguityAlternative::Temporal(anchor) => self.temporal_anchor_typed(document, anchor),
        }
    }

    fn semantic_target_typed(
        &mut self,
        document: &OccurrenceDocument,
        target: &SemanticTarget,
    ) -> Result<(ObjectId, OntologyTypeExpr, String), LoweringError> {
        match target {
            SemanticTarget::Occurrence(id) => {
                let object = self
                    .occurrence_objects
                    .get(id)
                    .cloned()
                    .ok_or_else(|| LoweringError::UnknownOccurrence(id.clone()))?;
                let (ty, class) = self.object_type_and_class(&object)?;
                Ok((object, ty, class))
            }
            SemanticTarget::Proposition(id) => Ok((
                self.proposition(document, id)?,
                proposition(),
                "proposition".into(),
            )),
            SemanticTarget::Variable(id) => self.variable_object(id),
        }
    }

    fn temporal_anchor_typed(
        &mut self,
        document: &OccurrenceDocument,
        anchor: &TemporalAnchor,
    ) -> Result<(ObjectId, OntologyTypeExpr, String), LoweringError> {
        match anchor {
            TemporalAnchor::Target { target } => self.semantic_target_typed(document, target),
            TemporalAnchor::Variable { variable } => self.variable_object(variable),
            _ => Ok((
                self.quote("temporal-anchor", &serde_json::to_string(anchor)?),
                OntologyTypeExpr::QuotedObject,
                "quoted".into(),
            )),
        }
    }

    fn conjoin_formal(
        &mut self,
        operation_prefix: &str,
        members: Vec<ObjectId>,
    ) -> Result<ObjectId, LoweringError> {
        if members.is_empty() {
            return Err(LoweringError::Internal(
                "cannot lower an occurrence with no structural facts".into(),
            ));
        }
        if members.len() == 1 {
            return members.into_iter().next().ok_or_else(|| {
                LoweringError::Internal("cannot lower an empty structural conjunction".into())
            });
        }
        let operation = format!("{operation_prefix}/{}", members.len());
        let values = members
            .into_iter()
            .enumerate()
            .map(|(index, member)| (format!("member{index}"), member))
            .collect();
        self.apply_owned_prop_roles(&operation, values)
    }

    fn unary_prop(
        &mut self,
        document: &OccurrenceDocument,
        operation: &str,
        content: &PropositionId,
    ) -> Result<ObjectId, LoweringError> {
        let content = self.proposition(document, content)?;
        self.apply_typed(
            operation,
            vec![("content", content, proposition())],
            proposition(),
        )
    }
    fn metadata_prop<T: Serialize>(
        &mut self,
        document: &OccurrenceDocument,
        operation: &str,
        metadata: &T,
        content: &PropositionId,
    ) -> Result<ObjectId, LoweringError> {
        let meta = self.quote("metadata", &serde_json::to_string(metadata)?);
        let content = self.proposition(document, content)?;
        self.apply_typed(
            operation,
            vec![
                ("metadata", meta, OntologyTypeExpr::QuotedObject),
                ("content", content, proposition()),
            ],
            proposition(),
        )
    }
    fn nary_props(
        &mut self,
        document: &OccurrenceDocument,
        operation: &str,
        members: &BTreeSet<PropositionId>,
    ) -> Result<ObjectId, LoweringError> {
        let mut values = Vec::new();
        for (i, id) in members.iter().enumerate() {
            values.push((format!("member{i}"), self.proposition(document, id)?));
        }
        self.apply_owned_prop_roles(operation, values)
    }
    fn apply_owned_typed_roles(
        &mut self,
        operation: &str,
        values: Vec<(String, ObjectId, OntologyTypeExpr)>,
        result: OntologyTypeExpr,
    ) -> Result<ObjectId, LoweringError> {
        if !self
            .ontology
            .symbols
            .contains_key(&OntologySymbolId(operation.to_owned()))
        {
            self.declare(
                operation,
                values
                    .iter()
                    .map(|(role, _, ty)| (role.as_str(), ty.clone()))
                    .collect(),
                result.clone(),
                "Muse typed structural operator",
            )?;
        }
        let args = values
            .iter()
            .map(|(role, object, _)| (role.as_str(), object.clone()))
            .collect();
        self.apply(operation, args, result)
    }
    fn apply_owned_prop_roles(
        &mut self,
        operation: &str,
        values: Vec<(String, ObjectId)>,
    ) -> Result<ObjectId, LoweringError> {
        if !self
            .ontology
            .symbols
            .contains_key(&OntologySymbolId(operation.to_owned()))
        {
            self.declare(
                operation,
                values
                    .iter()
                    .map(|(r, _)| (r.as_str(), proposition()))
                    .collect(),
                proposition(),
                "Muse n-ary propositional operator",
            )?;
        }
        let args = values
            .iter()
            .map(|(r, o)| (r.as_str(), o.clone()))
            .collect();
        self.apply(operation, args, proposition())
    }
}

fn proposition() -> OntologyTypeExpr {
    OntologyTypeExpr::named(OntologyTypeId(PROPOSITION_TYPE.into()))
}
fn participant_role_class(role: &ParticipantRole) -> String {
    match role {
        ParticipantRole::Subject => "subject".to_owned(),
        ParticipantRole::DirectObject => "direct_object".to_owned(),
        ParticipantRole::IndirectObject => "indirect_object".to_owned(),
        ParticipantRole::PredicateComplement => "predicate_complement".to_owned(),
        ParticipantRole::Agent => "agent".to_owned(),
        ParticipantRole::Patient => "patient".to_owned(),
        ParticipantRole::Theme => "theme".to_owned(),
        ParticipantRole::Experiencer => "experiencer".to_owned(),
        ParticipantRole::Content => "content".to_owned(),
        ParticipantRole::Source => "source".to_owned(),
        ParticipantRole::Goal => "goal".to_owned(),
        ParticipantRole::Recipient => "recipient".to_owned(),
        ParticipantRole::Instrument => "instrument".to_owned(),
        ParticipantRole::Location => "location".to_owned(),
        ParticipantRole::Manner => "manner".to_owned(),
        ParticipantRole::Beneficiary => "beneficiary".to_owned(),
        ParticipantRole::Stimulus => "stimulus".to_owned(),
        ParticipantRole::Topic => "topic".to_owned(),
        ParticipantRole::Possessor => "possessor".to_owned(),
        ParticipantRole::Attribute => "attribute".to_owned(),
        ParticipantRole::Value => "value".to_owned(),
        ParticipantRole::Ontology(relation) => {
            format!("ontology/{}", sanitize(relation.as_str()))
        }
        ParticipantRole::SourceAnchored(_) => "source_anchored".to_owned(),
    }
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Failures at the pre-label semantic→formal boundary.
#[derive(Debug, Error)]
pub enum LoweringError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Occurrence(#[from] muse_occurrence::OccurrenceValidationError),
    #[error(transparent)]
    Canonical(#[from] TrainingCanonicalizationError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Graph(#[from] artist_formal::GraphError),
    #[error(transparent)]
    Interpretation(#[from] artist_formal::InterpretationError),
    #[error(transparent)]
    Kernel(#[from] OntologyCompileError),
    #[error("ontology index does not correspond exactly to document snapshot")]
    OntologySnapshotMismatch,
    #[error("semantic object has no ontology type: {0}")]
    MissingOntologyType(String),
    #[error("unknown ontology concept {0}")]
    UnknownConcept(ConceptId),
    #[error("bound variable {0} has no ontology type")]
    MissingVariableType(VariableId),
    #[error("occurrence ontology type must descend from ufo:Event or ufo:Situation, found {0:?}")]
    InvalidOccurrenceOntologyType(Vec<ConceptId>),
    #[error("bound variable {variable} has conflicting ontology types {left} and {right}")]
    VariableTypeConflict {
        variable: VariableId,
        left: ConceptId,
        right: ConceptId,
    },
    #[error("unknown ontology relation {0}")]
    UnknownRelation(RelationId),
    #[error("relation {relation} requires binary source expression, got arity {actual}")]
    RelationArity { relation: RelationId, actual: usize },
    #[error("relation {relation} subject does not satisfy any licensed domain {allowed:?}")]
    RelationDomain {
        relation: RelationId,
        allowed: BTreeSet<ConceptId>,
    },
    #[error("relation {relation} object does not satisfy any licensed range {allowed:?}")]
    RelationRange {
        relation: RelationId,
        allowed: BTreeSet<ConceptId>,
    },
    #[error("source assigns disjoint ontology types {left} and {right}")]
    DisjointTypes { left: ConceptId, right: ConceptId },
    #[error("unknown referent {0}")]
    UnknownReferent(ReferentId),
    #[error("unknown occurrence {0}")]
    UnknownOccurrence(OccurrenceId),
    #[error("unknown proposition {0}")]
    UnknownProposition(PropositionId),
    #[error("missing statement {0}")]
    MissingStatement(StatementId),
    #[error("formal graph has typing diagnostics: {0}")]
    FormalTyping(String),
    #[error("internal lowering invariant failed: {0}")]
    Internal(String),
}
