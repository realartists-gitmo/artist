//! Deterministic forward reasoning and ontology conformance checking.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use muse_core::{ConceptId, IdentityError, RelationId, SemanticObjectId};
use muse_ontology::{Atom, Axiom, OntologyIndex, RelationCharacteristic};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fact {
    InstanceOf {
        individual: SemanticObjectId,
        concept: ConceptId,
    },
    Relation {
        subject: SemanticObjectId,
        relation: RelationId,
        object: SemanticObjectId,
    },
}

impl Fact {
    pub fn validate(&self) -> Result<(), IdentityError> {
        match self {
            Self::InstanceOf {
                individual,
                concept,
            } => {
                individual.validate()?;
                concept.validate()?;
            }
            Self::Relation {
                subject,
                relation,
                object,
            } => {
                subject.validate()?;
                relation.validate()?;
                object.validate()?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeBase {
    pub facts: BTreeSet<Fact>,
}

impl KnowledgeBase {
    #[must_use]
    pub fn new(facts: impl IntoIterator<Item = Fact>) -> Self {
        Self {
            facts: facts.into_iter().collect(),
        }
    }

    pub fn validate(&self, index: &OntologyIndex) -> Result<(), ReasoningError> {
        for fact in &self.facts {
            fact.validate()?;
            match fact {
                Fact::InstanceOf { concept, .. } if !index.concepts.contains_key(concept) => {
                    return Err(ReasoningError::UnknownConcept(concept.clone()));
                }
                Fact::Relation { relation, .. } if !index.relations.contains_key(relation) => {
                    return Err(ReasoningError::UnknownRelation(relation.clone()));
                }
                _ => {}
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn types_of(&self, individual: &SemanticObjectId) -> BTreeSet<ConceptId> {
        self.facts
            .iter()
            .filter_map(|fact| match fact {
                Fact::InstanceOf {
                    individual: candidate,
                    concept,
                } if candidate == individual => Some(concept.clone()),
                _ => None,
            })
            .collect()
    }

    #[must_use]
    pub fn objects_of(
        &self,
        subject: &SemanticObjectId,
        relation: &RelationId,
    ) -> BTreeSet<SemanticObjectId> {
        self.facts
            .iter()
            .filter_map(|fact| match fact {
                Fact::Relation {
                    subject: candidate,
                    relation: candidate_relation,
                    object,
                } if candidate == subject && candidate_relation == relation => Some(object.clone()),
                _ => None,
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldAssumption {
    #[default]
    Open,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningOptions {
    pub world_assumption: WorldAssumption,
    pub max_iterations: usize,
}

impl Default for ReasoningOptions {
    fn default() -> Self {
        Self {
            world_assumption: WorldAssumption::Open,
            max_iterations: 1_024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViolationKind {
    DisjointTypes,
    IrreflexiveRelation,
    AsymmetricRelation,
    AntisymmetricRelation,
    FunctionalRelation,
    InverseFunctionalRelation,
    MinimumCardinality,
    MaximumCardinality,
    UniversalRestriction,
    ExistentialRestriction,
    IncompletePartition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstraintViolation {
    pub kind: ViolationKind,
    pub message: String,
    pub individuals: BTreeSet<SemanticObjectId>,
    pub concepts: BTreeSet<ConceptId>,
    pub relations: BTreeSet<RelationId>,
}

impl ConstraintViolation {
    fn new(kind: ViolationKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            individuals: BTreeSet::new(),
            concepts: BTreeSet::new(),
            relations: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningReport {
    pub closure: KnowledgeBase,
    pub derived_facts: BTreeSet<Fact>,
    pub violations: Vec<ConstraintViolation>,
    pub iterations: usize,
}

impl ReasoningReport {
    #[must_use]
    pub fn conforms(&self) -> bool {
        self.violations.is_empty()
    }
}

pub struct ForwardReasoner<'a> {
    index: &'a OntologyIndex,
}

impl<'a> ForwardReasoner<'a> {
    #[must_use]
    pub const fn new(index: &'a OntologyIndex) -> Self {
        Self { index }
    }

    pub fn reason(
        &self,
        input: &KnowledgeBase,
        options: &ReasoningOptions,
    ) -> Result<ReasoningReport, ReasoningError> {
        input.validate(self.index)?;
        let original = input.facts.clone();
        let mut closure = original.clone();
        let relation_ancestors = relation_ancestor_closure(self.index)?;
        let mut iterations = 0;

        loop {
            iterations += 1;
            if iterations > options.max_iterations {
                return Err(ReasoningError::IterationLimit(options.max_iterations));
            }
            let before = closure.len();
            self.apply_taxonomy(&mut closure);
            self.apply_relation_declarations(&mut closure, &relation_ancestors);
            self.apply_reflexivity(&mut closure);
            self.apply_equivalence(&mut closure);
            self.apply_transitivity(&mut closure);
            self.apply_rules(&mut closure)?;
            if closure.len() == before {
                break;
            }
        }

        let violations = self.check_constraints(&closure, options.world_assumption);
        let derived_facts = closure.difference(&original).cloned().collect();
        Ok(ReasoningReport {
            closure: KnowledgeBase { facts: closure },
            derived_facts,
            violations,
            iterations,
        })
    }

    fn apply_taxonomy(&self, closure: &mut BTreeSet<Fact>) {
        let current: Vec<Fact> = closure.iter().cloned().collect();
        for fact in current {
            if let Fact::InstanceOf {
                individual,
                concept,
            } = fact
            {
                if let Some(ancestors) = self.index.ancestors.get(&concept) {
                    for ancestor in ancestors {
                        closure.insert(Fact::InstanceOf {
                            individual: individual.clone(),
                            concept: ancestor.clone(),
                        });
                    }
                }
            }
        }
    }

    fn apply_relation_declarations(
        &self,
        closure: &mut BTreeSet<Fact>,
        relation_ancestors: &BTreeMap<RelationId, BTreeSet<RelationId>>,
    ) {
        let current: Vec<Fact> = closure.iter().cloned().collect();
        for fact in current {
            let Fact::Relation {
                subject,
                relation,
                object,
            } = fact
            else {
                continue;
            };
            let Some(declaration) = self.index.relations.get(&relation) else {
                continue;
            };
            if let Some(ancestors) = relation_ancestors.get(&relation) {
                for ancestor in ancestors {
                    closure.insert(Fact::Relation {
                        subject: subject.clone(),
                        relation: ancestor.clone(),
                        object: object.clone(),
                    });
                }
            }
            if let Some(inverse) = &declaration.inverse {
                closure.insert(Fact::Relation {
                    subject: object.clone(),
                    relation: inverse.clone(),
                    object: subject.clone(),
                });
            }
            if declaration
                .characteristics
                .contains(&RelationCharacteristic::Symmetric)
            {
                closure.insert(Fact::Relation {
                    subject: object.clone(),
                    relation: relation.clone(),
                    object: subject.clone(),
                });
            }
            for domain in &declaration.domains {
                closure.insert(Fact::InstanceOf {
                    individual: subject.clone(),
                    concept: domain.clone(),
                });
            }
            for range in &declaration.ranges {
                closure.insert(Fact::InstanceOf {
                    individual: object.clone(),
                    concept: range.clone(),
                });
            }
        }
    }

    fn apply_reflexivity(&self, closure: &mut BTreeSet<Fact>) {
        let kb = KnowledgeBase {
            facts: closure.clone(),
        };
        let individuals = all_individuals(closure);
        for relation in self.index.relations.values().filter(|relation| {
            relation
                .characteristics
                .contains(&RelationCharacteristic::Reflexive)
        }) {
            for individual in &individuals {
                let types = kb.types_of(individual);
                let domains_match = relation.domains.is_empty()
                    || relation.domains.iter().all(|domain| types.contains(domain));
                let ranges_match = relation.ranges.is_empty()
                    || relation.ranges.iter().all(|range| types.contains(range));
                if domains_match && ranges_match {
                    closure.insert(Fact::Relation {
                        subject: individual.clone(),
                        relation: relation.id.clone(),
                        object: individual.clone(),
                    });
                }
            }
        }
    }

    fn apply_equivalence(&self, closure: &mut BTreeSet<Fact>) {
        let current: Vec<Fact> = closure.iter().cloned().collect();
        for axiom in &self.index.axioms {
            let Axiom::Equivalent { members } = axiom else {
                continue;
            };
            for fact in &current {
                let Fact::InstanceOf {
                    individual,
                    concept,
                } = fact
                else {
                    continue;
                };
                if members.contains(concept) {
                    for member in members {
                        closure.insert(Fact::InstanceOf {
                            individual: individual.clone(),
                            concept: member.clone(),
                        });
                    }
                }
            }
        }
    }

    fn apply_transitivity(&self, closure: &mut BTreeSet<Fact>) {
        let transitive: BTreeSet<RelationId> = self
            .index
            .relations
            .values()
            .filter(|relation| {
                relation
                    .characteristics
                    .contains(&RelationCharacteristic::Transitive)
            })
            .map(|relation| relation.id.clone())
            .collect();
        let relations: Vec<(SemanticObjectId, RelationId, SemanticObjectId)> = closure
            .iter()
            .filter_map(|fact| match fact {
                Fact::Relation {
                    subject,
                    relation,
                    object,
                } if transitive.contains(relation) => {
                    Some((subject.clone(), relation.clone(), object.clone()))
                }
                _ => None,
            })
            .collect();
        for (left_subject, left_relation, left_object) in &relations {
            for (right_subject, right_relation, right_object) in &relations {
                if left_relation == right_relation && left_object == right_subject {
                    closure.insert(Fact::Relation {
                        subject: left_subject.clone(),
                        relation: left_relation.clone(),
                        object: right_object.clone(),
                    });
                }
            }
        }
    }

    fn apply_rules(&self, closure: &mut BTreeSet<Fact>) -> Result<(), ReasoningError> {
        for axiom in &self.index.axioms {
            let Axiom::Rule { rule } = axiom else {
                continue;
            };
            for substitution in match_premises(&rule.premises, closure, self.index)? {
                if let Some(fact) = instantiate_atom(&rule.conclusion, &substitution, self.index)? {
                    closure.insert(fact);
                }
            }
        }
        Ok(())
    }

    fn check_constraints(
        &self,
        closure: &BTreeSet<Fact>,
        world: WorldAssumption,
    ) -> Vec<ConstraintViolation> {
        let kb = KnowledgeBase {
            facts: closure.clone(),
        };
        let individuals = all_individuals(closure);
        let mut violations = Vec::new();

        for individual in &individuals {
            let types = kb.types_of(individual);
            for (left, right) in &self.index.disjoint_pairs {
                if types.contains(left) && types.contains(right) {
                    let mut violation = ConstraintViolation::new(
                        ViolationKind::DisjointTypes,
                        format!("{individual} instantiates disjoint concepts {left} and {right}"),
                    );
                    violation.individuals.insert(individual.clone());
                    violation.concepts.extend([left.clone(), right.clone()]);
                    violations.push(violation);
                }
            }
        }

        for declaration in self.index.relations.values() {
            let pairs: BTreeSet<(SemanticObjectId, SemanticObjectId)> = closure
                .iter()
                .filter_map(|fact| match fact {
                    Fact::Relation {
                        subject,
                        relation,
                        object,
                    } if relation == &declaration.id => Some((subject.clone(), object.clone())),
                    _ => None,
                })
                .collect();
            for (subject, object) in &pairs {
                if subject == object
                    && declaration
                        .characteristics
                        .contains(&RelationCharacteristic::Irreflexive)
                {
                    violations.push(relation_violation(
                        ViolationKind::IrreflexiveRelation,
                        declaration.id.clone(),
                        subject,
                        object,
                        format!("{} is irreflexive", declaration.id),
                    ));
                }
                if subject != object && pairs.contains(&(object.clone(), subject.clone())) {
                    if declaration
                        .characteristics
                        .contains(&RelationCharacteristic::Asymmetric)
                    {
                        violations.push(relation_violation(
                            ViolationKind::AsymmetricRelation,
                            declaration.id.clone(),
                            subject,
                            object,
                            format!("{} is asymmetric", declaration.id),
                        ));
                    }
                    if declaration
                        .characteristics
                        .contains(&RelationCharacteristic::Antisymmetric)
                    {
                        violations.push(relation_violation(
                            ViolationKind::AntisymmetricRelation,
                            declaration.id.clone(),
                            subject,
                            object,
                            format!("{} is antisymmetric", declaration.id),
                        ));
                    }
                }
            }
            if declaration
                .characteristics
                .contains(&RelationCharacteristic::Functional)
            {
                check_functional(&declaration.id, &pairs, false, &mut violations);
            }
            if declaration
                .characteristics
                .contains(&RelationCharacteristic::InverseFunctional)
            {
                check_functional(&declaration.id, &pairs, true, &mut violations);
            }
            if let Some(constraint) = &declaration.cardinality {
                let subjects: BTreeSet<SemanticObjectId> = individuals
                    .iter()
                    .filter(|individual| {
                        let types = kb.types_of(individual);
                        declaration.domains.is_empty()
                            || declaration
                                .domains
                                .iter()
                                .all(|domain| types.contains(domain))
                    })
                    .cloned()
                    .collect();
                for subject in subjects {
                    check_cardinality(
                        &kb,
                        &subject,
                        None,
                        &declaration.id,
                        None,
                        constraint.minimum,
                        constraint.maximum,
                        world,
                        &mut violations,
                    );
                }
            }
        }

        for axiom in &self.index.axioms {
            match axiom {
                Axiom::Cardinality {
                    subject,
                    relation,
                    object,
                    constraint,
                } => {
                    for individual in individuals_of_type(&kb, subject) {
                        check_cardinality(
                            &kb,
                            &individual,
                            Some(subject),
                            relation,
                            object.as_ref(),
                            constraint.minimum,
                            constraint.maximum,
                            world,
                            &mut violations,
                        );
                    }
                }
                Axiom::UniversalRestriction {
                    subject,
                    relation,
                    object,
                } => {
                    for individual in individuals_of_type(&kb, subject) {
                        for value in kb.objects_of(&individual, relation) {
                            if !kb.types_of(&value).contains(object) {
                                let mut violation = ConstraintViolation::new(
                                    ViolationKind::UniversalRestriction,
                                    format!(
                                        "{individual} has {relation} value {value} outside {object}"
                                    ),
                                );
                                violation.individuals.extend([individual.clone(), value]);
                                violation.concepts.extend([subject.clone(), object.clone()]);
                                violation.relations.insert(relation.clone());
                                violations.push(violation);
                            }
                        }
                    }
                }
                Axiom::ExistentialRestriction {
                    subject,
                    relation,
                    object,
                } if world == WorldAssumption::Closed => {
                    for individual in individuals_of_type(&kb, subject) {
                        let satisfies = kb
                            .objects_of(&individual, relation)
                            .iter()
                            .any(|value| kb.types_of(value).contains(object));
                        if !satisfies {
                            let mut violation = ConstraintViolation::new(
                                ViolationKind::ExistentialRestriction,
                                format!(
                                    "{individual} lacks required {relation} value of type {object}"
                                ),
                            );
                            violation.individuals.insert(individual);
                            violation.concepts.extend([subject.clone(), object.clone()]);
                            violation.relations.insert(relation.clone());
                            violations.push(violation);
                        }
                    }
                }
                Axiom::CompletePartition { parent, members }
                    if world == WorldAssumption::Closed =>
                {
                    for individual in individuals_of_type(&kb, parent) {
                        let count = members
                            .iter()
                            .filter(|member| kb.types_of(&individual).contains(*member))
                            .count();
                        if count == 0 {
                            let mut violation = ConstraintViolation::new(
                                ViolationKind::IncompletePartition,
                                format!("{individual} has no known member of partition {parent}"),
                            );
                            violation.individuals.insert(individual);
                            violation.concepts.insert(parent.clone());
                            violation.concepts.extend(members.iter().cloned());
                            violations.push(violation);
                        }
                    }
                }
                _ => {}
            }
        }
        violations.sort_by(|left, right| left.message.cmp(&right.message));
        violations.dedup();
        violations
    }
}

fn all_individuals(facts: &BTreeSet<Fact>) -> BTreeSet<SemanticObjectId> {
    facts
        .iter()
        .flat_map(|fact| match fact {
            Fact::InstanceOf { individual, .. } => vec![individual.clone()],
            Fact::Relation {
                subject, object, ..
            } => vec![subject.clone(), object.clone()],
        })
        .collect()
}

fn individuals_of_type(kb: &KnowledgeBase, concept: &ConceptId) -> BTreeSet<SemanticObjectId> {
    kb.facts
        .iter()
        .filter_map(|fact| match fact {
            Fact::InstanceOf {
                individual,
                concept: candidate,
            } if candidate == concept => Some(individual.clone()),
            _ => None,
        })
        .collect()
}

fn relation_violation(
    kind: ViolationKind,
    relation: RelationId,
    subject: &SemanticObjectId,
    object: &SemanticObjectId,
    message: String,
) -> ConstraintViolation {
    let mut violation = ConstraintViolation::new(kind, message);
    violation
        .individuals
        .extend([subject.clone(), object.clone()]);
    violation.relations.insert(relation);
    violation
}

fn check_functional(
    relation: &RelationId,
    pairs: &BTreeSet<(SemanticObjectId, SemanticObjectId)>,
    inverse: bool,
    violations: &mut Vec<ConstraintViolation>,
) {
    let mut groups: BTreeMap<SemanticObjectId, BTreeSet<SemanticObjectId>> = BTreeMap::new();
    for (subject, object) in pairs {
        let (key, value) = if inverse {
            (object, subject)
        } else {
            (subject, object)
        };
        groups.entry(key.clone()).or_default().insert(value.clone());
    }
    for (key, values) in groups {
        if values.len() > 1 {
            let kind = if inverse {
                ViolationKind::InverseFunctionalRelation
            } else {
                ViolationKind::FunctionalRelation
            };
            let mut violation = ConstraintViolation::new(
                kind,
                format!(
                    "{relation} has {} conflicting values for {key}",
                    values.len()
                ),
            );
            violation.individuals.insert(key);
            violation.individuals.extend(values);
            violation.relations.insert(relation.clone());
            violations.push(violation);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_cardinality(
    kb: &KnowledgeBase,
    individual: &SemanticObjectId,
    subject_type: Option<&ConceptId>,
    relation: &RelationId,
    object_type: Option<&ConceptId>,
    minimum: Option<u32>,
    maximum: Option<u32>,
    world: WorldAssumption,
    violations: &mut Vec<ConstraintViolation>,
) {
    let mut values = kb.objects_of(individual, relation);
    if let Some(required) = object_type {
        values.retain(|value| kb.types_of(value).contains(required));
    }
    if let Some(minimum) = minimum {
        if world == WorldAssumption::Closed
            && values.len() < usize::try_from(minimum).unwrap_or(usize::MAX)
        {
            let mut violation = ConstraintViolation::new(
                ViolationKind::MinimumCardinality,
                format!(
                    "{individual} has {} {relation} values; minimum is {minimum}",
                    values.len()
                ),
            );
            violation.individuals.insert(individual.clone());
            if let Some(concept) = subject_type {
                violation.concepts.insert(concept.clone());
            }
            violation.relations.insert(relation.clone());
            violations.push(violation);
        }
    }
    if let Some(maximum) = maximum {
        if values.len() > usize::try_from(maximum).unwrap_or(usize::MAX) {
            let mut violation = ConstraintViolation::new(
                ViolationKind::MaximumCardinality,
                format!(
                    "{individual} has {} {relation} values; maximum is {maximum}",
                    values.len()
                ),
            );
            violation.individuals.insert(individual.clone());
            if let Some(concept) = subject_type {
                violation.concepts.insert(concept.clone());
            }
            violation.relations.insert(relation.clone());
            violations.push(violation);
        }
    }
}

fn relation_ancestor_closure(
    index: &OntologyIndex,
) -> Result<BTreeMap<RelationId, BTreeSet<RelationId>>, ReasoningError> {
    fn visit(
        relation: &RelationId,
        index: &OntologyIndex,
        visiting: &mut BTreeSet<RelationId>,
        memo: &mut BTreeMap<RelationId, BTreeSet<RelationId>>,
    ) -> Result<BTreeSet<RelationId>, ReasoningError> {
        if let Some(known) = memo.get(relation) {
            return Ok(known.clone());
        }
        if !visiting.insert(relation.clone()) {
            return Err(ReasoningError::RelationHierarchyCycle(relation.clone()));
        }
        let declaration = index
            .relations
            .get(relation)
            .ok_or_else(|| ReasoningError::UnknownRelation(relation.clone()))?;
        let mut result = BTreeSet::new();
        for parent in &declaration.super_relations {
            result.insert(parent.clone());
            result.extend(visit(parent, index, visiting, memo)?);
        }
        visiting.remove(relation);
        memo.insert(relation.clone(), result.clone());
        Ok(result)
    }

    let mut memo = BTreeMap::new();
    for relation in index.relations.keys() {
        visit(relation, index, &mut BTreeSet::new(), &mut memo)?;
    }
    Ok(memo)
}

type Substitution = BTreeMap<String, String>;

fn match_premises(
    premises: &[Atom],
    facts: &BTreeSet<Fact>,
    index: &OntologyIndex,
) -> Result<Vec<Substitution>, ReasoningError> {
    let mut substitutions = vec![Substitution::new()];
    for premise in premises {
        let mut next = Vec::new();
        for substitution in &substitutions {
            for candidate in candidate_bindings(premise, facts, index)? {
                if let Some(merged) = merge_substitutions(substitution, &candidate) {
                    next.push(merged);
                }
            }
        }
        substitutions = next;
        if substitutions.is_empty() {
            break;
        }
    }
    substitutions.sort();
    substitutions.dedup();
    Ok(substitutions)
}

fn candidate_bindings(
    atom: &Atom,
    facts: &BTreeSet<Fact>,
    index: &OntologyIndex,
) -> Result<Vec<Substitution>, ReasoningError> {
    if atom.predicate == "instance_of" {
        if atom.arguments.len() != 2 || atom.arguments[1].starts_with('?') {
            return Err(ReasoningError::InvalidRuleAtom(atom.clone()));
        }
        let concept = ConceptId::from(atom.arguments[1].as_str());
        if !index.concepts.contains_key(&concept) {
            return Err(ReasoningError::UnknownConcept(concept));
        }
        return Ok(facts
            .iter()
            .filter_map(|fact| match fact {
                Fact::InstanceOf {
                    individual,
                    concept: candidate,
                } if candidate == &concept => bind(&atom.arguments[0], individual.as_str()),
                _ => None,
            })
            .collect());
    }
    if atom.arguments.len() != 2 {
        return Err(ReasoningError::InvalidRuleAtom(atom.clone()));
    }
    let relation = RelationId::from(atom.predicate.as_str());
    if !index.relations.contains_key(&relation) {
        return Err(ReasoningError::UnknownRelation(relation));
    }
    Ok(facts
        .iter()
        .filter_map(|fact| match fact {
            Fact::Relation {
                subject,
                relation: candidate,
                object,
            } if candidate == &relation => {
                let mut substitution = Substitution::new();
                if !bind_argument(&mut substitution, &atom.arguments[0], subject.as_str()) {
                    return None;
                }
                if !bind_argument(&mut substitution, &atom.arguments[1], object.as_str()) {
                    return None;
                }
                Some(substitution)
            }
            _ => None,
        })
        .collect())
}

fn instantiate_atom(
    atom: &Atom,
    substitution: &Substitution,
    index: &OntologyIndex,
) -> Result<Option<Fact>, ReasoningError> {
    if atom.predicate == "instance_of" {
        if atom.arguments.len() != 2 || atom.arguments[1].starts_with('?') {
            return Err(ReasoningError::InvalidRuleAtom(atom.clone()));
        }
        let Some(individual) = value_of(&atom.arguments[0], substitution) else {
            return Ok(None);
        };
        let concept = ConceptId::from(atom.arguments[1].as_str());
        if !index.concepts.contains_key(&concept) {
            return Err(ReasoningError::UnknownConcept(concept));
        }
        return Ok(Some(Fact::InstanceOf {
            individual: SemanticObjectId::from(individual),
            concept,
        }));
    }
    if atom.arguments.len() != 2 {
        return Err(ReasoningError::InvalidRuleAtom(atom.clone()));
    }
    let relation = RelationId::from(atom.predicate.as_str());
    if !index.relations.contains_key(&relation) {
        return Err(ReasoningError::UnknownRelation(relation));
    }
    let Some(subject) = value_of(&atom.arguments[0], substitution) else {
        return Ok(None);
    };
    let Some(object) = value_of(&atom.arguments[1], substitution) else {
        return Ok(None);
    };
    Ok(Some(Fact::Relation {
        subject: SemanticObjectId::from(subject),
        relation,
        object: SemanticObjectId::from(object),
    }))
}

fn bind(argument: &str, value: &str) -> Option<Substitution> {
    let mut substitution = Substitution::new();
    bind_argument(&mut substitution, argument, value).then_some(substitution)
}

fn bind_argument(substitution: &mut Substitution, argument: &str, value: &str) -> bool {
    if argument.starts_with('?') {
        if let Some(existing) = substitution.get(argument) {
            existing == value
        } else {
            substitution.insert(argument.to_owned(), value.to_owned());
            true
        }
    } else {
        argument == value
    }
}

fn value_of(argument: &str, substitution: &Substitution) -> Option<String> {
    if argument.starts_with('?') {
        substitution.get(argument).cloned()
    } else {
        Some(argument.to_owned())
    }
}

fn merge_substitutions(left: &Substitution, right: &Substitution) -> Option<Substitution> {
    let mut merged = left.clone();
    for (key, value) in right {
        if let Some(existing) = merged.get(key) {
            if existing != value {
                return None;
            }
        } else {
            merged.insert(key.clone(), value.clone());
        }
    }
    Some(merged)
}

#[derive(Debug, Error)]
pub enum ReasoningError {
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error("unknown concept {0}")]
    UnknownConcept(ConceptId),
    #[error("unknown relation {0}")]
    UnknownRelation(RelationId),
    #[error("relation hierarchy cycle includes {0}")]
    RelationHierarchyCycle(RelationId),
    #[error("invalid rule atom {0:?}")]
    InvalidRuleAtom(Atom),
    #[error("reasoning did not reach a fixed point within {0} iterations")]
    IterationLimit(usize),
}
