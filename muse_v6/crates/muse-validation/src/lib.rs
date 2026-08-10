//! Cross-package validation, competency cases, regression pins, and release gates.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use muse_core::{
    CompetencyId, ConceptId, ContentDigest, Diagnostic, EvidenceId, LexicalSenseId, PackageRef,
    RelationId, Severity,
};
use muse_interpretation::{
    DeterministicInterpreter, InterpretationBundle, InterpretationRequest, SemanticInterpreter,
};
use muse_lexicon::SemanticTarget;
use muse_reasoning::{Fact, ForwardReasoner, KnowledgeBase, ReasoningOptions, ReasoningReport};
use muse_registry::{PackageRegistry, RegistryError, RegistrySnapshot, SemanticPackage};
use muse_resolution::{LexicalResolver, RegistryResolver, ResolutionRequest, ResolutionResult};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationOptions {
    pub require_evidence_for_concepts: bool,
    pub require_evidence_for_senses: bool,
    pub reject_deprecated_references: bool,
    pub warnings_are_errors: bool,
}

impl Default for ValidationOptions {
    fn default() -> Self {
        Self {
            require_evidence_for_concepts: false,
            require_evidence_for_senses: false,
            reject_deprecated_references: true,
            warnings_are_errors: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationStats {
    pub packages: usize,
    pub concepts: usize,
    pub relations: usize,
    pub lexical_entries: usize,
    pub lexical_senses: usize,
    pub predicate_frames: usize,
    pub attestations: usize,
    pub evidence_records: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub diagnostics: Vec<Diagnostic>,
    pub stats: ValidationStats,
}

impl ValidationReport {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    #[must_use]
    pub fn has_warnings(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Warning)
    }

    #[must_use]
    pub fn passed(&self, options: &ValidationOptions) -> bool {
        !self.has_errors() && !(options.warnings_are_errors && self.has_warnings())
    }

    pub fn push_error(&mut self, code: &str, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(code, message));
    }

    pub fn push_warning(&mut self, code: &str, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::warning(code, message));
    }
}

pub fn validate_registry(
    registry: &PackageRegistry,
    snapshot: &RegistrySnapshot,
    options: &ValidationOptions,
) -> Result<ValidationReport, ValidationError> {
    registry.validate_snapshot(snapshot)?;
    let ontology = registry.ontology_index(snapshot)?;
    let mut report = ValidationReport {
        diagnostics: Vec::new(),
        stats: ValidationStats {
            packages: snapshot.packages.len(),
            concepts: ontology.concepts.len(),
            relations: ontology.relations.len(),
            ..ValidationStats::default()
        },
    };

    let evidence_ids: BTreeSet<EvidenceId> = registry
        .provenance(snapshot)?
        .into_iter()
        .flat_map(|package| package.evidence.keys().cloned())
        .collect();
    report.stats.evidence_records = evidence_ids.len();

    for package_ref in &snapshot.packages {
        let Some(package) = registry.get(package_ref) else {
            report.push_error(
                "registry.unknown_package",
                format!("snapshot references unknown package {package_ref}"),
            );
            continue;
        };
        validate_evidence_slice(
            &mut report,
            &evidence_ids,
            match package {
                SemanticPackage::Ontology(package) => &package.header.evidence,
                SemanticPackage::Lexicon(package) => &package.header.evidence,
                SemanticPackage::Provenance(package) => &package.header.evidence,
            },
            "package.header_evidence",
            &package_ref.to_string(),
        );
        match package {
            SemanticPackage::Ontology(package) => {
                for concept in package.concepts.values() {
                    validate_evidence_set(
                        &mut report,
                        &evidence_ids,
                        &concept.evidence,
                        "ontology.concept_evidence",
                        &concept.id.to_string(),
                    );
                    if options.require_evidence_for_concepts && concept.evidence.is_empty() {
                        report.push_error(
                            "ontology.missing_evidence",
                            format!("concept {} has no evidence", concept.id),
                        );
                    }
                }
                for relation in package.relations.values() {
                    validate_evidence_set(
                        &mut report,
                        &evidence_ids,
                        &relation.evidence,
                        "ontology.relation_evidence",
                        &relation.id.to_string(),
                    );
                }
            }
            SemanticPackage::Lexicon(package) => {
                for ontology_package in &package.ontology_packages {
                    if !snapshot.packages.contains(ontology_package) {
                        report.push_error(
                            "lexicon.missing_ontology_package",
                            format!(
                                "lexicon {} requires ontology package {} outside the snapshot",
                                package.header.package, ontology_package
                            ),
                        );
                    }
                }
                report.stats.lexical_entries += package.entries.len();
                report.stats.lexical_senses += package.senses.len();
                report.stats.predicate_frames += package.frames.len();
                report.stats.attestations += package.attestations.len();

                for sense in package.senses.values() {
                    match &sense.target {
                        SemanticTarget::Concept { id } => {
                            if !ontology.concepts.contains_key(id) {
                                report.push_error(
                                    "lexicon.unknown_concept_target",
                                    format!("sense {} targets unknown concept {id}", sense.id),
                                );
                            } else if options.reject_deprecated_references
                                && ontology
                                    .concepts
                                    .get(id)
                                    .is_some_and(|concept| concept.deprecated)
                            {
                                report.push_error(
                                    "lexicon.deprecated_concept_target",
                                    format!("sense {} targets deprecated concept {id}", sense.id),
                                );
                            }
                        }
                        SemanticTarget::Relation { id } => {
                            if !ontology.relations.contains_key(id) {
                                report.push_error(
                                    "lexicon.unknown_relation_target",
                                    format!("sense {} targets unknown relation {id}", sense.id),
                                );
                            } else if options.reject_deprecated_references
                                && ontology
                                    .relations
                                    .get(id)
                                    .is_some_and(|relation| relation.deprecated)
                            {
                                report.push_error(
                                    "lexicon.deprecated_relation_target",
                                    format!("sense {} targets deprecated relation {id}", sense.id),
                                );
                            }
                        }
                        SemanticTarget::FormalSymbol { .. } | SemanticTarget::Template { .. } => {}
                    }
                    for relation in &sense.relations {
                        validate_evidence_set(
                            &mut report,
                            &evidence_ids,
                            &relation.evidence,
                            "lexicon.sense_relation_evidence",
                            &sense.id.to_string(),
                        );
                    }
                    validate_evidence_set(
                        &mut report,
                        &evidence_ids,
                        &sense.evidence,
                        "lexicon.sense_evidence",
                        &sense.id.to_string(),
                    );
                    if options.require_evidence_for_senses && sense.evidence.is_empty() {
                        report.push_error(
                            "lexicon.missing_evidence",
                            format!("sense {} has no evidence", sense.id),
                        );
                    }
                }

                for frame in package.frames.values() {
                    validate_evidence_set(
                        &mut report,
                        &evidence_ids,
                        &frame.evidence,
                        "lexicon.frame_evidence",
                        &frame.id.to_string(),
                    );
                    for argument in &frame.arguments {
                        for selection in &argument.selection {
                            if !ontology.concepts.contains_key(&selection.required_type) {
                                report.push_error(
                                    "lexicon.unknown_frame_type",
                                    format!(
                                        "frame {} argument {} requires unknown type {}",
                                        frame.id, argument.id, selection.required_type
                                    ),
                                );
                            }
                        }
                    }
                }
            }
            SemanticPackage::Provenance(_) => {}
        }
    }

    Ok(report)
}

fn validate_evidence_slice(
    report: &mut ValidationReport,
    known: &BTreeSet<EvidenceId>,
    referenced: &[EvidenceId],
    code: &str,
    owner: &str,
) {
    for evidence in referenced {
        if !known.contains(evidence) {
            report.push_error(
                code,
                format!("{owner} references unknown evidence {evidence}"),
            );
        }
    }
}

fn validate_evidence_set(
    report: &mut ValidationReport,
    known: &BTreeSet<EvidenceId>,
    referenced: &BTreeSet<EvidenceId>,
    code: &str,
    owner: &str,
) {
    for evidence in referenced {
        if !known.contains(evidence) {
            report.push_error(
                code,
                format!("{owner} references unknown evidence {evidence}"),
            );
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExpectedResolution {
    Resolved { sense: LexicalSenseId },
    Ambiguous { senses: BTreeSet<LexicalSenseId> },
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionCompetencyCase {
    pub id: CompetencyId,
    pub request: ResolutionRequest,
    pub expected: ExpectedResolution,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretationCompetencyCase {
    pub id: CompetencyId,
    pub request: InterpretationRequest,
    pub required_concepts: BTreeSet<ConceptId>,
    pub required_relations: BTreeSet<RelationId>,
    pub expected_issue_count: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompetencyResult {
    pub id: CompetencyId,
    pub passed: bool,
    pub message: String,
}

pub fn run_resolution_competency(
    registry: &PackageRegistry,
    case: &ResolutionCompetencyCase,
) -> Result<CompetencyResult, ValidationError> {
    let result = RegistryResolver::new(registry).resolve(&case.request)?;
    let passed = match (&case.expected, &result) {
        (ExpectedResolution::Resolved { sense }, ResolutionResult::Resolved { candidate, .. }) => {
            sense == &candidate.sense
        }
        (
            ExpectedResolution::Ambiguous { senses },
            ResolutionResult::Ambiguous { candidates, .. },
        ) => {
            let actual: BTreeSet<LexicalSenseId> = candidates
                .iter()
                .map(|candidate| candidate.sense.clone())
                .collect();
            senses == &actual
        }
        (ExpectedResolution::Unknown, ResolutionResult::Unknown { .. }) => true,
        _ => false,
    };
    Ok(CompetencyResult {
        id: case.id.clone(),
        passed,
        message: if passed {
            "resolution matched expectation".to_owned()
        } else {
            format!("resolution mismatch: actual {result:?}")
        },
    })
}

pub fn run_interpretation_competency(
    registry: &PackageRegistry,
    case: &InterpretationCompetencyCase,
) -> Result<CompetencyResult, ValidationError> {
    let bundle = DeterministicInterpreter::new(registry).interpret(&case.request)?;
    let concepts = bundle.referenced_concepts();
    let relations = bundle.referenced_relations();
    let concepts_ok = case.required_concepts.is_subset(&concepts);
    let relations_ok = case.required_relations.is_subset(&relations);
    let issues_ok = case
        .expected_issue_count
        .is_none_or(|expected| expected == bundle.issues.len());
    let passed = concepts_ok && relations_ok && issues_ok;
    Ok(CompetencyResult {
        id: case.id.clone(),
        passed,
        message: if passed {
            "interpretation matched expectation".to_owned()
        } else {
            format!(
                "interpretation mismatch: concepts_ok={concepts_ok}, relations_ok={relations_ok}, issues_ok={issues_ok}"
            )
        },
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegressionPin {
    pub package: PackageRef,
    pub expected_digest: ContentDigest,
}

pub fn check_regression_pins(
    registry: &PackageRegistry,
    pins: &[RegressionPin],
) -> Vec<Diagnostic> {
    pins.iter()
        .filter_map(|pin| match registry.get(&pin.package) {
            None => Some(Diagnostic::error(
                "release.regression_pin_missing",
                format!(
                    "regression-pinned package {} is not registered",
                    pin.package
                ),
            )),
            Some(package) => {
                let actual = package.package_ref().digest.clone();
                (actual != pin.expected_digest).then(|| {
                    Diagnostic::error(
                        "release.regression_pin_mismatch",
                        format!(
                            "package {} expected digest {}, found {}",
                            pin.package, pin.expected_digest, actual
                        ),
                    )
                })
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseGate {
    pub require_no_errors: bool,
    pub require_no_warnings: bool,
    pub required_competencies: BTreeSet<CompetencyId>,
}

impl Default for ReleaseGate {
    fn default() -> Self {
        Self {
            require_no_errors: true,
            require_no_warnings: false,
            required_competencies: BTreeSet::new(),
        }
    }
}

impl ReleaseGate {
    #[must_use]
    pub fn evaluate(
        &self,
        report: &ValidationReport,
        competency_results: &[CompetencyResult],
    ) -> bool {
        if self.require_no_errors && report.has_errors() {
            return false;
        }
        if self.require_no_warnings && report.has_warnings() {
            return false;
        }
        let passed: BTreeSet<CompetencyId> = competency_results
            .iter()
            .filter(|result| result.passed)
            .map(|result| result.id.clone())
            .collect();
        self.required_competencies.is_subset(&passed)
    }
}

/// Validate an interpreted graph against executable ontology constraints.
pub fn validate_interpretation_conformance(
    registry: &PackageRegistry,
    bundle: &InterpretationBundle,
    options: &ReasoningOptions,
) -> Result<ReasoningReport, ValidationError> {
    bundle.validate()?;
    registry.validate_snapshot(&bundle.snapshot)?;
    let index = registry.ontology_index(&bundle.snapshot)?;
    let mut facts = BTreeSet::new();
    for node in bundle.graph.nodes.values() {
        for concept in &node.classification.guaranteed_types {
            facts.insert(Fact::InstanceOf {
                individual: node.id.clone(),
                concept: concept.clone(),
            });
        }
    }
    for edge in bundle.graph.edges.values() {
        facts.insert(Fact::Relation {
            subject: edge.subject.clone(),
            relation: edge.relation.clone(),
            object: edge.object.clone(),
        });
    }
    Ok(ForwardReasoner::new(&index).reason(&KnowledgeBase { facts }, options)?)
}

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Resolution(#[from] muse_resolution::ResolutionError),
    #[error(transparent)]
    Interpretation(#[from] muse_interpretation::InterpretationError),
    #[error(transparent)]
    InterpretationValidation(#[from] muse_interpretation::InterpretationValidationError),
    #[error(transparent)]
    Reasoning(#[from] muse_reasoning::ReasoningError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_gate_rejects_required_failed_competency() {
        let gate = ReleaseGate {
            required_competencies: BTreeSet::from([CompetencyId::from("case:one")]),
            ..ReleaseGate::default()
        };
        let report = ValidationReport::default();
        let results = vec![CompetencyResult {
            id: CompetencyId::from("case:one"),
            passed: false,
            message: "failed".to_owned(),
        }];
        assert!(!gate.evaluate(&report, &results));
    }
}
