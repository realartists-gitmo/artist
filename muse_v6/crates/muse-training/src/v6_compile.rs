//! Validation and deterministic compilation of the minimal semantic-v6 target.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use muse_core::{
    AmbiguityId, ConceptId, ContentId, OccurrenceDocumentId, OccurrenceId, PropositionId,
    ReferentId, RelationId, SourceSpanId, StatementId, VariableId,
};
use muse_occurrence::{
    Ambiguity, AmbiguityAlternative, ByteRange, Derivation, DiscourseRole, LexicalAnchor, Literal,
    Occurrence, OccurrenceDocument, PresentationMode, Proposition, PropositionExpr, Quantifier,
    Referent, SCHEMA_VERSION, SemanticVariable, SourceSpan, Statement, StatementBasis, Term,
};
use muse_ontology::OntologyIndex;
use muse_registry::RegistrySnapshot;

use crate::{
    CardinalityComparator, ContextTerm, GroundedLiteral, LearnedAnchor, LearnedArithmeticOperator,
    LearnedContent, LearnedContentBody, LearnedLiteral, LearnedProposition, LearnedPropositionExpr,
    LearnedReading, LearnedRoot, LearnedSemanticTarget, LearnedSpan, LearnedTerm, SpeakerRole,
    StructuredChannel, StructuredFieldValue, StructuredWindow, TrainingError, TrainingWindow,
    WINDOW_ACTOR_EXTERNAL_ID, WINDOW_ADDRESSEE_EXTERNAL_ID, WINDOW_RECORDER_EXTERNAL_ID,
    WINDOW_SPEAKER_EXTERNAL_ID,
};

const CONTEXT_SPEAKER: &str = "muse-context-speaker";
const CONTEXT_ACTOR: &str = "muse-context-actor";
const CONTEXT_RECORDER: &str = "muse-context-recorder";
const CONTEXT_TOOL: &str = "muse-context-tool";
const CONTEXT_INVOCATION: &str = "muse-context-invocation";
const CONTEXT_RESULT: &str = "muse-context-result";
const CONTEXT_UPDATE: &str = "muse-context-update";
const CONTENT_PREFIX: &str = "muse-content-";
const GENERATED_PROP_PREFIX: &str = "muse-compiled-";
const GENERATED_STATEMENT_PREFIX: &str = "muse-root-";

pub(crate) fn validate_and_compile_v6(
    window: &TrainingWindow,
    target: &LearnedSemanticTarget,
    ontology: RegistrySnapshot,
    derivation: Derivation,
    index: &OntologyIndex,
) -> Result<OccurrenceDocument, TrainingError> {
    validate_v6(window, target, index)?;
    Compiler::new(window, target, ontology, derivation, index)?.compile()
}

fn semantic_error(message: impl Into<String>) -> TrainingError {
    TrainingError::SemanticV6(message.into())
}

fn validate_v6(
    window: &TrainingWindow,
    target: &LearnedSemanticTarget,
    index: &OntologyIndex,
) -> Result<(), TrainingError> {
    if target.roots.is_empty() {
        return Err(semantic_error("semantic-v6 target has no roots"));
    }
    for (id, referent) in &target.referents {
        if id != &referent.id {
            return Err(semantic_error(format!(
                "referent map key {id} disagrees with declared id {}",
                referent.id
            )));
        }
        validate_model_id(id.as_str())?;
        validate_sort(index, &referent.sort)?;
        validate_anchor(window, &referent.anchor, "referent")?;
    }
    for (id, occurrence) in &target.occurrences {
        if id != &occurrence.id {
            return Err(semantic_error(format!(
                "occurrence map key {id} disagrees with declared id {}",
                occurrence.id
            )));
        }
        validate_model_id(id.as_str())?;
        validate_sort(index, &occurrence.sort)?;
        validate_anchor(window, &occurrence.anchor, "occurrence")?;
        let resolved = index.resolve_concept(&occurrence.sort).ok_or_else(|| {
            semantic_error(format!("unknown occurrence sort {}", occurrence.sort))
        })?;
        let event = index
            .resolve_concept(&ConceptId::from("Event"))
            .ok_or_else(|| semantic_error("ontology lacks Event"))?;
        let situation = index
            .resolve_concept(&ConceptId::from("Situation"))
            .ok_or_else(|| semantic_error("ontology lacks Situation"))?;
        if !index.is_subtype(&resolved, &event) && !index.is_subtype(&resolved, &situation) {
            return Err(semantic_error(format!(
                "occurrence {} sort {} is neither Event nor Situation",
                id, occurrence.sort
            )));
        }
    }
    for (id, variable) in &target.variables {
        if id != &variable.id {
            return Err(semantic_error(format!(
                "variable map key {id} disagrees with declared id {}",
                variable.id
            )));
        }
        validate_model_id(id.as_str())?;
        validate_sort(index, &variable.sort)?;
        validate_anchor(window, &variable.anchor, "variable")?;
    }
    for (id, proposition) in &target.propositions {
        if id != &proposition.id {
            return Err(semantic_error(format!(
                "proposition map key {id} disagrees with declared id {}",
                proposition.id
            )));
        }
        validate_model_id(id.as_str())?;
        if let Some(anchor) = &proposition.operator_anchor {
            validate_anchor(window, anchor, "proposition operator")?;
            validate_operator_anchor_policy(target, proposition, anchor)?;
        }
        validate_prop_expr(window, target, index, &proposition.expression)?;
    }
    for (id, content) in &target.contents {
        if id != &content.id {
            return Err(semantic_error(format!(
                "content map key {id} disagrees with declared id {}",
                content.id
            )));
        }
        validate_model_id(id.as_str())?;
        validate_sort(index, &content.sort)?;
        let semantic_content = index
            .resolve_concept(&ConceptId::from("SemanticContent"))
            .ok_or_else(|| semantic_error("ontology lacks SemanticContent"))?;
        let resolved = index
            .resolve_concept(&content.sort)
            .ok_or_else(|| semantic_error(format!("unknown content sort {}", content.sort)))?;
        if !index.is_subtype(&resolved, &semantic_content) {
            return Err(semantic_error(format!(
                "content {} sort {} is not SemanticContent",
                id, content.sort
            )));
        }
        validate_content(window, target, index, content)?;
    }
    for (id, ambiguity) in &target.ambiguities {
        if id != &ambiguity.id {
            return Err(semantic_error(format!(
                "ambiguity map key {id} disagrees with declared id {}",
                ambiguity.id
            )));
        }
        validate_model_id(id.as_str())?;
        if ambiguity.alternatives.len() < 2 {
            return Err(semantic_error(format!(
                "ambiguity {id} needs at least two complete readings"
            )));
        }
        for alternative in &ambiguity.alternatives {
            validate_reading(target, alternative)?;
        }
        let mut footprints = ambiguity
            .alternatives
            .iter()
            .map(|reading| reading_footprint(target, reading))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(first) = footprints.pop() {
            if footprints.iter().any(|candidate| candidate != &first) {
                return Err(semantic_error(format!(
                    "ambiguity {id} alternatives do not cover the same source footprint"
                )));
            }
        }
    }
    validate_roots(target)?;
    validate_cycles(target)?;
    validate_reachability(target)?;
    validate_source_coverage(window, target)?;
    for literal in grounded_literals(target) {
        validate_grounded_literal(window, target, literal)?;
    }
    Ok(())
}

fn validate_operator_anchor_policy(
    target: &LearnedSemanticTarget,
    proposition: &LearnedProposition,
    anchor: &LearnedAnchor,
) -> Result<(), TrainingError> {
    use LearnedPropositionExpr as E;
    let internally_grounded = matches!(
        &proposition.expression,
        E::Occurrence { .. }
            | E::PluralScaleCardinality { .. }
            | E::GeneralizedQuantified { .. }
            | E::ScopedOperator { .. }
            | E::Focus { .. }
    );
    if internally_grounded {
        return Err(semantic_error(format!(
            "proposition {} must not duplicate its internally grounded operator with operator_anchor",
            proposition.id
        )));
    }
    let full = reading_footprint(target, &LearnedReading::Proposition(proposition.id.clone()))?;
    for operator_span in &anchor.spans {
        for other in &full {
            if other == operator_span || anchor.spans.contains(other) {
                continue;
            }
            if learned_spans_overlap(operator_span, other) {
                return Err(semantic_error(format!(
                    "proposition {} operator cue overlaps scoped semantic material",
                    proposition.id
                )));
            }
        }
    }
    Ok(())
}

fn learned_spans_overlap(left: &LearnedSpan, right: &LearnedSpan) -> bool {
    match (left, right) {
        (
            LearnedSpan::Prose {
                start_byte: a0,
                end_byte: a1,
            },
            LearnedSpan::Prose {
                start_byte: b0,
                end_byte: b1,
            },
        ) => a0 < b1 && b0 < a1,
        (
            LearnedSpan::Structured {
                field_path: ap,
                start_byte: Some(a0),
                end_byte: Some(a1),
            },
            LearnedSpan::Structured {
                field_path: bp,
                start_byte: Some(b0),
                end_byte: Some(b1),
            },
        ) => ap == bp && a0 < b1 && b0 < a1,
        _ => false,
    }
}

fn validate_model_id(value: &str) -> Result<(), TrainingError> {
    if value.starts_with("muse-context-")
        || value.starts_with(CONTENT_PREFIX)
        || value.starts_with(GENERATED_PROP_PREFIX)
        || value.starts_with(GENERATED_STATEMENT_PREFIX)
    {
        Err(semantic_error(format!(
            "learned id {value} uses compiler-reserved prefix"
        )))
    } else {
        Ok(())
    }
}

fn validate_sort(index: &OntologyIndex, sort: &ConceptId) -> Result<(), TrainingError> {
    if sort.as_str().contains(':') {
        return Err(semantic_error(format!(
            "learned ontology sort must be namespaceless: {sort}"
        )));
    }
    let resolved = index
        .resolve_concept(sort)
        .ok_or_else(|| semantic_error(format!("unknown ontology sort {sort}")))?;
    if &resolved == sort {
        return Err(semantic_error(format!(
            "learned ontology sort is not canonical namespaceless symbol: {sort}"
        )));
    }
    Ok(())
}
fn validate_relation(index: &OntologyIndex, relation: &RelationId) -> Result<(), TrainingError> {
    if relation.as_str().contains(':') {
        return Err(semantic_error(format!(
            "learned ontology relation must be namespaceless: {relation}"
        )));
    }
    let resolved = index
        .resolve_relation(relation)
        .ok_or_else(|| semantic_error(format!("unknown ontology relation {relation}")))?;
    if &resolved == relation {
        return Err(semantic_error(format!(
            "learned ontology relation is not canonical namespaceless symbol: {relation}"
        )));
    }
    Ok(())
}
fn validate_anchor(
    window: &TrainingWindow,
    anchor: &LearnedAnchor,
    label: &str,
) -> Result<(), TrainingError> {
    if anchor.spans.is_empty() {
        return Err(semantic_error(format!("{label} anchor is empty")));
    }
    for span in &anchor.spans {
        validate_learned_span(window, span)?;
    }
    Ok(())
}

fn validate_learned_span(window: &TrainingWindow, span: &LearnedSpan) -> Result<(), TrainingError> {
    match (window, span) {
        (
            TrainingWindow::Prose(window),
            LearnedSpan::Prose {
                start_byte,
                end_byte,
            },
        ) => {
            if start_byte >= end_byte || *end_byte > window.text.len() as u64 {
                return Err(semantic_error(format!("invalid prose span {span:?}")));
            }
            let start = usize::try_from(*start_byte)
                .map_err(|_| semantic_error(format!("invalid prose span {span:?}")))?;
            let end = usize::try_from(*end_byte)
                .map_err(|_| semantic_error(format!("invalid prose span {span:?}")))?;
            if !window.text.is_char_boundary(start) || !window.text.is_char_boundary(end) {
                return Err(semantic_error(format!("non-UTF8 prose span {span:?}")));
            }
            if !window
                .blocks
                .iter()
                .any(|block| *start_byte >= block.start_byte && *end_byte <= block.end_byte)
            {
                return Err(semantic_error(format!(
                    "prose span {span:?} crosses a source block"
                )));
            }
        }
        (
            TrainingWindow::Structured(window),
            LearnedSpan::Structured {
                field_path,
                start_byte,
                end_byte,
            },
        ) => {
            let source = structured_source_value(window, field_path).ok_or_else(|| {
                semantic_error(format!(
                    "structured span {span:?} targets unavailable path {field_path}"
                ))
            })?;
            match (source, start_byte, end_byte) {
                (
                    StructuredSourceValue::String {
                        text, base_start, ..
                    },
                    Some(start),
                    Some(end),
                ) => {
                    if start >= end {
                        return Err(semantic_error(format!(
                            "empty structured string span {span:?}"
                        )));
                    }
                    let local_start = start.checked_sub(base_start).ok_or_else(|| {
                        semantic_error(format!(
                            "structured span {span:?} precedes visible fragment"
                        ))
                    })?;
                    let local_end = end.checked_sub(base_start).ok_or_else(|| {
                        semantic_error(format!(
                            "structured span {span:?} precedes visible fragment"
                        ))
                    })?;
                    let ls = usize::try_from(local_start)
                        .map_err(|_| semantic_error(format!("invalid structured span {span:?}")))?;
                    let le = usize::try_from(local_end)
                        .map_err(|_| semantic_error(format!("invalid structured span {span:?}")))?;
                    if le > text.len() || !text.is_char_boundary(ls) || !text.is_char_boundary(le) {
                        return Err(semantic_error(format!(
                            "structured span {span:?} exceeds visible string fragment"
                        )));
                    }
                }
                (StructuredSourceValue::String { primary: false, .. }, None, None) => {}
                (StructuredSourceValue::String { primary: true, .. }, None, None) => {
                    return Err(semantic_error(format!(
                        "structured string span {span:?} requires exact byte offsets"
                    )));
                }
                (
                    StructuredSourceValue::Scalar(_)
                    | StructuredSourceValue::Container
                    | StructuredSourceValue::Opaque,
                    None,
                    None,
                ) => {}
                (_, _, _) => {
                    return Err(semantic_error(format!(
                        "structured span {span:?} uses byte offsets on a non-string/container source"
                    )));
                }
            }
        }
        _ => {
            return Err(semantic_error(format!(
                "span {span:?} kind does not match window kind"
            )));
        }
    }
    Ok(())
}

enum StructuredSourceValue<'a> {
    String {
        text: &'a str,
        base_start: u64,
        primary: bool,
    },
    Scalar(&'a serde_json::Value),
    Container,
    Opaque,
}
fn structured_source_value<'a>(
    window: &'a StructuredWindow,
    path: &str,
) -> Option<StructuredSourceValue<'a>> {
    for block in &window.fields {
        if block.field_path == path || block.alias_paths.iter().any(|alias| alias == path) {
            return match &block.value {
                StructuredFieldValue::Json { value } => value
                    .as_str()
                    .map(|text| StructuredSourceValue::String {
                        text,
                        base_start: block.source_start_byte.unwrap_or(0),
                        primary: true,
                    })
                    .or(Some(StructuredSourceValue::Scalar(value))),
                StructuredFieldValue::Opaque { .. } => Some(StructuredSourceValue::Opaque),
            };
        }
        if block
            .container_paths
            .iter()
            .any(|container| container == path)
        {
            return Some(StructuredSourceValue::Container);
        }
        if let Some(context) = block
            .context
            .iter()
            .find(|context| context.field_path == path)
        {
            return context
                .value
                .as_str()
                .map(|text| StructuredSourceValue::String {
                    text,
                    base_start: 0,
                    primary: false,
                })
                .or(Some(StructuredSourceValue::Scalar(&context.value)));
        }
    }
    None
}

fn validate_prop_expr(
    window: &TrainingWindow,
    target: &LearnedSemanticTarget,
    index: &OntologyIndex,
    expr: &LearnedPropositionExpr,
) -> Result<(), TrainingError> {
    match expr {
        LearnedPropositionExpr::TypeAssertion { subject, r#type } => {
            validate_term(window, target, subject)?;
            validate_sort(index, r#type)?;
            reject_redundant_type_assertion(target, index, subject, r#type)?;
        }
        LearnedPropositionExpr::Relation {
            relation,
            arguments,
        } => {
            validate_relation(index, relation)?;
            if arguments.len() != 2 {
                return Err(semantic_error(format!(
                    "ontology relation {relation} must have exactly two arguments, found {}",
                    arguments.len()
                )));
            }
            for term in arguments {
                validate_term(window, target, term)?;
            }
        }
        LearnedPropositionExpr::Occurrence { occurrence } => {
            if !target.occurrences.contains_key(occurrence) {
                return Err(semantic_error(format!("unknown occurrence {occurrence}")));
            }
        }
        LearnedPropositionExpr::Equality { left, right } => {
            validate_term(window, target, left)?;
            validate_term(window, target, right)?;
        }
        LearnedPropositionExpr::Comparison {
            left,
            right,
            dimension,
            ..
        } => {
            validate_term(window, target, left)?;
            validate_term(window, target, right)?;
            if let Some(term) = dimension {
                validate_term(window, target, term)?;
            }
        }
        LearnedPropositionExpr::Negation { content } => require_prop(target, content)?,
        LearnedPropositionExpr::ScopedOperator { operator, content } => {
            let op = target
                .referents
                .get(operator)
                .ok_or_else(|| semantic_error(format!("unknown scoped operator {operator}")))?;
            require_subtype(index, &op.sort, "PropositionalOperator", "scoped operator")?;
            require_prop(target, content)?;
        }
        LearnedPropositionExpr::Conjunction { members }
        | LearnedPropositionExpr::Disjunction { members } => {
            if members.len() < 2 {
                return Err(semantic_error(
                    "conjunction/disjunction requires at least two members",
                ));
            }
            for id in members {
                require_prop(target, id)?;
            }
        }
        LearnedPropositionExpr::Implication {
            antecedent,
            consequent,
        }
        | LearnedPropositionExpr::Counterfactual {
            antecedent,
            consequent,
        } => {
            require_prop(target, antecedent)?;
            require_prop(target, consequent)?;
        }
        LearnedPropositionExpr::Unless {
            condition,
            consequent,
        } => {
            require_prop(target, condition)?;
            require_prop(target, consequent)?;
        }
        LearnedPropositionExpr::Exists { variable, body }
        | LearnedPropositionExpr::ForAll { variable, body } => {
            require_variable(target, variable)?;
            require_prop(target, body)?;
        }
        LearnedPropositionExpr::Cardinality {
            count,
            variable,
            body,
            ..
        } => {
            require_variable(target, variable)?;
            require_prop(target, body)?;
            validate_anchor(window, &count.anchor, "cardinality")?;
            if !matches!(count.value, LearnedLiteral::Integer { .. }) {
                return Err(semantic_error(
                    "cardinality count must be an integer literal",
                ));
            }
        }
        LearnedPropositionExpr::PluralScaleCardinality {
            scale,
            variable,
            body,
        } => {
            require_variable(target, variable)?;
            require_prop(target, body)?;
            validate_anchor(window, &scale.anchor, "plural-scale cardinality")?;
            if !matches!(scale.value, LearnedLiteral::PluralScale { .. }) {
                return Err(semantic_error(
                    "plural-scale cardinality requires a PluralScale literal",
                ));
            }
        }
        LearnedPropositionExpr::GeneralizedQuantified {
            quantifier,
            variable,
            body,
        } => {
            let q = target.referents.get(quantifier).ok_or_else(|| {
                semantic_error(format!("unknown generalized quantifier {quantifier}"))
            })?;
            require_variable(target, variable)?;
            require_prop(target, body)?;
            require_subtype(
                index,
                &q.sort,
                "GeneralizedQuantifier",
                "generalized quantifier",
            )?;
        }
        LearnedPropositionExpr::Focus {
            operator,
            focus,
            content,
        } => {
            let op = target
                .referents
                .get(operator)
                .ok_or_else(|| semantic_error(format!("unknown focus operator {operator}")))?;
            require_subtype(index, &op.sort, "FocusOperator", "focus operator")?;
            validate_anchor(window, focus, "focus constituent")?;
            require_prop(target, content)?;
        }
        LearnedPropositionExpr::Presuppositional {
            asserted,
            presupposed,
        } => {
            require_prop(target, asserted)?;
            require_prop(target, presupposed)?;
        }
    }
    Ok(())
}
fn structural_sort_of_term<'a>(
    target: &'a LearnedSemanticTarget,
    term: &LearnedTerm,
) -> Option<&'a ConceptId> {
    match term {
        LearnedTerm::Referent(id) => target.referents.get(id).map(|v| &v.sort),
        LearnedTerm::Occurrence(id) => target.occurrences.get(id).map(|v| &v.sort),
        LearnedTerm::Variable(id) => target.variables.get(id).map(|v| &v.sort),
        _ => None,
    }
}
fn reject_redundant_type_assertion(
    target: &LearnedSemanticTarget,
    index: &OntologyIndex,
    subject: &LearnedTerm,
    asserted: &ConceptId,
) -> Result<(), TrainingError> {
    let Some(structural) = structural_sort_of_term(target, subject) else {
        return Ok(());
    };
    let child = index
        .resolve_concept(structural)
        .ok_or_else(|| semantic_error(format!("unknown structural sort {structural}")))?;
    let parent = index
        .resolve_concept(asserted)
        .ok_or_else(|| semantic_error(format!("unknown asserted sort {asserted}")))?;
    if index.is_subtype(&child, &parent) {
        return Err(semantic_error(format!(
            "type assertion {asserted} is already entailed by structural sort {structural}"
        )));
    }
    Ok(())
}
fn validate_content(
    window: &TrainingWindow,
    target: &LearnedSemanticTarget,
    index: &OntologyIndex,
    content: &LearnedContent,
) -> Result<(), TrainingError> {
    match &content.body {
        LearnedContentBody::Proposition { proposition } => require_prop(target, proposition)?,
        LearnedContentBody::Question {
            condition,
            answer_variables,
            alternatives,
        } => {
            require_prop(target, condition)?;
            for variable in answer_variables {
                require_variable(target, variable)?;
            }
            for alt in alternatives {
                require_prop(target, alt)?;
            }
            if subtype(index, &content.sort, "WhQuestionContent")? && answer_variables.is_empty() {
                return Err(semantic_error(format!(
                    "WH question content {} has no answer variable",
                    content.id
                )));
            }
            if subtype(index, &content.sort, "PolarQuestionContent")?
                && (!answer_variables.is_empty() || !alternatives.is_empty())
            {
                return Err(semantic_error(format!(
                    "polar question content {} carries WH/alternative structure",
                    content.id
                )));
            }
            if subtype(index, &content.sort, "AlternativeQuestionContent")?
                && alternatives.len() < 2
            {
                return Err(semantic_error(format!(
                    "alternative question content {} needs at least two alternatives",
                    content.id
                )));
            }
        }
        LearnedContentBody::Quotation { quoted } => validate_anchor(window, quoted, "quotation")?,
    }
    Ok(())
}
fn require_subtype(
    index: &OntologyIndex,
    child: &ConceptId,
    parent: &str,
    label: &str,
) -> Result<(), TrainingError> {
    if subtype(index, child, parent)? {
        Ok(())
    } else {
        Err(semantic_error(format!(
            "{label} sort {child} is not a {parent}"
        )))
    }
}
fn subtype(index: &OntologyIndex, child: &ConceptId, parent: &str) -> Result<bool, TrainingError> {
    let c = index
        .resolve_concept(child)
        .ok_or_else(|| semantic_error(format!("unknown sort {child}")))?;
    let p = index
        .resolve_concept(&ConceptId::from(parent))
        .ok_or_else(|| semantic_error(format!("ontology lacks {parent}")))?;
    Ok(index.is_subtype(&c, &p))
}
fn require_prop(target: &LearnedSemanticTarget, id: &PropositionId) -> Result<(), TrainingError> {
    if target.propositions.contains_key(id) {
        Ok(())
    } else {
        Err(semantic_error(format!("unknown proposition {id}")))
    }
}
fn require_variable(target: &LearnedSemanticTarget, id: &VariableId) -> Result<(), TrainingError> {
    if target.variables.contains_key(id) {
        Ok(())
    } else {
        Err(semantic_error(format!("unknown variable {id}")))
    }
}
fn validate_term(
    window: &TrainingWindow,
    target: &LearnedSemanticTarget,
    term: &LearnedTerm,
) -> Result<(), TrainingError> {
    match term {
        LearnedTerm::Referent(id) => {
            if target.referents.contains_key(id) {
                Ok(())
            } else {
                Err(semantic_error(format!("unknown referent {id}")))
            }
        }
        LearnedTerm::Occurrence(id) => {
            if target.occurrences.contains_key(id) {
                Ok(())
            } else {
                Err(semantic_error(format!("unknown occurrence {id}")))
            }
        }
        LearnedTerm::Proposition(id) => require_prop(target, id),
        LearnedTerm::Variable(id) => require_variable(target, id),
        LearnedTerm::Literal(value) => validate_anchor(window, &value.anchor, "literal"),
        LearnedTerm::Context(context) => validate_context(window, *context),
    }
}
fn validate_context(window: &TrainingWindow, context: ContextTerm) -> Result<(), TrainingError> {
    let ok = match (window, context) {
        (TrainingWindow::Prose(_), ContextTerm::Speaker) => true,
        (TrainingWindow::Prose(w), ContextTerm::Addressee) => {
            w.addressee_id.is_some() && w.addressee_role.is_some()
        }
        (TrainingWindow::Structured(_), ContextTerm::Actor) => true,
        (TrainingWindow::Structured(w), ContextTerm::Recorder) => w.recorder_id.is_some(),
        (TrainingWindow::Structured(w), ContextTerm::Tool) => w.tool_name.is_some(),
        (TrainingWindow::Structured(w), ContextTerm::Invocation) => {
            matches!(w.channel, StructuredChannel::ToolCall) || w.invocation_id.is_some()
        }
        (TrainingWindow::Structured(w), ContextTerm::Result) => {
            matches!(w.channel, StructuredChannel::ToolResult)
        }
        (TrainingWindow::Structured(w), ContextTerm::Update) => {
            matches!(w.channel, StructuredChannel::ToolCallUpdate)
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(semantic_error(format!(
            "context term {context:?} is not established by this window"
        )))
    }
}
fn validate_reading(
    target: &LearnedSemanticTarget,
    reading: &LearnedReading,
) -> Result<(), TrainingError> {
    match reading {
        LearnedReading::Proposition(id) => require_prop(target, id),
        LearnedReading::Content(id) => {
            if target.contents.contains_key(id) {
                Ok(())
            } else {
                Err(semantic_error(format!("unknown content {id}")))
            }
        }
    }
}
fn validate_roots(target: &LearnedSemanticTarget) -> Result<(), TrainingError> {
    let mut seen = BTreeSet::new();
    for root in &target.roots {
        if !seen.insert(root) {
            return Err(semantic_error("duplicate semantic root"));
        }
        match root {
            LearnedRoot::Proposition(id) => require_prop(target, id)?,
            LearnedRoot::Content(id) => {
                if !target.contents.contains_key(id) {
                    return Err(semantic_error(format!("unknown content root {id}")));
                }
            }
            LearnedRoot::Ambiguity(id) => {
                if !target.ambiguities.contains_key(id) {
                    return Err(semantic_error(format!("unknown ambiguity root {id}")));
                }
            }
        }
    }
    Ok(())
}

fn proposition_dependencies(
    target: &LearnedSemanticTarget,
    id: &PropositionId,
    out: &mut BTreeSet<PropositionId>,
    contents: &mut BTreeSet<ContentId>,
) -> Result<(), TrainingError> {
    let p = target
        .propositions
        .get(id)
        .ok_or_else(|| semantic_error(format!("unknown proposition {id}")))?;
    let mut term = |term: &LearnedTerm| {
        if let LearnedTerm::Proposition(child) = term {
            out.insert(child.clone());
        }
        if let LearnedTerm::Literal(_) = term {}
    };
    match &p.expression {
        LearnedPropositionExpr::TypeAssertion { subject, .. } => term(subject),
        LearnedPropositionExpr::Relation { arguments, .. } => {
            for t in arguments {
                term(t);
            }
        }
        LearnedPropositionExpr::Equality { left, right } => {
            term(left);
            term(right);
        }
        LearnedPropositionExpr::Comparison {
            left,
            right,
            dimension,
            ..
        } => {
            term(left);
            term(right);
            if let Some(t) = dimension {
                term(t);
            }
        }
        LearnedPropositionExpr::Negation { content } => {
            out.insert(content.clone());
        }
        LearnedPropositionExpr::ScopedOperator {
            operator: _,
            content,
        } => {
            out.insert(content.clone());
        }
        LearnedPropositionExpr::Conjunction { members }
        | LearnedPropositionExpr::Disjunction { members } => out.extend(members.iter().cloned()),
        LearnedPropositionExpr::Implication {
            antecedent,
            consequent,
        }
        | LearnedPropositionExpr::Counterfactual {
            antecedent,
            consequent,
        } => {
            out.insert(antecedent.clone());
            out.insert(consequent.clone());
        }
        LearnedPropositionExpr::Unless {
            condition,
            consequent,
        } => {
            out.insert(condition.clone());
            out.insert(consequent.clone());
        }
        LearnedPropositionExpr::Exists { body, .. }
        | LearnedPropositionExpr::ForAll { body, .. }
        | LearnedPropositionExpr::Cardinality { body, .. }
        | LearnedPropositionExpr::PluralScaleCardinality { body, .. }
        | LearnedPropositionExpr::GeneralizedQuantified { body, .. } => {
            out.insert(body.clone());
        }
        LearnedPropositionExpr::Focus { content, .. } => {
            out.insert(content.clone());
        }
        LearnedPropositionExpr::Presuppositional {
            asserted,
            presupposed,
        } => {
            out.insert(asserted.clone());
            out.insert(presupposed.clone());
        }
        LearnedPropositionExpr::Occurrence { .. } => {}
    }
    let _ = contents;
    Ok(())
}
fn validate_cycles(target: &LearnedSemanticTarget) -> Result<(), TrainingError> {
    fn visit(
        target: &LearnedSemanticTarget,
        id: &PropositionId,
        visiting: &mut BTreeSet<PropositionId>,
        done: &mut BTreeSet<PropositionId>,
    ) -> Result<(), TrainingError> {
        if done.contains(id) {
            return Ok(());
        }
        if !visiting.insert(id.clone()) {
            return Err(semantic_error(format!(
                "cyclic proposition/content dependency at {id}"
            )));
        }
        let mut deps = BTreeSet::new();
        proposition_dependencies(target, id, &mut deps, &mut BTreeSet::new())?;
        for dep in deps {
            visit(target, &dep, visiting, done)?;
        }
        visiting.remove(id);
        done.insert(id.clone());
        Ok(())
    }
    let mut done = BTreeSet::new();
    for id in target.propositions.keys() {
        visit(target, id, &mut BTreeSet::new(), &mut done)?;
    }
    Ok(())
}

#[derive(Default)]
struct Reach {
    refs: BTreeSet<ReferentId>,
    occs: BTreeSet<OccurrenceId>,
    vars: BTreeSet<VariableId>,
    props: BTreeSet<PropositionId>,
    contents: BTreeSet<ContentId>,
    ambiguities: BTreeSet<AmbiguityId>,
    spans: BTreeSet<LearnedSpan>,
}
fn reach_anchor(anchor: &LearnedAnchor, r: &mut Reach) {
    r.spans.extend(anchor.spans.iter().cloned());
}
fn reach_term(
    target: &LearnedSemanticTarget,
    term: &LearnedTerm,
    r: &mut Reach,
) -> Result<(), TrainingError> {
    match term {
        LearnedTerm::Referent(id) => {
            if r.refs.insert(id.clone()) {
                reach_anchor(&target.referents[id].anchor, r);
            }
        }
        LearnedTerm::Occurrence(id) => {
            if r.occs.insert(id.clone()) {
                reach_anchor(&target.occurrences[id].anchor, r);
            }
        }
        LearnedTerm::Proposition(id) => reach_prop(target, id, r)?,
        LearnedTerm::Variable(id) => {
            if r.vars.insert(id.clone()) {
                reach_anchor(&target.variables[id].anchor, r);
            }
        }
        LearnedTerm::Literal(v) => reach_anchor(&v.anchor, r),
        LearnedTerm::Context(_) => {}
    }
    Ok(())
}
fn reach_prop(
    target: &LearnedSemanticTarget,
    id: &PropositionId,
    r: &mut Reach,
) -> Result<(), TrainingError> {
    if !r.props.insert(id.clone()) {
        return Ok(());
    }
    let p = &target.propositions[id];
    if let Some(a) = &p.operator_anchor {
        reach_anchor(a, r);
    }
    match &p.expression {
        LearnedPropositionExpr::TypeAssertion { subject, .. } => reach_term(target, subject, r)?,
        LearnedPropositionExpr::Relation { arguments, .. } => {
            for t in arguments {
                reach_term(target, t, r)?;
            }
        }
        LearnedPropositionExpr::Occurrence { occurrence } => {
            r.occs.insert(occurrence.clone());
            reach_anchor(&target.occurrences[occurrence].anchor, r);
        }
        LearnedPropositionExpr::Equality { left, right } => {
            reach_term(target, left, r)?;
            reach_term(target, right, r)?;
        }
        LearnedPropositionExpr::Comparison {
            left,
            right,
            dimension,
            ..
        } => {
            reach_term(target, left, r)?;
            reach_term(target, right, r)?;
            if let Some(t) = dimension {
                reach_term(target, t, r)?;
            }
        }
        LearnedPropositionExpr::Negation { content } => reach_prop(target, content, r)?,
        LearnedPropositionExpr::ScopedOperator { operator, content } => {
            r.refs.insert(operator.clone());
            reach_anchor(&target.referents[operator].anchor, r);
            reach_prop(target, content, r)?;
        }
        LearnedPropositionExpr::Conjunction { members }
        | LearnedPropositionExpr::Disjunction { members } => {
            for p in members {
                reach_prop(target, p, r)?;
            }
        }
        LearnedPropositionExpr::Implication {
            antecedent,
            consequent,
        }
        | LearnedPropositionExpr::Counterfactual {
            antecedent,
            consequent,
        } => {
            reach_prop(target, antecedent, r)?;
            reach_prop(target, consequent, r)?;
        }
        LearnedPropositionExpr::Unless {
            condition,
            consequent,
        } => {
            reach_prop(target, condition, r)?;
            reach_prop(target, consequent, r)?;
        }
        LearnedPropositionExpr::Exists { variable, body }
        | LearnedPropositionExpr::ForAll { variable, body } => {
            r.vars.insert(variable.clone());
            reach_anchor(&target.variables[variable].anchor, r);
            reach_prop(target, body, r)?;
        }
        LearnedPropositionExpr::Cardinality {
            count,
            variable,
            body,
            ..
        } => {
            reach_anchor(&count.anchor, r);
            r.vars.insert(variable.clone());
            reach_anchor(&target.variables[variable].anchor, r);
            reach_prop(target, body, r)?;
        }
        LearnedPropositionExpr::PluralScaleCardinality {
            scale,
            variable,
            body,
        } => {
            reach_anchor(&scale.anchor, r);
            r.vars.insert(variable.clone());
            reach_anchor(&target.variables[variable].anchor, r);
            reach_prop(target, body, r)?;
        }
        LearnedPropositionExpr::GeneralizedQuantified {
            quantifier,
            variable,
            body,
        } => {
            r.refs.insert(quantifier.clone());
            reach_anchor(&target.referents[quantifier].anchor, r);
            r.vars.insert(variable.clone());
            reach_anchor(&target.variables[variable].anchor, r);
            reach_prop(target, body, r)?;
        }
        LearnedPropositionExpr::Focus {
            operator,
            focus,
            content,
        } => {
            r.refs.insert(operator.clone());
            reach_anchor(&target.referents[operator].anchor, r);
            reach_anchor(focus, r);
            reach_prop(target, content, r)?;
        }
        LearnedPropositionExpr::Presuppositional {
            asserted,
            presupposed,
        } => {
            reach_prop(target, asserted, r)?;
            reach_prop(target, presupposed, r)?;
        }
    }
    Ok(())
}
fn reach_content(
    target: &LearnedSemanticTarget,
    id: &ContentId,
    r: &mut Reach,
) -> Result<(), TrainingError> {
    if !r.contents.insert(id.clone()) {
        return Ok(());
    }
    let c = &target.contents[id];
    match &c.body {
        LearnedContentBody::Proposition { proposition } => reach_prop(target, proposition, r)?,
        LearnedContentBody::Question {
            condition,
            answer_variables,
            alternatives,
        } => {
            reach_prop(target, condition, r)?;
            for v in answer_variables {
                r.vars.insert(v.clone());
                reach_anchor(&target.variables[v].anchor, r);
            }
            for p in alternatives {
                reach_prop(target, p, r)?;
            }
        }
        LearnedContentBody::Quotation { quoted } => reach_anchor(quoted, r),
    }
    Ok(())
}
fn reach_ambiguity(
    target: &LearnedSemanticTarget,
    id: &AmbiguityId,
    r: &mut Reach,
) -> Result<(), TrainingError> {
    if !r.ambiguities.insert(id.clone()) {
        return Ok(());
    }
    for alt in &target.ambiguities[id].alternatives {
        match alt {
            LearnedReading::Proposition(p) => reach_prop(target, p, r)?,
            LearnedReading::Content(c) => reach_content(target, c, r)?,
        }
    }
    Ok(())
}
fn rooted_reach(target: &LearnedSemanticTarget) -> Result<Reach, TrainingError> {
    let mut r = Reach::default();
    for root in &target.roots {
        match root {
            LearnedRoot::Proposition(p) => reach_prop(target, p, &mut r)?,
            LearnedRoot::Content(c) => reach_content(target, c, &mut r)?,
            LearnedRoot::Ambiguity(a) => reach_ambiguity(target, a, &mut r)?,
        }
    }
    Ok(r)
}
fn validate_reachability(target: &LearnedSemanticTarget) -> Result<(), TrainingError> {
    let r = rooted_reach(target)?;
    if r.refs.len() != target.referents.len()
        || r.occs.len() != target.occurrences.len()
        || r.vars.len() != target.variables.len()
        || r.props.len() != target.propositions.len()
        || r.contents.len() != target.contents.len()
        || r.ambiguities.len() != target.ambiguities.len()
    {
        return Err(semantic_error(
            "learned target contains semantic objects unreachable from any root",
        ));
    }
    Ok(())
}
fn reading_footprint(
    target: &LearnedSemanticTarget,
    reading: &LearnedReading,
) -> Result<BTreeSet<LearnedSpan>, TrainingError> {
    let mut r = Reach::default();
    match reading {
        LearnedReading::Proposition(p) => reach_prop(target, p, &mut r)?,
        LearnedReading::Content(c) => reach_content(target, c, &mut r)?,
    }
    Ok(r.spans)
}

fn char_requires_semantic_coverage(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}
fn ranges_cover_lexical_text(text: &str, base: u64, ranges: &[(u64, u64)]) -> bool {
    text.char_indices().all(|(offset, ch)| {
        if !char_requires_semantic_coverage(ch) {
            return true;
        }
        let start = base.saturating_add(offset as u64);
        let end = start.saturating_add(ch.len_utf8() as u64);
        ranges
            .iter()
            .any(|(left, right)| *left <= start && *right >= end)
    })
}
fn validate_source_coverage(
    window: &TrainingWindow,
    target: &LearnedSemanticTarget,
) -> Result<(), TrainingError> {
    let reachable = rooted_reach(target)?;
    match window {
        TrainingWindow::Prose(window) => {
            let ranges = reachable
                .spans
                .iter()
                .filter_map(|span| match span {
                    LearnedSpan::Prose {
                        start_byte,
                        end_byte,
                    } => Some((*start_byte, *end_byte)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if !ranges_cover_lexical_text(&window.text, 0, &ranges) {
                return Err(semantic_error(
                    "model-visible prose contains lexical source material not claimed by rooted semantics",
                ));
            }
        }
        TrainingWindow::Structured(window) => {
            for field in &window.fields {
                if matches!(field.value, StructuredFieldValue::Opaque { .. }) {
                    continue;
                }
                let accepted = std::iter::once(field.field_path.as_str())
                    .chain(field.alias_paths.iter().map(String::as_str))
                    .collect::<BTreeSet<_>>();
                let matching = reachable
                    .spans
                    .iter()
                    .filter_map(|span| match span {
                        LearnedSpan::Structured {
                            field_path,
                            start_byte,
                            end_byte,
                        } if accepted.contains(field_path.as_str()) => {
                            Some((*start_byte, *end_byte))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                match &field.value {
                    StructuredFieldValue::Json { value } if value.is_string() => {
                        let text = value
                            .as_str()
                            .ok_or_else(|| semantic_error("string field lost string value"))?;
                        let base = field.source_start_byte.ok_or_else(|| {
                            semantic_error(format!(
                                "string field {} lacks source range",
                                field.field_path
                            ))
                        })?;
                        let ranges = matching
                            .into_iter()
                            .filter_map(|(start, end)| start.zip(end))
                            .collect::<Vec<_>>();
                        if !ranges_cover_lexical_text(text, base, &ranges) {
                            return Err(semantic_error(format!(
                                "model-visible structured string field {} contains lexical material not claimed by rooted semantics",
                                field.field_path
                            )));
                        }
                    }
                    StructuredFieldValue::Json { .. } => {
                        if !matching
                            .iter()
                            .any(|(start, end)| start.is_none() && end.is_none())
                        {
                            return Err(semantic_error(format!(
                                "model-visible structured field {} is not semantically claimed at its exact path",
                                field.field_path
                            )));
                        }
                    }
                    StructuredFieldValue::Opaque { .. } => unreachable!(),
                }
            }
        }
    }
    Ok(())
}
fn grounded_literals(target: &LearnedSemanticTarget) -> Vec<&GroundedLiteral> {
    let mut out = Vec::new();
    for p in target.propositions.values() {
        collect_literals_expr(&p.expression, &mut out);
    }
    out
}
fn collect_literals_expr<'a>(expr: &'a LearnedPropositionExpr, out: &mut Vec<&'a GroundedLiteral>) {
    let mut term = |t: &'a LearnedTerm| {
        if let LearnedTerm::Literal(v) = t {
            out.push(v);
        }
    };
    match expr {
        LearnedPropositionExpr::TypeAssertion { subject, .. } => term(subject),
        LearnedPropositionExpr::Relation { arguments, .. } => {
            for t in arguments {
                term(t);
            }
        }
        LearnedPropositionExpr::Equality { left, right } => {
            term(left);
            term(right);
        }
        LearnedPropositionExpr::Comparison {
            left,
            right,
            dimension,
            ..
        } => {
            term(left);
            term(right);
            if let Some(t) = dimension {
                term(t);
            }
        }
        LearnedPropositionExpr::Cardinality { count, .. } => out.push(count),
        LearnedPropositionExpr::PluralScaleCardinality { scale, .. } => out.push(scale),
        _ => {}
    }
}
fn validate_grounded_literal(
    window: &TrainingWindow,
    _target: &LearnedSemanticTarget,
    value: &GroundedLiteral,
) -> Result<(), TrainingError> {
    validate_learned_literal_shape(&value.value)?;
    if value.anchor.spans.len() != 1 {
        return Err(semantic_error(
            "grounded literal must use exactly one exact source span",
        ));
    }
    let span = value.anchor.spans.iter().next().expect("checked one span");
    match (window, span) {
        (
            TrainingWindow::Prose(w),
            LearnedSpan::Prose {
                start_byte,
                end_byte,
            },
        ) => {
            let text = w
                .text
                .get(
                    usize::try_from(*start_byte).unwrap_or(usize::MAX)
                        ..usize::try_from(*end_byte).unwrap_or(usize::MAX),
                )
                .ok_or_else(|| semantic_error("invalid prose literal span"))?;
            if !learned_literal_matches_text(&value.value, text) {
                return Err(semantic_error(format!(
                    "literal {:?} disagrees with exact source {text:?}",
                    value.value
                )));
            }
        }
        (
            TrainingWindow::Structured(w),
            LearnedSpan::Structured {
                field_path,
                start_byte,
                end_byte,
            },
        ) => {
            match structured_source_value(w, field_path)
                .ok_or_else(|| semantic_error("invalid structured literal span"))?
            {
                StructuredSourceValue::String {
                    text, base_start, ..
                } => {
                    let exact = match (start_byte, end_byte) {
                        (Some(s), Some(e)) => text
                            .get(
                                usize::try_from(s.checked_sub(base_start).ok_or_else(|| {
                                    semantic_error("literal span precedes fragment")
                                })?)
                                .unwrap_or(usize::MAX)
                                    ..usize::try_from(e.checked_sub(base_start).ok_or_else(
                                        || semantic_error("literal span precedes fragment"),
                                    )?)
                                    .unwrap_or(usize::MAX),
                            )
                            .ok_or_else(|| semantic_error("invalid structured literal bytes"))?,
                        (None, None) => text,
                        _ => return Err(semantic_error("incomplete structured literal bytes")),
                    };
                    if !learned_literal_matches_text(&value.value, exact) {
                        return Err(semantic_error(format!(
                            "literal {:?} disagrees with exact structured string {exact:?}",
                            value.value
                        )));
                    }
                }
                StructuredSourceValue::Scalar(scalar) => {
                    if !learned_literal_matches_scalar(&value.value, scalar) {
                        return Err(semantic_error(format!(
                            "literal {:?} disagrees with exact structured scalar {scalar}",
                            value.value
                        )));
                    }
                }
                StructuredSourceValue::Container => {
                    return Err(semantic_error(
                        "literal cannot be grounded by a structured container; type a semantic object instead",
                    ));
                }
                StructuredSourceValue::Opaque => {
                    return Err(semantic_error(
                        "literal cannot claim an externalized opaque field",
                    ));
                }
            }
        }
        _ => return Err(semantic_error("literal span/window mismatch")),
    }
    Ok(())
}

fn learned_numeric_literal(value: &LearnedLiteral) -> bool {
    matches!(
        value,
        LearnedLiteral::Integer { .. }
            | LearnedLiteral::Decimal { .. }
            | LearnedLiteral::Ratio { .. }
            | LearnedLiteral::Percentage { .. }
            | LearnedLiteral::Approximate { .. }
            | LearnedLiteral::Interval { .. }
            | LearnedLiteral::PluralScale { .. }
            | LearnedLiteral::Arithmetic { .. }
    )
}
fn learned_numeric_point_literal(value: &LearnedLiteral) -> bool {
    matches!(
        value,
        LearnedLiteral::Integer { .. }
            | LearnedLiteral::Decimal { .. }
            | LearnedLiteral::Ratio { .. }
            | LearnedLiteral::Percentage { .. }
            | LearnedLiteral::Approximate { .. }
            | LearnedLiteral::Arithmetic { .. }
    )
}
fn validate_learned_literal_shape(value: &LearnedLiteral) -> Result<(), TrainingError> {
    match value {
        LearnedLiteral::Integer { value } | LearnedLiteral::Decimal { value } => {
            let canonical = canonical_number(value)
                .ok_or_else(|| semantic_error(format!("invalid numeric literal {value:?}")))?;
            if &canonical != value {
                return Err(semantic_error(format!(
                    "numeric literal must use canonical mathematical value {canonical:?}, found {value:?}"
                )));
            }
        }
        LearnedLiteral::Ratio {
            numerator,
            denominator,
        } => {
            validate_learned_literal_shape(numerator)?;
            validate_learned_literal_shape(denominator)?;
            if !learned_numeric_literal(numerator) || !learned_numeric_literal(denominator) {
                return Err(semantic_error("ratio operands must be numeric"));
            }
            if literal_is_zero(denominator) {
                return Err(semantic_error("ratio denominator must be nonzero"));
            }
        }
        LearnedLiteral::Percentage { magnitude } => {
            validate_learned_literal_shape(magnitude)?;
            let exact = match &**magnitude {
                LearnedLiteral::Integer { .. }
                | LearnedLiteral::Decimal { .. }
                | LearnedLiteral::Ratio { .. } => true,
                LearnedLiteral::Arithmetic { .. } => true,
                _ => false,
            };
            if !exact {
                return Err(semantic_error(
                    "percentage magnitude must be an exact numeric expression; put approximation outside the percentage",
                ));
            }
        }
        LearnedLiteral::Approximate { value: magnitude } => {
            validate_learned_literal_shape(magnitude)?;
            if !learned_numeric_literal(magnitude) {
                return Err(semantic_error("approximation operand must be numeric"));
            }
        }
        LearnedLiteral::Interval { lower, upper, .. } => {
            validate_learned_literal_shape(lower)?;
            validate_learned_literal_shape(upper)?;
            if !learned_numeric_point_literal(lower) || !learned_numeric_point_literal(upper) {
                return Err(semantic_error(
                    "interval bounds must be numeric point values",
                ));
            }
        }
        LearnedLiteral::PluralScale { base } => {
            validate_learned_literal_shape(base)?;
            if !matches!(&**base,LearnedLiteral::Integer{value} if !value.starts_with('-')) {
                return Err(semantic_error(
                    "plural-scale base must be a nonnegative canonical integer",
                ));
            }
        }
        LearnedLiteral::Arithmetic { left, right, .. } => {
            validate_learned_literal_shape(left)?;
            validate_learned_literal_shape(right)?;
            if !learned_numeric_literal(left) || !learned_numeric_literal(right) {
                return Err(semantic_error("arithmetic operands must be numeric"));
            }
        }
        LearnedLiteral::Measurement { magnitude, unit } => {
            validate_learned_literal_shape(magnitude)?;
            if !learned_numeric_literal(magnitude) {
                return Err(semantic_error("measurement magnitude must be numeric"));
            }
            if unit.is_empty() || unit.trim() != unit {
                return Err(semantic_error(
                    "measurement unit must be nonempty and exact",
                ));
            }
        }
        LearnedLiteral::String { .. } | LearnedLiteral::Boolean { .. } | LearnedLiteral::Null => {}
    }
    Ok(())
}

fn learned_literal_matches_scalar(value: &LearnedLiteral, scalar: &serde_json::Value) -> bool {
    match (value, scalar) {
        (LearnedLiteral::String { value }, serde_json::Value::String(actual)) => value == actual,
        (LearnedLiteral::Integer { value }, serde_json::Value::Number(actual)) => {
            canonical_number(value) == canonical_number(&actual.to_string())
                && canonical_number(value).is_some_and(|v| !v.contains('.'))
        }
        (LearnedLiteral::Decimal { value }, serde_json::Value::Number(actual)) => {
            canonical_number(value) == canonical_number(&actual.to_string())
        }
        (LearnedLiteral::Boolean { value }, serde_json::Value::Bool(actual)) => value == actual,
        (LearnedLiteral::Null, serde_json::Value::Null) => true,
        _ => false,
    }
}

fn learned_literal_matches_text(value: &LearnedLiteral, text: &str) -> bool {
    match value {
        LearnedLiteral::String { value } => value == text,
        LearnedLiteral::Integer { value } => {
            canonical_number(value) == canonical_number(text)
                && canonical_number(value).is_some_and(|v| !v.contains('.'))
        }
        LearnedLiteral::Decimal { value } => canonical_number(value) == canonical_number(text),
        LearnedLiteral::Boolean { value } => text == if *value { "true" } else { "false" },
        LearnedLiteral::Null => text == "null",
        LearnedLiteral::Ratio {
            numerator,
            denominator,
        } => split_binary(text, '/').is_some_and(|(left, right)| {
            learned_literal_matches_text(numerator, left.trim())
                && learned_literal_matches_text(denominator, right.trim())
                && !literal_is_zero(denominator)
        }),
        LearnedLiteral::Percentage { magnitude } => text
            .strip_suffix('%')
            .is_some_and(|body| learned_literal_matches_text(magnitude, body.trim())),
        LearnedLiteral::Approximate { value } => {
            let body = text
                .strip_prefix('~')
                .or_else(|| text.strip_prefix('≈'))
                .or_else(|| text.strip_prefix("about "))
                .or_else(|| text.strip_prefix("approximately "));
            body.is_some_and(|body| learned_literal_matches_text(value, body.trim()))
        }
        LearnedLiteral::Interval { lower, upper, .. } => {
            let normalized = text.replace(['–', '—'], "-");
            split_interval(&normalized).is_some_and(|(left, right)| {
                learned_literal_matches_text(lower, left.trim())
                    && learned_literal_matches_text(upper, right.trim())
            })
        }
        LearnedLiteral::PluralScale { base } => {
            if text
                .strip_suffix('s')
                .is_some_and(|body| learned_literal_matches_text(base, body.trim()))
            {
                true
            } else {
                plural_scale_word_base(text).is_some_and(
                    |expected| matches!(&**base,LearnedLiteral::Integer{value} if value==expected),
                )
            }
        }
        LearnedLiteral::Arithmetic {
            operator,
            left,
            right,
        } => {
            let marker = match operator {
                LearnedArithmeticOperator::Add => '+',
                LearnedArithmeticOperator::Subtract => '-',
                LearnedArithmeticOperator::Multiply => '*',
            };
            split_binary(text, marker).is_some_and(|(a, b)| {
                learned_literal_matches_text(left, a.trim())
                    && learned_literal_matches_text(right, b.trim())
            })
        }
        LearnedLiteral::Measurement { magnitude, unit } => {
            text.strip_suffix(unit).is_some_and(|body| {
                !unit.is_empty() && learned_literal_matches_text(magnitude, body.trim_end())
            })
        }
    }
}
fn plural_scale_word_base(text: &str) -> Option<&'static str> {
    match text.trim().to_ascii_lowercase().as_str() {
        "hundreds" => Some("100"),
        "thousands" => Some("1000"),
        "millions" => Some("1000000"),
        "billions" => Some("1000000000"),
        "trillions" => Some("1000000000000"),
        _ => None,
    }
}

fn split_binary(text: &str, marker: char) -> Option<(&str, &str)> {
    let mut found = text.match_indices(marker);
    let (index, _) = found.next()?;
    if found.next().is_some() {
        return None;
    }
    Some((&text[..index], &text[index + marker.len_utf8()..]))
}
fn split_interval(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    for index in 1..bytes.len() {
        if bytes[index] == b'-'
            && bytes[..index].iter().any(u8::is_ascii_digit)
            && bytes[index + 1..].iter().any(u8::is_ascii_digit)
        {
            return Some((&text[..index], &text[index + 1..]));
        }
    }
    None
}
fn literal_is_zero(value: &LearnedLiteral) -> bool {
    match value {
        LearnedLiteral::Integer { value } | LearnedLiteral::Decimal { value } => {
            canonical_number(value).as_deref() == Some("0")
        }
        _ => false,
    }
}
fn canonical_number(input: &str) -> Option<String> {
    let s = input.trim().replace([',', '_'], "");
    if s.is_empty() {
        return None;
    }
    let (mantissa, exp) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], s[i + 1..].parse::<i32>().ok()?),
        None => (s.as_str(), 0),
    };
    let (neg, m) = if let Some(v) = mantissa.strip_prefix('-') {
        (true, v)
    } else if let Some(v) = mantissa.strip_prefix('+') {
        (false, v)
    } else {
        (false, mantissa)
    };
    let mut parts = m.split('.');
    let whole = parts.next()?;
    let frac = parts.next().unwrap_or("");
    if parts.next().is_some()
        || whole.is_empty() && frac.is_empty()
        || !whole
            .chars()
            .chain(frac.chars())
            .all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let mut digits = format!("{whole}{frac}");
    let mut scale = i32::try_from(frac.len()).ok()?.saturating_sub(exp);
    while digits.starts_with('0') && digits.len() > 1 {
        digits.remove(0);
    }
    if digits.chars().all(|c| c == '0') {
        return Some("0".into());
    }
    while scale > 0 && digits.ends_with('0') {
        digits.pop();
        scale -= 1;
    }
    let mut out = if scale <= 0 {
        format!("{}{}", digits, "0".repeat(usize::try_from(-scale).ok()?))
    } else {
        let scale_usize = usize::try_from(scale).ok()?;
        if scale_usize >= digits.len() {
            format!("0.{}{}", "0".repeat(scale_usize - digits.len()), digits)
        } else {
            let split = digits.len() - scale_usize;
            format!("{}.{}", &digits[..split], &digits[split..])
        }
    };
    if neg {
        out.insert(0, '-');
    }
    Some(out)
}
fn compile_literal(value: &LearnedLiteral) -> Literal {
    match value {
        LearnedLiteral::String { value } => Literal::String(value.clone()),
        LearnedLiteral::Integer { value } => Literal::Integer(value.clone()),
        LearnedLiteral::Decimal { value } => Literal::Decimal(value.clone()),
        LearnedLiteral::Boolean { value } => Literal::Boolean(*value),
        LearnedLiteral::Null => Literal::Null,
        LearnedLiteral::Ratio {
            numerator,
            denominator,
        } => Literal::Ratio {
            numerator: Box::new(compile_literal(numerator)),
            denominator: Box::new(compile_literal(denominator)),
        },
        LearnedLiteral::Percentage { magnitude } => Literal::Percentage {
            magnitude: Box::new(compile_literal(magnitude)),
        },
        LearnedLiteral::Approximate { value } => Literal::Approximate {
            value: Box::new(compile_literal(value)),
        },
        LearnedLiteral::Interval {
            lower,
            upper,
            lower_inclusive,
            upper_inclusive,
        } => Literal::Interval {
            lower: Box::new(compile_literal(lower)),
            upper: Box::new(compile_literal(upper)),
            lower_inclusive: *lower_inclusive,
            upper_inclusive: *upper_inclusive,
        },
        LearnedLiteral::PluralScale { base } => Literal::PluralScale {
            base: Box::new(compile_literal(base)),
        },
        LearnedLiteral::Arithmetic {
            operator,
            left,
            right,
        } => Literal::Arithmetic {
            operator: match operator {
                LearnedArithmeticOperator::Add => muse_occurrence::ArithmeticOperator::Add,
                LearnedArithmeticOperator::Subtract => {
                    muse_occurrence::ArithmeticOperator::Subtract
                }
                LearnedArithmeticOperator::Multiply => {
                    muse_occurrence::ArithmeticOperator::Multiply
                }
            },
            left: Box::new(compile_literal(left)),
            right: Box::new(compile_literal(right)),
        },
        LearnedLiteral::Measurement { magnitude, unit } => Literal::Measurement {
            magnitude: Box::new(compile_literal(magnitude)),
            unit: unit.clone(),
        },
    }
}
fn anchor_text(window: &TrainingWindow, anchor: &LearnedAnchor) -> Result<String, TrainingError> {
    let mut pieces = Vec::new();
    for span in &anchor.spans {
        let text = match (window, span) {
            (
                TrainingWindow::Prose(w),
                LearnedSpan::Prose {
                    start_byte,
                    end_byte,
                },
            ) => w
                .text
                .get(
                    usize::try_from(*start_byte).unwrap_or(usize::MAX)
                        ..usize::try_from(*end_byte).unwrap_or(usize::MAX),
                )
                .ok_or_else(|| semantic_error("invalid prose literal span"))?
                .to_string(),
            (
                TrainingWindow::Structured(w),
                LearnedSpan::Structured {
                    field_path,
                    start_byte,
                    end_byte,
                },
            ) => match structured_source_value(w, field_path)
                .ok_or_else(|| semantic_error("invalid structured literal span"))?
            {
                StructuredSourceValue::String {
                    text, base_start, ..
                } => match (start_byte, end_byte) {
                    (Some(s), Some(e)) => text
                        .get(
                            usize::try_from(
                                s.checked_sub(base_start)
                                    .ok_or_else(|| semantic_error("span precedes fragment"))?,
                            )
                            .unwrap_or(usize::MAX)
                                ..usize::try_from(
                                    e.checked_sub(base_start)
                                        .ok_or_else(|| semantic_error("span precedes fragment"))?,
                                )
                                .unwrap_or(usize::MAX),
                        )
                        .ok_or_else(|| semantic_error("invalid structured literal bytes"))?
                        .to_string(),
                    (None, None) => text.to_string(),
                    _ => return Err(semantic_error("incomplete structured literal bytes")),
                },
                StructuredSourceValue::Scalar(value) => serde_json::to_string(value)?,
                StructuredSourceValue::Container => {
                    return Err(semantic_error("text anchor targets structured container"));
                }
                StructuredSourceValue::Opaque => {
                    return Err(semantic_error("text anchor targets opaque field"));
                }
            },
            _ => return Err(semantic_error("literal span/window mismatch")),
        };
        pieces.push(text);
    }
    Ok(pieces.join(""))
}

struct Compiler<'a> {
    window: &'a TrainingWindow,
    target: &'a LearnedSemanticTarget,
    index: &'a OntologyIndex,
    doc: OccurrenceDocument,
    context: BTreeMap<ContextTerm, Term>,
    span_ids: BTreeMap<LearnedSpan, SourceSpanId>,
    generated: u64,
}
impl<'a> Compiler<'a> {
    fn new(
        window: &'a TrainingWindow,
        target: &'a LearnedSemanticTarget,
        ontology: RegistrySnapshot,
        derivation: Derivation,
        index: &'a OntologyIndex,
    ) -> Result<Self, TrainingError> {
        let mut c = Self {
            window,
            target,
            index,
            doc: OccurrenceDocument {
                schema_version: SCHEMA_VERSION.into(),
                id: OccurrenceDocumentId::from(format!("occurrence-document:{}", window.id())),
                ontology,
                derivation,
                source_spans: BTreeMap::new(),
                referents: BTreeMap::new(),
                occurrences: BTreeMap::new(),
                variables: BTreeMap::new(),
                propositions: BTreeMap::new(),
                statements: BTreeMap::new(),
                ambiguities: BTreeMap::new(),
                statement_order: Vec::new(),
            },
            context: BTreeMap::new(),
            span_ids: BTreeMap::new(),
            generated: 0,
        };
        c.compile_spans()?;
        c.compile_context()?;
        Ok(c)
    }
    fn compile(mut self) -> Result<OccurrenceDocument, TrainingError> {
        for r in self.target.referents.values() {
            let spans = self.anchor(&r.anchor);
            self.doc.referents.insert(
                r.id.clone(),
                Referent {
                    id: r.id.clone(),
                    types: BTreeSet::from([r.sort.clone()]),
                    lexical_anchor: Some(LexicalAnchor {
                        spans: spans.clone(),
                    }),
                    labels: BTreeSet::new(),
                    external_ids: BTreeMap::new(),
                    discourse_roles: BTreeSet::new(),
                    source_spans: spans,
                },
            );
        }
        for o in self.target.occurrences.values() {
            let spans = self.anchor(&o.anchor);
            self.doc.occurrences.insert(
                o.id.clone(),
                Occurrence {
                    id: o.id.clone(),
                    types: BTreeSet::from([o.sort.clone()]),
                    lexical_anchor: Some(LexicalAnchor {
                        spans: spans.clone(),
                    }),
                    participants: Vec::new(),
                    attributes: BTreeMap::new(),
                    tense_aspect: None,
                    grammatical_tense: o.tense,
                    grammatical_voice: None,
                    grammatical_spans: BTreeSet::new(),
                    reported_outcome: None,
                    source_spans: spans,
                    evidence: BTreeSet::new(),
                },
            );
        }
        for v in self.target.variables.values() {
            let spans = self.anchor(&v.anchor);
            self.doc.variables.insert(
                v.id.clone(),
                SemanticVariable {
                    id: v.id.clone(),
                    sort: v.sort.clone(),
                    lexical_anchor: Some(LexicalAnchor {
                        spans: spans.clone(),
                    }),
                    source_spans: spans,
                },
            );
        }
        for p in self.target.propositions.values() {
            let expr = self.compile_expr(&p.expression)?;
            let spans = self.prop_footprint(&p.id)?;
            self.doc.propositions.insert(
                p.id.clone(),
                Proposition {
                    id: p.id.clone(),
                    expression: expr,
                    operator_spans: p
                        .operator_anchor
                        .as_ref()
                        .map_or_else(BTreeSet::new, |a| self.anchor(a)),
                    operator_tense_aspect: None,
                    operator_voice: None,
                    operator_grammatical_spans: BTreeSet::new(),
                    source_spans: spans,
                    evidence: BTreeSet::new(),
                },
            );
        }
        self.inject_structured_shell()?;
        for root in &self.target.roots {
            match root {
                LearnedRoot::Proposition(id) => self.root_proposition(
                    id,
                    if matches!(self.window, TrainingWindow::Prose(_)) {
                        PresentationMode::Assertion
                    } else {
                        PresentationMode::Record
                    },
                )?,
                LearnedRoot::Content(id) => self.root_content(id)?,
                LearnedRoot::Ambiguity(id) => self.compile_ambiguity(id)?,
            }
        }
        validate_compiled_v6_contract(&self.doc)?;
        self.doc.validate()?;
        Ok(self.doc)
    }
    fn compile_spans(&mut self) -> Result<(), TrainingError> {
        let spans = rooted_reach(self.target)?.spans;
        for (index, span) in spans.into_iter().enumerate() {
            let id = SourceSpanId::from(format!("v6-span-{index}"));
            let rich = match (self.window, &span) {
                (
                    TrainingWindow::Prose(w),
                    LearnedSpan::Prose {
                        start_byte,
                        end_byte,
                    },
                ) => {
                    let (block_index, block) = w
                        .blocks
                        .iter()
                        .enumerate()
                        .find(|(_, b)| *start_byte >= b.start_byte && *end_byte <= b.end_byte)
                        .ok_or_else(|| {
                            semantic_error(format!("prose span {span:?} has no containing block"))
                        })?;
                    SourceSpan {
                        id: id.clone(),
                        source: w.semantic_source(),
                        run: w.run.clone(),
                        turn: None,
                        message: Some(block.message.clone()),
                        block: Some(
                            u32::try_from(block_index)
                                .map_err(|_| semantic_error("too many prose blocks"))?,
                        ),
                        bytes: Some(ByteRange {
                            start: *start_byte,
                            end: *end_byte,
                        }),
                        field_path: None,
                    }
                }
                (
                    TrainingWindow::Structured(w),
                    LearnedSpan::Structured {
                        field_path,
                        start_byte,
                        end_byte,
                    },
                ) => {
                    let block_index = w
                        .fields
                        .iter()
                        .position(|b| {
                            b.field_path == *field_path
                                || b.alias_paths.iter().any(|a| a == field_path)
                                || b.container_paths.iter().any(|a| a == field_path)
                                || b.context.iter().any(|c| c.field_path == *field_path)
                        })
                        .ok_or_else(|| {
                            semantic_error(format!("structured span {span:?} has no source field"))
                        })?;
                    SourceSpan {
                        id: id.clone(),
                        source: w.semantic_source(),
                        run: w.run.clone(),
                        turn: None,
                        message: Some(w.message.clone()),
                        block: Some(
                            u32::try_from(block_index)
                                .map_err(|_| semantic_error("too many structured fields"))?,
                        ),
                        bytes: (*start_byte)
                            .zip(*end_byte)
                            .map(|(start, end)| ByteRange { start, end }),
                        field_path: Some(field_path.clone()),
                    }
                }
                _ => return Err(semantic_error("span/window mismatch")),
            };
            self.doc.source_spans.insert(id.clone(), rich);
            self.span_ids.insert(span, id);
        }
        Ok(())
    }
    fn anchor(&self, a: &LearnedAnchor) -> BTreeSet<SourceSpanId> {
        a.spans
            .iter()
            .map(|span| self.span_ids[span].clone())
            .collect()
    }

    fn inject_referent(
        &mut self,
        id: &str,
        sort: &str,
        external: Option<(&str, &str)>,
        role: Option<DiscourseRole>,
    ) -> ReferentId {
        let id = ReferentId::from(id);
        let mut external_ids = BTreeMap::new();
        if let Some((k, v)) = external {
            external_ids.insert(k.into(), v.into());
        }
        let mut roles = BTreeSet::new();
        if let Some(role) = role {
            roles.insert(role);
        }
        self.doc.referents.insert(
            id.clone(),
            Referent {
                id: id.clone(),
                types: BTreeSet::from([ConceptId::from(sort)]),
                lexical_anchor: None,
                labels: BTreeSet::new(),
                external_ids,
                discourse_roles: roles,
                source_spans: BTreeSet::new(),
            },
        );
        id
    }
    fn inject_occurrence(&mut self, id: &str, sort: &str) -> OccurrenceId {
        let id = OccurrenceId::from(id);
        self.doc.occurrences.insert(
            id.clone(),
            Occurrence {
                id: id.clone(),
                types: BTreeSet::from([ConceptId::from(sort)]),
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
            },
        );
        id
    }
    fn compile_context(&mut self) -> Result<(), TrainingError> {
        match self.window {
            TrainingWindow::Prose(w) => {
                let id = self.inject_referent(
                    CONTEXT_SPEAKER,
                    actor_sort(w.speaker_role),
                    Some((WINDOW_SPEAKER_EXTERNAL_ID, &w.speaker_id)),
                    Some(w.speaker_role.discourse_role()),
                );
                self.context
                    .insert(ContextTerm::Speaker, Term::Referent(id));
                if let (Some(addressee_id), Some(addressee_role)) =
                    (&w.addressee_id, w.addressee_role)
                {
                    let addressee = self.inject_referent(
                        "muse-context-addressee",
                        actor_sort(addressee_role),
                        Some((WINDOW_ADDRESSEE_EXTERNAL_ID, addressee_id)),
                        Some(addressee_role.discourse_role()),
                    );
                    self.context
                        .insert(ContextTerm::Addressee, Term::Referent(addressee));
                }
            }
            TrainingWindow::Structured(w) => {
                let actor = self.inject_referent(
                    CONTEXT_ACTOR,
                    actor_sort(w.actor_role),
                    Some((WINDOW_ACTOR_EXTERNAL_ID, &w.actor_id)),
                    Some(w.actor_role.discourse_role()),
                );
                self.context
                    .insert(ContextTerm::Actor, Term::Referent(actor));
                if let (Some(id), Some(role)) = (&w.recorder_id, w.recorder_role) {
                    let recorder = self.inject_referent(
                        CONTEXT_RECORDER,
                        actor_sort(role),
                        Some((WINDOW_RECORDER_EXTERNAL_ID, id)),
                        Some(role.discourse_role()),
                    );
                    self.context
                        .insert(ContextTerm::Recorder, Term::Referent(recorder));
                }
                if w.tool_name.is_some() {
                    let tool =
                        self.inject_referent(CONTEXT_TOOL, "Tool", None, Some(DiscourseRole::Tool));
                    self.context.insert(ContextTerm::Tool, Term::Referent(tool));
                }
                if matches!(w.channel, StructuredChannel::ToolCall) || w.invocation_id.is_some() {
                    let invocation = self.inject_occurrence(CONTEXT_INVOCATION, "ToolInvocation");
                    self.context
                        .insert(ContextTerm::Invocation, Term::Occurrence(invocation));
                }
                match w.channel {
                    StructuredChannel::ToolResult => {
                        let id = self.inject_referent(CONTEXT_RESULT, "ToolResult", None, None);
                        self.context.insert(ContextTerm::Result, Term::Referent(id));
                    }
                    StructuredChannel::ToolCallUpdate => {
                        let id = self.inject_referent(
                            CONTEXT_UPDATE,
                            "ToolInvocationUpdate",
                            None,
                            None,
                        );
                        self.context.insert(ContextTerm::Update, Term::Referent(id));
                    }
                    StructuredChannel::ToolCall => {}
                }
            }
        }
        Ok(())
    }
    fn term(&self, t: &LearnedTerm) -> Result<Term, TrainingError> {
        Ok(match t {
            LearnedTerm::Referent(id) => Term::Referent(id.clone()),
            LearnedTerm::Occurrence(id) => Term::Occurrence(id.clone()),
            LearnedTerm::Proposition(id) => Term::Proposition(id.clone()),
            LearnedTerm::Variable(id) => Term::Variable(id.clone()),
            LearnedTerm::Literal(v) => Term::Literal(compile_literal(&v.value)),
            LearnedTerm::Context(c) => self
                .context
                .get(c)
                .cloned()
                .ok_or_else(|| semantic_error(format!("missing compiler context for {c:?}")))?,
        })
    }
    fn compile_expr(&self, e: &LearnedPropositionExpr) -> Result<PropositionExpr, TrainingError> {
        Ok(match e {
            LearnedPropositionExpr::TypeAssertion { subject, r#type } => {
                PropositionExpr::TypeAssertion {
                    subject: self.term(subject)?,
                    r#type: r#type.clone(),
                }
            }
            LearnedPropositionExpr::Relation {
                relation,
                arguments,
            } => PropositionExpr::Relation {
                relation: relation.clone(),
                arguments: arguments
                    .iter()
                    .map(|t| self.term(t))
                    .collect::<Result<_, _>>()?,
            },
            LearnedPropositionExpr::Occurrence { occurrence } => PropositionExpr::Occurrence {
                occurrence: occurrence.clone(),
            },
            LearnedPropositionExpr::Equality { left, right } => PropositionExpr::Equality {
                left: self.term(left)?,
                right: self.term(right)?,
            },
            LearnedPropositionExpr::Comparison {
                operator,
                left,
                right,
                dimension,
            } => PropositionExpr::Comparison {
                operator: *operator,
                left: self.term(left)?,
                right: self.term(right)?,
                dimension: dimension.as_ref().map(|t| self.term(t)).transpose()?,
            },
            LearnedPropositionExpr::Negation { content } => PropositionExpr::Negation {
                content: content.clone(),
            },
            LearnedPropositionExpr::Conjunction { members } => PropositionExpr::Conjunction {
                members: members.clone(),
            },
            LearnedPropositionExpr::Disjunction { members } => PropositionExpr::Disjunction {
                members: members.clone(),
            },
            LearnedPropositionExpr::Implication {
                antecedent,
                consequent,
            } => PropositionExpr::Implication {
                antecedent: antecedent.clone(),
                consequent: consequent.clone(),
            },
            LearnedPropositionExpr::Counterfactual {
                antecedent,
                consequent,
            } => PropositionExpr::Counterfactual {
                antecedent: antecedent.clone(),
                consequent: consequent.clone(),
            },
            LearnedPropositionExpr::Unless {
                condition,
                consequent,
            } => PropositionExpr::Unless {
                condition: condition.clone(),
                consequent: consequent.clone(),
            },
            LearnedPropositionExpr::Exists { variable, body } => PropositionExpr::Quantified {
                quantifier: Quantifier::Exists,
                variable: variable.clone(),
                domain: Some(self.target.variables[variable].sort.clone()),
                body: body.clone(),
            },
            LearnedPropositionExpr::ForAll { variable, body } => PropositionExpr::Quantified {
                quantifier: Quantifier::ForAll,
                variable: variable.clone(),
                domain: Some(self.target.variables[variable].sort.clone()),
                body: body.clone(),
            },
            LearnedPropositionExpr::Cardinality {
                comparator,
                count,
                variable,
                body,
            } => {
                let q = match comparator {
                    CardinalityComparator::Exactly => {
                        Quantifier::Exactly(compile_literal(&count.value))
                    }
                    CardinalityComparator::AtLeast => {
                        Quantifier::AtLeast(compile_literal(&count.value))
                    }
                    CardinalityComparator::AtMost => {
                        Quantifier::AtMost(compile_literal(&count.value))
                    }
                    CardinalityComparator::MoreThan => {
                        Quantifier::MoreThan(compile_literal(&count.value))
                    }
                    CardinalityComparator::FewerThan => {
                        Quantifier::FewerThan(compile_literal(&count.value))
                    }
                    CardinalityComparator::Approximately => {
                        Quantifier::Approximately(compile_literal(&count.value))
                    }
                };
                PropositionExpr::Quantified {
                    quantifier: q,
                    variable: variable.clone(),
                    domain: Some(self.target.variables[variable].sort.clone()),
                    body: body.clone(),
                }
            }
            LearnedPropositionExpr::PluralScaleCardinality {
                scale,
                variable,
                body,
            } => PropositionExpr::Quantified {
                quantifier: Quantifier::PluralScale(compile_literal(&scale.value)),
                variable: variable.clone(),
                domain: Some(self.target.variables[variable].sort.clone()),
                body: body.clone(),
            },
            LearnedPropositionExpr::GeneralizedQuantified {
                quantifier,
                variable,
                body,
            } => PropositionExpr::GeneralizedQuantified {
                quantifier: quantifier.clone(),
                variable: variable.clone(),
                domain: Some(self.target.variables[variable].sort.clone()),
                body: body.clone(),
            },
            LearnedPropositionExpr::ScopedOperator { operator, content } => {
                PropositionExpr::ScopedOperator {
                    operator: operator.clone(),
                    content: content.clone(),
                }
            }
            LearnedPropositionExpr::Focus {
                operator,
                focus,
                content,
            } => PropositionExpr::Focus {
                operator: operator.clone(),
                focus: LexicalAnchor {
                    spans: self.anchor(focus),
                },
                content: content.clone(),
            },
            LearnedPropositionExpr::Presuppositional {
                asserted,
                presupposed,
            } => PropositionExpr::Presuppositional {
                asserted: asserted.clone(),
                presupposed: presupposed.clone(),
            },
        })
    }
    fn gen_prop(&mut self, expr: PropositionExpr, spans: BTreeSet<SourceSpanId>) -> PropositionId {
        let id = PropositionId::from(format!("{GENERATED_PROP_PREFIX}{}", self.generated));
        self.generated += 1;
        self.doc.propositions.insert(
            id.clone(),
            Proposition {
                id: id.clone(),
                expression: expr,
                operator_spans: BTreeSet::new(),
                operator_tense_aspect: None,
                operator_voice: None,
                operator_grammatical_spans: BTreeSet::new(),
                source_spans: spans,
                evidence: BTreeSet::new(),
            },
        );
        id
    }
    fn inject_structured_shell(&mut self) -> Result<(), TrainingError> {
        let TrainingWindow::Structured(w) = self.window else {
            return Ok(());
        };
        match w.channel {
            StructuredChannel::ToolCall => {
                let invocation = self
                    .context
                    .get(&ContextTerm::Invocation)
                    .cloned()
                    .ok_or_else(|| semantic_error("tool call with no invocation context"))?;
                let actor = self.context[&ContextTerm::Actor].clone();
                let tool = self.context[&ContextTerm::Tool].clone();
                let occurrence = match &invocation {
                    Term::Occurrence(id) => id.clone(),
                    _ => unreachable!(),
                };
                let p_occ =
                    self.gen_prop(PropositionExpr::Occurrence { occurrence }, BTreeSet::new());
                let p_actor = self.gen_prop(
                    PropositionExpr::Relation {
                        relation: RelationId::from("toolInvocationPrincipal"),
                        arguments: vec![invocation.clone(), actor],
                    },
                    BTreeSet::new(),
                );
                let p_tool = self.gen_prop(
                    PropositionExpr::Relation {
                        relation: RelationId::from("invokesTool"),
                        arguments: vec![invocation, tool],
                    },
                    BTreeSet::new(),
                );
                let shell = self.gen_prop(
                    PropositionExpr::Conjunction {
                        members: BTreeSet::from([p_occ, p_actor, p_tool]),
                    },
                    BTreeSet::new(),
                );
                self.root_proposition(&shell, PresentationMode::Record)?;
            }
            StructuredChannel::ToolCallUpdate => {
                if let (Some(update), Some(invocation)) = (
                    self.context.get(&ContextTerm::Update).cloned(),
                    self.context.get(&ContextTerm::Invocation).cloned(),
                ) {
                    let p = self.gen_prop(
                        PropositionExpr::Relation {
                            relation: RelationId::from("toolInvocationUpdateFor"),
                            arguments: vec![update, invocation],
                        },
                        BTreeSet::new(),
                    );
                    self.root_proposition(&p, PresentationMode::Record)?;
                }
            }
            StructuredChannel::ToolResult => {
                if let (Some(result), Some(invocation)) = (
                    self.context.get(&ContextTerm::Result).cloned(),
                    self.context.get(&ContextTerm::Invocation).cloned(),
                ) {
                    let p = self.gen_prop(
                        PropositionExpr::Relation {
                            relation: RelationId::from("toolResultOfInvocation"),
                            arguments: vec![result, invocation],
                        },
                        BTreeSet::new(),
                    );
                    self.root_proposition(&p, PresentationMode::Record)?;
                }
            }
        }
        Ok(())
    }
    fn presenter(&self) -> Option<ReferentId> {
        match self.window {
            TrainingWindow::Prose(_) => match self.context.get(&ContextTerm::Speaker) {
                Some(Term::Referent(id)) => Some(id.clone()),
                _ => None,
            },
            TrainingWindow::Structured(_) => match self.context.get(&ContextTerm::Recorder) {
                Some(Term::Referent(id)) => Some(id.clone()),
                _ => None,
            },
        }
    }
    fn statement_addressees(&self) -> BTreeSet<ReferentId> {
        match self.context.get(&ContextTerm::Addressee) {
            Some(Term::Referent(id)) => BTreeSet::from([id.clone()]),
            _ => BTreeSet::new(),
        }
    }
    fn root_proposition(
        &mut self,
        id: &PropositionId,
        mode: PresentationMode,
    ) -> Result<(), TrainingError> {
        let sid = StatementId::from(format!(
            "{GENERATED_STATEMENT_PREFIX}{}",
            self.doc.statement_order.len()
        ));
        let spans = self
            .doc
            .propositions
            .get(id)
            .map(|p| p.source_spans.clone())
            .unwrap_or_default();
        self.doc.statements.insert(
            sid.clone(),
            Statement {
                id: sid.clone(),
                presenter: self.presenter(),
                addressees: self.statement_addressees(),
                mode,
                basis: StatementBasis::Expressed,
                content: id.clone(),
                source_spans: spans,
                evidence: BTreeSet::new(),
            },
        );
        self.doc.statement_order.push(sid);
        Ok(())
    }
    fn root_content(&mut self, id: &ContentId) -> Result<(), TrainingError> {
        let content = self
            .target
            .contents
            .get(id)
            .ok_or_else(|| semantic_error(format!("unknown content {id}")))?
            .clone();
        let content_ref = ReferentId::from(format!("{CONTENT_PREFIX}{}", id.as_str()));
        let spans = self.content_footprint(&content.id)?;
        self.doc.referents.insert(
            content_ref.clone(),
            Referent {
                id: content_ref.clone(),
                types: BTreeSet::from([content.sort.clone()]),
                lexical_anchor: Some(LexicalAnchor {
                    spans: spans.clone(),
                }),
                labels: BTreeSet::new(),
                external_ids: BTreeMap::new(),
                discourse_roles: BTreeSet::new(),
                source_spans: spans.clone(),
            },
        );
        let (relation_prop, mode) = match content.body {
            LearnedContentBody::Proposition { proposition } => (
                self.gen_prop(
                    PropositionExpr::Relation {
                        relation: RelationId::from("contentProposition"),
                        arguments: vec![
                            Term::Referent(content_ref.clone()),
                            Term::Proposition(proposition),
                        ],
                    },
                    spans.clone(),
                ),
                self.content_mode(&content.sort)?,
            ),
            LearnedContentBody::Question {
                condition,
                answer_variables,
                alternatives,
            } => {
                let mut members = BTreeSet::new();
                members.insert(self.gen_prop(
                    PropositionExpr::Relation {
                        relation: RelationId::from("contentProposition"),
                        arguments: vec![
                            Term::Referent(content_ref.clone()),
                            Term::Proposition(condition),
                        ],
                    },
                    spans.clone(),
                ));
                for variable in answer_variables {
                    members.insert(self.gen_prop(
                        PropositionExpr::Relation {
                            relation: RelationId::from("questionAnswerVariable"),
                            arguments: vec![
                                Term::Referent(content_ref.clone()),
                                Term::Variable(variable),
                            ],
                        },
                        spans.clone(),
                    ));
                }
                for alt in alternatives {
                    members.insert(self.gen_prop(
                        PropositionExpr::Relation {
                            relation: RelationId::from("alternativeProposition"),
                            arguments: vec![
                                Term::Referent(content_ref.clone()),
                                Term::Proposition(alt),
                            ],
                        },
                        spans.clone(),
                    ));
                }
                let p = if members.len() == 1 {
                    members.into_iter().next().expect("one member")
                } else {
                    self.gen_prop(PropositionExpr::Conjunction { members }, spans.clone())
                };
                (p, PresentationMode::Question)
            }
            LearnedContentBody::Quotation { quoted } => {
                let text = anchor_text(self.window, &quoted)?;
                (
                    self.gen_prop(
                        PropositionExpr::Relation {
                            relation: RelationId::from("quotedText"),
                            arguments: vec![
                                Term::Referent(content_ref.clone()),
                                Term::Literal(Literal::String(text)),
                            ],
                        },
                        spans.clone(),
                    ),
                    PresentationMode::Quotation,
                )
            }
        };
        let mut roots = BTreeSet::from([relation_prop]);
        if matches!(self.window, TrainingWindow::Prose(_)) {
            if let Some(Term::Referent(presenter)) =
                self.context.get(&ContextTerm::Speaker).cloned()
            {
                roots.insert(self.gen_prop(
                    PropositionExpr::Relation {
                        relation: RelationId::from("contentPresenter"),
                        arguments: vec![
                            Term::Referent(content_ref.clone()),
                            Term::Referent(presenter),
                        ],
                    },
                    spans.clone(),
                ));
            }
            if let Some(Term::Referent(addressee)) =
                self.context.get(&ContextTerm::Addressee).cloned()
            {
                roots.insert(self.gen_prop(
                    PropositionExpr::Relation {
                        relation: RelationId::from("contentAddressee"),
                        arguments: vec![
                            Term::Referent(content_ref.clone()),
                            Term::Referent(addressee),
                        ],
                    },
                    spans.clone(),
                ));
            }
        }
        let root = if roots.len() == 1 {
            roots.into_iter().next().expect("one content relation")
        } else {
            self.gen_prop(PropositionExpr::Conjunction { members: roots }, spans)
        };
        self.root_proposition(&root, mode)
    }
    fn content_mode(&self, sort: &ConceptId) -> Result<PresentationMode, TrainingError> {
        if subtype(self.index, sort, "CommandContent")? {
            Ok(PresentationMode::Command)
        } else if subtype(self.index, sort, "RequestContent")? {
            Ok(PresentationMode::Request)
        } else if subtype(self.index, sort, "SuggestionContent")? {
            Ok(PresentationMode::Suggestion)
        } else if subtype(self.index, sort, "PromiseContent")? {
            Ok(PresentationMode::Promise)
        } else {
            Ok(PresentationMode::Mention)
        }
    }
    fn compile_ambiguity(&mut self, id: &AmbiguityId) -> Result<(), TrainingError> {
        let a = &self.target.ambiguities[id];
        let mut alternatives = BTreeSet::new();
        let mut spans = BTreeSet::new();
        for reading in &a.alternatives {
            match reading {
                LearnedReading::Proposition(p) => {
                    alternatives.insert(AmbiguityAlternative::Proposition(p.clone()));
                    spans.extend(self.prop_footprint(p)?);
                }
                LearnedReading::Content(c) => {
                    let p = self.compile_content_unrooted(c)?;
                    alternatives.insert(AmbiguityAlternative::Proposition(p.clone()));
                    spans.extend(self.doc.propositions[&p].source_spans.clone());
                }
            }
        }
        self.doc.ambiguities.insert(
            id.clone(),
            Ambiguity {
                id: id.clone(),
                alternatives,
                source_spans: spans,
            },
        );
        Ok(())
    }
    fn compile_content_unrooted(&mut self, id: &ContentId) -> Result<PropositionId, TrainingError> {
        let before = self.doc.statement_order.len();
        self.root_content(id)?;
        let sid = self
            .doc
            .statement_order
            .pop()
            .ok_or_else(|| semantic_error("content compiler did not emit statement"))?;
        let statement = self
            .doc
            .statements
            .remove(&sid)
            .ok_or_else(|| semantic_error("content compiler statement missing"))?;
        debug_assert_eq!(before, self.doc.statement_order.len());
        Ok(statement.content)
    }
    fn content_footprint(&self, id: &ContentId) -> Result<BTreeSet<SourceSpanId>, TrainingError> {
        let mut r = Reach::default();
        reach_content(self.target, id, &mut r)?;
        Ok(r.spans
            .iter()
            .map(|span| self.span_ids[span].clone())
            .collect())
    }
    fn prop_footprint(&self, id: &PropositionId) -> Result<BTreeSet<SourceSpanId>, TrainingError> {
        let mut r = Reach::default();
        reach_prop(self.target, id, &mut r)?;
        Ok(r.spans
            .iter()
            .map(|span| self.span_ids[span].clone())
            .collect())
    }
}

/// Compatibility-IR firewall for semantic-v6. The rich occurrence document still
/// supports historical adapters, but the learned-v6 compiler may emit only the
/// canonical subset below. This prevents legacy escape hatches from contaminating
/// the learned target through deterministic compilation.
fn validate_compiled_v6_contract(doc: &OccurrenceDocument) -> Result<(), TrainingError> {
    for (id, referent) in &doc.referents {
        if referent.types.len() != 1 {
            return Err(semantic_error(format!(
                "compiled-v6 referent {id} must have exactly one ontology sort"
            )));
        }
    }
    for (id, occurrence) in &doc.occurrences {
        if occurrence.types.len() != 1 {
            return Err(semantic_error(format!(
                "compiled-v6 occurrence {id} must have exactly one ontology sort"
            )));
        }
        if !occurrence.participants.is_empty()
            || !occurrence.attributes.is_empty()
            || occurrence.tense_aspect.is_some()
            || occurrence.grammatical_voice.is_some()
            || !occurrence.grammatical_spans.is_empty()
            || occurrence.reported_outcome.is_some()
            || !occurrence.evidence.is_empty()
        {
            return Err(semantic_error(format!(
                "compiled-v6 occurrence {id} contains legacy semantic channels"
            )));
        }
    }
    for (id, variable) in &doc.variables {
        if variable.sort.as_str().is_empty() {
            return Err(semantic_error(format!(
                "compiled-v6 variable {id} has no ontology sort"
            )));
        }
    }
    for (id, proposition) in &doc.propositions {
        if proposition.operator_tense_aspect.is_some()
            || proposition.operator_voice.is_some()
            || !proposition.operator_grammatical_spans.is_empty()
            || !proposition.evidence.is_empty()
        {
            return Err(semantic_error(format!(
                "compiled-v6 proposition {id} contains legacy semantic metadata"
            )));
        }
        match &proposition.expression {
            PropositionExpr::TypeAssertion { .. }
            | PropositionExpr::Relation { .. }
            | PropositionExpr::Occurrence { .. }
            | PropositionExpr::Equality { .. }
            | PropositionExpr::Comparison { .. }
            | PropositionExpr::Negation { .. }
            | PropositionExpr::Conjunction { .. }
            | PropositionExpr::Disjunction { .. }
            | PropositionExpr::Implication { .. }
            | PropositionExpr::Counterfactual { .. }
            | PropositionExpr::Unless { .. }
            | PropositionExpr::GeneralizedQuantified { .. }
            | PropositionExpr::ScopedOperator { .. }
            | PropositionExpr::Focus { .. }
            | PropositionExpr::Presuppositional { .. } => {}
            PropositionExpr::Quantified { quantifier, .. } => {
                if matches!(quantifier, Quantifier::SourceAnchored(_)) {
                    return Err(semantic_error(format!(
                        "compiled-v6 proposition {id} uses SourceAnchored quantification"
                    )));
                }
            }
            PropositionExpr::Modal { .. }
            | PropositionExpr::Generic { .. }
            | PropositionExpr::Perfect { .. }
            | PropositionExpr::Progressive { .. } => {
                return Err(semantic_error(format!(
                    "compiled-v6 proposition {id} uses a legacy scoped-operator channel"
                )));
            }
            PropositionExpr::Capability { .. }
            | PropositionExpr::Interrogative { .. }
            | PropositionExpr::Phase { .. }
            | PropositionExpr::Attitude { .. }
            | PropositionExpr::SpeechAct { .. }
            | PropositionExpr::Temporal { .. }
            | PropositionExpr::Causal { .. }
            | PropositionExpr::Quotation { .. } => {
                return Err(semantic_error(format!(
                    "compiled-v6 proposition {id} uses a legacy proposition encoding"
                )));
            }
        }
    }
    for (id, statement) in &doc.statements {
        if !matches!(statement.basis, StatementBasis::Expressed) || !statement.evidence.is_empty() {
            return Err(semantic_error(format!(
                "compiled-v6 statement {id} contains derived/legacy target metadata"
            )));
        }
        if matches!(statement.mode, PresentationMode::SourceAnchored(_)) {
            return Err(semantic_error(format!(
                "compiled-v6 statement {id} uses SourceAnchored presentation mode"
            )));
        }
    }
    for (id, ambiguity) in &doc.ambiguities {
        if ambiguity
            .alternatives
            .iter()
            .any(|alt| !matches!(alt, AmbiguityAlternative::Proposition(_)))
        {
            return Err(semantic_error(format!(
                "compiled-v6 ambiguity {id} contains a non-propositional compatibility alternative"
            )));
        }
    }
    Ok(())
}

fn actor_sort(role: SpeakerRole) -> &'static str {
    match role {
        SpeakerRole::Tool => "Tool",
        SpeakerRole::Unknown => "Entity",
        SpeakerRole::Harness | SpeakerRole::System => "SoftwareAgent",
        SpeakerRole::User | SpeakerRole::Agent => "Agent",
    }
}
