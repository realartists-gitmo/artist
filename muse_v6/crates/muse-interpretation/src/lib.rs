//! Total semantic interpretation over content-pinned ontology and lexicon snapshots.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use muse_classification::{
    ClassificationPolicy, ClassificationRequest, OntologyClassifier, RegistryClassifier,
};
use muse_core::{
    ConceptId, EdgeId, EvidenceId, IdentityError, InterpretationId, LanguageTag, LexicalSenseId,
    RelationId, SemanticObjectId,
};
use muse_lexicon::SemanticTarget;
use muse_ontology::{ClassificationResult, ClassificationStatus, OntologyIndex};
use muse_registry::{PackageRegistry, RegistryError, RegistrySnapshot};
use muse_resolution::{ReferenceMode, ResolutionResult, SenseCandidate};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DenotationHint {
    Literal { datatype: String, lexical: String },
    Quoted { text: String },
    FormalSymbol { id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretationUnit {
    pub id: SemanticObjectId,
    pub surface: String,
    pub language: LanguageTag,
    pub resolution: ResolutionResult,
    pub asserted_types: BTreeSet<ConceptId>,
    pub possible_refinements: BTreeSet<ConceptId>,
    pub excluded_types: BTreeSet<ConceptId>,
    pub hint: Option<DenotationHint>,
    pub evidence: BTreeSet<EvidenceId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationApplication {
    pub id: EdgeId,
    pub relation: RelationId,
    pub subject: SemanticObjectId,
    pub object: SemanticObjectId,
    pub qualifiers: BTreeMap<String, String>,
    pub evidence: BTreeSet<EvidenceId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretationRequest {
    pub id: InterpretationId,
    pub snapshot: RegistrySnapshot,
    pub roots: BTreeMap<String, SemanticObjectId>,
    pub units: Vec<InterpretationUnit>,
    pub relations: Vec<RelationApplication>,
    pub context: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SemanticDenotation {
    Entity {
        concept: ConceptId,
    },
    Relation {
        relation: RelationId,
    },
    FormalSymbol {
        id: String,
    },
    Literal {
        datatype: String,
        lexical: String,
    },
    Mention {
        text: String,
        senses: BTreeSet<LexicalSenseId>,
    },
    Ambiguous {
        candidates: Vec<SemanticTarget>,
    },
    Unresolved {
        surface: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticNode {
    pub id: SemanticObjectId,
    pub surface: String,
    pub language: LanguageTag,
    pub source_senses: BTreeSet<LexicalSenseId>,
    pub classification: ClassificationResult,
    pub denotation: SemanticDenotation,
    pub evidence: BTreeSet<EvidenceId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticEdge {
    pub id: EdgeId,
    pub relation: RelationId,
    pub subject: SemanticObjectId,
    pub object: SemanticObjectId,
    pub qualifiers: BTreeMap<String, String>,
    pub evidence: BTreeSet<EvidenceId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterpretationIssueKind {
    UnknownSurface,
    LexicalAmbiguity,
    OntologicalUnderspecification,
    ContradictoryClassification,
    UnknownRelation,
    InvalidReference,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretationIssue {
    pub kind: InterpretationIssueKind,
    pub object: Option<SemanticObjectId>,
    pub message: String,
    pub evidence: BTreeSet<EvidenceId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticGraph {
    pub roots: BTreeMap<String, SemanticObjectId>,
    pub nodes: BTreeMap<SemanticObjectId, SemanticNode>,
    pub edges: BTreeMap<EdgeId, SemanticEdge>,
}

impl SemanticGraph {
    pub fn validate(&self) -> Result<(), InterpretationValidationError> {
        for (label, root) in &self.roots {
            if label.trim().is_empty() {
                return Err(InterpretationValidationError::EmptyRootLabel);
            }
            if !self.nodes.contains_key(root) {
                return Err(InterpretationValidationError::UnknownRoot(root.clone()));
            }
        }
        for (key, node) in &self.nodes {
            if key != &node.id {
                return Err(InterpretationValidationError::NodeIdentityMismatch {
                    key: key.clone(),
                    declared: node.id.clone(),
                });
            }
            key.validate()?;
            node.language.validate()?;
        }
        for (key, edge) in &self.edges {
            if key != &edge.id {
                return Err(InterpretationValidationError::EdgeIdentityMismatch {
                    key: key.clone(),
                    declared: edge.id.clone(),
                });
            }
            key.validate()?;
            edge.relation.validate()?;
            if !self.nodes.contains_key(&edge.subject) {
                return Err(InterpretationValidationError::UnknownEdgeSubject {
                    edge: key.clone(),
                    object: edge.subject.clone(),
                });
            }
            if !self.nodes.contains_key(&edge.object) {
                return Err(InterpretationValidationError::UnknownEdgeObject {
                    edge: key.clone(),
                    object: edge.object.clone(),
                });
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretationBundle {
    pub id: InterpretationId,
    pub snapshot: RegistrySnapshot,
    pub graph: SemanticGraph,
    pub issues: Vec<InterpretationIssue>,
    pub context: BTreeMap<String, String>,
}

impl InterpretationBundle {
    pub fn validate(&self) -> Result<(), InterpretationValidationError> {
        self.id.validate()?;
        self.graph.validate()
    }

    #[must_use]
    pub fn referenced_concepts(&self) -> BTreeSet<ConceptId> {
        let mut concepts = BTreeSet::new();
        for node in self.graph.nodes.values() {
            concepts.extend(node.classification.guaranteed_types.iter().cloned());
            concepts.extend(node.classification.possible_refinements.iter().cloned());
            concepts.extend(node.classification.excluded_types.iter().cloned());
            if let SemanticDenotation::Entity { concept } = &node.denotation {
                concepts.insert(concept.clone());
            }
        }
        concepts
    }

    #[must_use]
    pub fn referenced_relations(&self) -> BTreeSet<RelationId> {
        let mut relations: BTreeSet<RelationId> = self
            .graph
            .edges
            .values()
            .map(|edge| edge.relation.clone())
            .collect();
        for node in self.graph.nodes.values() {
            if let SemanticDenotation::Relation { relation } = &node.denotation {
                relations.insert(relation.clone());
            }
        }
        relations
    }
}

pub trait SemanticInterpreter {
    fn interpret(
        &self,
        request: &InterpretationRequest,
    ) -> Result<InterpretationBundle, InterpretationError>;
}

pub struct DeterministicInterpreter<'a> {
    registry: &'a PackageRegistry,
}

impl<'a> DeterministicInterpreter<'a> {
    #[must_use]
    pub const fn new(registry: &'a PackageRegistry) -> Self {
        Self { registry }
    }

    fn interpret_unit(
        &self,
        unit: &InterpretationUnit,
        snapshot: &RegistrySnapshot,
        index: &OntologyIndex,
        issues: &mut Vec<InterpretationIssue>,
    ) -> Result<SemanticNode, InterpretationError> {
        let mut guaranteed = unit.asserted_types.clone();
        let mut possible = unit.possible_refinements.clone();
        let mut source_senses = BTreeSet::new();
        let mut evidence = unit.evidence.clone();

        let (denotation, status) = match &unit.hint {
            Some(DenotationHint::Literal { datatype, lexical }) => (
                SemanticDenotation::Literal {
                    datatype: datatype.clone(),
                    lexical: lexical.clone(),
                },
                ClassificationStatus::LeastSpecificGuaranteed,
            ),
            Some(DenotationHint::Quoted { text }) => (
                SemanticDenotation::Mention {
                    text: text.clone(),
                    senses: senses_from_resolution(&unit.resolution),
                },
                ClassificationStatus::LeastSpecificGuaranteed,
            ),
            Some(DenotationHint::FormalSymbol { id }) => (
                SemanticDenotation::FormalSymbol { id: id.clone() },
                ClassificationStatus::LeastSpecificGuaranteed,
            ),
            None => Self::denotation_from_resolution(
                unit,
                index,
                &mut guaranteed,
                &mut possible,
                &mut source_senses,
                &mut evidence,
                issues,
            ),
        };

        let classification =
            RegistryClassifier::new(self.registry).classify(&ClassificationRequest {
                snapshot: snapshot.clone(),
                status_hint: status,
                guaranteed_types: guaranteed,
                possible_refinements: possible,
                excluded_types: unit.excluded_types.clone(),
                evidence: evidence.clone(),
                notes: Vec::new(),
                policy: ClassificationPolicy::default(),
            })?;

        if classification.status == ClassificationStatus::Contradictory {
            issues.push(InterpretationIssue {
                kind: InterpretationIssueKind::ContradictoryClassification,
                object: Some(unit.id.clone()),
                message: "guaranteed and excluded/disjoint types conflict".to_owned(),
                evidence: evidence.clone(),
            });
        }

        Ok(SemanticNode {
            id: unit.id.clone(),
            surface: unit.surface.clone(),
            language: unit.language.clone(),
            source_senses,
            classification,
            denotation,
            evidence,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn denotation_from_resolution(
        unit: &InterpretationUnit,
        index: &OntologyIndex,
        guaranteed: &mut BTreeSet<ConceptId>,
        possible: &mut BTreeSet<ConceptId>,
        source_senses: &mut BTreeSet<LexicalSenseId>,
        evidence: &mut BTreeSet<EvidenceId>,
        issues: &mut Vec<InterpretationIssue>,
    ) -> (SemanticDenotation, ClassificationStatus) {
        match &unit.resolution {
            ResolutionResult::Resolved {
                candidate,
                reference_mode,
                ..
            } => {
                source_senses.insert(candidate.sense.clone());
                evidence.extend(candidate.evidence.iter().cloned());
                if matches!(
                    reference_mode,
                    ReferenceMode::Mention | ReferenceMode::Quotation
                ) {
                    return (
                        SemanticDenotation::Mention {
                            text: unit.surface.clone(),
                            senses: source_senses.clone(),
                        },
                        ClassificationStatus::LeastSpecificGuaranteed,
                    );
                }
                match &candidate.target {
                    SemanticTarget::Concept { id } => {
                        guaranteed.insert(id.clone());
                        (
                            SemanticDenotation::Entity {
                                concept: id.clone(),
                            },
                            ClassificationStatus::Exact,
                        )
                    }
                    SemanticTarget::Relation { id } => (
                        SemanticDenotation::Relation {
                            relation: id.clone(),
                        },
                        ClassificationStatus::Exact,
                    ),
                    SemanticTarget::FormalSymbol { id } | SemanticTarget::Template { id } => (
                        SemanticDenotation::FormalSymbol { id: id.clone() },
                        ClassificationStatus::Exact,
                    ),
                }
            }
            ResolutionResult::Ambiguous { candidates, .. } => {
                let concept_candidates: BTreeSet<ConceptId> = candidates
                    .iter()
                    .filter_map(|candidate| match &candidate.target {
                        SemanticTarget::Concept { id } => Some(id.clone()),
                        _ => None,
                    })
                    .collect();
                for candidate in candidates {
                    source_senses.insert(candidate.sense.clone());
                    evidence.extend(candidate.evidence.iter().cloned());
                }
                possible.extend(concept_candidates.iter().cloned());
                let common = index.most_specific(&index.common_ancestors(&concept_candidates));
                guaranteed.extend(common);
                issues.push(InterpretationIssue {
                    kind: InterpretationIssueKind::LexicalAmbiguity,
                    object: Some(unit.id.clone()),
                    message: format!(
                        "surface {:?} has {} viable lexical senses",
                        unit.surface,
                        candidates.len()
                    ),
                    evidence: evidence.clone(),
                });
                (
                    SemanticDenotation::Ambiguous {
                        candidates: candidates
                            .iter()
                            .map(|candidate| candidate.target.clone())
                            .collect(),
                    },
                    ClassificationStatus::Ambiguous,
                )
            }
            ResolutionResult::Unknown { .. } => {
                issues.push(InterpretationIssue {
                    kind: InterpretationIssueKind::UnknownSurface,
                    object: Some(unit.id.clone()),
                    message: format!("surface {:?} has no registered lexical sense", unit.surface),
                    evidence: evidence.clone(),
                });
                (
                    SemanticDenotation::Unresolved {
                        surface: unit.surface.clone(),
                    },
                    if guaranteed.is_empty() {
                        ClassificationStatus::Unknown
                    } else {
                        ClassificationStatus::LeastSpecificGuaranteed
                    },
                )
            }
        }
    }
}

impl SemanticInterpreter for DeterministicInterpreter<'_> {
    fn interpret(
        &self,
        request: &InterpretationRequest,
    ) -> Result<InterpretationBundle, InterpretationError> {
        request.id.validate()?;
        self.registry.validate_snapshot(&request.snapshot)?;
        let index = self.registry.ontology_index(&request.snapshot)?;
        let mut nodes = BTreeMap::new();
        let mut issues = Vec::new();

        for unit in &request.units {
            unit.id.validate()?;
            if nodes.contains_key(&unit.id) {
                return Err(InterpretationError::DuplicateObject(unit.id.clone()));
            }
            let node = self.interpret_unit(unit, &request.snapshot, &index, &mut issues)?;
            nodes.insert(unit.id.clone(), node);
        }

        let mut edges = BTreeMap::new();
        for relation in &request.relations {
            relation.id.validate()?;
            if edges.contains_key(&relation.id) {
                return Err(InterpretationError::DuplicateEdge(relation.id.clone()));
            }
            if !nodes.contains_key(&relation.subject) || !nodes.contains_key(&relation.object) {
                issues.push(InterpretationIssue {
                    kind: InterpretationIssueKind::InvalidReference,
                    object: None,
                    message: format!("edge {} references an unknown object", relation.id),
                    evidence: relation.evidence.clone(),
                });
            }
            if !index.relations.contains_key(&relation.relation) {
                issues.push(InterpretationIssue {
                    kind: InterpretationIssueKind::UnknownRelation,
                    object: None,
                    message: format!(
                        "edge {} uses unknown relation {}",
                        relation.id, relation.relation
                    ),
                    evidence: relation.evidence.clone(),
                });
            }
            edges.insert(
                relation.id.clone(),
                SemanticEdge {
                    id: relation.id.clone(),
                    relation: relation.relation.clone(),
                    subject: relation.subject.clone(),
                    object: relation.object.clone(),
                    qualifiers: relation.qualifiers.clone(),
                    evidence: relation.evidence.clone(),
                },
            );
        }

        let bundle = InterpretationBundle {
            id: request.id.clone(),
            snapshot: request.snapshot.clone(),
            graph: SemanticGraph {
                roots: request.roots.clone(),
                nodes,
                edges,
            },
            issues,
            context: request.context.clone(),
        };
        bundle.validate()?;
        Ok(bundle)
    }
}

fn senses_from_resolution(resolution: &ResolutionResult) -> BTreeSet<LexicalSenseId> {
    resolution
        .candidates()
        .iter()
        .map(|candidate: &SenseCandidate| candidate.sense.clone())
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum InterpretationValidationError {
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error("root label must not be empty")]
    EmptyRootLabel,
    #[error("root references unknown node {0}")]
    UnknownRoot(SemanticObjectId),
    #[error("node map key {key} contains declaration {declared}")]
    NodeIdentityMismatch {
        key: SemanticObjectId,
        declared: SemanticObjectId,
    },
    #[error("edge map key {key} contains declaration {declared}")]
    EdgeIdentityMismatch { key: EdgeId, declared: EdgeId },
    #[error("edge {edge} references unknown subject {object}")]
    UnknownEdgeSubject {
        edge: EdgeId,
        object: SemanticObjectId,
    },
    #[error("edge {edge} references unknown object {object}")]
    UnknownEdgeObject {
        edge: EdgeId,
        object: SemanticObjectId,
    },
}

#[derive(Debug, Error)]
pub enum InterpretationError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Classification(#[from] muse_classification::ClassificationError),
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    Validation(#[from] InterpretationValidationError),
    #[error("duplicate semantic object id: {0}")]
    DuplicateObject(SemanticObjectId),
    #[error("duplicate semantic edge id: {0}")]
    DuplicateEdge(EdgeId),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use muse_core::{
        ContentDigest, DigestAlgorithm, PackageHeader, PackageId, PackageKind, PackageRef,
        PackageVersion,
    };
    use muse_ontology::{ConceptDeclaration, ConceptProfile, DefinitionRecord, OntologyPackage};
    use muse_registry::SemanticPackage;
    use muse_resolution::{ResolutionPolicy, ResolutionRequest};

    use super::*;

    #[test]
    fn unknown_surface_still_produces_a_total_node() {
        let entity = ConceptId::from("ufo:Entity");
        let ontology = OntologyPackage {
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
                description: "Test package".to_owned(),
                license: "CC0-1.0".to_owned(),
                imports: Vec::new(),
                evidence: Vec::new(),
            },
            concepts: BTreeMap::from([(
                entity.clone(),
                ConceptDeclaration {
                    id: entity.clone(),
                    preferred_labels: BTreeMap::from([(
                        LanguageTag::from("en"),
                        "entity".to_owned(),
                    )]),
                    alternate_labels: BTreeMap::new(),
                    definition: DefinitionRecord {
                        text: "Anything admitted by the ontology.".to_owned(),
                        genus: None,
                        differentia: Vec::new(),
                        necessary_conditions: Vec::new(),
                        sufficient_conditions: Vec::new(),
                        scope: None,
                        notes: Vec::new(),
                    },
                    parents: BTreeSet::new(),
                    disjoint_with: BTreeSet::new(),
                    profile: ConceptProfile::default(),
                    deprecated: false,
                    evidence: BTreeSet::new(),
                },
            )]),
            relations: BTreeMap::new(),
            axioms: Vec::new(),
        };
        let mut registry = PackageRegistry::new();
        let ontology_ref = registry
            .register(SemanticPackage::Ontology(ontology).seal().unwrap())
            .unwrap();
        let snapshot = registry.snapshot([ontology_ref]).unwrap();
        let resolution = ResolutionResult::Unknown {
            surface: "flarn".to_owned(),
            language: LanguageTag::from("en"),
            reference_mode: ReferenceMode::Use,
        };
        let request = InterpretationRequest {
            id: InterpretationId::from("interpretation:test"),
            snapshot,
            roots: BTreeMap::from([("main".to_owned(), SemanticObjectId::from("object:1"))]),
            units: vec![InterpretationUnit {
                id: SemanticObjectId::from("object:1"),
                surface: "flarn".to_owned(),
                language: LanguageTag::from("en"),
                resolution,
                asserted_types: BTreeSet::from([entity]),
                possible_refinements: BTreeSet::new(),
                excluded_types: BTreeSet::new(),
                hint: None,
                evidence: BTreeSet::new(),
            }],
            relations: Vec::new(),
            context: BTreeMap::new(),
        };
        let bundle = DeterministicInterpreter::new(&registry)
            .interpret(&request)
            .unwrap();
        assert_eq!(bundle.graph.nodes.len(), 1);
        assert_eq!(bundle.issues.len(), 1);

        let _unused = ResolutionRequest {
            surface: "flarn".to_owned(),
            language: LanguageTag::from("en"),
            snapshot: bundle.snapshot.clone(),
            domain_hints: BTreeSet::new(),
            context_concepts: BTreeSet::new(),
            reference_mode: ReferenceMode::Use,
            policy: ResolutionPolicy::default(),
        };
    }
}
