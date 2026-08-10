//! Replaceable ontology classification over a content-pinned registry snapshot.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use muse_core::{ConceptId, EvidenceId};
use muse_ontology::{ClassificationResult, ClassificationStatus};
use muse_registry::{PackageRegistry, RegistryError, RegistrySnapshot};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassificationPolicy {
    pub include_ancestors: bool,
    pub reject_unknown_types: bool,
    pub infer_common_guarantees: bool,
}

impl Default for ClassificationPolicy {
    fn default() -> Self {
        Self {
            include_ancestors: true,
            reject_unknown_types: true,
            infer_common_guarantees: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassificationRequest {
    pub snapshot: RegistrySnapshot,
    pub status_hint: ClassificationStatus,
    pub guaranteed_types: BTreeSet<ConceptId>,
    pub possible_refinements: BTreeSet<ConceptId>,
    pub excluded_types: BTreeSet<ConceptId>,
    pub evidence: BTreeSet<EvidenceId>,
    pub notes: Vec<String>,
    pub policy: ClassificationPolicy,
}

pub trait OntologyClassifier {
    fn classify(
        &self,
        request: &ClassificationRequest,
    ) -> Result<ClassificationResult, ClassificationError>;
}

pub struct RegistryClassifier<'a> {
    registry: &'a PackageRegistry,
}

impl<'a> RegistryClassifier<'a> {
    #[must_use]
    pub const fn new(registry: &'a PackageRegistry) -> Self {
        Self { registry }
    }
}

impl OntologyClassifier for RegistryClassifier<'_> {
    fn classify(
        &self,
        request: &ClassificationRequest,
    ) -> Result<ClassificationResult, ClassificationError> {
        self.registry.validate_snapshot(&request.snapshot)?;
        let index = self.registry.ontology_index(&request.snapshot)?;

        if request.policy.reject_unknown_types {
            for concept in request
                .guaranteed_types
                .iter()
                .chain(&request.possible_refinements)
                .chain(&request.excluded_types)
            {
                if !index.concepts.contains_key(concept) {
                    return Err(ClassificationError::UnknownConcept(concept.clone()));
                }
            }
        }

        let mut guaranteed = request.guaranteed_types.clone();
        if guaranteed.is_empty()
            && request.policy.infer_common_guarantees
            && !request.possible_refinements.is_empty()
        {
            guaranteed.extend(
                index.most_specific(&index.common_ancestors(&request.possible_refinements)),
            );
        }

        let mut result = ClassificationResult {
            status: request.status_hint,
            guaranteed_types: guaranteed,
            possible_refinements: request.possible_refinements.clone(),
            excluded_types: request.excluded_types.clone(),
            evidence: request.evidence.clone(),
            notes: request.notes.clone(),
        };

        if request.policy.include_ancestors {
            result = result.normalize(&index);
        } else {
            let contradictory = result.guaranteed_types.iter().any(|left| {
                result
                    .guaranteed_types
                    .iter()
                    .any(|right| left < right && index.are_disjoint(left, right))
            });
            if contradictory {
                result.status = ClassificationStatus::Contradictory;
            }
        }
        Ok(result)
    }
}

#[derive(Debug, Error)]
pub enum ClassificationError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error("classification references unknown concept {0}")]
    UnknownConcept(ConceptId),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use muse_core::{
        ContentDigest, DigestAlgorithm, LanguageTag, PackageHeader, PackageId, PackageKind,
        PackageRef, PackageVersion,
    };
    use muse_ontology::{ConceptDeclaration, ConceptProfile, DefinitionRecord, OntologyPackage};
    use muse_registry::SemanticPackage;

    use super::*;

    fn declaration(id: &str, parent: Option<&str>) -> ConceptDeclaration {
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
            parents: parent.into_iter().map(ConceptId::from).collect(),
            disjoint_with: BTreeSet::new(),
            profile: ConceptProfile::default(),
            deprecated: false,
            evidence: BTreeSet::new(),
        }
    }

    #[test]
    fn infers_common_guaranteed_ancestor() {
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
            concepts: BTreeMap::from([
                (ConceptId::from("Entity"), declaration("Entity", None)),
                (
                    ConceptId::from("Object"),
                    declaration("Object", Some("Entity")),
                ),
                (
                    ConceptId::from("Event"),
                    declaration("Event", Some("Entity")),
                ),
            ]),
            relations: BTreeMap::new(),
            axioms: Vec::new(),
        };
        let mut registry = PackageRegistry::new();
        let package = registry
            .register(SemanticPackage::Ontology(ontology).seal().unwrap())
            .unwrap();
        let request = ClassificationRequest {
            snapshot: registry.snapshot([package]).unwrap(),
            status_hint: ClassificationStatus::Ambiguous,
            guaranteed_types: BTreeSet::new(),
            possible_refinements: BTreeSet::from([
                ConceptId::from("Object"),
                ConceptId::from("Event"),
            ]),
            excluded_types: BTreeSet::new(),
            evidence: BTreeSet::new(),
            notes: Vec::new(),
            policy: ClassificationPolicy::default(),
        };
        let result = RegistryClassifier::new(&registry)
            .classify(&request)
            .unwrap();
        assert!(result.guaranteed_types.contains(&ConceptId::from("Entity")));
    }
}
