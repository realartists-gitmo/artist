#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use muse_core::{
    AmbiguityId, CanonicalHashError, OccurrenceDocumentId, OccurrenceId, PropositionId, ReferentId,
    SourceSpanId, StatementId, VariableId, canonical_digest,
};
use serde::Serialize;
use serde_json::{Value, json};
use thiserror::Error;

use super::{
    AmbiguityAlternative, AttitudeKind, ContentDigest, Derivation, InterrogativeKind,
    LexicalAnchor, Modality, OccurrenceDocument, OccurrenceValidationError, Participant,
    ParticipantRole, PhaseKind, PresentationMode, PropositionExpr, Quantifier, SemanticTarget,
    SourceSpan, SpeechActKind, StatementBasis, TemporalAnchor, Term,
};

/// Canonicalization contract used for prose-training targets and deterministic
/// cross-source convergence checks.
pub const TRAINING_CANONICALIZATION_VERSION: &str = "muse-occurrence-training-canonical-7";

impl OccurrenceDocument {
    /// Return a semantically equivalent document whose producer-local IDs,
    /// unordered collections, and bound-variable names are canonicalized for
    /// training. Source coordinates and ontology snapshot identity are retained.
    pub fn canonical_training_projection(
        &self,
    ) -> Result<OccurrenceDocument, TrainingCanonicalizationError> {
        self.validate()?;
        ScopeValidator::new(self).validate()?;
        let mut canonicalizer = Canonicalizer::new(self)?;
        canonicalizer.assign_all()?;
        let projected = canonicalizer.project()?;
        projected.validate()?;
        Ok(projected)
    }

    /// Canonical JSON bytes for a prose→formal target.
    pub fn canonical_training_bytes(&self) -> Result<Vec<u8>, TrainingCanonicalizationError> {
        Ok(serde_json::to_vec(&self.canonical_training_projection()?)?)
    }

    /// Digest of the canonical training projection, invariant to producer-local
    /// semantic IDs and alpha-renaming of bound variables.
    pub fn canonical_training_digest(
        &self,
    ) -> Result<ContentDigest, TrainingCanonicalizationError> {
        Ok(ContentDigest::sha256_bytes(
            &self.canonical_training_bytes()?,
        ))
    }
}

struct Canonicalizer<'a> {
    source: &'a OccurrenceDocument,
    spans: BTreeMap<SourceSpanId, SourceSpanId>,
    referents: BTreeMap<ReferentId, ReferentId>,
    occurrences: BTreeMap<OccurrenceId, OccurrenceId>,
    propositions: BTreeMap<PropositionId, PropositionId>,
    statements: BTreeMap<StatementId, StatementId>,
    variables: BTreeMap<VariableId, VariableId>,
    ambiguities: BTreeMap<AmbiguityId, AmbiguityId>,
}

impl<'a> Canonicalizer<'a> {
    fn new(source: &'a OccurrenceDocument) -> Result<Self, TrainingCanonicalizationError> {
        let mut span_rows = source
            .source_spans
            .iter()
            .map(|(id, span)| Ok((span_fingerprint(span)?, id.clone())))
            .collect::<Result<Vec<_>, TrainingCanonicalizationError>>()?;
        span_rows.sort_by(|left, right| left.0.cmp(&right.0));
        // Semantically identical source coordinates are one canonical span even
        // when a producer redundantly interned them under different local IDs.
        let mut spans = BTreeMap::new();
        let mut previous_fingerprint: Option<String> = None;
        let mut canonical: Option<SourceSpanId> = None;
        let mut unique_index = 0usize;
        for (fingerprint, id) in span_rows {
            if previous_fingerprint.as_deref() != Some(fingerprint.as_str()) {
                canonical = Some(SourceSpanId::from(canonical_id("span", unique_index)));
                unique_index += 1;
                previous_fingerprint = Some(fingerprint);
            }
            spans.insert(
                id,
                canonical
                    .clone()
                    .ok_or(TrainingCanonicalizationError::MissingCanonicalId(
                        "source span",
                    ))?,
            );
        }
        Ok(Self {
            source,
            spans,
            referents: BTreeMap::new(),
            occurrences: BTreeMap::new(),
            propositions: BTreeMap::new(),
            statements: BTreeMap::new(),
            variables: BTreeMap::new(),
            ambiguities: BTreeMap::new(),
        })
    }

    fn assign_all(&mut self) -> Result<(), TrainingCanonicalizationError> {
        let mut environment = Vec::new();
        for statement in &self.source.statement_order {
            self.assign_statement(statement, &mut environment)?;
        }

        let referenced = self.referenced_propositions();
        let roots = self
            .source
            .propositions
            .keys()
            .filter(|id| !self.propositions.contains_key(*id) && !referenced.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        let roots = sort_by_fallible_key(roots, |id| self.proposition_fingerprint(id, &[]))?;
        for id in roots {
            self.assign_proposition(&id, &mut environment)?;
        }
        let remaining = self
            .source
            .propositions
            .keys()
            .filter(|id| !self.propositions.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        let remaining =
            sort_by_fallible_key(remaining, |id| self.proposition_fingerprint(id, &[]))?;
        for id in remaining {
            self.assign_proposition(&id, &mut environment)?;
        }

        let occurrences = self
            .source
            .occurrences
            .keys()
            .filter(|id| !self.occurrences.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        let occurrences =
            sort_by_fallible_key(occurrences, |id| self.occurrence_fingerprint(id, &[]))?;
        for id in occurrences {
            self.assign_occurrence(&id, &mut environment)?;
        }

        let referents = self
            .source
            .referents
            .keys()
            .filter(|id| !self.referents.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        let referents = sort_by_fallible_key(referents, |id| self.referent_fingerprint(id))?;
        for id in referents {
            self.assign_referent(&id);
        }
        let variables = self
            .source
            .variables
            .keys()
            .filter(|id| !self.variables.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        let variables = sort_by_fallible_key(variables, |id| self.variable_fingerprint(id))?;
        for id in variables {
            self.assign_declared_variable(&id);
        }

        let ambiguities = self.source.ambiguities.keys().cloned().collect::<Vec<_>>();
        let ambiguities = sort_by_fallible_key(ambiguities, |id| self.ambiguity_fingerprint(id))?;
        for id in ambiguities {
            self.assign_ambiguity_dependencies(&id, &mut environment)?;
            let next = self.ambiguities.len();
            self.ambiguities
                .entry(id)
                .or_insert_with(|| AmbiguityId::from(canonical_id("ambiguity", next)));
        }
        Ok(())
    }

    fn assign_statement(
        &mut self,
        id: &StatementId,
        environment: &mut Vec<VariableId>,
    ) -> Result<(), TrainingCanonicalizationError> {
        if self.statements.contains_key(id) {
            return Ok(());
        }
        let next = self.statements.len();
        self.statements.insert(
            id.clone(),
            StatementId::from(canonical_id("statement", next)),
        );
        let statement = &self.source.statements[id];
        if let Some(presenter) = &statement.presenter {
            self.assign_referent(presenter);
        }
        let addressees = statement.addressees.iter().cloned().collect::<Vec<_>>();
        let addressees = sort_by_fallible_key(addressees, |id| self.referent_fingerprint(id))?;
        for id in addressees {
            self.assign_referent(&id);
        }
        self.assign_proposition(&statement.content, environment)?;
        if let StatementBasis::CompositionalEntailment { premises, .. } = &statement.basis {
            let premises = premises.iter().cloned().collect::<Vec<_>>();
            let premises =
                sort_by_fallible_key(premises, |id| self.proposition_fingerprint(id, environment))?;
            for premise in premises {
                self.assign_proposition(&premise, environment)?;
            }
        }
        Ok(())
    }

    fn assign_proposition(
        &mut self,
        id: &PropositionId,
        environment: &mut Vec<VariableId>,
    ) -> Result<(), TrainingCanonicalizationError> {
        if self.propositions.contains_key(id) {
            return Ok(());
        }
        let next = self.propositions.len();
        self.propositions.insert(
            id.clone(),
            PropositionId::from(canonical_id("proposition", next)),
        );
        let proposition = &self.source.propositions[id];
        match &proposition.expression {
            PropositionExpr::TypeAssertion { subject, .. } => {
                self.assign_term(subject, environment)?;
            }
            PropositionExpr::Relation { arguments, .. } => {
                for term in arguments {
                    self.assign_term(term, environment)?;
                }
            }
            PropositionExpr::Occurrence { occurrence } => {
                self.assign_occurrence(occurrence, environment)?;
            }
            PropositionExpr::Equality { left, right } => {
                let terms = [left, right];
                let terms = sort_by_fallible_key(terms.into_iter().collect(), |term| {
                    self.term_fingerprint(term, environment)
                })?;
                for term in terms {
                    self.assign_term(term, environment)?;
                }
            }
            PropositionExpr::Comparison {
                left,
                right,
                dimension,
                ..
            } => {
                self.assign_term(left, environment)?;
                self.assign_term(right, environment)?;
                if let Some(dimension) = dimension {
                    self.assign_term(dimension, environment)?;
                }
            }
            PropositionExpr::Negation { content }
            | PropositionExpr::Modal { content, .. }
            | PropositionExpr::Generic { content }
            | PropositionExpr::Perfect { content }
            | PropositionExpr::Progressive { content }
            | PropositionExpr::Phase { content, .. }
            | PropositionExpr::Quotation { content } => {
                self.assign_proposition(content, environment)?;
            }
            PropositionExpr::ScopedOperator { operator, content } => {
                self.assign_referent(operator);
                self.assign_proposition(content, environment)?;
            }
            PropositionExpr::Focus {
                operator, content, ..
            } => {
                self.assign_referent(operator);
                self.assign_proposition(content, environment)?;
            }
            PropositionExpr::Capability { bearer, content } => {
                self.assign_term(bearer, environment)?;
                self.assign_proposition(content, environment)?;
            }
            PropositionExpr::Conjunction { members } | PropositionExpr::Disjunction { members } => {
                let members = members.iter().cloned().collect::<Vec<_>>();
                let members = sort_by_fallible_key(members, |member| {
                    self.proposition_fingerprint(member, environment)
                })?;
                for member in members {
                    self.assign_proposition(&member, environment)?;
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
                self.assign_proposition(antecedent, environment)?;
                self.assign_proposition(consequent, environment)?;
            }
            PropositionExpr::Unless {
                condition,
                consequent,
            } => {
                self.assign_proposition(condition, environment)?;
                self.assign_proposition(consequent, environment)?;
            }
            PropositionExpr::Presuppositional {
                asserted,
                presupposed,
            } => {
                self.assign_proposition(asserted, environment)?;
                self.assign_proposition(presupposed, environment)?;
            }
            PropositionExpr::Quantified { variable, body, .. } => {
                if self.variables.contains_key(variable) {
                    return Err(TrainingCanonicalizationError::VariableRebound(
                        variable.clone(),
                    ));
                }
                let next = self.variables.len();
                self.variables.insert(
                    variable.clone(),
                    VariableId::from(canonical_id("variable", next)),
                );
                environment.push(variable.clone());
                self.assign_proposition(body, environment)?;
                environment.pop();
            }
            PropositionExpr::GeneralizedQuantified {
                quantifier,
                variable,
                body,
                ..
            } => {
                self.assign_referent(quantifier);
                if self.variables.contains_key(variable) {
                    return Err(TrainingCanonicalizationError::VariableRebound(
                        variable.clone(),
                    ));
                }
                let next = self.variables.len();
                self.variables.insert(
                    variable.clone(),
                    VariableId::from(canonical_id("variable", next)),
                );
                environment.push(variable.clone());
                self.assign_proposition(body, environment)?;
                environment.pop();
            }
            PropositionExpr::Interrogative { variable, body, .. } => {
                if let Some(variable) = variable {
                    if self.variables.contains_key(variable) {
                        return Err(TrainingCanonicalizationError::VariableRebound(
                            variable.clone(),
                        ));
                    }
                    let next = self.variables.len();
                    self.variables.insert(
                        variable.clone(),
                        VariableId::from(canonical_id("variable", next)),
                    );
                    environment.push(variable.clone());
                    self.assign_proposition(body, environment)?;
                    environment.pop();
                } else {
                    self.assign_proposition(body, environment)?;
                }
            }
            PropositionExpr::Attitude {
                holder, content, ..
            } => {
                self.assign_term(holder, environment)?;
                self.assign_proposition(content, environment)?;
            }
            PropositionExpr::SpeechAct {
                speaker,
                addressees,
                content,
                ..
            } => {
                self.assign_term(speaker, environment)?;
                let addressees = addressees.iter().collect::<Vec<_>>();
                let addressees = sort_by_fallible_key(addressees, |term| {
                    self.term_fingerprint(term, environment)
                })?;
                for addressee in addressees {
                    self.assign_term(addressee, environment)?;
                }
                self.assign_proposition(content, environment)?;
            }
            PropositionExpr::Temporal {
                subject, object, ..
            } => {
                self.assign_target(subject, environment)?;
                self.assign_anchor(object, environment)?;
            }
            PropositionExpr::Causal { cause, effect, .. } => {
                self.assign_target(cause, environment)?;
                self.assign_target(effect, environment)?;
            }
        }
        Ok(())
    }

    fn assign_occurrence(
        &mut self,
        id: &OccurrenceId,
        environment: &mut Vec<VariableId>,
    ) -> Result<(), TrainingCanonicalizationError> {
        if self.occurrences.contains_key(id) {
            return Ok(());
        }
        let next = self.occurrences.len();
        self.occurrences.insert(
            id.clone(),
            OccurrenceId::from(canonical_id("occurrence", next)),
        );
        let occurrence = &self.source.occurrences[id];
        let participants = occurrence.participants.iter().collect::<Vec<_>>();
        let participants = sort_by_fallible_key(participants, |participant| {
            Ok::<_, TrainingCanonicalizationError>((
                canonical_json_sort_key(&self.structural_value_fingerprint(&participant.role)?),
                self.term_fingerprint(&participant.value, environment)?,
            ))
        })?;
        for participant in participants {
            self.assign_term(&participant.value, environment)?;
        }
        for values in occurrence.attributes.values() {
            let values = values.iter().collect::<Vec<_>>();
            let values =
                sort_by_fallible_key(values, |term| self.term_fingerprint(term, environment))?;
            for term in values {
                self.assign_term(term, environment)?;
            }
        }
        if let Some(outcome) = &occurrence.reported_outcome {
            if let Some(reporter) = &outcome.reporter {
                self.assign_referent(reporter);
            }
        }
        Ok(())
    }

    fn assign_term(
        &mut self,
        term: &Term,
        environment: &mut Vec<VariableId>,
    ) -> Result<(), TrainingCanonicalizationError> {
        match term {
            Term::Referent(id) => self.assign_referent(id),
            Term::Occurrence(id) => self.assign_occurrence(id, environment)?,
            Term::Proposition(id) => self.assign_proposition(id, environment)?,
            Term::Variable(id) => {
                if !environment.contains(id) {
                    if !self.source.variables.contains_key(id) {
                        return Err(TrainingCanonicalizationError::UnboundVariable(id.clone()));
                    }
                    self.assign_declared_variable(id);
                }
            }
            Term::Literal(_) => {}
        }
        Ok(())
    }

    fn assign_referent(&mut self, id: &ReferentId) {
        if !self.referents.contains_key(id) {
            let next = self.referents.len();
            self.referents
                .insert(id.clone(), ReferentId::from(canonical_id("referent", next)));
        }
    }

    fn assign_declared_variable(&mut self, id: &VariableId) {
        if !self.variables.contains_key(id) {
            let next = self.variables.len();
            self.variables
                .insert(id.clone(), VariableId::from(canonical_id("variable", next)));
        }
    }

    fn assign_target(
        &mut self,
        target: &SemanticTarget,
        environment: &mut Vec<VariableId>,
    ) -> Result<(), TrainingCanonicalizationError> {
        match target {
            SemanticTarget::Occurrence(id) => self.assign_occurrence(id, environment),
            SemanticTarget::Proposition(id) => self.assign_proposition(id, environment),
            SemanticTarget::Variable(id) => {
                if environment.contains(id) {
                    Ok(())
                } else if self.source.variables.contains_key(id) {
                    self.assign_declared_variable(id);
                    Ok(())
                } else {
                    Err(TrainingCanonicalizationError::UnboundVariable(id.clone()))
                }
            }
        }
    }

    fn assign_anchor(
        &mut self,
        anchor: &TemporalAnchor,
        environment: &mut Vec<VariableId>,
    ) -> Result<(), TrainingCanonicalizationError> {
        match anchor {
            TemporalAnchor::Target { target } => self.assign_target(target, environment)?,
            TemporalAnchor::Variable { variable } if !environment.contains(variable) => {
                return Err(TrainingCanonicalizationError::UnboundVariable(
                    variable.clone(),
                ));
            }
            _ => {}
        }
        Ok(())
    }

    fn assign_ambiguity_dependencies(
        &mut self,
        id: &AmbiguityId,
        environment: &mut Vec<VariableId>,
    ) -> Result<(), TrainingCanonicalizationError> {
        let ambiguity = &self.source.ambiguities[id];
        let alternatives = ambiguity.alternatives.iter().collect::<Vec<_>>();
        let alternatives = sort_by_fallible_key(alternatives, |alternative| {
            self.ambiguity_alternative_fingerprint(alternative)
        })?;
        for alternative in alternatives {
            match alternative {
                AmbiguityAlternative::Referent(id) => self.assign_referent(id),
                AmbiguityAlternative::Occurrence(id) => self.assign_occurrence(id, environment)?,
                AmbiguityAlternative::Proposition(id) => {
                    self.assign_proposition(id, environment)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn project(&self) -> Result<OccurrenceDocument, TrainingCanonicalizationError> {
        let source_spans = self
            .source
            .source_spans
            .iter()
            .map(|(id, span)| {
                let mut span = span.clone();
                span.id = self.map_span(id)?;
                Ok((span.id.clone(), span))
            })
            .collect::<Result<BTreeMap<_, _>, TrainingCanonicalizationError>>()?;
        let referents = self
            .source
            .referents
            .iter()
            .map(|(id, referent)| {
                let mut referent = referent.clone();
                referent.id = self.map_referent(id)?;
                // Surface labels are debug metadata and must not alter a training target.
                referent.labels.clear();
                referent.lexical_anchor = referent
                    .lexical_anchor
                    .as_ref()
                    .map(|anchor| self.map_lexical_anchor(anchor))
                    .transpose()?;
                referent.source_spans =
                    map_set(&referent.source_spans, |span| self.map_span(span))?;
                Ok((referent.id.clone(), referent))
            })
            .collect::<Result<BTreeMap<_, _>, TrainingCanonicalizationError>>()?;
        let occurrences = self
            .source
            .occurrences
            .iter()
            .map(|(id, occurrence)| {
                let mut occurrence = occurrence.clone();
                occurrence.id = self.map_occurrence(id)?;
                occurrence.lexical_anchor = occurrence
                    .lexical_anchor
                    .as_ref()
                    .map(|anchor| self.map_lexical_anchor(anchor))
                    .transpose()?;
                occurrence.grammatical_spans =
                    map_set(&occurrence.grammatical_spans, |span| self.map_span(span))?;
                occurrence.participants = occurrence
                    .participants
                    .iter()
                    .map(|participant| {
                        Ok(Participant {
                            role: self.map_participant_role(&participant.role)?,
                            value: self.map_term(&participant.value)?,
                        })
                    })
                    .collect::<Result<Vec<_>, TrainingCanonicalizationError>>()?;
                occurrence.participants.sort_by(|left, right| {
                    (&left.role, &left.value).cmp(&(&right.role, &right.value))
                });
                occurrence.attributes = occurrence
                    .attributes
                    .iter()
                    .map(|(relation, values)| {
                        let mut values = values
                            .iter()
                            .map(|value| self.map_term(value))
                            .collect::<Result<Vec<_>, TrainingCanonicalizationError>>()?;
                        values.sort();
                        Ok((relation.clone(), values))
                    })
                    .collect::<Result<BTreeMap<_, _>, TrainingCanonicalizationError>>()?;
                if let Some(outcome) = &mut occurrence.reported_outcome {
                    outcome.reporter = outcome
                        .reporter
                        .as_ref()
                        .map(|id| self.map_referent(id))
                        .transpose()?;
                    outcome.source_spans =
                        map_set(&outcome.source_spans, |span| self.map_span(span))?;
                }
                occurrence.source_spans =
                    map_set(&occurrence.source_spans, |span| self.map_span(span))?;
                Ok((occurrence.id.clone(), occurrence))
            })
            .collect::<Result<BTreeMap<_, _>, TrainingCanonicalizationError>>()?;
        let variables = self
            .source
            .variables
            .iter()
            .map(|(id, variable)| {
                let mut variable = variable.clone();
                variable.id = self.map_variable(id)?;
                variable.lexical_anchor = variable
                    .lexical_anchor
                    .as_ref()
                    .map(|anchor| self.map_lexical_anchor(anchor))
                    .transpose()?;
                variable.source_spans =
                    map_set(&variable.source_spans, |span| self.map_span(span))?;
                Ok((variable.id.clone(), variable))
            })
            .collect::<Result<BTreeMap<_, _>, TrainingCanonicalizationError>>()?;
        let propositions = self
            .source
            .propositions
            .iter()
            .map(|(id, proposition)| {
                let mut proposition = proposition.clone();
                proposition.id = self.map_proposition(id)?;
                proposition.expression = self.map_expression(&proposition.expression)?;
                proposition.operator_spans =
                    map_set(&proposition.operator_spans, |span| self.map_span(span))?;
                proposition.operator_grammatical_spans =
                    map_set(&proposition.operator_grammatical_spans, |span| {
                        self.map_span(span)
                    })?;
                proposition.source_spans =
                    map_set(&proposition.source_spans, |span| self.map_span(span))?;
                Ok((proposition.id.clone(), proposition))
            })
            .collect::<Result<BTreeMap<_, _>, TrainingCanonicalizationError>>()?;
        let statements = self
            .source
            .statements
            .iter()
            .map(|(id, statement)| {
                let mut statement = statement.clone();
                statement.id = self.map_statement(id)?;
                statement.presenter = statement
                    .presenter
                    .as_ref()
                    .map(|id| self.map_referent(id))
                    .transpose()?;
                statement.addressees = map_set(&statement.addressees, |id| self.map_referent(id))?;
                statement.mode = self.map_presentation_mode(&statement.mode)?;
                statement.content = self.map_proposition(&statement.content)?;
                if let StatementBasis::CompositionalEntailment { premises, .. } =
                    &mut statement.basis
                {
                    *premises = map_set(premises, |id| self.map_proposition(id))?;
                }
                statement.source_spans =
                    map_set(&statement.source_spans, |span| self.map_span(span))?;
                Ok((statement.id.clone(), statement))
            })
            .collect::<Result<BTreeMap<_, _>, TrainingCanonicalizationError>>()?;
        let ambiguities = self
            .source
            .ambiguities
            .iter()
            .map(|(id, ambiguity)| {
                let mut ambiguity = ambiguity.clone();
                ambiguity.id = self.map_ambiguity(id)?;
                ambiguity.alternatives = ambiguity
                    .alternatives
                    .iter()
                    .map(|alternative| self.map_ambiguity_alternative(alternative))
                    .collect::<Result<BTreeSet<_>, TrainingCanonicalizationError>>()?;
                ambiguity.source_spans =
                    map_set(&ambiguity.source_spans, |span| self.map_span(span))?;
                Ok((ambiguity.id.clone(), ambiguity))
            })
            .collect::<Result<BTreeMap<_, _>, TrainingCanonicalizationError>>()?;
        Ok(OccurrenceDocument {
            schema_version: self.source.schema_version.clone(),
            id: OccurrenceDocumentId::from("occurrence-document:canonical-training"),
            ontology: self.source.ontology.clone(),
            derivation: Derivation::Imported {
                source: TRAINING_CANONICALIZATION_VERSION.into(),
            },
            source_spans,
            referents,
            occurrences,
            variables,
            propositions,
            statements,
            ambiguities,
            statement_order: self
                .source
                .statement_order
                .iter()
                .map(|id| self.map_statement(id))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    fn map_expression(
        &self,
        expression: &PropositionExpr,
    ) -> Result<PropositionExpr, TrainingCanonicalizationError> {
        Ok(match expression {
            PropositionExpr::TypeAssertion { subject, r#type } => PropositionExpr::TypeAssertion {
                subject: self.map_term(subject)?,
                r#type: r#type.clone(),
            },
            PropositionExpr::Relation {
                relation,
                arguments,
            } => PropositionExpr::Relation {
                relation: relation.clone(),
                arguments: arguments
                    .iter()
                    .map(|term| self.map_term(term))
                    .collect::<Result<_, _>>()?,
            },
            PropositionExpr::Occurrence { occurrence } => PropositionExpr::Occurrence {
                occurrence: self.map_occurrence(occurrence)?,
            },
            PropositionExpr::Equality { left, right } => {
                let mut terms = [self.map_term(left)?, self.map_term(right)?];
                terms.sort();
                PropositionExpr::Equality {
                    left: terms[0].clone(),
                    right: terms[1].clone(),
                }
            }
            PropositionExpr::Comparison {
                operator,
                left,
                right,
                dimension,
            } => PropositionExpr::Comparison {
                operator: *operator,
                left: self.map_term(left)?,
                right: self.map_term(right)?,
                dimension: dimension
                    .as_ref()
                    .map(|term| self.map_term(term))
                    .transpose()?,
            },
            PropositionExpr::Negation { content } => PropositionExpr::Negation {
                content: self.map_proposition(content)?,
            },
            PropositionExpr::Conjunction { members } => PropositionExpr::Conjunction {
                members: map_set(members, |id| self.map_proposition(id))?,
            },
            PropositionExpr::Disjunction { members } => PropositionExpr::Disjunction {
                members: map_set(members, |id| self.map_proposition(id))?,
            },
            PropositionExpr::Implication {
                antecedent,
                consequent,
            } => PropositionExpr::Implication {
                antecedent: self.map_proposition(antecedent)?,
                consequent: self.map_proposition(consequent)?,
            },
            PropositionExpr::Counterfactual {
                antecedent,
                consequent,
            } => PropositionExpr::Counterfactual {
                antecedent: self.map_proposition(antecedent)?,
                consequent: self.map_proposition(consequent)?,
            },
            PropositionExpr::Unless {
                condition,
                consequent,
            } => PropositionExpr::Unless {
                condition: self.map_proposition(condition)?,
                consequent: self.map_proposition(consequent)?,
            },
            PropositionExpr::Presuppositional {
                asserted,
                presupposed,
            } => PropositionExpr::Presuppositional {
                asserted: self.map_proposition(asserted)?,
                presupposed: self.map_proposition(presupposed)?,
            },
            PropositionExpr::Quantified {
                quantifier,
                variable,
                domain,
                body,
            } => PropositionExpr::Quantified {
                quantifier: self.map_quantifier(quantifier)?,
                variable: self.map_variable(variable)?,
                domain: domain.clone(),
                body: self.map_proposition(body)?,
            },
            PropositionExpr::GeneralizedQuantified {
                quantifier,
                variable,
                domain,
                body,
            } => PropositionExpr::GeneralizedQuantified {
                quantifier: self.map_referent(quantifier)?,
                variable: self.map_variable(variable)?,
                domain: domain.clone(),
                body: self.map_proposition(body)?,
            },
            PropositionExpr::ScopedOperator { operator, content } => {
                PropositionExpr::ScopedOperator {
                    operator: self.map_referent(operator)?,
                    content: self.map_proposition(content)?,
                }
            }
            PropositionExpr::Modal { modality, content } => PropositionExpr::Modal {
                modality: self.map_modality(modality)?,
                content: self.map_proposition(content)?,
            },
            PropositionExpr::Generic { content } => PropositionExpr::Generic {
                content: self.map_proposition(content)?,
            },
            PropositionExpr::Perfect { content } => PropositionExpr::Perfect {
                content: self.map_proposition(content)?,
            },
            PropositionExpr::Progressive { content } => PropositionExpr::Progressive {
                content: self.map_proposition(content)?,
            },
            PropositionExpr::Focus {
                operator,
                focus,
                content,
            } => PropositionExpr::Focus {
                operator: self.map_referent(operator)?,
                focus: self.map_lexical_anchor(focus)?,
                content: self.map_proposition(content)?,
            },
            PropositionExpr::Capability { bearer, content } => PropositionExpr::Capability {
                bearer: self.map_term(bearer)?,
                content: self.map_proposition(content)?,
            },
            PropositionExpr::Interrogative {
                interrogative,
                variable,
                domain,
                body,
            } => PropositionExpr::Interrogative {
                interrogative: self.map_interrogative(interrogative)?,
                variable: variable
                    .as_ref()
                    .map(|id| self.map_variable(id))
                    .transpose()?,
                domain: domain.clone(),
                body: self.map_proposition(body)?,
            },
            PropositionExpr::Phase { phase, content } => PropositionExpr::Phase {
                phase: self.map_phase(phase)?,
                content: self.map_proposition(content)?,
            },
            PropositionExpr::Attitude {
                holder,
                attitude,
                content,
            } => PropositionExpr::Attitude {
                holder: self.map_term(holder)?,
                attitude: self.map_attitude(attitude)?,
                content: self.map_proposition(content)?,
            },
            PropositionExpr::SpeechAct {
                speaker,
                act,
                addressees,
                content,
            } => PropositionExpr::SpeechAct {
                speaker: self.map_term(speaker)?,
                act: self.map_speech_act(act)?,
                addressees: addressees
                    .iter()
                    .map(|term| self.map_term(term))
                    .collect::<Result<BTreeSet<_>, _>>()?,
                content: self.map_proposition(content)?,
            },
            PropositionExpr::Temporal {
                subject,
                relation,
                object,
            } => PropositionExpr::Temporal {
                subject: self.map_target(subject)?,
                relation: *relation,
                object: self.map_anchor(object)?,
            },
            PropositionExpr::Causal {
                relation,
                cause,
                effect,
            } => PropositionExpr::Causal {
                relation: relation.clone(),
                cause: self.map_target(cause)?,
                effect: self.map_target(effect)?,
            },
            PropositionExpr::Quotation { content } => PropositionExpr::Quotation {
                content: self.map_proposition(content)?,
            },
        })
    }

    fn map_lexical_anchor(
        &self,
        anchor: &LexicalAnchor,
    ) -> Result<LexicalAnchor, TrainingCanonicalizationError> {
        Ok(LexicalAnchor {
            spans: map_set(&anchor.spans, |span| self.map_span(span))?,
        })
    }

    fn map_participant_role(
        &self,
        role: &ParticipantRole,
    ) -> Result<ParticipantRole, TrainingCanonicalizationError> {
        Ok(match role {
            ParticipantRole::SourceAnchored(anchor) => {
                ParticipantRole::SourceAnchored(self.map_lexical_anchor(anchor)?)
            }
            _ => role.clone(),
        })
    }

    fn map_quantifier(
        &self,
        value: &Quantifier,
    ) -> Result<Quantifier, TrainingCanonicalizationError> {
        Ok(match value {
            Quantifier::Exists => Quantifier::Exists,
            Quantifier::ForAll => Quantifier::ForAll,
            Quantifier::Exactly(value) => Quantifier::Exactly(value.clone()),
            Quantifier::AtLeast(value) => Quantifier::AtLeast(value.clone()),
            Quantifier::AtMost(value) => Quantifier::AtMost(value.clone()),
            Quantifier::MoreThan(value) => Quantifier::MoreThan(value.clone()),
            Quantifier::FewerThan(value) => Quantifier::FewerThan(value.clone()),
            Quantifier::Approximately(value) => Quantifier::Approximately(value.clone()),
            Quantifier::PluralScale(value) => Quantifier::PluralScale(value.clone()),
            Quantifier::SourceAnchored(anchor) => {
                Quantifier::SourceAnchored(self.map_lexical_anchor(anchor)?)
            }
        })
    }

    fn map_modality(&self, value: &Modality) -> Result<Modality, TrainingCanonicalizationError> {
        Ok(match value {
            Modality::SourceAnchored(anchor) => {
                Modality::SourceAnchored(self.map_lexical_anchor(anchor)?)
            }
            _ => value.clone(),
        })
    }
    fn map_attitude(
        &self,
        value: &AttitudeKind,
    ) -> Result<AttitudeKind, TrainingCanonicalizationError> {
        Ok(match value {
            AttitudeKind::SourceAnchored(anchor) => {
                AttitudeKind::SourceAnchored(self.map_lexical_anchor(anchor)?)
            }
            _ => value.clone(),
        })
    }
    fn map_speech_act(
        &self,
        value: &SpeechActKind,
    ) -> Result<SpeechActKind, TrainingCanonicalizationError> {
        Ok(match value {
            SpeechActKind::SourceAnchored(anchor) => {
                SpeechActKind::SourceAnchored(self.map_lexical_anchor(anchor)?)
            }
            _ => value.clone(),
        })
    }
    fn map_interrogative(
        &self,
        value: &InterrogativeKind,
    ) -> Result<InterrogativeKind, TrainingCanonicalizationError> {
        Ok(match value {
            InterrogativeKind::SourceAnchored(anchor) => {
                InterrogativeKind::SourceAnchored(self.map_lexical_anchor(anchor)?)
            }
            _ => value.clone(),
        })
    }
    fn map_phase(&self, value: &PhaseKind) -> Result<PhaseKind, TrainingCanonicalizationError> {
        Ok(match value {
            PhaseKind::SourceAnchored(anchor) => {
                PhaseKind::SourceAnchored(self.map_lexical_anchor(anchor)?)
            }
            _ => value.clone(),
        })
    }
    fn map_presentation_mode(
        &self,
        value: &PresentationMode,
    ) -> Result<PresentationMode, TrainingCanonicalizationError> {
        Ok(match value {
            PresentationMode::SourceAnchored(anchor) => {
                PresentationMode::SourceAnchored(self.map_lexical_anchor(anchor)?)
            }
            _ => value.clone(),
        })
    }

    fn map_term(&self, term: &Term) -> Result<Term, TrainingCanonicalizationError> {
        Ok(match term {
            Term::Referent(id) => Term::Referent(self.map_referent(id)?),
            Term::Occurrence(id) => Term::Occurrence(self.map_occurrence(id)?),
            Term::Proposition(id) => Term::Proposition(self.map_proposition(id)?),
            Term::Variable(id) => Term::Variable(self.map_variable(id)?),
            Term::Literal(value) => Term::Literal(value.clone()),
        })
    }

    fn map_target(
        &self,
        target: &SemanticTarget,
    ) -> Result<SemanticTarget, TrainingCanonicalizationError> {
        Ok(match target {
            SemanticTarget::Occurrence(id) => SemanticTarget::Occurrence(self.map_occurrence(id)?),
            SemanticTarget::Proposition(id) => {
                SemanticTarget::Proposition(self.map_proposition(id)?)
            }
            SemanticTarget::Variable(id) => SemanticTarget::Variable(self.map_variable(id)?),
        })
    }

    fn map_anchor(
        &self,
        anchor: &TemporalAnchor,
    ) -> Result<TemporalAnchor, TrainingCanonicalizationError> {
        Ok(match anchor {
            TemporalAnchor::UnixMillis { value } => TemporalAnchor::UnixMillis { value: *value },
            TemporalAnchor::Calendar {
                value,
                precision,
                timezone,
            } => TemporalAnchor::Calendar {
                value: value.clone(),
                precision: *precision,
                timezone: timezone.clone(),
            },
            TemporalAnchor::Target { target } => TemporalAnchor::Target {
                target: self.map_target(target)?,
            },
            TemporalAnchor::Variable { variable } => TemporalAnchor::Variable {
                variable: self.map_variable(variable)?,
            },
            TemporalAnchor::SourceTime { source_span } => TemporalAnchor::SourceTime {
                source_span: self.map_span(source_span)?,
            },
            TemporalAnchor::UnresolvedExpression { text } => {
                TemporalAnchor::UnresolvedExpression { text: text.clone() }
            }
        })
    }

    fn map_ambiguity_alternative(
        &self,
        alternative: &AmbiguityAlternative,
    ) -> Result<AmbiguityAlternative, TrainingCanonicalizationError> {
        Ok(match alternative {
            AmbiguityAlternative::Referent(id) => {
                AmbiguityAlternative::Referent(self.map_referent(id)?)
            }
            AmbiguityAlternative::Occurrence(id) => {
                AmbiguityAlternative::Occurrence(self.map_occurrence(id)?)
            }
            AmbiguityAlternative::Proposition(id) => {
                AmbiguityAlternative::Proposition(self.map_proposition(id)?)
            }
            AmbiguityAlternative::Concept(id) => AmbiguityAlternative::Concept(id.clone()),
            AmbiguityAlternative::Relation(id) => AmbiguityAlternative::Relation(id.clone()),
            AmbiguityAlternative::Temporal(anchor) => {
                AmbiguityAlternative::Temporal(self.map_anchor(anchor)?)
            }
            AmbiguityAlternative::Literal(value) => AmbiguityAlternative::Literal(value.clone()),
        })
    }

    fn map_span(&self, id: &SourceSpanId) -> Result<SourceSpanId, TrainingCanonicalizationError> {
        mapped(&self.spans, id, "source span")
    }
    fn map_referent(&self, id: &ReferentId) -> Result<ReferentId, TrainingCanonicalizationError> {
        mapped(&self.referents, id, "referent")
    }
    fn map_occurrence(
        &self,
        id: &OccurrenceId,
    ) -> Result<OccurrenceId, TrainingCanonicalizationError> {
        mapped(&self.occurrences, id, "occurrence")
    }
    fn map_proposition(
        &self,
        id: &PropositionId,
    ) -> Result<PropositionId, TrainingCanonicalizationError> {
        mapped(&self.propositions, id, "proposition")
    }
    fn map_statement(
        &self,
        id: &StatementId,
    ) -> Result<StatementId, TrainingCanonicalizationError> {
        mapped(&self.statements, id, "statement")
    }
    fn map_variable(&self, id: &VariableId) -> Result<VariableId, TrainingCanonicalizationError> {
        mapped(&self.variables, id, "variable")
    }
    fn map_ambiguity(
        &self,
        id: &AmbiguityId,
    ) -> Result<AmbiguityId, TrainingCanonicalizationError> {
        mapped(&self.ambiguities, id, "ambiguity")
    }

    fn variable_fingerprint(
        &self,
        id: &VariableId,
    ) -> Result<String, TrainingCanonicalizationError> {
        let variable = &self.source.variables[id];
        fingerprint(
            &json!({"sort":variable.sort,"lexical_anchor":variable.lexical_anchor.as_ref().map(|anchor|self.span_fingerprints(&anchor.spans)).transpose()?,"spans":self.span_fingerprints(&variable.source_spans)?}),
        )
    }

    fn referent_fingerprint(
        &self,
        id: &ReferentId,
    ) -> Result<String, TrainingCanonicalizationError> {
        let referent = &self.source.referents[id];
        fingerprint(&json!({
            "types": referent.types,
            "lexical_anchor": referent.lexical_anchor.as_ref().map(|anchor| self.span_fingerprints(&anchor.spans)).transpose()?,
            "external_ids": referent.external_ids,
            "roles": referent.discourse_roles,
            "spans": self.span_fingerprints(&referent.source_spans)?,
        }))
    }

    fn occurrence_fingerprint(
        &self,
        id: &OccurrenceId,
        environment: &[VariableId],
    ) -> Result<String, TrainingCanonicalizationError> {
        self.occurrence_fingerprint_inner(
            id,
            environment,
            &mut BTreeSet::new(),
            &mut BTreeSet::new(),
        )
    }

    fn occurrence_fingerprint_inner(
        &self,
        id: &OccurrenceId,
        environment: &[VariableId],
        proposition_visiting: &mut BTreeSet<PropositionId>,
        occurrence_visiting: &mut BTreeSet<OccurrenceId>,
    ) -> Result<String, TrainingCanonicalizationError> {
        if !occurrence_visiting.insert(id.clone()) {
            return Err(TrainingCanonicalizationError::CyclicOccurrenceReference(
                id.clone(),
            ));
        }
        let occurrence = &self.source.occurrences[id];
        let mut participants = occurrence
            .participants
            .iter()
            .map(|participant| {
                Ok(json!({
                    "role": self.structural_value_fingerprint(&participant.role)?,
                    "value": self.term_fingerprint_inner(&participant.value, environment, proposition_visiting, occurrence_visiting)?,
                }))
            })
            .collect::<Result<Vec<_>, TrainingCanonicalizationError>>()?;
        participants.sort_by(|left, right| {
            canonical_json_sort_key(left).cmp(&canonical_json_sort_key(right))
        });
        let mut attributes = Vec::new();
        for (relation, values) in &occurrence.attributes {
            let mut values = values
                .iter()
                .map(|term| {
                    self.term_fingerprint_inner(
                        term,
                        environment,
                        proposition_visiting,
                        occurrence_visiting,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            values.sort();
            attributes.push(json!({"relation": relation, "values": values}));
        }
        let outcome = occurrence.reported_outcome.as_ref().map(|outcome| {
            Ok::<_, TrainingCanonicalizationError>(json!({
                "status": outcome.status,
                "reporter": outcome.reporter.as_ref().map(|id| self.referent_fingerprint(id)).transpose()?,
                "detail": outcome.detail,
                "spans": self.span_fingerprints(&outcome.source_spans)?,
            }))
        }).transpose()?;
        occurrence_visiting.remove(id);
        fingerprint(&json!({
            "types": occurrence.types,
            "lexical_anchor": occurrence.lexical_anchor.as_ref().map(|anchor| self.span_fingerprints(&anchor.spans)).transpose()?,
            "participants": participants,
            "attributes": attributes,
            "tense_aspect": occurrence.tense_aspect,
            "grammatical_tense": occurrence.grammatical_tense,
            "grammatical_voice": occurrence.grammatical_voice,
            "grammatical_spans": self.span_fingerprints(&occurrence.grammatical_spans)?,
            "outcome": outcome,
            "spans": self.span_fingerprints(&occurrence.source_spans)?,
            "evidence": occurrence.evidence,
        }))
    }

    fn proposition_fingerprint(
        &self,
        id: &PropositionId,
        environment: &[VariableId],
    ) -> Result<String, TrainingCanonicalizationError> {
        self.proposition_fingerprint_inner(
            id,
            environment,
            &mut BTreeSet::new(),
            &mut BTreeSet::new(),
        )
    }

    fn proposition_fingerprint_inner(
        &self,
        id: &PropositionId,
        environment: &[VariableId],
        proposition_visiting: &mut BTreeSet<PropositionId>,
        occurrence_visiting: &mut BTreeSet<OccurrenceId>,
    ) -> Result<String, TrainingCanonicalizationError> {
        if !proposition_visiting.insert(id.clone()) {
            return Err(TrainingCanonicalizationError::CyclicCanonicalReference(
                id.clone(),
            ));
        }
        let proposition = &self.source.propositions[id];
        let expression = match &proposition.expression {
            PropositionExpr::TypeAssertion { subject, r#type } => {
                json!({"kind":"type_assertion","subject":self.term_fingerprint_inner(subject, environment, proposition_visiting, occurrence_visiting)?,"type":r#type})
            }
            PropositionExpr::Relation {
                relation,
                arguments,
            } => {
                json!({"kind":"relation","relation":relation,"arguments":arguments.iter().map(|term| self.term_fingerprint_inner(term, environment, proposition_visiting, occurrence_visiting)).collect::<Result<Vec<_>,_>>()?})
            }
            PropositionExpr::Occurrence { occurrence } => {
                json!({"kind":"occurrence","occurrence":self.occurrence_fingerprint_inner(occurrence, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Equality { left, right } => {
                let mut terms = vec![
                    self.term_fingerprint_inner(
                        left,
                        environment,
                        proposition_visiting,
                        occurrence_visiting,
                    )?,
                    self.term_fingerprint_inner(
                        right,
                        environment,
                        proposition_visiting,
                        occurrence_visiting,
                    )?,
                ];
                terms.sort();
                json!({"kind":"equality","terms":terms})
            }
            PropositionExpr::Comparison {
                operator,
                left,
                right,
                dimension,
            } => json!({
                "kind":"comparison",
                "operator":operator,
                "left":self.term_fingerprint_inner(left, environment, proposition_visiting, occurrence_visiting)?,
                "right":self.term_fingerprint_inner(right, environment, proposition_visiting, occurrence_visiting)?,
                "dimension":dimension.as_ref().map(|term| self.term_fingerprint_inner(term, environment, proposition_visiting, occurrence_visiting)).transpose()?,
            }),
            PropositionExpr::Negation { content } => {
                json!({"kind":"negation","content":self.proposition_fingerprint_inner(content, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Conjunction { members } | PropositionExpr::Disjunction { members } => {
                let mut members = members
                    .iter()
                    .map(|member| {
                        self.proposition_fingerprint_inner(
                            member,
                            environment,
                            proposition_visiting,
                            occurrence_visiting,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                members.sort();
                json!({"kind": if matches!(&proposition.expression, PropositionExpr::Conjunction { .. }) {"conjunction"} else {"disjunction"}, "members":members})
            }
            PropositionExpr::Implication {
                antecedent,
                consequent,
            } => {
                json!({"kind":"implication","antecedent":self.proposition_fingerprint_inner(antecedent, environment, proposition_visiting, occurrence_visiting)?,"consequent":self.proposition_fingerprint_inner(consequent, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Counterfactual {
                antecedent,
                consequent,
            } => {
                json!({"kind":"counterfactual","antecedent":self.proposition_fingerprint_inner(antecedent, environment, proposition_visiting, occurrence_visiting)?,"consequent":self.proposition_fingerprint_inner(consequent, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Unless {
                condition,
                consequent,
            } => {
                json!({"kind":"unless","condition":self.proposition_fingerprint_inner(condition, environment, proposition_visiting, occurrence_visiting)?,"consequent":self.proposition_fingerprint_inner(consequent, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Presuppositional {
                asserted,
                presupposed,
            } => {
                json!({"kind":"presuppositional","asserted":self.proposition_fingerprint_inner(asserted, environment, proposition_visiting, occurrence_visiting)?,"presupposed":self.proposition_fingerprint_inner(presupposed, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Quantified {
                quantifier,
                variable,
                domain,
                body,
            } => {
                let mut nested = environment.to_vec();
                nested.push(variable.clone());
                json!({"kind":"quantified","quantifier":self.structural_value_fingerprint(quantifier)?,"domain":domain,"body":self.proposition_fingerprint_inner(body, &nested, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::GeneralizedQuantified {
                quantifier,
                variable,
                domain,
                body,
            } => {
                let mut nested = environment.to_vec();
                nested.push(variable.clone());
                json!({"kind":"generalized_quantified","quantifier":self.referent_fingerprint(quantifier)?,"domain":domain,"body":self.proposition_fingerprint_inner(body, &nested, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::ScopedOperator { operator, content } => {
                json!({"kind":"scoped_operator","operator":self.referent_fingerprint(operator)?,"content":self.proposition_fingerprint_inner(content, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Modal { modality, content } => {
                json!({"kind":"modal","modality":self.structural_value_fingerprint(modality)?,"content":self.proposition_fingerprint_inner(content, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Generic { content } => {
                json!({"kind":"generic","content":self.proposition_fingerprint_inner(content, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Perfect { content } => {
                json!({"kind":"perfect","content":self.proposition_fingerprint_inner(content, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Progressive { content } => {
                json!({"kind":"progressive","content":self.proposition_fingerprint_inner(content, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Focus {
                operator,
                focus,
                content,
            } => {
                json!({"kind":"focus","operator":self.referent_fingerprint(operator)?,"focus":self.span_fingerprints(&focus.spans)?,"content":self.proposition_fingerprint_inner(content, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Capability { bearer, content } => {
                json!({"kind":"capability","bearer":self.term_fingerprint_inner(bearer, environment, proposition_visiting, occurrence_visiting)?,"content":self.proposition_fingerprint_inner(content, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Interrogative {
                interrogative,
                variable,
                domain,
                body,
            } => {
                let mut nested = environment.to_vec();
                if let Some(variable) = variable {
                    nested.push(variable.clone());
                }
                json!({"kind":"interrogative","interrogative":self.structural_value_fingerprint(interrogative)?,"has_variable":variable.is_some(),"domain":domain,"body":self.proposition_fingerprint_inner(body, &nested, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Phase { phase, content } => {
                json!({"kind":"phase","phase":self.structural_value_fingerprint(phase)?,"content":self.proposition_fingerprint_inner(content, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Attitude {
                holder,
                attitude,
                content,
            } => {
                json!({"kind":"attitude","holder":self.term_fingerprint_inner(holder, environment, proposition_visiting, occurrence_visiting)?,"attitude":self.structural_value_fingerprint(attitude)?,"content":self.proposition_fingerprint_inner(content, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::SpeechAct {
                speaker,
                act,
                addressees,
                content,
            } => {
                let mut addressees = addressees
                    .iter()
                    .map(|term| {
                        self.term_fingerprint_inner(
                            term,
                            environment,
                            proposition_visiting,
                            occurrence_visiting,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                addressees.sort();
                json!({"kind":"speech_act","speaker":self.term_fingerprint_inner(speaker, environment, proposition_visiting, occurrence_visiting)?,"act":self.structural_value_fingerprint(act)?,"addressees":addressees,"content":self.proposition_fingerprint_inner(content, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Temporal {
                subject,
                relation,
                object,
            } => {
                json!({"kind":"temporal","subject":self.target_fingerprint(subject, environment, proposition_visiting, occurrence_visiting)?,"relation":relation,"object":self.anchor_fingerprint(object, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Causal {
                relation,
                cause,
                effect,
            } => {
                json!({"kind":"causal","relation":relation,"cause":self.target_fingerprint(cause, environment, proposition_visiting, occurrence_visiting)?,"effect":self.target_fingerprint(effect, environment, proposition_visiting, occurrence_visiting)?})
            }
            PropositionExpr::Quotation { content } => {
                json!({"kind":"quotation","content":self.proposition_fingerprint_inner(content, environment, proposition_visiting, occurrence_visiting)?})
            }
        };
        proposition_visiting.remove(id);
        fingerprint(&json!({
            "expression": expression,
            "operator_spans": self.span_fingerprints(&proposition.operator_spans)?,
            "operator_tense_aspect": proposition.operator_tense_aspect,
            "operator_voice": proposition.operator_voice,
            "operator_grammatical_spans": self.span_fingerprints(&proposition.operator_grammatical_spans)?,
            "spans": self.span_fingerprints(&proposition.source_spans)?,
            "evidence": proposition.evidence,
        }))
    }

    fn structural_value_fingerprint<T: Serialize>(
        &self,
        value: &T,
    ) -> Result<Value, TrainingCanonicalizationError> {
        let mut json = serde_json::to_value(value)?;
        self.replace_source_span_ids_with_fingerprints(&mut json)?;
        Ok(json)
    }

    fn replace_source_span_ids_with_fingerprints(
        &self,
        value: &mut Value,
    ) -> Result<(), TrainingCanonicalizationError> {
        match value {
            Value::Object(map) => {
                if let Some(Value::Array(spans)) = map.get_mut("spans") {
                    let mut fingerprints = Vec::new();
                    for span in spans.iter() {
                        let Some(raw) = span.as_str() else {
                            continue;
                        };
                        let id = SourceSpanId::from(raw.to_owned());
                        if let Some(source_span) = self.source.source_spans.get(&id) {
                            fingerprints.push(Value::String(span_fingerprint(source_span)?));
                        }
                    }
                    fingerprints.sort_by(|left, right| {
                        canonical_json_sort_key(left).cmp(&canonical_json_sort_key(right))
                    });
                    *spans = fingerprints;
                }
                for child in map.values_mut() {
                    self.replace_source_span_ids_with_fingerprints(child)?;
                }
            }
            Value::Array(values) => {
                for child in values {
                    self.replace_source_span_ids_with_fingerprints(child)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn term_fingerprint(
        &self,
        term: &Term,
        environment: &[VariableId],
    ) -> Result<String, TrainingCanonicalizationError> {
        self.term_fingerprint_inner(
            term,
            environment,
            &mut BTreeSet::new(),
            &mut BTreeSet::new(),
        )
    }

    fn term_fingerprint_inner(
        &self,
        term: &Term,
        environment: &[VariableId],
        proposition_visiting: &mut BTreeSet<PropositionId>,
        occurrence_visiting: &mut BTreeSet<OccurrenceId>,
    ) -> Result<String, TrainingCanonicalizationError> {
        match term {
            Term::Referent(id) => Ok(format!("referent:{}", self.referent_fingerprint(id)?)),
            Term::Occurrence(id) => Ok(format!(
                "occurrence:{}",
                self.occurrence_fingerprint_inner(
                    id,
                    environment,
                    proposition_visiting,
                    occurrence_visiting
                )?
            )),
            Term::Proposition(id) => Ok(format!(
                "proposition:{}",
                self.proposition_fingerprint_inner(
                    id,
                    environment,
                    proposition_visiting,
                    occurrence_visiting
                )?
            )),
            Term::Variable(id) => {
                if let Some(position) = environment
                    .iter()
                    .rev()
                    .position(|candidate| candidate == id)
                {
                    Ok(format!("bound-variable:{position}"))
                } else if self.source.variables.contains_key(id) {
                    Ok(format!("free-variable:{}", self.variable_fingerprint(id)?))
                } else {
                    Err(TrainingCanonicalizationError::UnboundVariable(id.clone()))
                }
            }
            Term::Literal(value) => fingerprint(value),
        }
    }

    fn target_fingerprint(
        &self,
        target: &SemanticTarget,
        environment: &[VariableId],
        proposition_visiting: &mut BTreeSet<PropositionId>,
        occurrence_visiting: &mut BTreeSet<OccurrenceId>,
    ) -> Result<String, TrainingCanonicalizationError> {
        match target {
            SemanticTarget::Occurrence(id) => Ok(format!(
                "occurrence:{}",
                self.occurrence_fingerprint_inner(
                    id,
                    environment,
                    proposition_visiting,
                    occurrence_visiting
                )?
            )),
            SemanticTarget::Proposition(id) => Ok(format!(
                "proposition:{}",
                self.proposition_fingerprint_inner(
                    id,
                    environment,
                    proposition_visiting,
                    occurrence_visiting
                )?
            )),
            SemanticTarget::Variable(id) => {
                let Some(position) = environment
                    .iter()
                    .rev()
                    .position(|candidate| candidate == id)
                else {
                    return Err(TrainingCanonicalizationError::UnboundVariable(id.clone()));
                };
                Ok(format!("bound-variable:{position}"))
            }
        }
    }

    fn anchor_fingerprint(
        &self,
        anchor: &TemporalAnchor,
        environment: &[VariableId],
        proposition_visiting: &mut BTreeSet<PropositionId>,
        occurrence_visiting: &mut BTreeSet<OccurrenceId>,
    ) -> Result<Value, TrainingCanonicalizationError> {
        Ok(match anchor {
            TemporalAnchor::Target { target } => {
                json!({"kind":"target","value":self.target_fingerprint(target, environment, proposition_visiting, occurrence_visiting)?})
            }
            TemporalAnchor::Variable { variable } => {
                let Some(position) = environment
                    .iter()
                    .rev()
                    .position(|candidate| candidate == variable)
                else {
                    return Err(TrainingCanonicalizationError::UnboundVariable(
                        variable.clone(),
                    ));
                };
                json!({"kind":"variable","position":position})
            }
            TemporalAnchor::SourceTime { source_span } => {
                json!({"kind":"source_time","span":span_fingerprint(&self.source.source_spans[source_span])?})
            }
            _ => serde_json::to_value(anchor)?,
        })
    }

    fn ambiguity_fingerprint(
        &self,
        id: &AmbiguityId,
    ) -> Result<String, TrainingCanonicalizationError> {
        let ambiguity = &self.source.ambiguities[id];
        let mut alternatives = ambiguity
            .alternatives
            .iter()
            .map(|alternative| self.ambiguity_alternative_fingerprint(alternative))
            .collect::<Result<Vec<_>, _>>()?;
        alternatives.sort();
        fingerprint(&json!({
            "alternatives": alternatives,
            "spans": self.span_fingerprints(&ambiguity.source_spans)?,
        }))
    }

    fn ambiguity_alternative_fingerprint(
        &self,
        alternative: &AmbiguityAlternative,
    ) -> Result<String, TrainingCanonicalizationError> {
        match alternative {
            AmbiguityAlternative::Referent(id) => {
                Ok(format!("referent:{}", self.referent_fingerprint(id)?))
            }
            AmbiguityAlternative::Occurrence(id) => Ok(format!(
                "occurrence:{}",
                self.occurrence_fingerprint(id, &[])?
            )),
            AmbiguityAlternative::Proposition(id) => Ok(format!(
                "proposition:{}",
                self.proposition_fingerprint(id, &[])?
            )),
            AmbiguityAlternative::Temporal(anchor) => {
                let value = self.anchor_fingerprint(
                    anchor,
                    &[],
                    &mut BTreeSet::new(),
                    &mut BTreeSet::new(),
                )?;
                Ok(format!("temporal:{}", fingerprint(&value)?))
            }
            AmbiguityAlternative::Concept(id) => Ok(format!("concept:{}", id.as_str())),
            AmbiguityAlternative::Relation(id) => Ok(format!("relation:{}", id.as_str())),
            AmbiguityAlternative::Literal(value) => Ok(format!("literal:{}", fingerprint(value)?)),
        }
    }

    fn span_fingerprints(
        &self,
        spans: &BTreeSet<SourceSpanId>,
    ) -> Result<Vec<String>, TrainingCanonicalizationError> {
        let mut result = spans
            .iter()
            .map(|id| span_fingerprint(&self.source.source_spans[id]))
            .collect::<Result<Vec<_>, _>>()?;
        result.sort();
        Ok(result)
    }

    fn referenced_propositions(&self) -> BTreeSet<PropositionId> {
        let mut result = BTreeSet::new();
        for proposition in self.source.propositions.values() {
            collect_proposition_references(&proposition.expression, &mut result);
        }
        result
    }
}

struct ScopeValidator<'a> {
    source: &'a OccurrenceDocument,
    binders: BTreeMap<VariableId, PropositionId>,
}

impl<'a> ScopeValidator<'a> {
    fn new(source: &'a OccurrenceDocument) -> Self {
        Self {
            source,
            binders: BTreeMap::new(),
        }
    }

    fn validate(mut self) -> Result<(), TrainingCanonicalizationError> {
        for (id, proposition) in &self.source.propositions {
            let variable = match &proposition.expression {
                PropositionExpr::Quantified { variable, .. }
                | PropositionExpr::GeneralizedQuantified { variable, .. } => Some(variable),
                PropositionExpr::Interrogative {
                    variable: Some(variable),
                    ..
                } => Some(variable),
                _ => None,
            };
            if let Some(variable) = variable {
                if let Some(previous) = self.binders.insert(variable.clone(), id.clone()) {
                    if previous != *id {
                        return Err(TrainingCanonicalizationError::VariableRebound(
                            variable.clone(),
                        ));
                    }
                }
            }
        }

        let referenced = {
            let mut referenced = BTreeSet::new();
            for proposition in self.source.propositions.values() {
                collect_proposition_references(&proposition.expression, &mut referenced);
            }
            referenced
        };
        let statement_roots = self
            .source
            .statement_order
            .iter()
            .map(|statement| self.source.statements[statement].content.clone())
            .collect::<BTreeSet<_>>();
        let orphan_roots = self
            .source
            .propositions
            .keys()
            .filter(|id| !referenced.contains(*id) && !statement_roots.contains(*id))
            .cloned()
            .collect::<Vec<_>>();

        for root in statement_roots.into_iter().chain(orphan_roots) {
            let mut seen_props = BTreeSet::new();
            let mut occurrence_stack = BTreeSet::new();
            self.validate_proposition(&root, &[], &mut seen_props, &mut occurrence_stack)?;
        }

        let referenced_occurrences = self.referenced_occurrences();
        for id in self.source.occurrences.keys() {
            if !referenced_occurrences.contains(id) {
                let mut seen_props = BTreeSet::new();
                let mut occurrence_stack = BTreeSet::new();
                self.validate_occurrence(id, &[], &mut seen_props, &mut occurrence_stack)?;
            }
        }
        Ok(())
    }

    fn validate_proposition(
        &self,
        id: &PropositionId,
        environment: &[VariableId],
        seen_props: &mut BTreeSet<(PropositionId, Vec<VariableId>)>,
        occurrence_stack: &mut BTreeSet<OccurrenceId>,
    ) -> Result<(), TrainingCanonicalizationError> {
        let key = (id.clone(), environment.to_vec());
        if !seen_props.insert(key) {
            return Ok(());
        }
        let expression = &self.source.propositions[id].expression;
        match expression {
            PropositionExpr::TypeAssertion { subject, .. } => {
                self.validate_term(subject, environment, seen_props, occurrence_stack)?;
            }
            PropositionExpr::Relation { arguments, .. } => {
                for term in arguments {
                    self.validate_term(term, environment, seen_props, occurrence_stack)?;
                }
            }
            PropositionExpr::Occurrence { occurrence } => {
                self.validate_occurrence(occurrence, environment, seen_props, occurrence_stack)?;
            }
            PropositionExpr::Equality { left, right } => {
                self.validate_term(left, environment, seen_props, occurrence_stack)?;
                self.validate_term(right, environment, seen_props, occurrence_stack)?;
            }
            PropositionExpr::Comparison {
                left,
                right,
                dimension,
                ..
            } => {
                self.validate_term(left, environment, seen_props, occurrence_stack)?;
                self.validate_term(right, environment, seen_props, occurrence_stack)?;
                if let Some(dimension) = dimension {
                    self.validate_term(dimension, environment, seen_props, occurrence_stack)?;
                }
            }
            PropositionExpr::Negation { content }
            | PropositionExpr::ScopedOperator { content, .. }
            | PropositionExpr::Modal { content, .. }
            | PropositionExpr::Generic { content }
            | PropositionExpr::Perfect { content }
            | PropositionExpr::Progressive { content }
            | PropositionExpr::Phase { content, .. }
            | PropositionExpr::Quotation { content } => {
                self.validate_proposition(content, environment, seen_props, occurrence_stack)?;
            }
            PropositionExpr::Focus { content, .. } => {
                self.validate_proposition(content, environment, seen_props, occurrence_stack)?;
            }
            PropositionExpr::Capability { bearer, content } => {
                self.validate_term(bearer, environment, seen_props, occurrence_stack)?;
                self.validate_proposition(content, environment, seen_props, occurrence_stack)?;
            }
            PropositionExpr::Conjunction { members } | PropositionExpr::Disjunction { members } => {
                for member in members {
                    self.validate_proposition(member, environment, seen_props, occurrence_stack)?;
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
                self.validate_proposition(antecedent, environment, seen_props, occurrence_stack)?;
                self.validate_proposition(consequent, environment, seen_props, occurrence_stack)?;
            }
            PropositionExpr::Unless {
                condition,
                consequent,
            } => {
                self.validate_proposition(condition, environment, seen_props, occurrence_stack)?;
                self.validate_proposition(consequent, environment, seen_props, occurrence_stack)?;
            }
            PropositionExpr::Presuppositional {
                asserted,
                presupposed,
            } => {
                self.validate_proposition(asserted, environment, seen_props, occurrence_stack)?;
                self.validate_proposition(presupposed, environment, seen_props, occurrence_stack)?;
            }
            PropositionExpr::Quantified { variable, body, .. }
            | PropositionExpr::GeneralizedQuantified { variable, body, .. } => {
                let mut nested = environment.to_vec();
                nested.push(variable.clone());
                self.validate_proposition(body, &nested, seen_props, occurrence_stack)?;
            }
            PropositionExpr::Interrogative { variable, body, .. } => {
                let mut nested = environment.to_vec();
                if let Some(variable) = variable {
                    nested.push(variable.clone());
                    if !self.variable_occurs_in_proposition(
                        body,
                        variable,
                        &mut BTreeSet::new(),
                        &mut BTreeSet::new(),
                    )? {
                        return Err(TrainingCanonicalizationError::UnusedInterrogativeVariable(
                            variable.clone(),
                        ));
                    }
                }
                self.validate_proposition(body, &nested, seen_props, occurrence_stack)?;
            }
            PropositionExpr::Attitude {
                holder, content, ..
            } => {
                self.validate_term(holder, environment, seen_props, occurrence_stack)?;
                self.validate_proposition(content, environment, seen_props, occurrence_stack)?;
            }
            PropositionExpr::SpeechAct {
                speaker,
                addressees,
                content,
                ..
            } => {
                self.validate_term(speaker, environment, seen_props, occurrence_stack)?;
                for addressee in addressees {
                    self.validate_term(addressee, environment, seen_props, occurrence_stack)?;
                }
                self.validate_proposition(content, environment, seen_props, occurrence_stack)?;
            }
            PropositionExpr::Temporal {
                subject, object, ..
            } => {
                self.validate_target(subject, environment, seen_props, occurrence_stack)?;
                match object {
                    TemporalAnchor::Target { target } => {
                        self.validate_target(target, environment, seen_props, occurrence_stack)?;
                    }
                    TemporalAnchor::Variable { variable } => {
                        self.validate_variable(variable, environment)?;
                    }
                    _ => {}
                }
            }
            PropositionExpr::Causal { cause, effect, .. } => {
                self.validate_target(cause, environment, seen_props, occurrence_stack)?;
                self.validate_target(effect, environment, seen_props, occurrence_stack)?;
            }
        }
        Ok(())
    }

    fn variable_occurs_in_proposition(
        &self,
        id: &PropositionId,
        variable: &VariableId,
        seen_props: &mut BTreeSet<PropositionId>,
        seen_occurrences: &mut BTreeSet<OccurrenceId>,
    ) -> Result<bool, TrainingCanonicalizationError> {
        if !seen_props.insert(id.clone()) {
            return Ok(false);
        }
        let expression = &self.source.propositions[id].expression;
        let term_has =
            |term: &Term| matches!(term, Term::Variable(candidate) if candidate == variable);
        let result = match expression {
            PropositionExpr::TypeAssertion { subject, .. } => term_has(subject),
            PropositionExpr::Relation { arguments, .. } => arguments.iter().any(term_has),
            PropositionExpr::Occurrence { occurrence } => self.variable_occurs_in_occurrence(
                occurrence,
                variable,
                seen_props,
                seen_occurrences,
            )?,
            PropositionExpr::Equality { left, right } => term_has(left) || term_has(right),
            PropositionExpr::Comparison {
                left,
                right,
                dimension,
                ..
            } => term_has(left) || term_has(right) || dimension.as_ref().is_some_and(term_has),
            PropositionExpr::Negation { content }
            | PropositionExpr::ScopedOperator { content, .. }
            | PropositionExpr::Modal { content, .. }
            | PropositionExpr::Generic { content }
            | PropositionExpr::Perfect { content }
            | PropositionExpr::Progressive { content }
            | PropositionExpr::Focus { content, .. }
            | PropositionExpr::Phase { content, .. }
            | PropositionExpr::Quotation { content } => self.variable_occurs_in_proposition(
                content,
                variable,
                seen_props,
                seen_occurrences,
            )?,
            PropositionExpr::Capability { bearer, content } => {
                term_has(bearer)
                    || self.variable_occurs_in_proposition(
                        content,
                        variable,
                        seen_props,
                        seen_occurrences,
                    )?
            }
            PropositionExpr::Conjunction { members } | PropositionExpr::Disjunction { members } => {
                let mut found = false;
                for member in members {
                    if self.variable_occurs_in_proposition(
                        member,
                        variable,
                        seen_props,
                        seen_occurrences,
                    )? {
                        found = true;
                        break;
                    }
                }
                found
            }
            PropositionExpr::Implication {
                antecedent,
                consequent,
            }
            | PropositionExpr::Counterfactual {
                antecedent,
                consequent,
            } => {
                self.variable_occurs_in_proposition(
                    antecedent,
                    variable,
                    seen_props,
                    seen_occurrences,
                )? || self.variable_occurs_in_proposition(
                    consequent,
                    variable,
                    seen_props,
                    seen_occurrences,
                )?
            }
            PropositionExpr::Unless {
                condition,
                consequent,
            } => {
                self.variable_occurs_in_proposition(
                    condition,
                    variable,
                    seen_props,
                    seen_occurrences,
                )? || self.variable_occurs_in_proposition(
                    consequent,
                    variable,
                    seen_props,
                    seen_occurrences,
                )?
            }
            PropositionExpr::Presuppositional {
                asserted,
                presupposed,
            } => {
                self.variable_occurs_in_proposition(
                    asserted,
                    variable,
                    seen_props,
                    seen_occurrences,
                )? || self.variable_occurs_in_proposition(
                    presupposed,
                    variable,
                    seen_props,
                    seen_occurrences,
                )?
            }
            PropositionExpr::Quantified {
                variable: inner,
                body,
                ..
            }
            | PropositionExpr::Interrogative {
                variable: Some(inner),
                body,
                ..
            } if inner == variable => {
                // Inner rebinding of the same id is rejected elsewhere; do not count the binder itself as use.
                self.variable_occurs_in_proposition(body, variable, seen_props, seen_occurrences)?
            }
            PropositionExpr::Quantified { body, .. }
            | PropositionExpr::GeneralizedQuantified { body, .. }
            | PropositionExpr::Interrogative { body, .. } => {
                self.variable_occurs_in_proposition(body, variable, seen_props, seen_occurrences)?
            }
            PropositionExpr::Attitude {
                holder, content, ..
            } => {
                term_has(holder)
                    || self.variable_occurs_in_proposition(
                        content,
                        variable,
                        seen_props,
                        seen_occurrences,
                    )?
            }
            PropositionExpr::SpeechAct {
                speaker,
                addressees,
                content,
                ..
            } => {
                term_has(speaker)
                    || addressees.iter().any(term_has)
                    || self.variable_occurs_in_proposition(
                        content,
                        variable,
                        seen_props,
                        seen_occurrences,
                    )?
            }
            PropositionExpr::Temporal {
                subject, object, ..
            } => {
                self.variable_occurs_in_target(subject, variable, seen_props, seen_occurrences)?
                    || self.variable_occurs_in_anchor(
                        object,
                        variable,
                        seen_props,
                        seen_occurrences,
                    )?
            }
            PropositionExpr::Causal { cause, effect, .. } => {
                self.variable_occurs_in_target(cause, variable, seen_props, seen_occurrences)?
                    || self.variable_occurs_in_target(
                        effect,
                        variable,
                        seen_props,
                        seen_occurrences,
                    )?
            }
        };
        Ok(result)
    }

    fn variable_occurs_in_target(
        &self,
        target: &SemanticTarget,
        variable: &VariableId,
        seen_props: &mut BTreeSet<PropositionId>,
        seen_occurrences: &mut BTreeSet<OccurrenceId>,
    ) -> Result<bool, TrainingCanonicalizationError> {
        match target {
            SemanticTarget::Variable(candidate) => Ok(candidate == variable),
            SemanticTarget::Occurrence(id) => {
                self.variable_occurs_in_occurrence(id, variable, seen_props, seen_occurrences)
            }
            SemanticTarget::Proposition(id) => {
                self.variable_occurs_in_proposition(id, variable, seen_props, seen_occurrences)
            }
        }
    }

    fn variable_occurs_in_anchor(
        &self,
        anchor: &TemporalAnchor,
        variable: &VariableId,
        seen_props: &mut BTreeSet<PropositionId>,
        seen_occurrences: &mut BTreeSet<OccurrenceId>,
    ) -> Result<bool, TrainingCanonicalizationError> {
        match anchor {
            TemporalAnchor::Variable {
                variable: candidate,
            } => Ok(candidate == variable),
            TemporalAnchor::Target { target } => {
                self.variable_occurs_in_target(target, variable, seen_props, seen_occurrences)
            }
            _ => Ok(false),
        }
    }

    fn variable_occurs_in_occurrence(
        &self,
        id: &OccurrenceId,
        variable: &VariableId,
        seen_props: &mut BTreeSet<PropositionId>,
        seen_occurrences: &mut BTreeSet<OccurrenceId>,
    ) -> Result<bool, TrainingCanonicalizationError> {
        if !seen_occurrences.insert(id.clone()) {
            return Ok(false);
        }
        let occurrence = &self.source.occurrences[id];
        for participant in &occurrence.participants {
            match &participant.value {
                Term::Variable(candidate) if candidate == variable => return Ok(true),
                Term::Proposition(proposition)
                    if self.variable_occurs_in_proposition(
                        proposition,
                        variable,
                        seen_props,
                        seen_occurrences,
                    )? =>
                {
                    return Ok(true);
                }
                Term::Occurrence(occurrence)
                    if self.variable_occurs_in_occurrence(
                        occurrence,
                        variable,
                        seen_props,
                        seen_occurrences,
                    )? =>
                {
                    return Ok(true);
                }
                _ => {}
            }
        }
        for values in occurrence.attributes.values() {
            for term in values {
                if matches!(term, Term::Variable(candidate) if candidate == variable) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn validate_term(
        &self,
        term: &Term,
        environment: &[VariableId],
        seen_props: &mut BTreeSet<(PropositionId, Vec<VariableId>)>,
        occurrence_stack: &mut BTreeSet<OccurrenceId>,
    ) -> Result<(), TrainingCanonicalizationError> {
        match term {
            Term::Variable(id)
                if !environment.contains(id) && !self.source.variables.contains_key(id) =>
            {
                Err(TrainingCanonicalizationError::UnboundVariable(id.clone()))
            }
            Term::Proposition(id) => {
                self.validate_proposition(id, environment, seen_props, occurrence_stack)
            }
            Term::Occurrence(id) => {
                self.validate_occurrence(id, environment, seen_props, occurrence_stack)
            }
            _ => Ok(()),
        }
    }

    fn validate_target(
        &self,
        target: &SemanticTarget,
        environment: &[VariableId],
        seen_props: &mut BTreeSet<(PropositionId, Vec<VariableId>)>,
        occurrence_stack: &mut BTreeSet<OccurrenceId>,
    ) -> Result<(), TrainingCanonicalizationError> {
        match target {
            SemanticTarget::Proposition(id) => {
                self.validate_proposition(id, environment, seen_props, occurrence_stack)
            }
            SemanticTarget::Occurrence(id) => {
                self.validate_occurrence(id, environment, seen_props, occurrence_stack)
            }
            SemanticTarget::Variable(id) => self.validate_variable(id, environment),
        }
    }

    fn validate_variable(
        &self,
        id: &VariableId,
        environment: &[VariableId],
    ) -> Result<(), TrainingCanonicalizationError> {
        if environment.contains(id) {
            Ok(())
        } else {
            Err(TrainingCanonicalizationError::UnboundVariable(id.clone()))
        }
    }

    fn validate_occurrence(
        &self,
        id: &OccurrenceId,
        environment: &[VariableId],
        seen_props: &mut BTreeSet<(PropositionId, Vec<VariableId>)>,
        occurrence_stack: &mut BTreeSet<OccurrenceId>,
    ) -> Result<(), TrainingCanonicalizationError> {
        if !occurrence_stack.insert(id.clone()) {
            return Err(TrainingCanonicalizationError::CyclicOccurrenceReference(
                id.clone(),
            ));
        }
        let occurrence = &self.source.occurrences[id];
        for participant in &occurrence.participants {
            self.validate_term(
                &participant.value,
                environment,
                seen_props,
                occurrence_stack,
            )?;
        }
        for values in occurrence.attributes.values() {
            for term in values {
                self.validate_term(term, environment, seen_props, occurrence_stack)?;
            }
        }
        occurrence_stack.remove(id);
        Ok(())
    }

    fn referenced_occurrences(&self) -> BTreeSet<OccurrenceId> {
        let mut result = BTreeSet::new();
        for proposition in self.source.propositions.values() {
            collect_occurrence_references_from_proposition(&proposition.expression, &mut result);
        }
        for occurrence in self.source.occurrences.values() {
            for participant in &occurrence.participants {
                collect_occurrence_references_from_term(&participant.value, &mut result);
            }
            for values in occurrence.attributes.values() {
                for term in values {
                    collect_occurrence_references_from_term(term, &mut result);
                }
            }
        }
        result
    }
}

fn collect_proposition_references(
    expression: &PropositionExpr,
    output: &mut BTreeSet<PropositionId>,
) {
    match expression {
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
        | PropositionExpr::Quotation { content } => {
            output.insert(content.clone());
        }
        PropositionExpr::Interrogative { body, .. } => {
            output.insert(body.clone());
        }
        PropositionExpr::Conjunction { members } | PropositionExpr::Disjunction { members } => {
            output.extend(members.iter().cloned());
        }
        PropositionExpr::Implication {
            antecedent,
            consequent,
        }
        | PropositionExpr::Counterfactual {
            antecedent,
            consequent,
        } => {
            output.insert(antecedent.clone());
            output.insert(consequent.clone());
        }
        PropositionExpr::Unless {
            condition,
            consequent,
        } => {
            output.insert(condition.clone());
            output.insert(consequent.clone());
        }
        PropositionExpr::Presuppositional {
            asserted,
            presupposed,
        } => {
            output.insert(asserted.clone());
            output.insert(presupposed.clone());
        }
        PropositionExpr::Quantified { body, .. }
        | PropositionExpr::GeneralizedQuantified { body, .. } => {
            output.insert(body.clone());
        }
        PropositionExpr::Temporal {
            subject, object, ..
        } => {
            if let SemanticTarget::Proposition(id) = subject {
                output.insert(id.clone());
            }
            if let TemporalAnchor::Target {
                target: SemanticTarget::Proposition(id),
            } = object
            {
                output.insert(id.clone());
            }
        }
        PropositionExpr::Causal { cause, effect, .. } => {
            if let SemanticTarget::Proposition(id) = cause {
                output.insert(id.clone());
            }
            if let SemanticTarget::Proposition(id) = effect {
                output.insert(id.clone());
            }
        }
        PropositionExpr::Relation { arguments, .. } => {
            for term in arguments {
                if let Term::Proposition(id) = term {
                    output.insert(id.clone());
                }
            }
        }
        PropositionExpr::TypeAssertion { subject, .. } => {
            if let Term::Proposition(id) = subject {
                output.insert(id.clone());
            }
        }
        PropositionExpr::Equality { left, right } => {
            for term in [left, right] {
                if let Term::Proposition(id) = term {
                    output.insert(id.clone());
                }
            }
        }
        PropositionExpr::Comparison {
            left,
            right,
            dimension,
            ..
        } => {
            for term in [Some(left), Some(right), dimension.as_ref()]
                .into_iter()
                .flatten()
            {
                if let Term::Proposition(id) = term {
                    output.insert(id.clone());
                }
            }
        }
        _ => {}
    }
}

fn collect_occurrence_references_from_proposition(
    expression: &PropositionExpr,
    output: &mut BTreeSet<OccurrenceId>,
) {
    match expression {
        PropositionExpr::Occurrence { occurrence } => {
            output.insert(occurrence.clone());
        }
        PropositionExpr::Relation { arguments, .. } => {
            for term in arguments {
                collect_occurrence_references_from_term(term, output);
            }
        }
        PropositionExpr::TypeAssertion { subject, .. } => {
            collect_occurrence_references_from_term(subject, output);
        }
        PropositionExpr::Equality { left, right } => {
            collect_occurrence_references_from_term(left, output);
            collect_occurrence_references_from_term(right, output);
        }
        PropositionExpr::Temporal {
            subject, object, ..
        } => {
            collect_occurrence_references_from_target(subject, output);
            if let TemporalAnchor::Target { target } = object {
                collect_occurrence_references_from_target(target, output);
            }
        }
        PropositionExpr::Causal { cause, effect, .. } => {
            collect_occurrence_references_from_target(cause, output);
            collect_occurrence_references_from_target(effect, output);
        }
        PropositionExpr::Capability { bearer, .. }
        | PropositionExpr::Attitude { holder: bearer, .. } => {
            collect_occurrence_references_from_term(bearer, output);
        }
        PropositionExpr::SpeechAct {
            speaker,
            addressees,
            ..
        } => {
            collect_occurrence_references_from_term(speaker, output);
            for addressee in addressees {
                collect_occurrence_references_from_term(addressee, output);
            }
        }
        _ => {}
    }
}

fn collect_occurrence_references_from_term(term: &Term, output: &mut BTreeSet<OccurrenceId>) {
    if let Term::Occurrence(id) = term {
        output.insert(id.clone());
    }
}

fn collect_occurrence_references_from_target(
    target: &SemanticTarget,
    output: &mut BTreeSet<OccurrenceId>,
) {
    if let SemanticTarget::Occurrence(id) = target {
        output.insert(id.clone());
    }
}

fn sort_by_fallible_key<T, K, E>(
    values: Vec<T>,
    mut key: impl FnMut(&T) -> Result<K, E>,
) -> Result<Vec<T>, E>
where
    K: Ord,
{
    let mut keyed = values
        .into_iter()
        .map(|value| key(&value).map(|key| (key, value)))
        .collect::<Result<Vec<_>, E>>()?;
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(keyed.into_iter().map(|(_, value)| value).collect())
}

fn canonical_json_sort_key(value: &Value) -> String {
    // Values here are synthesized with statically ordered object keys. This is
    // only a local deterministic ordering key; semantic hashing still uses the
    // canonical JSON implementation in muse-core.
    serde_json::to_string(value).expect("serde_json::Value serialization is infallible")
}

fn map_set<T, U, E>(
    source: &BTreeSet<T>,
    mut map: impl FnMut(&T) -> Result<U, E>,
) -> Result<BTreeSet<U>, E>
where
    T: Ord,
    U: Ord,
{
    source.iter().map(&mut map).collect()
}

fn mapped<K: Ord + Clone, V: Clone>(
    map: &BTreeMap<K, V>,
    id: &K,
    kind: &'static str,
) -> Result<V, TrainingCanonicalizationError> {
    map.get(id)
        .cloned()
        .ok_or(TrainingCanonicalizationError::MissingCanonicalId(kind))
}

fn canonical_id(kind: &str, index: usize) -> String {
    format!("{kind}:{index:08}")
}

fn span_fingerprint(span: &SourceSpan) -> Result<String, TrainingCanonicalizationError> {
    fingerprint(&json!({
        "source": span.source,
        "run": span.run,
        "turn": span.turn,
        "message": span.message,
        "block": span.block,
        "bytes": span.bytes,
        "field_path": span.field_path,
    }))
}

fn fingerprint<T: Serialize>(value: &T) -> Result<String, TrainingCanonicalizationError> {
    Ok(canonical_digest(value)?.value)
}

#[derive(Debug, Error)]
pub enum TrainingCanonicalizationError {
    #[error(transparent)]
    Occurrence(#[from] OccurrenceValidationError),
    #[error(transparent)]
    Canonical(#[from] CanonicalHashError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("unbound variable in training projection: {0}")]
    UnboundVariable(VariableId),
    #[error("variable is bound by more than one quantifier/interrogative: {0}")]
    VariableRebound(VariableId),
    #[error("interrogative answer variable is not used in its body: {0}")]
    UnusedInterrogativeVariable(VariableId),
    #[error("cyclic proposition/object reference cannot be canonicalized: {0}")]
    CyclicCanonicalReference(PropositionId),
    #[error("cyclic occurrence reference cannot be canonicalized: {0}")]
    CyclicOccurrenceReference(OccurrenceId),
    #[error("missing canonical id for {0}")]
    MissingCanonicalId(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ByteRange, CausalRelation, DiscourseRole, Literal, Occurrence, Proposition, Referent,
        SCHEMA_VERSION, SemanticVariable, Statement,
    };
    use muse_core::{ConceptId, RelationId, canonical_digest};
    use muse_registry::RegistrySnapshot;

    fn snapshot() -> RegistrySnapshot {
        let packages = BTreeSet::new();
        RegistrySnapshot {
            identity: canonical_digest(&packages).unwrap(),
            packages,
        }
    }

    fn referent(id: ReferentId, role: DiscourseRole, span: SourceSpanId) -> Referent {
        Referent {
            id,
            types: BTreeSet::from([ConceptId::from("agent:Agent")]),
            lexical_anchor: None,
            labels: BTreeSet::new(),
            external_ids: BTreeMap::new(),
            discourse_roles: BTreeSet::from([role]),
            source_spans: BTreeSet::from([span]),
        }
    }

    fn invariant_document(rename: bool) -> OccurrenceDocument {
        let span_id = SourceSpanId::from(if rename {
            "span:renamed"
        } else {
            "span:original"
        });
        let user = ReferentId::from(if rename {
            "referent:z"
        } else {
            "referent:user"
        });
        let variable = VariableId::from(if rename {
            "variable:renamed"
        } else {
            "variable:x"
        });

        // Deliberately reverse the local lexical ordering of the two conjunction
        // members between documents. Their semantic order must still converge.
        let equality = PropositionId::from(if rename {
            "proposition:z"
        } else {
            "proposition:a"
        });
        let quantified = PropositionId::from(if rename {
            "proposition:a"
        } else {
            "proposition:z"
        });
        let variable_body = PropositionId::from(if rename {
            "proposition:y"
        } else {
            "proposition:b"
        });
        let conjunction = PropositionId::from(if rename {
            "proposition:root-b"
        } else {
            "proposition:root-a"
        });
        let statement_id = StatementId::from(if rename {
            "statement:renamed"
        } else {
            "statement:original"
        });

        let equality_expr = if rename {
            PropositionExpr::Equality {
                left: Term::Literal(Literal::String("right".into())),
                right: Term::Literal(Literal::String("left".into())),
            }
        } else {
            PropositionExpr::Equality {
                left: Term::Literal(Literal::String("left".into())),
                right: Term::Literal(Literal::String("right".into())),
            }
        };
        let propositions = BTreeMap::from([
            (
                equality.clone(),
                Proposition {
                    id: equality.clone(),
                    expression: equality_expr,
                    operator_spans: BTreeSet::new(),
                    operator_tense_aspect: None,
                    operator_voice: None,
                    operator_grammatical_spans: BTreeSet::new(),
                    source_spans: BTreeSet::from([span_id.clone()]),
                    evidence: BTreeSet::new(),
                },
            ),
            (
                variable_body.clone(),
                Proposition {
                    id: variable_body.clone(),
                    expression: PropositionExpr::Relation {
                        relation: RelationId::from("agent:hasContent"),
                        arguments: vec![
                            Term::Variable(variable.clone()),
                            Term::Literal(Literal::String("value".into())),
                        ],
                    },
                    operator_spans: BTreeSet::new(),
                    operator_tense_aspect: None,
                    operator_voice: None,
                    operator_grammatical_spans: BTreeSet::new(),
                    source_spans: BTreeSet::from([span_id.clone()]),
                    evidence: BTreeSet::new(),
                },
            ),
            (
                quantified.clone(),
                Proposition {
                    id: quantified.clone(),
                    expression: PropositionExpr::Quantified {
                        quantifier: Quantifier::Exists,
                        variable: variable.clone(),
                        domain: Some(ConceptId::from("agent:Agent")),
                        body: variable_body,
                    },
                    operator_spans: BTreeSet::new(),
                    operator_tense_aspect: None,
                    operator_voice: None,
                    operator_grammatical_spans: BTreeSet::new(),
                    source_spans: BTreeSet::from([span_id.clone()]),
                    evidence: BTreeSet::new(),
                },
            ),
            (
                conjunction.clone(),
                Proposition {
                    id: conjunction.clone(),
                    expression: PropositionExpr::Conjunction {
                        members: BTreeSet::from([equality, quantified]),
                    },
                    operator_spans: BTreeSet::new(),
                    operator_tense_aspect: None,
                    operator_voice: None,
                    operator_grammatical_spans: BTreeSet::new(),
                    source_spans: BTreeSet::from([span_id.clone()]),
                    evidence: BTreeSet::new(),
                },
            ),
        ]);
        let statement = Statement {
            id: statement_id.clone(),
            presenter: Some(user.clone()),
            addressees: BTreeSet::new(),
            mode: PresentationMode::Assertion,
            basis: StatementBasis::Expressed,
            content: conjunction,
            source_spans: BTreeSet::from([span_id.clone()]),
            evidence: BTreeSet::new(),
        };
        OccurrenceDocument {
            schema_version: SCHEMA_VERSION.into(),
            id: OccurrenceDocumentId::from(if rename {
                "occurrence-document:renamed"
            } else {
                "occurrence-document:original"
            }),
            ontology: snapshot(),
            derivation: Derivation::HumanAnnotation {
                annotator: if rename { "other" } else { "one" }.into(),
                protocol: "test".into(),
            },
            source_spans: BTreeMap::from([(
                span_id.clone(),
                SourceSpan {
                    id: span_id.clone(),
                    source: "dataset:one".into(),
                    run: Some("run:1".into()),
                    turn: Some("turn:1".into()),
                    message: Some("message:1".into()),
                    block: Some(0),
                    bytes: Some(ByteRange { start: 0, end: 10 }),
                    field_path: None,
                },
            )]),
            referents: BTreeMap::from([(
                user.clone(),
                referent(user, DiscourseRole::User, span_id.clone()),
            )]),
            occurrences: BTreeMap::new(),
            variables: BTreeMap::from([(
                variable.clone(),
                SemanticVariable {
                    id: variable,
                    sort: ConceptId::from("agent:Agent"),
                    lexical_anchor: None,
                    source_spans: BTreeSet::from([span_id.clone()]),
                },
            )]),
            propositions,
            statements: BTreeMap::from([(statement_id.clone(), statement)]),
            ambiguities: BTreeMap::new(),
            statement_order: vec![statement_id],
        }
    }

    #[test]
    fn training_projection_is_id_alpha_and_commutative_invariant() {
        let left = invariant_document(false);
        let right = invariant_document(true);
        assert_eq!(
            left.canonical_training_bytes().unwrap(),
            right.canonical_training_bytes().unwrap()
        );
        assert_eq!(
            left.canonical_training_digest().unwrap(),
            right.canonical_training_digest().unwrap()
        );
    }

    fn source_anchored_quantifier_document(rename: bool) -> OccurrenceDocument {
        let mut document = invariant_document(rename);
        let span = document.source_spans.keys().next().unwrap().clone();
        let mut found = false;
        for proposition in document.propositions.values_mut() {
            if let PropositionExpr::Quantified { quantifier, .. } = &mut proposition.expression {
                *quantifier = Quantifier::SourceAnchored(LexicalAnchor {
                    spans: BTreeSet::from([span.clone()]),
                });
                proposition.operator_spans = BTreeSet::from([span.clone()]);
                found = true;
            }
        }
        assert!(found);
        document
    }

    #[test]
    fn source_anchored_quantifier_is_local_span_id_invariant() {
        let left = source_anchored_quantifier_document(false);
        let right = source_anchored_quantifier_document(true);
        assert_eq!(
            left.canonical_training_bytes().unwrap(),
            right.canonical_training_bytes().unwrap()
        );
        assert_eq!(
            left.canonical_training_digest().unwrap(),
            right.canonical_training_digest().unwrap()
        );
    }

    fn attitude_document(holder_role: DiscourseRole) -> OccurrenceDocument {
        let span = SourceSpanId::from("span:one");
        let user = ReferentId::from("referent:user");
        let agent = ReferentId::from("referent:agent");
        let content = PropositionId::from("proposition:content");
        let thought = PropositionId::from("proposition:thought");
        let statement_id = StatementId::from("statement:one");
        let holder = match holder_role {
            DiscourseRole::User => user.clone(),
            DiscourseRole::Agent => agent.clone(),
            _ => panic!("test holder must be user or agent"),
        };
        let statement = Statement {
            id: statement_id.clone(),
            presenter: Some(holder.clone()),
            addressees: BTreeSet::new(),
            mode: PresentationMode::Assertion,
            basis: StatementBasis::Expressed,
            content: thought.clone(),
            source_spans: BTreeSet::from([span.clone()]),
            evidence: BTreeSet::new(),
        };
        OccurrenceDocument {
            schema_version: SCHEMA_VERSION.into(),
            id: OccurrenceDocumentId::from("occurrence-document:attitude"),
            ontology: snapshot(),
            derivation: Derivation::Imported {
                source: "test".into(),
            },
            source_spans: BTreeMap::from([(
                span.clone(),
                SourceSpan {
                    id: span.clone(),
                    source: "dataset:perspective".into(),
                    run: None,
                    turn: None,
                    message: None,
                    block: None,
                    bytes: Some(ByteRange { start: 0, end: 11 }),
                    field_path: None,
                },
            )]),
            referents: BTreeMap::from([
                (
                    user.clone(),
                    referent(user, DiscourseRole::User, span.clone()),
                ),
                (
                    agent.clone(),
                    referent(agent, DiscourseRole::Agent, span.clone()),
                ),
            ]),
            occurrences: BTreeMap::new(),
            variables: BTreeMap::new(),
            propositions: BTreeMap::from([
                (
                    content.clone(),
                    Proposition {
                        id: content.clone(),
                        expression: PropositionExpr::Equality {
                            left: Term::Literal(Literal::String("x".into())),
                            right: Term::Literal(Literal::String("x".into())),
                        },
                        operator_spans: BTreeSet::new(),
                        operator_tense_aspect: None,
                        operator_voice: None,
                        operator_grammatical_spans: BTreeSet::new(),
                        source_spans: BTreeSet::from([span.clone()]),
                        evidence: BTreeSet::new(),
                    },
                ),
                (
                    thought.clone(),
                    Proposition {
                        id: thought.clone(),
                        expression: PropositionExpr::Attitude {
                            holder: Term::Referent(holder),
                            attitude: AttitudeKind::Thought,
                            content,
                        },
                        operator_spans: BTreeSet::new(),
                        operator_tense_aspect: None,
                        operator_voice: None,
                        operator_grammatical_spans: BTreeSet::new(),
                        source_spans: BTreeSet::from([span]),
                        evidence: BTreeSet::new(),
                    },
                ),
            ]),
            statements: BTreeMap::from([(statement_id.clone(), statement)]),
            ambiguities: BTreeMap::new(),
            statement_order: vec![statement_id],
        }
    }

    #[test]
    fn training_projection_preserves_perspective_holder() {
        let user = attitude_document(DiscourseRole::User);
        let agent = attitude_document(DiscourseRole::Agent);
        assert_ne!(
            user.canonical_training_digest().unwrap(),
            agent.canonical_training_digest().unwrap()
        );
    }

    #[test]
    fn training_projection_preserves_declared_open_variables() {
        let mut document = invariant_document(false);
        let id = PropositionId::from("proposition:unbound-root");
        document.propositions.insert(
            id.clone(),
            Proposition {
                id: id.clone(),
                expression: PropositionExpr::TypeAssertion {
                    subject: Term::Variable(VariableId::from("variable:free")),
                    r#type: ConceptId::from("agent:Agent"),
                },
                operator_spans: BTreeSet::new(),
                operator_tense_aspect: None,
                operator_voice: None,
                operator_grammatical_spans: BTreeSet::new(),
                source_spans: BTreeSet::new(),
                evidence: BTreeSet::new(),
            },
        );
        let free = VariableId::from("variable:free");
        document.variables.insert(
            free.clone(),
            SemanticVariable {
                id: free,
                sort: ConceptId::from("agent:Agent"),
                lexical_anchor: None,
                source_spans: BTreeSet::new(),
            },
        );
        document.statements.values_mut().next().unwrap().content = id;
        assert!(document.canonical_training_projection().is_ok());
    }

    #[test]
    fn training_projection_rejects_recursive_occurrences() {
        let mut document = invariant_document(false);
        let occurrence = OccurrenceId::from("occurrence:recursive");
        document.occurrences.insert(
            occurrence.clone(),
            Occurrence {
                id: occurrence.clone(),
                types: BTreeSet::new(),
                lexical_anchor: None,
                participants: vec![Participant {
                    role: ParticipantRole::Ontology(RelationId::from("agent:hasContent")),
                    value: Term::Occurrence(occurrence),
                }],
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
        assert!(matches!(
            document.canonical_training_projection(),
            Err(TrainingCanonicalizationError::CyclicOccurrenceReference(_))
        ));
    }
    #[test]
    fn training_projection_ignores_referent_display_labels() {
        let left = invariant_document(false);
        let mut right = left.clone();
        right
            .referents
            .values_mut()
            .next()
            .unwrap()
            .labels
            .insert("surface-only".into());
        assert_eq!(
            left.canonical_training_digest().unwrap(),
            right.canonical_training_digest().unwrap()
        );
    }

    #[test]
    fn training_projection_coalesces_duplicate_source_coordinates() {
        let left = invariant_document(false);
        let mut right = left.clone();
        let original = right.source_spans.values().next().unwrap().clone();
        let duplicate_id = SourceSpanId::from("span:duplicate-local-id");
        let mut duplicate = original;
        duplicate.id = duplicate_id.clone();
        right.source_spans.insert(duplicate_id, duplicate);
        assert_eq!(
            left.canonical_training_digest().unwrap(),
            right.canonical_training_digest().unwrap()
        );
    }

    fn wh_document(rename: bool, use_answer: bool) -> OccurrenceDocument {
        let span = SourceSpanId::from(if rename { "span:wh-renamed" } else { "span:wh" });
        let user = ReferentId::from(if rename {
            "referent:wh-renamed"
        } else {
            "referent:wh-user"
        });
        let variable = VariableId::from(if rename {
            "variable:answer-renamed"
        } else {
            "variable:answer"
        });
        let occurrence = OccurrenceId::from(if rename {
            "occurrence:wh-renamed"
        } else {
            "occurrence:wh"
        });
        let body = PropositionId::from(if rename {
            "proposition:body-renamed"
        } else {
            "proposition:body"
        });
        let question = PropositionId::from(if rename {
            "proposition:q-renamed"
        } else {
            "proposition:q"
        });
        let statement = StatementId::from(if rename {
            "statement:q-renamed"
        } else {
            "statement:q"
        });
        let answer_term = if use_answer {
            Term::Variable(variable.clone())
        } else {
            Term::Referent(user.clone())
        };
        let occurrence_value = Occurrence {
            id: occurrence.clone(),
            types: BTreeSet::new(),
            lexical_anchor: Some(LexicalAnchor {
                spans: BTreeSet::from([span.clone()]),
            }),
            participants: vec![Participant {
                role: ParticipantRole::SourceAnchored(LexicalAnchor {
                    spans: BTreeSet::from([span.clone()]),
                }),
                value: answer_term,
            }],
            attributes: BTreeMap::new(),
            tense_aspect: None,
            grammatical_tense: None,
            grammatical_voice: None,
            grammatical_spans: BTreeSet::new(),
            reported_outcome: None,
            source_spans: BTreeSet::from([span.clone()]),
            evidence: BTreeSet::new(),
        };
        OccurrenceDocument {
            schema_version: SCHEMA_VERSION.into(),
            id: OccurrenceDocumentId::from(if rename {
                "occurrence-document:wh-renamed"
            } else {
                "occurrence-document:wh"
            }),
            ontology: snapshot(),
            derivation: Derivation::HumanAnnotation {
                annotator: "test".into(),
                protocol: "test".into(),
            },
            source_spans: BTreeMap::from([(
                span.clone(),
                SourceSpan {
                    id: span.clone(),
                    source: "dataset:wh".into(),
                    run: Some("run".into()),
                    turn: None,
                    message: Some("message".into()),
                    block: Some(0),
                    bytes: Some(ByteRange { start: 0, end: 8 }),
                    field_path: None,
                },
            )]),
            referents: BTreeMap::from([(
                user.clone(),
                Referent {
                    id: user.clone(),
                    types: BTreeSet::new(),
                    lexical_anchor: None,
                    labels: BTreeSet::new(),
                    external_ids: BTreeMap::new(),
                    discourse_roles: BTreeSet::from([DiscourseRole::User]),
                    source_spans: BTreeSet::from([span.clone()]),
                },
            )]),
            occurrences: BTreeMap::from([(occurrence.clone(), occurrence_value)]),
            variables: BTreeMap::from([(
                variable.clone(),
                SemanticVariable {
                    id: variable.clone(),
                    sort: ConceptId::from("ufo:Entity"),
                    lexical_anchor: Some(LexicalAnchor {
                        spans: BTreeSet::from([span.clone()]),
                    }),
                    source_spans: BTreeSet::from([span.clone()]),
                },
            )]),
            propositions: BTreeMap::from([
                (
                    body.clone(),
                    Proposition {
                        id: body.clone(),
                        expression: PropositionExpr::Occurrence { occurrence },
                        operator_spans: BTreeSet::new(),
                        operator_tense_aspect: None,
                        operator_voice: None,
                        operator_grammatical_spans: BTreeSet::new(),
                        source_spans: BTreeSet::from([span.clone()]),
                        evidence: BTreeSet::new(),
                    },
                ),
                (
                    question.clone(),
                    Proposition {
                        id: question.clone(),
                        expression: PropositionExpr::Interrogative {
                            interrogative: InterrogativeKind::Wh,
                            variable: Some(variable),
                            domain: None,
                            body,
                        },
                        operator_spans: BTreeSet::from([span.clone()]),
                        operator_tense_aspect: None,
                        operator_voice: None,
                        operator_grammatical_spans: BTreeSet::new(),
                        source_spans: BTreeSet::from([span.clone()]),
                        evidence: BTreeSet::new(),
                    },
                ),
            ]),
            statements: BTreeMap::from([(
                statement.clone(),
                Statement {
                    id: statement.clone(),
                    presenter: Some(user),
                    addressees: BTreeSet::new(),
                    mode: PresentationMode::Question,
                    basis: StatementBasis::Expressed,
                    content: question,
                    source_spans: BTreeSet::from([span]),
                    evidence: BTreeSet::new(),
                },
            )]),
            ambiguities: BTreeMap::new(),
            statement_order: vec![statement],
        }
    }

    #[test]
    fn wh_projection_is_alpha_and_source_id_invariant() {
        let left = wh_document(false, true);
        let right = wh_document(true, true);
        assert_eq!(
            left.canonical_training_bytes().unwrap(),
            right.canonical_training_bytes().unwrap()
        );
    }

    #[test]
    fn wh_projection_rejects_unused_answer_variable() {
        let document = wh_document(false, false);
        assert!(matches!(
            document.canonical_training_projection(),
            Err(TrainingCanonicalizationError::UnusedInterrogativeVariable(
                _
            ))
        ));
    }

    #[test]
    fn reason_interrogative_binds_semantic_target_variable() {
        let mut document = wh_document(false, true);
        let question_id = document
            .statement_order
            .first()
            .and_then(|sid| document.statements.get(sid))
            .unwrap()
            .content
            .clone();
        let variable = match &document.propositions[&question_id].expression {
            PropositionExpr::Interrogative {
                variable: Some(variable),
                ..
            } => variable.clone(),
            _ => panic!("expected WH"),
        };
        let occurrence = document.occurrences.keys().next().unwrap().clone();
        // The old entity use is removed; the answer now occurs as the unknown cause.
        document
            .occurrences
            .get_mut(&occurrence)
            .unwrap()
            .participants[0]
            .value = Term::Referent(document.referents.keys().next().unwrap().clone());
        let causal = PropositionId::from("proposition:reason-body");
        let span = document.source_spans.keys().next().unwrap().clone();
        document.propositions.insert(
            causal.clone(),
            Proposition {
                id: causal.clone(),
                expression: PropositionExpr::Causal {
                    relation: CausalRelation::Explains,
                    cause: SemanticTarget::Variable(variable),
                    effect: SemanticTarget::Occurrence(occurrence),
                },
                operator_spans: BTreeSet::from([span.clone()]),
                operator_tense_aspect: None,
                operator_voice: None,
                operator_grammatical_spans: BTreeSet::new(),
                source_spans: BTreeSet::from([span]),
                evidence: BTreeSet::new(),
            },
        );
        if let PropositionExpr::Interrogative {
            interrogative,
            body,
            ..
        } = &mut document
            .propositions
            .get_mut(&question_id)
            .unwrap()
            .expression
        {
            *interrogative = InterrogativeKind::Reason;
            *body = causal;
        }
        assert!(document.canonical_training_projection().is_ok());
    }
}
