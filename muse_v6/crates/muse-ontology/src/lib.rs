//! Ontology declarations, axioms, taxonomic indexing, and classification.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use muse_core::{
    ConceptId, EvidenceId, HeaderError, IdentityError, LanguageTag, PackageHeader, PackageKind,
    PackageRef, RelationId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityMode {
    Individual,
    Type,
    Either,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rigidity {
    Rigid,
    AntiRigid,
    SemiRigid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sortality {
    Sortal,
    NonSortal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityProfile {
    Supplies,
    Carries,
    None,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Abstractness {
    Abstract,
    Concrete,
    Mixed,
    Unspecified,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConceptProfile {
    pub entity_mode: Option<EntityMode>,
    pub rigidity: Option<Rigidity>,
    pub sortality: Option<Sortality>,
    pub identity: Option<IdentityProfile>,
    pub abstractness: Option<Abstractness>,
    pub metaproperties: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionRecord {
    pub text: String,
    pub genus: Option<ConceptId>,
    pub differentia: Vec<String>,
    pub necessary_conditions: Vec<String>,
    pub sufficient_conditions: Vec<String>,
    pub scope: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConceptDeclaration {
    pub id: ConceptId,
    pub preferred_labels: BTreeMap<LanguageTag, String>,
    pub alternate_labels: BTreeMap<LanguageTag, BTreeSet<String>>,
    pub definition: DefinitionRecord,
    pub parents: BTreeSet<ConceptId>,
    pub disjoint_with: BTreeSet<ConceptId>,
    pub profile: ConceptProfile,
    pub deprecated: bool,
    pub evidence: BTreeSet<EvidenceId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationCharacteristic {
    Reflexive,
    Irreflexive,
    Symmetric,
    Asymmetric,
    Antisymmetric,
    Transitive,
    Functional,
    InverseFunctional,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardinalityConstraint {
    pub minimum: Option<u32>,
    pub maximum: Option<u32>,
}

impl CardinalityConstraint {
    pub fn validate(&self) -> Result<(), OntologyValidationError> {
        if let (Some(minimum), Some(maximum)) = (self.minimum, self.maximum) {
            if minimum > maximum {
                return Err(OntologyValidationError::InvalidCardinality { minimum, maximum });
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationDeclaration {
    pub id: RelationId,
    pub preferred_labels: BTreeMap<LanguageTag, String>,
    pub definition: String,
    pub domains: BTreeSet<ConceptId>,
    pub ranges: BTreeSet<ConceptId>,
    pub super_relations: BTreeSet<RelationId>,
    pub inverse: Option<RelationId>,
    pub characteristics: BTreeSet<RelationCharacteristic>,
    pub cardinality: Option<CardinalityConstraint>,
    pub deprecated: bool,
    pub evidence: BTreeSet<EvidenceId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Atom {
    pub predicate: String,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleAxiom {
    pub variables: BTreeSet<String>,
    pub premises: Vec<Atom>,
    pub conclusion: Atom,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Axiom {
    Subsumption {
        child: ConceptId,
        parent: ConceptId,
    },
    Disjoint {
        members: BTreeSet<ConceptId>,
    },
    CompletePartition {
        parent: ConceptId,
        members: BTreeSet<ConceptId>,
    },
    Equivalent {
        members: BTreeSet<ConceptId>,
    },
    RelationSubsumption {
        child: RelationId,
        parent: RelationId,
    },
    Domain {
        relation: RelationId,
        concept: ConceptId,
    },
    Range {
        relation: RelationId,
        concept: ConceptId,
    },
    ExistentialRestriction {
        subject: ConceptId,
        relation: RelationId,
        object: ConceptId,
    },
    UniversalRestriction {
        subject: ConceptId,
        relation: RelationId,
        object: ConceptId,
    },
    Cardinality {
        subject: ConceptId,
        relation: RelationId,
        object: Option<ConceptId>,
        constraint: CardinalityConstraint,
    },
    Rule {
        rule: RuleAxiom,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OntologyPackage {
    pub header: PackageHeader,
    pub concepts: BTreeMap<ConceptId, ConceptDeclaration>,
    pub relations: BTreeMap<RelationId, RelationDeclaration>,
    pub axioms: Vec<Axiom>,
}

impl OntologyPackage {
    pub fn validate(&self) -> Result<(), OntologyValidationError> {
        self.header.validate()?;
        if self.header.kind != PackageKind::Ontology {
            return Err(OntologyValidationError::WrongPackageKind);
        }

        for (key, concept) in &self.concepts {
            if key != &concept.id {
                return Err(OntologyValidationError::ConceptIdentityMismatch {
                    key: key.clone(),
                    declared: concept.id.clone(),
                });
            }
            key.validate()?;
            if concept.preferred_labels.is_empty() {
                return Err(OntologyValidationError::MissingPreferredLabel(key.clone()));
            }
            validate_text("concept definition", &concept.definition.text)?;
            if concept.parents.contains(key) {
                return Err(OntologyValidationError::SelfParent(key.clone()));
            }
            if concept.disjoint_with.contains(key) {
                return Err(OntologyValidationError::SelfDisjoint(key.clone()));
            }
            if let Some(genus) = &concept.definition.genus {
                if !self.concepts.contains_key(genus) && !concept.parents.contains(genus) {
                    // Cross-package genera are allowed and checked by the registry index.
                    genus.validate()?;
                }
            }
        }

        detect_local_parent_cycle(&self.concepts)?;

        for (key, relation) in &self.relations {
            if key != &relation.id {
                return Err(OntologyValidationError::RelationIdentityMismatch {
                    key: key.clone(),
                    declared: relation.id.clone(),
                });
            }
            key.validate()?;
            if relation.preferred_labels.is_empty() {
                return Err(OntologyValidationError::MissingRelationLabel(key.clone()));
            }
            validate_text("relation definition", &relation.definition)?;
            if relation.super_relations.contains(key) {
                return Err(OntologyValidationError::SelfSuperRelation(key.clone()));
            }
            if relation.inverse.as_ref() == Some(key)
                && !relation
                    .characteristics
                    .contains(&RelationCharacteristic::Symmetric)
            {
                return Err(OntologyValidationError::NonSymmetricSelfInverse(
                    key.clone(),
                ));
            }
            if relation
                .characteristics
                .contains(&RelationCharacteristic::Reflexive)
                && relation
                    .characteristics
                    .contains(&RelationCharacteristic::Irreflexive)
            {
                return Err(
                    OntologyValidationError::ContradictoryRelationCharacteristics(key.clone()),
                );
            }
            if relation
                .characteristics
                .contains(&RelationCharacteristic::Symmetric)
                && relation
                    .characteristics
                    .contains(&RelationCharacteristic::Asymmetric)
            {
                return Err(
                    OntologyValidationError::ContradictoryRelationCharacteristics(key.clone()),
                );
            }
            if let Some(cardinality) = &relation.cardinality {
                cardinality.validate()?;
            }
        }

        for axiom in &self.axioms {
            validate_axiom_shape(axiom)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn package_ref(&self) -> &PackageRef {
        &self.header.package
    }
}

fn validate_text(kind: &'static str, value: &str) -> Result<(), IdentityError> {
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

fn validate_axiom_shape(axiom: &Axiom) -> Result<(), OntologyValidationError> {
    match axiom {
        Axiom::Disjoint { members } | Axiom::Equivalent { members } if members.len() < 2 => {
            Err(OntologyValidationError::AxiomNeedsTwoMembers)
        }
        Axiom::CompletePartition { members, .. } if members.len() < 2 => {
            Err(OntologyValidationError::AxiomNeedsTwoMembers)
        }
        Axiom::Cardinality { constraint, .. } => constraint.validate(),
        Axiom::Rule { rule } if rule.premises.is_empty() => {
            Err(OntologyValidationError::RuleHasNoPremises)
        }
        _ => Ok(()),
    }
}

fn detect_local_parent_cycle(
    concepts: &BTreeMap<ConceptId, ConceptDeclaration>,
) -> Result<(), OntologyValidationError> {
    fn visit(
        current: &ConceptId,
        concepts: &BTreeMap<ConceptId, ConceptDeclaration>,
        visiting: &mut BTreeSet<ConceptId>,
        visited: &mut BTreeSet<ConceptId>,
    ) -> Result<(), OntologyValidationError> {
        if visited.contains(current) {
            return Ok(());
        }
        if !visiting.insert(current.clone()) {
            return Err(OntologyValidationError::ParentCycle(current.clone()));
        }
        if let Some(concept) = concepts.get(current) {
            for parent in &concept.parents {
                if concepts.contains_key(parent) {
                    visit(parent, concepts, visiting, visited)?;
                }
            }
        }
        visiting.remove(current);
        visited.insert(current.clone());
        Ok(())
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for concept in concepts.keys() {
        visit(concept, concepts, &mut visiting, &mut visited)?;
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum OntologyValidationError {
    #[error(transparent)]
    Header(#[from] HeaderError),
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error("ontology package header has a non-ontology kind")]
    WrongPackageKind,
    #[error("concept map key {key} contains declaration {declared}")]
    ConceptIdentityMismatch { key: ConceptId, declared: ConceptId },
    #[error("relation map key {key} contains declaration {declared}")]
    RelationIdentityMismatch {
        key: RelationId,
        declared: RelationId,
    },
    #[error("concept {0} has no preferred label")]
    MissingPreferredLabel(ConceptId),
    #[error("relation {0} has no preferred label")]
    MissingRelationLabel(RelationId),
    #[error("concept {0} lists itself as a parent")]
    SelfParent(ConceptId),
    #[error("concept {0} is disjoint with itself")]
    SelfDisjoint(ConceptId),
    #[error("relation {0} lists itself as a super-relation")]
    SelfSuperRelation(RelationId),
    #[error("relation {0} lists itself as inverse without being symmetric")]
    NonSymmetricSelfInverse(RelationId),
    #[error("relation {0} declares contradictory characteristics")]
    ContradictoryRelationCharacteristics(RelationId),
    #[error("concept parent cycle contains {0}")]
    ParentCycle(ConceptId),
    #[error("invalid cardinality: minimum {minimum} exceeds maximum {maximum}")]
    InvalidCardinality { minimum: u32, maximum: u32 },
    #[error("set-valued axiom requires at least two members")]
    AxiomNeedsTwoMembers,
    #[error("rule axiom has no premises")]
    RuleHasNoPremises,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OntologyIndex {
    pub packages: BTreeSet<PackageRef>,
    pub concepts: BTreeMap<ConceptId, ConceptDeclaration>,
    pub relations: BTreeMap<RelationId, RelationDeclaration>,
    /// Namespaceless learned-label symbol -> authoritative qualified concept ID.
    pub concept_symbols: BTreeMap<ConceptId, ConceptId>,
    /// Namespaceless learned-label symbol -> authoritative qualified relation ID.
    pub relation_symbols: BTreeMap<RelationId, RelationId>,
    pub ancestors: BTreeMap<ConceptId, BTreeSet<ConceptId>>,
    pub descendants: BTreeMap<ConceptId, BTreeSet<ConceptId>>,
    pub disjoint_pairs: BTreeSet<(ConceptId, ConceptId)>,
    pub axioms: Vec<Axiom>,
}

/// Canonical namespaceless concept symbol used in learned labels.
///
/// Almost every concept simply drops its source namespace. The small collision
/// set is deliberately renamed by meaning so the model never has to predict a
/// package namespace merely to identify an ontology concept.
#[must_use]
pub fn canonical_concept_symbol(qualified: &ConceptId) -> ConceptId {
    let value = qualified.as_str();
    ConceptId::from(match value {
        "ufo:Delegation" => "DelegationRelation",
        "agent:Delegation" => "DelegationAction",
        "mlt:Individual" => "MLTIndividual",
        "ufo:Individual" => "Individual",
        "mlt:Type" => "MLTType",
        "ufo:Type" => "Type",
        _ => value.split_once(':').map_or(value, |(_, local)| local),
    })
}

/// Canonical namespaceless relation symbol used in learned labels.
#[must_use]
pub fn canonical_relation_symbol(qualified: &RelationId) -> RelationId {
    let value = qualified.as_str();
    RelationId::from(match value {
        "mlt:instantiates" => "mltInstantiates",
        "ufo:instantiates" => "instantiates",
        "mlt:specializes" => "mltSpecializes",
        "ufo:specializes" => "specializes",
        "service:fulfillsCommitment" => "serviceDeliveryFulfillsCommitment",
        "ufo:fulfillsCommitment" => "fulfillsCommitment",
        "service:violatesCommitment" => "serviceDeliveryViolatesCommitment",
        "ufo:violatesCommitment" => "violatesCommitment",
        _ => value.split_once(':').map_or(value, |(_, local)| local),
    })
}

impl OntologyIndex {
    pub fn build<'a>(
        packages: impl IntoIterator<Item = &'a OntologyPackage>,
    ) -> Result<Self, OntologyIndexError> {
        let packages: Vec<&OntologyPackage> = packages.into_iter().collect();
        let mut index = Self::default();

        for package in &packages {
            package.validate()?;
            index.packages.insert(package.header.package.clone());
            for (id, declaration) in &package.concepts {
                if index
                    .concepts
                    .insert(id.clone(), declaration.clone())
                    .is_some()
                {
                    return Err(OntologyIndexError::DuplicateConcept(id.clone()));
                }
            }
            for (id, declaration) in &package.relations {
                if index
                    .relations
                    .insert(id.clone(), declaration.clone())
                    .is_some()
                {
                    return Err(OntologyIndexError::DuplicateRelation(id.clone()));
                }
            }
        }

        index.build_canonical_symbols()?;

        for concept in index.concepts.values() {
            for parent in &concept.parents {
                if !index.concepts.contains_key(parent) {
                    return Err(OntologyIndexError::UnknownParent {
                        concept: concept.id.clone(),
                        parent: parent.clone(),
                    });
                }
            }
            if let Some(genus) = &concept.definition.genus {
                if !index.concepts.contains_key(genus) {
                    return Err(OntologyIndexError::UnknownGenus {
                        concept: concept.id.clone(),
                        genus: genus.clone(),
                    });
                }
            }
            for other in &concept.disjoint_with {
                if !index.concepts.contains_key(other) {
                    return Err(OntologyIndexError::UnknownDisjointConcept {
                        concept: concept.id.clone(),
                        other: other.clone(),
                    });
                }
                index
                    .disjoint_pairs
                    .insert(ordered_pair(&concept.id, other));
            }
        }

        for package in &packages {
            for axiom in &package.axioms {
                index.add_axiom(axiom)?;
            }
        }

        for relation in index.relations.values() {
            for domain in &relation.domains {
                if !index.concepts.contains_key(domain) {
                    return Err(OntologyIndexError::UnknownRelationDomain {
                        relation: relation.id.clone(),
                        concept: domain.clone(),
                    });
                }
            }
            for range in &relation.ranges {
                if !index.concepts.contains_key(range) {
                    return Err(OntologyIndexError::UnknownRelationRange {
                        relation: relation.id.clone(),
                        concept: range.clone(),
                    });
                }
            }
            if let Some(inverse) = &relation.inverse {
                let Some(inverse_declaration) = index.relations.get(inverse) else {
                    return Err(OntologyIndexError::UnknownInverseRelation {
                        relation: relation.id.clone(),
                        inverse: inverse.clone(),
                    });
                };
                if inverse_declaration.inverse.as_ref() != Some(&relation.id) {
                    return Err(OntologyIndexError::NonReciprocalInverse {
                        relation: relation.id.clone(),
                        inverse: inverse.clone(),
                    });
                }
            }
            for parent in &relation.super_relations {
                if !index.relations.contains_key(parent) {
                    return Err(OntologyIndexError::UnknownSuperRelation {
                        relation: relation.id.clone(),
                        parent: parent.clone(),
                    });
                }
            }
        }

        index.compute_closure()?;
        Ok(index)
    }

    fn build_canonical_symbols(&mut self) -> Result<(), OntologyIndexError> {
        for qualified in self.concepts.keys() {
            let symbol = canonical_concept_symbol(qualified);
            if symbol.as_str().contains(':') {
                return Err(OntologyIndexError::QualifiedCanonicalConceptSymbol(symbol));
            }
            if let Some(previous) = self
                .concept_symbols
                .insert(symbol.clone(), qualified.clone())
            {
                if previous != *qualified {
                    return Err(OntologyIndexError::CanonicalConceptCollision {
                        symbol,
                        left: previous,
                        right: qualified.clone(),
                    });
                }
            }
        }
        for qualified in self.relations.keys() {
            let symbol = canonical_relation_symbol(qualified);
            if symbol.as_str().contains(':') {
                return Err(OntologyIndexError::QualifiedCanonicalRelationSymbol(symbol));
            }
            if let Some(previous) = self
                .relation_symbols
                .insert(symbol.clone(), qualified.clone())
            {
                if previous != *qualified {
                    return Err(OntologyIndexError::CanonicalRelationCollision {
                        symbol,
                        left: previous,
                        right: qualified.clone(),
                    });
                }
            }
        }
        for (symbol, concept) in &self.concept_symbols {
            if let Some(relation) = self
                .relation_symbols
                .get(&RelationId::from(symbol.as_str()))
            {
                return Err(OntologyIndexError::CanonicalOntologySymbolCollision {
                    symbol: symbol.as_str().to_owned(),
                    concept: concept.clone(),
                    relation: relation.clone(),
                });
            }
        }
        Ok(())
    }

    /// Resolve a learned namespaceless concept symbol or an internal qualified ID.
    #[must_use]
    pub fn resolve_concept(&self, id: &ConceptId) -> Option<ConceptId> {
        if self.concepts.contains_key(id) {
            Some(id.clone())
        } else {
            self.concept_symbols.get(id).cloned()
        }
    }

    /// Resolve a learned namespaceless relation symbol or an internal qualified ID.
    #[must_use]
    pub fn resolve_relation(&self, id: &RelationId) -> Option<RelationId> {
        if self.relations.contains_key(id) {
            Some(id.clone())
        } else {
            self.relation_symbols.get(id).cloned()
        }
    }

    fn add_axiom(&mut self, axiom: &Axiom) -> Result<(), OntologyIndexError> {
        match axiom {
            Axiom::Subsumption { child, parent } => {
                self.require_concept(child)?;
                self.require_concept(parent)?;
                self.concepts
                    .get_mut(child)
                    .expect("required concept exists")
                    .parents
                    .insert(parent.clone());
            }
            Axiom::Disjoint { members } => {
                for member in members {
                    self.require_concept(member)?;
                }
                for left in members {
                    for right in members {
                        if left < right {
                            self.disjoint_pairs.insert((left.clone(), right.clone()));
                        }
                    }
                }
            }
            Axiom::CompletePartition { parent, members } => {
                self.require_concept(parent)?;
                for member in members {
                    self.require_concept(member)?;
                    self.concepts
                        .get_mut(member)
                        .expect("required concept exists")
                        .parents
                        .insert(parent.clone());
                }
                for left in members {
                    for right in members {
                        if left < right {
                            self.disjoint_pairs.insert((left.clone(), right.clone()));
                        }
                    }
                }
            }
            Axiom::RelationSubsumption { child, parent } => {
                self.require_relation(child)?;
                self.require_relation(parent)?;
                self.relations
                    .get_mut(child)
                    .expect("required relation exists")
                    .super_relations
                    .insert(parent.clone());
            }
            Axiom::Domain { relation, concept } => {
                self.require_relation(relation)?;
                self.require_concept(concept)?;
                self.relations
                    .get_mut(relation)
                    .expect("required relation exists")
                    .domains
                    .insert(concept.clone());
            }
            Axiom::Range { relation, concept } => {
                self.require_relation(relation)?;
                self.require_concept(concept)?;
                self.relations
                    .get_mut(relation)
                    .expect("required relation exists")
                    .ranges
                    .insert(concept.clone());
            }
            Axiom::ExistentialRestriction {
                subject,
                relation,
                object,
            }
            | Axiom::UniversalRestriction {
                subject,
                relation,
                object,
            } => {
                self.require_concept(subject)?;
                self.require_relation(relation)?;
                self.require_concept(object)?;
            }
            Axiom::Cardinality {
                subject,
                relation,
                object,
                constraint,
            } => {
                self.require_concept(subject)?;
                self.require_relation(relation)?;
                if let Some(object) = object {
                    self.require_concept(object)?;
                }
                constraint.validate()?;
            }
            Axiom::Equivalent { members } => {
                for member in members {
                    self.require_concept(member)?;
                }
            }
            Axiom::Rule { rule } => {
                let mut used_variables = BTreeSet::new();
                for atom in rule
                    .premises
                    .iter()
                    .chain(std::iter::once(&rule.conclusion))
                {
                    for argument in &atom.arguments {
                        if argument.starts_with('?') {
                            used_variables.insert(argument.clone());
                        }
                    }
                    if atom.predicate == "instance_of" {
                        if atom.arguments.len() != 2 || atom.arguments[1].starts_with('?') {
                            return Err(OntologyIndexError::InvalidRuleAtom(atom.clone()));
                        }
                        self.require_concept(&ConceptId::from(atom.arguments[1].as_str()))?;
                    } else {
                        if atom.arguments.len() != 2 {
                            return Err(OntologyIndexError::InvalidRuleAtom(atom.clone()));
                        }
                        self.require_relation(&RelationId::from(atom.predicate.as_str()))?;
                    }
                }
                if used_variables != rule.variables {
                    return Err(OntologyIndexError::RuleVariableMismatch {
                        declared: rule.variables.clone(),
                        used: used_variables,
                    });
                }
            }
        }
        self.axioms.push(axiom.clone());
        Ok(())
    }

    fn require_concept(&self, concept: &ConceptId) -> Result<(), OntologyIndexError> {
        if self.concepts.contains_key(concept) {
            Ok(())
        } else {
            Err(OntologyIndexError::UnknownConcept(concept.clone()))
        }
    }

    fn require_relation(&self, relation: &RelationId) -> Result<(), OntologyIndexError> {
        if self.relations.contains_key(relation) {
            Ok(())
        } else {
            Err(OntologyIndexError::UnknownRelation(relation.clone()))
        }
    }

    fn compute_closure(&mut self) -> Result<(), OntologyIndexError> {
        fn ancestors_for(
            current: &ConceptId,
            concepts: &BTreeMap<ConceptId, ConceptDeclaration>,
            visiting: &mut BTreeSet<ConceptId>,
            memo: &mut BTreeMap<ConceptId, BTreeSet<ConceptId>>,
        ) -> Result<BTreeSet<ConceptId>, OntologyIndexError> {
            if let Some(known) = memo.get(current) {
                return Ok(known.clone());
            }
            if !visiting.insert(current.clone()) {
                return Err(OntologyIndexError::ParentCycle(current.clone()));
            }
            let mut result = BTreeSet::new();
            if let Some(concept) = concepts.get(current) {
                for parent in &concept.parents {
                    result.insert(parent.clone());
                    result.extend(ancestors_for(parent, concepts, visiting, memo)?);
                }
            }
            visiting.remove(current);
            memo.insert(current.clone(), result.clone());
            Ok(result)
        }

        let mut memo = BTreeMap::new();
        for concept in self.concepts.keys() {
            let closure = ancestors_for(concept, &self.concepts, &mut BTreeSet::new(), &mut memo)?;
            self.ancestors.insert(concept.clone(), closure);
        }
        for (child, ancestors) in &self.ancestors {
            for ancestor in ancestors {
                self.descendants
                    .entry(ancestor.clone())
                    .or_default()
                    .insert(child.clone());
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn is_subtype(&self, child: &ConceptId, parent: &ConceptId) -> bool {
        child == parent
            || self
                .ancestors
                .get(child)
                .is_some_and(|ancestors| ancestors.contains(parent))
    }

    #[must_use]
    pub fn are_disjoint(&self, left: &ConceptId, right: &ConceptId) -> bool {
        if left == right {
            return false;
        }
        let left_lineage = self.lineage(left);
        let right_lineage = self.lineage(right);
        left_lineage.iter().any(|left_member| {
            right_lineage.iter().any(|right_member| {
                self.disjoint_pairs
                    .contains(&ordered_pair(left_member, right_member))
            })
        })
    }

    #[must_use]
    pub fn lineage(&self, concept: &ConceptId) -> BTreeSet<ConceptId> {
        let mut result = BTreeSet::from([concept.clone()]);
        if let Some(ancestors) = self.ancestors.get(concept) {
            result.extend(ancestors.iter().cloned());
        }
        result
    }

    #[must_use]
    pub fn common_ancestors(&self, concepts: &BTreeSet<ConceptId>) -> BTreeSet<ConceptId> {
        let mut iterator = concepts.iter();
        let Some(first) = iterator.next() else {
            return BTreeSet::new();
        };
        let mut common = self.lineage(first);
        for concept in iterator {
            common = common
                .intersection(&self.lineage(concept))
                .cloned()
                .collect();
        }
        common
    }

    #[must_use]
    pub fn most_specific(&self, concepts: &BTreeSet<ConceptId>) -> BTreeSet<ConceptId> {
        concepts
            .iter()
            .filter(|candidate| {
                !concepts
                    .iter()
                    .any(|other| *candidate != other && self.is_subtype(other, candidate))
            })
            .cloned()
            .collect()
    }
}

fn ordered_pair(left: &ConceptId, right: &ConceptId) -> (ConceptId, ConceptId) {
    if left <= right {
        (left.clone(), right.clone())
    } else {
        (right.clone(), left.clone())
    }
}

#[derive(Debug, Error)]
pub enum OntologyIndexError {
    #[error(transparent)]
    Validation(#[from] OntologyValidationError),
    #[error("duplicate concept declaration: {0}")]
    DuplicateConcept(ConceptId),
    #[error("duplicate relation declaration: {0}")]
    DuplicateRelation(RelationId),
    #[error(
        "canonical concept symbol {symbol} collides between {left} and {right}; assign an explicit semantic rename"
    )]
    CanonicalConceptCollision {
        symbol: ConceptId,
        left: ConceptId,
        right: ConceptId,
    },
    #[error(
        "canonical relation symbol {symbol} collides between {left} and {right}; assign an explicit semantic rename"
    )]
    CanonicalRelationCollision {
        symbol: RelationId,
        left: RelationId,
        right: RelationId,
    },
    #[error(
        "canonical ontology symbol {symbol} is shared by concept {concept} and relation {relation}; assign an explicit semantic rename"
    )]
    CanonicalOntologySymbolCollision {
        symbol: String,
        concept: ConceptId,
        relation: RelationId,
    },
    #[error("canonical concept symbol must be namespaceless: {0}")]
    QualifiedCanonicalConceptSymbol(ConceptId),
    #[error("canonical relation symbol must be namespaceless: {0}")]
    QualifiedCanonicalRelationSymbol(RelationId),
    #[error("unknown concept: {0}")]
    UnknownConcept(ConceptId),
    #[error("unknown relation: {0}")]
    UnknownRelation(RelationId),
    #[error("concept {concept} references unknown parent {parent}")]
    UnknownParent {
        concept: ConceptId,
        parent: ConceptId,
    },
    #[error("concept {concept} references unknown genus {genus}")]
    UnknownGenus {
        concept: ConceptId,
        genus: ConceptId,
    },
    #[error("concept {concept} references unknown disjoint concept {other}")]
    UnknownDisjointConcept {
        concept: ConceptId,
        other: ConceptId,
    },
    #[error("relation {relation} references unknown domain {concept}")]
    UnknownRelationDomain {
        relation: RelationId,
        concept: ConceptId,
    },
    #[error("relation {relation} references unknown range {concept}")]
    UnknownRelationRange {
        relation: RelationId,
        concept: ConceptId,
    },
    #[error("relation {relation} references unknown inverse relation {inverse}")]
    UnknownInverseRelation {
        relation: RelationId,
        inverse: RelationId,
    },
    #[error(
        "relation {relation} names {inverse} as inverse, but the declaration is not reciprocal"
    )]
    NonReciprocalInverse {
        relation: RelationId,
        inverse: RelationId,
    },
    #[error("relation {relation} references unknown super-relation {parent}")]
    UnknownSuperRelation {
        relation: RelationId,
        parent: RelationId,
    },
    #[error("parent cycle contains {0}")]
    ParentCycle(ConceptId),
    #[error("invalid rule atom {0:?}")]
    InvalidRuleAtom(Atom),
    #[error("rule variables differ: declared {declared:?}, used {used:?}")]
    RuleVariableMismatch {
        declared: BTreeSet<String>,
        used: BTreeSet<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassificationStatus {
    Exact,
    LeastSpecificGuaranteed,
    Ambiguous,
    Contradictory,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassificationResult {
    pub status: ClassificationStatus,
    pub guaranteed_types: BTreeSet<ConceptId>,
    pub possible_refinements: BTreeSet<ConceptId>,
    pub excluded_types: BTreeSet<ConceptId>,
    pub evidence: BTreeSet<EvidenceId>,
    pub notes: Vec<String>,
}

impl ClassificationResult {
    #[must_use]
    pub fn unknown() -> Self {
        Self {
            status: ClassificationStatus::Unknown,
            guaranteed_types: BTreeSet::new(),
            possible_refinements: BTreeSet::new(),
            excluded_types: BTreeSet::new(),
            evidence: BTreeSet::new(),
            notes: Vec::new(),
        }
    }

    #[must_use]
    pub fn normalize(mut self, index: &OntologyIndex) -> Self {
        let original = self.guaranteed_types.clone();
        for concept in original {
            self.guaranteed_types.extend(index.lineage(&concept));
        }
        self.possible_refinements
            .retain(|candidate| !self.guaranteed_types.contains(candidate));

        let contradictory = self.guaranteed_types.iter().any(|left| {
            self.guaranteed_types
                .iter()
                .any(|right| left < right && index.are_disjoint(left, right))
        }) || self.guaranteed_types.iter().any(|guaranteed| {
            self.excluded_types
                .iter()
                .any(|excluded| index.is_subtype(guaranteed, excluded))
        });

        if contradictory {
            self.status = ClassificationStatus::Contradictory;
        } else if self.status != ClassificationStatus::Ambiguous
            && self.guaranteed_types.is_empty()
            && self.possible_refinements.is_empty()
        {
            self.status = ClassificationStatus::Unknown;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use muse_core::{ContentDigest, DigestAlgorithm, PackageId, PackageRef, PackageVersion};

    fn concept(id: &str, parents: &[&str]) -> ConceptDeclaration {
        ConceptDeclaration {
            id: ConceptId::from(id),
            preferred_labels: BTreeMap::from([(LanguageTag::from("en"), id.to_owned())]),
            alternate_labels: BTreeMap::new(),
            definition: DefinitionRecord {
                text: format!("Definition of {id}"),
                genus: None,
                differentia: Vec::new(),
                necessary_conditions: Vec::new(),
                sufficient_conditions: Vec::new(),
                scope: None,
                notes: Vec::new(),
            },
            parents: parents
                .iter()
                .map(|parent| ConceptId::from(*parent))
                .collect(),
            disjoint_with: BTreeSet::new(),
            profile: ConceptProfile::default(),
            deprecated: false,
            evidence: BTreeSet::new(),
        }
    }

    fn package(concepts: BTreeMap<ConceptId, ConceptDeclaration>) -> OntologyPackage {
        OntologyPackage {
            header: PackageHeader {
                package: PackageRef {
                    id: PackageId::from("muse.test.ontology"),
                    version: PackageVersion::from("0.1.0"),
                    digest: ContentDigest {
                        algorithm: DigestAlgorithm::Sha256,
                        value: "a".repeat(64),
                    },
                },
                kind: PackageKind::Ontology,
                title: "Test ontology".to_owned(),
                description: "Test ontology package".to_owned(),
                license: "CC0-1.0".to_owned(),
                imports: Vec::new(),
                evidence: Vec::new(),
            },
            concepts,
            relations: BTreeMap::new(),
            axioms: Vec::new(),
        }
    }

    #[test]
    fn canonical_symbols_drop_namespaces_and_semantically_rename_collisions() {
        assert_eq!(
            canonical_concept_symbol(&ConceptId::from("ufo:Agent")),
            ConceptId::from("Agent")
        );
        assert_eq!(
            canonical_concept_symbol(&ConceptId::from("ufo:Delegation")),
            ConceptId::from("DelegationRelation")
        );
        assert_eq!(
            canonical_concept_symbol(&ConceptId::from("agent:Delegation")),
            ConceptId::from("DelegationAction")
        );
        assert_eq!(
            canonical_relation_symbol(&RelationId::from("ufo:instantiates")),
            RelationId::from("instantiates")
        );
        assert_eq!(
            canonical_relation_symbol(&RelationId::from("mlt:instantiates")),
            RelationId::from("mltInstantiates")
        );
    }

    #[test]
    fn unresolved_namespaceless_collision_is_rejected() {
        let package = package(BTreeMap::from([
            (ConceptId::from("left:Foo"), concept("left:Foo", &[])),
            (ConceptId::from("right:Foo"), concept("right:Foo", &[])),
        ]));
        assert!(matches!(
            OntologyIndex::build([&package]),
            Err(OntologyIndexError::CanonicalConceptCollision { .. })
        ));
    }

    #[test]
    fn computes_transitive_ancestors() {
        let package = package(BTreeMap::from([
            (ConceptId::from("Entity"), concept("Entity", &[])),
            (
                ConceptId::from("Individual"),
                concept("Individual", &["Entity"]),
            ),
            (
                ConceptId::from("Endurant"),
                concept("Endurant", &["Individual"]),
            ),
        ]));
        let index = OntologyIndex::build([&package]).unwrap();
        assert!(index.is_subtype(&ConceptId::from("Endurant"), &ConceptId::from("Entity")));
    }
}
