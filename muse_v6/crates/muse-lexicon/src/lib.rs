//! Controlled lexicalization metadata: canonical labels, forms, senses, frames, attestations, and usage constraints.
//! This crate is supporting ontology metadata; it is not Muse's unrestricted prose parser.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use muse_core::{
    AttestationId, ConceptId, EvidenceId, FrameId, HeaderError, IdentityError, LanguageTag,
    LexicalEntryId, LexicalSenseId, PackageHeader, PackageKind, PackageRef, RelationId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartOfSpeech {
    Noun,
    Verb,
    Adjective,
    Adverb,
    Adposition,
    Determiner,
    Pronoun,
    Conjunction,
    Particle,
    Interjection,
    MultiwordExpression,
    Symbol,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormKind {
    Lemma,
    Inflected,
    Variant,
    Abbreviation,
    Acronym,
    Symbol,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LexicalForm {
    pub written: String,
    pub kind: FormKind,
    pub features: BTreeMap<String, String>,
    pub script: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LexicalEntry {
    pub id: LexicalEntryId,
    pub language: LanguageTag,
    pub lemma: String,
    pub part_of_speech: PartOfSpeech,
    pub forms: Vec<LexicalForm>,
    pub senses: BTreeSet<LexicalSenseId>,
    pub deprecated: bool,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case")]
pub enum SemanticTarget {
    Concept { id: ConceptId },
    Relation { id: RelationId },
    FormalSymbol { id: String },
    Template { id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SenseRelationKind {
    Broader,
    Narrower,
    Related,
    Antonym,
    MetaphoricalExtension,
    HistoricalPredecessor,
    TranslationEquivalent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SenseRelation {
    pub kind: SenseRelationKind,
    pub target: LexicalSenseId,
    pub evidence: BTreeSet<EvidenceId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageConstraint {
    pub domains: BTreeSet<String>,
    pub registers: BTreeSet<String>,
    pub communities: BTreeSet<String>,
    pub jurisdictions: BTreeSet<String>,
    pub temporal_scope: Option<String>,
    pub requires_quotation: bool,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LexicalSense {
    pub id: LexicalSenseId,
    pub entry: LexicalEntryId,
    pub definition: String,
    pub target: SemanticTarget,
    pub usage: UsageConstraint,
    pub relations: Vec<SenseRelation>,
    pub evidence: BTreeSet<EvidenceId>,
    pub deprecated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionConstraint {
    pub required_type: ConceptId,
    pub allow_subtypes: bool,
    pub negated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameArgument {
    pub id: String,
    pub semantic_role: String,
    pub syntactic_realizations: BTreeSet<String>,
    pub selection: Vec<SelectionConstraint>,
    pub optional: bool,
    pub repeated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FramePattern {
    pub pattern: String,
    pub voice: Option<String>,
    pub construction: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredicateFrame {
    pub id: FrameId,
    pub sense: LexicalSenseId,
    pub arguments: Vec<FrameArgument>,
    pub patterns: Vec<FramePattern>,
    pub presuppositions: Vec<String>,
    pub result_conditions: Vec<String>,
    pub evidence: BTreeSet<EvidenceId>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExampleKind {
    Positive,
    Negative,
    Boundary,
    Metaphorical,
    Mention,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestation {
    pub id: AttestationId,
    pub sense: LexicalSenseId,
    pub kind: ExampleKind,
    pub excerpt: String,
    pub source: String,
    pub locator: Option<String>,
    pub register: Option<String>,
    pub domain: Option<String>,
    pub annotations: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LexiconPackage {
    pub header: PackageHeader,
    pub ontology_packages: BTreeSet<PackageRef>,
    pub entries: BTreeMap<LexicalEntryId, LexicalEntry>,
    pub senses: BTreeMap<LexicalSenseId, LexicalSense>,
    pub frames: BTreeMap<FrameId, PredicateFrame>,
    pub attestations: BTreeMap<AttestationId, Attestation>,
}

impl LexiconPackage {
    pub fn validate(&self) -> Result<(), LexiconValidationError> {
        self.header.validate()?;
        if self.header.kind != PackageKind::Lexicon {
            return Err(LexiconValidationError::WrongPackageKind);
        }
        for package in &self.ontology_packages {
            package.validate()?;
        }

        for (key, entry) in &self.entries {
            if key != &entry.id {
                return Err(LexiconValidationError::EntryIdentityMismatch {
                    key: key.clone(),
                    declared: entry.id.clone(),
                });
            }
            key.validate()?;
            entry.language.validate()?;
            validate_text("lexical lemma", &entry.lemma)?;
            let mut seen_forms = BTreeSet::new();
            for form in &entry.forms {
                validate_text("lexical form", &form.written)?;
                let signature = (
                    form.written.clone(),
                    form.kind.clone(),
                    form.features.clone(),
                );
                if !seen_forms.insert(signature) {
                    return Err(LexiconValidationError::DuplicateForm(key.clone()));
                }
            }
            for sense in &entry.senses {
                if !self.senses.contains_key(sense) {
                    return Err(LexiconValidationError::UnknownEntrySense {
                        entry: key.clone(),
                        sense: sense.clone(),
                    });
                }
            }
        }

        for (key, sense) in &self.senses {
            if key != &sense.id {
                return Err(LexiconValidationError::SenseIdentityMismatch {
                    key: key.clone(),
                    declared: sense.id.clone(),
                });
            }
            key.validate()?;
            validate_text("sense definition", &sense.definition)?;
            let Some(entry) = self.entries.get(&sense.entry) else {
                return Err(LexiconValidationError::UnknownSenseEntry {
                    sense: key.clone(),
                    entry: sense.entry.clone(),
                });
            };
            if !entry.senses.contains(key) {
                return Err(LexiconValidationError::SenseMissingFromEntry {
                    sense: key.clone(),
                    entry: sense.entry.clone(),
                });
            }
            for relation in &sense.relations {
                if !self.senses.contains_key(&relation.target) {
                    return Err(LexiconValidationError::UnknownSenseRelationTarget {
                        sense: key.clone(),
                        target: relation.target.clone(),
                    });
                }
            }
        }

        for (key, frame) in &self.frames {
            if key != &frame.id {
                return Err(LexiconValidationError::FrameIdentityMismatch {
                    key: key.clone(),
                    declared: frame.id.clone(),
                });
            }
            key.validate()?;
            if !self.senses.contains_key(&frame.sense) {
                return Err(LexiconValidationError::UnknownFrameSense {
                    frame: key.clone(),
                    sense: frame.sense.clone(),
                });
            }
            if frame.arguments.is_empty() {
                return Err(LexiconValidationError::FrameHasNoArguments(key.clone()));
            }
            let mut roles = BTreeSet::new();
            for argument in &frame.arguments {
                validate_text("frame argument id", &argument.id)?;
                validate_text("semantic role", &argument.semantic_role)?;
                if !roles.insert(argument.id.clone()) {
                    return Err(LexiconValidationError::DuplicateFrameArgument {
                        frame: key.clone(),
                        argument: argument.id.clone(),
                    });
                }
                for selection in &argument.selection {
                    selection.required_type.validate()?;
                }
            }
            for pattern in &frame.patterns {
                validate_text("frame pattern", &pattern.pattern)?;
            }
        }

        for (key, attestation) in &self.attestations {
            if key != &attestation.id {
                return Err(LexiconValidationError::AttestationIdentityMismatch {
                    key: key.clone(),
                    declared: attestation.id.clone(),
                });
            }
            key.validate()?;
            if !self.senses.contains_key(&attestation.sense) {
                return Err(LexiconValidationError::UnknownAttestationSense {
                    attestation: key.clone(),
                    sense: attestation.sense.clone(),
                });
            }
            validate_text("attestation excerpt", &attestation.excerpt)?;
            validate_text("attestation source", &attestation.source)?;
        }

        Ok(())
    }

    #[must_use]
    pub fn package_ref(&self) -> &PackageRef {
        &self.header.package
    }

    #[must_use]
    pub fn entries_for_surface<'a>(
        &'a self,
        language: &LanguageTag,
        surface: &str,
        case_sensitive: bool,
    ) -> Vec<&'a LexicalEntry> {
        self.entries
            .values()
            .filter(|entry| &entry.language == language)
            .filter(|entry| {
                text_matches(&entry.lemma, surface, case_sensitive)
                    || entry
                        .forms
                        .iter()
                        .any(|form| text_matches(&form.written, surface, case_sensitive))
            })
            .collect()
    }
}

fn text_matches(left: &str, right: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        left == right
    } else {
        left.eq_ignore_ascii_case(right)
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

#[derive(Debug, Error)]
pub enum LexiconValidationError {
    #[error(transparent)]
    Header(#[from] HeaderError),
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    Package(#[from] muse_core::PackageRefError),
    #[error("lexicon package header has a non-lexicon kind")]
    WrongPackageKind,
    #[error("entry map key {key} contains declaration {declared}")]
    EntryIdentityMismatch {
        key: LexicalEntryId,
        declared: LexicalEntryId,
    },
    #[error("sense map key {key} contains declaration {declared}")]
    SenseIdentityMismatch {
        key: LexicalSenseId,
        declared: LexicalSenseId,
    },
    #[error("frame map key {key} contains declaration {declared}")]
    FrameIdentityMismatch { key: FrameId, declared: FrameId },
    #[error("attestation map key {key} contains declaration {declared}")]
    AttestationIdentityMismatch {
        key: AttestationId,
        declared: AttestationId,
    },
    #[error("entry {0} repeats a lexical form")]
    DuplicateForm(LexicalEntryId),
    #[error("entry {entry} references unknown sense {sense}")]
    UnknownEntrySense {
        entry: LexicalEntryId,
        sense: LexicalSenseId,
    },
    #[error("sense {sense} belongs to unknown entry {entry}")]
    UnknownSenseEntry {
        sense: LexicalSenseId,
        entry: LexicalEntryId,
    },
    #[error("sense {sense} is not listed under entry {entry}")]
    SenseMissingFromEntry {
        sense: LexicalSenseId,
        entry: LexicalEntryId,
    },
    #[error("sense {sense} references unknown related sense {target}")]
    UnknownSenseRelationTarget {
        sense: LexicalSenseId,
        target: LexicalSenseId,
    },
    #[error("frame {frame} references unknown sense {sense}")]
    UnknownFrameSense {
        frame: FrameId,
        sense: LexicalSenseId,
    },
    #[error("frame {0} has no arguments")]
    FrameHasNoArguments(FrameId),
    #[error("frame {frame} repeats argument id {argument}")]
    DuplicateFrameArgument { frame: FrameId, argument: String },
    #[error("attestation {attestation} references unknown sense {sense}")]
    UnknownAttestationSense {
        attestation: AttestationId,
        sense: LexicalSenseId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use muse_core::{ContentDigest, DigestAlgorithm, PackageId, PackageVersion};

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
            description: "Test lexicon package".to_owned(),
            license: "CC0-1.0".to_owned(),
            imports: Vec::new(),
            evidence: Vec::new(),
        }
    }

    #[test]
    fn validates_entry_to_sense_back_reference() {
        let entry_id = LexicalEntryId::from("entry:bank");
        let sense_id = LexicalSenseId::from("sense:bank:finance");
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
                    senses: BTreeSet::from([sense_id.clone()]),
                    deprecated: false,
                    notes: Vec::new(),
                },
            )]),
            senses: BTreeMap::from([(
                sense_id.clone(),
                LexicalSense {
                    id: sense_id,
                    entry: entry_id,
                    definition: "A financial institution.".to_owned(),
                    target: SemanticTarget::Concept {
                        id: ConceptId::from("example:FinancialInstitution"),
                    },
                    usage: UsageConstraint {
                        domains: BTreeSet::new(),
                        registers: BTreeSet::new(),
                        communities: BTreeSet::new(),
                        jurisdictions: BTreeSet::new(),
                        temporal_scope: None,
                        requires_quotation: false,
                        notes: Vec::new(),
                    },
                    relations: Vec::new(),
                    evidence: BTreeSet::new(),
                    deprecated: false,
                },
            )]),
            frames: BTreeMap::new(),
            attestations: BTreeMap::new(),
        };
        assert!(package.validate().is_ok());
    }
}
