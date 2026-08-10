//! Provenance, evidence, agents, activities, and assertion status.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use muse_core::{
    ActivityId, AgentId, AssertionId, Confidence, EvidenceId, HeaderError, IdentityError,
    PackageHeader, PackageKind, SourceId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Standard,
    PeerReviewedPaper,
    Book,
    Corpus,
    Documentation,
    SourceCode,
    ExpertDecision,
    RuntimeObservation,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub id: SourceId,
    pub kind: SourceKind,
    pub title: String,
    pub creators: Vec<String>,
    pub locator: Option<String>,
    pub citation: String,
    pub issued: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Person,
    Organization,
    Software,
    ReviewBoard,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRecord {
    pub id: AgentId,
    pub kind: AgentKind,
    pub name: String,
    pub locator: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Extraction,
    Definition,
    Mapping,
    Review,
    Adjudication,
    Import,
    Migration,
    AutomatedProposal,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityRecord {
    pub id: ActivityId,
    pub kind: ActivityKind,
    pub label: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub agents: BTreeSet<AgentId>,
    pub used_sources: BTreeSet<SourceId>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Canonical,
    AdoptedFromStandard,
    ExpertAsserted,
    CorpusAttested,
    AutomaticallyProposed,
    Disputed,
    Provisional,
    Deprecated,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub id: EvidenceId,
    pub status: EvidenceStatus,
    pub source: SourceId,
    pub generated_by: Option<ActivityId>,
    pub attributed_to: BTreeSet<AgentId>,
    pub confidence: Confidence,
    pub excerpt: Option<String>,
    pub locator: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionStatus {
    Accepted,
    Provisional,
    Disputed,
    Superseded,
    Deprecated,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertionRecord {
    pub id: AssertionId,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub status: AssertionStatus,
    pub evidence: BTreeSet<EvidenceId>,
    pub supersedes: BTreeSet<AssertionId>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenancePackage {
    pub header: PackageHeader,
    pub sources: BTreeMap<SourceId, SourceRecord>,
    pub agents: BTreeMap<AgentId, AgentRecord>,
    pub activities: BTreeMap<ActivityId, ActivityRecord>,
    pub evidence: BTreeMap<EvidenceId, EvidenceRecord>,
    pub assertions: BTreeMap<AssertionId, AssertionRecord>,
}

impl ProvenancePackage {
    pub fn validate(&self) -> Result<(), ProvenanceValidationError> {
        self.header.validate()?;
        if self.header.kind != PackageKind::Provenance {
            return Err(ProvenanceValidationError::WrongPackageKind);
        }

        for (key, source) in &self.sources {
            if key != &source.id {
                return Err(ProvenanceValidationError::SourceIdentityMismatch {
                    key: key.clone(),
                    declared: source.id.clone(),
                });
            }
            key.validate()?;
            validate_nonempty("source title", &source.title)?;
            validate_nonempty("source citation", &source.citation)?;
        }

        for (key, agent) in &self.agents {
            if key != &agent.id {
                return Err(ProvenanceValidationError::AgentIdentityMismatch {
                    key: key.clone(),
                    declared: agent.id.clone(),
                });
            }
            key.validate()?;
            validate_nonempty("agent name", &agent.name)?;
        }

        for (key, activity) in &self.activities {
            if key != &activity.id {
                return Err(ProvenanceValidationError::ActivityIdentityMismatch {
                    key: key.clone(),
                    declared: activity.id.clone(),
                });
            }
            key.validate()?;
            validate_nonempty("activity label", &activity.label)?;
            for agent in &activity.agents {
                if !self.agents.contains_key(agent) {
                    return Err(ProvenanceValidationError::UnknownActivityAgent {
                        activity: key.clone(),
                        agent: agent.clone(),
                    });
                }
            }
            for source in &activity.used_sources {
                if !self.sources.contains_key(source) {
                    return Err(ProvenanceValidationError::UnknownActivitySource {
                        activity: key.clone(),
                        source_id: source.clone(),
                    });
                }
            }
        }

        for (key, evidence) in &self.evidence {
            if key != &evidence.id {
                return Err(ProvenanceValidationError::EvidenceIdentityMismatch {
                    key: key.clone(),
                    declared: evidence.id.clone(),
                });
            }
            key.validate()?;
            if !self.sources.contains_key(&evidence.source) {
                return Err(ProvenanceValidationError::UnknownEvidenceSource {
                    evidence: key.clone(),
                    source_id: evidence.source.clone(),
                });
            }
            if let Some(activity) = &evidence.generated_by {
                if !self.activities.contains_key(activity) {
                    return Err(ProvenanceValidationError::UnknownEvidenceActivity {
                        evidence: key.clone(),
                        activity: activity.clone(),
                    });
                }
            }
            for agent in &evidence.attributed_to {
                if !self.agents.contains_key(agent) {
                    return Err(ProvenanceValidationError::UnknownEvidenceAgent {
                        evidence: key.clone(),
                        agent: agent.clone(),
                    });
                }
            }
        }

        for (key, assertion) in &self.assertions {
            if key != &assertion.id {
                return Err(ProvenanceValidationError::AssertionIdentityMismatch {
                    key: key.clone(),
                    declared: assertion.id.clone(),
                });
            }
            key.validate()?;
            validate_nonempty("assertion subject", &assertion.subject)?;
            validate_nonempty("assertion predicate", &assertion.predicate)?;
            validate_nonempty("assertion object", &assertion.object)?;
            for evidence in &assertion.evidence {
                if !self.evidence.contains_key(evidence) {
                    return Err(ProvenanceValidationError::UnknownAssertionEvidence {
                        assertion: key.clone(),
                        evidence: evidence.clone(),
                    });
                }
            }
            for prior in &assertion.supersedes {
                if !self.assertions.contains_key(prior) {
                    return Err(ProvenanceValidationError::UnknownSupersededAssertion {
                        assertion: key.clone(),
                        prior: prior.clone(),
                    });
                }
            }
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

#[derive(Debug, Error)]
pub enum ProvenanceValidationError {
    #[error(transparent)]
    Header(#[from] HeaderError),
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error("provenance package header has a non-provenance kind")]
    WrongPackageKind,
    #[error("source map key {key} contains declaration {declared}")]
    SourceIdentityMismatch { key: SourceId, declared: SourceId },
    #[error("agent map key {key} contains declaration {declared}")]
    AgentIdentityMismatch { key: AgentId, declared: AgentId },
    #[error("activity map key {key} contains declaration {declared}")]
    ActivityIdentityMismatch {
        key: ActivityId,
        declared: ActivityId,
    },
    #[error("evidence map key {key} contains declaration {declared}")]
    EvidenceIdentityMismatch {
        key: EvidenceId,
        declared: EvidenceId,
    },
    #[error("assertion map key {key} contains declaration {declared}")]
    AssertionIdentityMismatch {
        key: AssertionId,
        declared: AssertionId,
    },
    #[error("activity {activity} references unknown agent {agent}")]
    UnknownActivityAgent {
        activity: ActivityId,
        agent: AgentId,
    },
    #[error("activity {activity} references unknown source {source_id}")]
    UnknownActivitySource {
        activity: ActivityId,
        source_id: SourceId,
    },
    #[error("evidence {evidence} references unknown source {source_id}")]
    UnknownEvidenceSource {
        evidence: EvidenceId,
        source_id: SourceId,
    },
    #[error("evidence {evidence} references unknown activity {activity}")]
    UnknownEvidenceActivity {
        evidence: EvidenceId,
        activity: ActivityId,
    },
    #[error("evidence {evidence} references unknown agent {agent}")]
    UnknownEvidenceAgent {
        evidence: EvidenceId,
        agent: AgentId,
    },
    #[error("assertion {assertion} references unknown evidence {evidence}")]
    UnknownAssertionEvidence {
        assertion: AssertionId,
        evidence: EvidenceId,
    },
    #[error("assertion {assertion} supersedes unknown assertion {prior}")]
    UnknownSupersededAssertion {
        assertion: AssertionId,
        prior: AssertionId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use muse_core::{ContentDigest, DigestAlgorithm, PackageId, PackageRef, PackageVersion};

    fn header() -> PackageHeader {
        PackageHeader {
            package: PackageRef {
                id: PackageId::from("muse.test.provenance"),
                version: PackageVersion::from("0.1.0"),
                digest: ContentDigest {
                    algorithm: DigestAlgorithm::Sha256,
                    value: "a".repeat(64),
                },
            },
            kind: PackageKind::Provenance,
            title: "Test provenance".to_owned(),
            description: "Test provenance package".to_owned(),
            license: "CC0-1.0".to_owned(),
            imports: Vec::new(),
            evidence: Vec::new(),
        }
    }

    #[test]
    fn rejects_evidence_with_unknown_source() {
        let evidence_id = EvidenceId::from("e:test");
        let package = ProvenancePackage {
            header: header(),
            sources: BTreeMap::new(),
            agents: BTreeMap::new(),
            activities: BTreeMap::new(),
            evidence: BTreeMap::from([(
                evidence_id.clone(),
                EvidenceRecord {
                    id: evidence_id,
                    status: EvidenceStatus::Provisional,
                    source: SourceId::from("source:missing"),
                    generated_by: None,
                    attributed_to: BTreeSet::new(),
                    confidence: Confidence::default(),
                    excerpt: None,
                    locator: None,
                    notes: Vec::new(),
                },
            )]),
            assertions: BTreeMap::new(),
        };
        assert!(matches!(
            package.validate(),
            Err(ProvenanceValidationError::UnknownEvidenceSource { .. })
        ));
    }
}
