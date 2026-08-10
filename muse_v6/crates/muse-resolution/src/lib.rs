//! Deterministic controlled-vocabulary resolution with explicit ambiguity and unknown states.
//! Resolution supports known lexical metadata; unrestricted prose formalization is owned by the prose transducer and shared occurrence contract.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use muse_core::{ConceptId, EvidenceId, LanguageTag, LexicalEntryId, LexicalSenseId};
use muse_lexicon::{LexicalEntry, LexicalSense, SemanticTarget};
use muse_registry::{PackageRegistry, RegistryError, RegistrySnapshot};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceMode {
    Use,
    Mention,
    Quotation,
    Hypothetical,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionPolicy {
    pub case_sensitive: bool,
    pub allow_deprecated: bool,
    pub require_evidence: bool,
    pub max_candidates: usize,
    pub ambiguity_margin: u16,
}

impl Default for ResolutionPolicy {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            allow_deprecated: false,
            require_evidence: false,
            max_candidates: 16,
            ambiguity_margin: 250,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionRequest {
    pub surface: String,
    pub language: LanguageTag,
    pub snapshot: RegistrySnapshot,
    pub domain_hints: BTreeSet<String>,
    pub context_concepts: BTreeSet<ConceptId>,
    pub reference_mode: ReferenceMode,
    pub policy: ResolutionPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SenseCandidate {
    pub entry: LexicalEntryId,
    pub sense: LexicalSenseId,
    pub target: SemanticTarget,
    pub score_basis_points: u16,
    pub evidence: BTreeSet<EvidenceId>,
    pub rationale: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResolutionResult {
    Resolved {
        surface: String,
        candidate: SenseCandidate,
        reference_mode: ReferenceMode,
    },
    Ambiguous {
        surface: String,
        candidates: Vec<SenseCandidate>,
        reference_mode: ReferenceMode,
    },
    Unknown {
        surface: String,
        language: LanguageTag,
        reference_mode: ReferenceMode,
    },
}

impl ResolutionResult {
    #[must_use]
    pub fn candidates(&self) -> &[SenseCandidate] {
        match self {
            Self::Resolved { candidate, .. } => std::slice::from_ref(candidate),
            Self::Ambiguous { candidates, .. } => candidates,
            Self::Unknown { .. } => &[],
        }
    }

    #[must_use]
    pub fn is_resolved(&self) -> bool {
        matches!(self, Self::Resolved { .. })
    }
}

pub trait LexicalResolver {
    fn resolve(&self, request: &ResolutionRequest) -> Result<ResolutionResult, ResolutionError>;
}

pub struct RegistryResolver<'a> {
    registry: &'a PackageRegistry,
}

impl<'a> RegistryResolver<'a> {
    #[must_use]
    pub const fn new(registry: &'a PackageRegistry) -> Self {
        Self { registry }
    }

    fn score(
        entry: &LexicalEntry,
        sense: &LexicalSense,
        request: &ResolutionRequest,
    ) -> Option<SenseCandidate> {
        if (!request.policy.allow_deprecated && (entry.deprecated || sense.deprecated))
            || (request.policy.require_evidence && sense.evidence.is_empty())
        {
            return None;
        }

        let exact_lemma = if request.policy.case_sensitive {
            entry.lemma == request.surface
        } else {
            entry.lemma.eq_ignore_ascii_case(&request.surface)
        };
        let exact_form = entry.forms.iter().any(|form| {
            if request.policy.case_sensitive {
                form.written == request.surface
            } else {
                form.written.eq_ignore_ascii_case(&request.surface)
            }
        });
        if !exact_lemma && !exact_form {
            return None;
        }

        let mut score: u32 = if entry.lemma == request.surface {
            8_500
        } else if exact_lemma {
            8_000
        } else if entry
            .forms
            .iter()
            .any(|form| form.written == request.surface)
        {
            7_750
        } else {
            7_250
        };
        let mut rationale = vec![if exact_lemma {
            "matched lemma".to_owned()
        } else {
            "matched lexical form".to_owned()
        }];

        let matching_domains = u32::try_from(
            sense
                .usage
                .domains
                .intersection(&request.domain_hints)
                .count(),
        )
        .unwrap_or(u32::MAX);
        if matching_domains > 0 {
            score = score.saturating_add((matching_domains * 350).min(1_050));
            rationale.push(format!("matched {matching_domains} domain hint(s)"));
        }

        if let SemanticTarget::Concept { id } = &sense.target {
            if request.context_concepts.contains(id) {
                score = score.saturating_add(600);
                rationale.push("matched a context concept".to_owned());
            }
        }

        if !sense.evidence.is_empty() {
            score = score.saturating_add(250);
            rationale.push("sense has evidence".to_owned());
        }

        if sense.usage.requires_quotation && request.reference_mode != ReferenceMode::Quotation {
            score = score.saturating_sub(1_000);
            rationale.push("usage normally requires quotation".to_owned());
        }

        Some(SenseCandidate {
            entry: entry.id.clone(),
            sense: sense.id.clone(),
            target: sense.target.clone(),
            score_basis_points: u16::try_from(score.min(10_000)).unwrap_or(10_000),
            evidence: sense.evidence.clone(),
            rationale,
        })
    }
}

impl LexicalResolver for RegistryResolver<'_> {
    fn resolve(&self, request: &ResolutionRequest) -> Result<ResolutionResult, ResolutionError> {
        if request.surface.trim().is_empty() {
            return Err(ResolutionError::EmptySurface);
        }
        self.registry.validate_snapshot(&request.snapshot)?;
        let lexicons = self.registry.lexicons(&request.snapshot)?;
        let mut candidates = Vec::new();

        for lexicon in lexicons {
            for entry in lexicon.entries_for_surface(
                &request.language,
                &request.surface,
                request.policy.case_sensitive,
            ) {
                for sense_id in &entry.senses {
                    let Some(sense) = lexicon.senses.get(sense_id) else {
                        continue;
                    };
                    if let Some(candidate) = Self::score(entry, sense, request) {
                        candidates.push(candidate);
                    }
                }
            }
        }

        candidates.sort_by(|left, right| {
            right
                .score_basis_points
                .cmp(&left.score_basis_points)
                .then_with(|| left.sense.cmp(&right.sense))
        });
        candidates.dedup_by(|left, right| left.sense == right.sense);
        candidates.truncate(request.policy.max_candidates.max(1));

        let Some(best) = candidates.first().cloned() else {
            return Ok(ResolutionResult::Unknown {
                surface: request.surface.clone(),
                language: request.language.clone(),
                reference_mode: request.reference_mode,
            });
        };

        let ambiguous = candidates.get(1).is_some_and(|second| {
            best.score_basis_points
                .saturating_sub(second.score_basis_points)
                <= request.policy.ambiguity_margin
        });

        if ambiguous {
            Ok(ResolutionResult::Ambiguous {
                surface: request.surface.clone(),
                candidates,
                reference_mode: request.reference_mode,
            })
        } else {
            Ok(ResolutionResult::Resolved {
                surface: request.surface.clone(),
                candidate: best,
                reference_mode: request.reference_mode,
            })
        }
    }
}

#[derive(Debug, Error)]
pub enum ResolutionError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error("resolution surface must not be empty")]
    EmptySurface,
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use muse_core::{
        ContentDigest, DigestAlgorithm, PackageHeader, PackageId, PackageKind, PackageRef,
        PackageVersion,
    };
    use muse_lexicon::{LexicalEntry, LexicalSense, LexiconPackage, PartOfSpeech, UsageConstraint};
    use muse_registry::SemanticPackage;

    use super::*;

    fn header() -> PackageHeader {
        PackageHeader {
            package: PackageRef {
                id: PackageId::from("muse.test.lexicon"),
                version: PackageVersion::from("0.1.0"),
                digest: ContentDigest {
                    algorithm: DigestAlgorithm::Sha256,
                    value: "a".repeat(64),
                },
            },
            kind: PackageKind::Lexicon,
            title: "Test lexicon".to_owned(),
            description: "Test package".to_owned(),
            license: "CC0-1.0".to_owned(),
            imports: Vec::new(),
            evidence: Vec::new(),
        }
    }

    #[test]
    fn preserves_equal_scoring_polysemy_as_ambiguity() {
        let entry_id = LexicalEntryId::from("entry:bank");
        let finance = LexicalSenseId::from("sense:bank:finance");
        let river = LexicalSenseId::from("sense:bank:river");
        let usage = UsageConstraint {
            domains: BTreeSet::new(),
            registers: BTreeSet::new(),
            communities: BTreeSet::new(),
            jurisdictions: BTreeSet::new(),
            temporal_scope: None,
            requires_quotation: false,
            notes: Vec::new(),
        };
        let package = LexiconPackage {
            header: header(),
            ontology_packages: BTreeSet::new(),
            entries: BTreeMap::from([(
                entry_id.clone(),
                LexicalEntry {
                    id: entry_id.clone(),
                    language: LanguageTag::from("en"),
                    lemma: "bank".to_owned(),
                    part_of_speech: PartOfSpeech::Noun,
                    forms: Vec::new(),
                    senses: BTreeSet::from([finance.clone(), river.clone()]),
                    deprecated: false,
                    notes: Vec::new(),
                },
            )]),
            senses: BTreeMap::from([
                (
                    finance.clone(),
                    LexicalSense {
                        id: finance,
                        entry: entry_id.clone(),
                        definition: "A financial institution.".to_owned(),
                        target: SemanticTarget::Concept {
                            id: ConceptId::from("example:FinancialInstitution"),
                        },
                        usage: usage.clone(),
                        relations: Vec::new(),
                        evidence: BTreeSet::new(),
                        deprecated: false,
                    },
                ),
                (
                    river.clone(),
                    LexicalSense {
                        id: river,
                        entry: entry_id,
                        definition: "The side of a river.".to_owned(),
                        target: SemanticTarget::Concept {
                            id: ConceptId::from("example:RiverBank"),
                        },
                        usage,
                        relations: Vec::new(),
                        evidence: BTreeSet::new(),
                        deprecated: false,
                    },
                ),
            ]),
            frames: BTreeMap::new(),
            attestations: BTreeMap::new(),
        };
        let mut registry = PackageRegistry::new();
        let package_ref = registry
            .register(SemanticPackage::Lexicon(package).seal().unwrap())
            .unwrap();
        let request = ResolutionRequest {
            surface: "bank".to_owned(),
            language: LanguageTag::from("en"),
            snapshot: registry.snapshot([package_ref]).unwrap(),
            domain_hints: BTreeSet::new(),
            context_concepts: BTreeSet::new(),
            reference_mode: ReferenceMode::Use,
            policy: ResolutionPolicy::default(),
        };
        let result = RegistryResolver::new(&registry).resolve(&request).unwrap();
        assert!(matches!(result, ResolutionResult::Ambiguous { .. }));
    }
}
