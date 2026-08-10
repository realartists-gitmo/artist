//! Lossless bridge from Artist's canonical session log into Muse's pinned
//! Artist adapter.
//!
//! The bridge deliberately converts the captured [`crate::Envelope`] values,
//! not a regenerated transcript. Editing `transcript.md` therefore cannot
//! alter Muse input or retarget provenance.

use artist_empirical::ModelId;
use muse_artist_adapter::{
    ARTIST_SESSION_SCHEMA_VERSION, ArtistAdapterError, ArtistEnvelope, ArtistEventFormalizer,
    ArtistNormalization, ArtistNormalizer,
};
use muse_core::{ConceptId, OccurrenceDocumentId, RelationId, SemanticObjectId};
use muse_occurrence::{
    Derivation, OccurrenceDocument, PresentationMode, PropositionExpr, StatementBasis, Term,
};
use muse_ontology::Axiom;
use muse_reasoning::{Fact, KnowledgeBase};
use muse_registry::RegistrySnapshot;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// A failed conversion or normalization of captured Artist events.
#[derive(Debug, Error)]
pub enum MuseCaptureError {
    #[error(
        "Artist session schema {actual} is unsupported by the pinned Muse adapter (expected {expected})"
    )]
    UnsupportedSchema { actual: u32, expected: u32 },
    #[error("Muse Artist adapter rejected captured event sequence: {0}")]
    Adapter(#[from] ArtistAdapterError),
}

/// Durable report for a Muse ingestion failure. Recording this report is
/// intentionally separate from the Artist operation that triggered capture:
/// callers can make diagnostics best-effort and never fail a successful file
/// read, edit, or write because Muse is temporarily unavailable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuseDiagnostic {
    pub id: String,
    pub operation: String,
    pub message: String,
    pub recorded_at_millis: u128,
}

/// Best-effort Muse ingestion outcome. Ingestion failures are data for the
/// diagnostic projection, never a reason to retroactively fail the Artist
/// read/edit/write or event capture that supplied the source.
#[derive(Clone, Debug)]
pub struct MuseIngestion {
    pub documents: Vec<OccurrenceDocument>,
    pub facts: Vec<MuseExplicitFacts>,
    pub findings: Vec<MuseFormalFinding>,
    pub diagnostic: Option<MuseDiagnostic>,
}

/// Source-explicit facts deterministically projected from an occurrence
/// document. Unsupported proposition forms remain out of the fact base rather
/// than being approximated into logical claims.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuseExplicitFacts {
    pub document: OccurrenceDocumentId,
    pub facts: KnowledgeBase,
}

/// The only authorities allowed to introduce a runtime normative rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MuseRuleAuthority {
    ProjectDocument(String),
    Profile(String),
}

/// A source-grounded rule over exact formal facts. The rule is applicable only
/// when every condition is present in the current retained fact corpus.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuseFormalRule {
    /// Canonical content-derived identity, checked on write.
    pub id: String,
    pub authority: MuseRuleAuthority,
    /// Formalized documents that establish the rule text/authority.
    pub source_documents: BTreeSet<OccurrenceDocumentId>,
    pub conditions: BTreeSet<Fact>,
    #[serde(default)]
    pub include_model_labels: bool,
    /// Compact context text emitted if the exact conditions apply.
    pub message: String,
}

/// One formal applicability result. A finding is explicitly a rule-derived
/// conclusion rather than an observed or Bayesian fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuseFormalFinding {
    pub id: String,
    pub rule: String,
    pub message: String,
    pub supporting_documents: BTreeSet<OccurrenceDocumentId>,
}

/// Explicit Beta–Bernoulli prior for one revisable hypothesis. The model pin
/// makes each posterior's assumptions inspectable after implementation changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuseBayesianPrior {
    pub id: String,
    pub proposition: String,
    pub model: ModelId,
    pub prior_model_pin: String,
    pub support_microunits: u64,
    pub contrary_microunits: u64,
}

/// One immutable source-grounded update to a pinned Bayesian prior.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuseBayesianObservation {
    pub id: String,
    pub prior: String,
    pub supports: bool,
    pub confidence_ppm: u32,
    pub source_documents: BTreeSet<OccurrenceDocumentId>,
}

/// Model-relative posterior predictive estimate, not a formal fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuseBayesianEstimate {
    pub id: String,
    pub prior: String,
    pub model: ModelId,
    pub prior_model_pin: String,
    pub support_microunits: u64,
    pub contrary_microunits: u64,
    pub posterior_support_ppm: u32,
    pub supporting_observations: BTreeSet<String>,
    pub contrary_observations: BTreeSet<String>,
}

/// A model-produced semantic label. Labels remain separately attributable from
/// deterministic source facts until a rule explicitly consumes them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuseModelLabel {
    pub id: String,
    pub model: String,
    pub source_documents: BTreeSet<OccurrenceDocumentId>,
    pub document: OccurrenceDocument,
}

/// A not-yet-promoted ontology extension. Candidates are deliberately stored
/// outside the immutable registry: they can be evaluated against the corpus,
/// but cannot alter global semantic results until a separate promotion action
/// seals and registers an ontology package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MuseOntologyCandidateKind {
    Concept {
        id: ConceptId,
        parents: BTreeSet<ConceptId>,
    },
    Relation {
        id: RelationId,
        parents: BTreeSet<RelationId>,
    },
}

/// Durable, source-grounded proposal for one ontology extension.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuseOntologyCandidate {
    /// Content-derived opaque identifier. It is verified on every write.
    pub id: String,
    pub candidate: MuseOntologyCandidateKind,
    /// Rules proposed alongside the new term. They remain inert while this is
    /// a candidate rather than part of a sealed registry package.
    pub declared_rules: Vec<Axiom>,
    /// Formalized source events that ground this proposal.
    pub source_documents: BTreeSet<OccurrenceDocumentId>,
    /// Compact source-grounded examples, retained verbatim for later review.
    pub examples: Vec<String>,
    /// Deterministic acceptance tests required before any promotion proposal.
    pub tests: Vec<String>,
}

/// Candidate validation or persistence failure.
#[derive(Debug, Error)]
pub enum MuseCandidateError {
    #[error("ontology candidate must declare at least one parent")]
    MissingParentage,
    #[error("ontology candidate must declare at least one rule")]
    MissingRules,
    #[error("ontology candidate must include source-grounded examples")]
    MissingExamples,
    #[error("ontology candidate must include deterministic tests")]
    MissingTests,
    #[error("ontology candidate must cite at least one retained occurrence document")]
    MissingEvidence,
    #[error("ontology candidate identity is not its canonical content-derived id")]
    IdMismatch,
    #[error("candidate source document {0} is not retained")]
    MissingDocument(OccurrenceDocumentId),
    #[error("candidate source document {id} is invalid: {message}")]
    InvalidDocument {
        id: OccurrenceDocumentId,
        message: String,
    },
    #[error("candidate text field must be nonempty and trimmed")]
    InvalidText,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Formal-rule validation, evaluation, or persistence failure.
#[derive(Debug, Error)]
pub enum MuseRuleError {
    #[error("formal rule must cite at least one retained occurrence document")]
    MissingEvidence,
    #[error("formal rule must contain at least one typed condition")]
    MissingConditions,
    #[error("formal rule message must be nonempty and trimmed")]
    InvalidMessage,
    #[error("formal rule identity is not its canonical content-derived id")]
    IdMismatch,
    #[error("formal rule source document {0} is not retained")]
    MissingDocument(OccurrenceDocumentId),
    #[error("formal rule source document {id} is invalid: {message}")]
    InvalidDocument {
        id: OccurrenceDocumentId,
        message: String,
    },
    #[error("stored formal rule {path} is invalid: {message}")]
    InvalidStoredRule { path: PathBuf, message: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum MuseBayesianError {
    #[error("Bayesian identity is not its canonical content-derived id")]
    IdMismatch,
    #[error("Bayesian proposition/model/pin text must be nonempty and trimmed")]
    InvalidText,
    #[error("Bayesian priors must assign positive support and contrary mass")]
    InvalidPrior,
    #[error("Bayesian observations must cite source documents and have nonzero confidence")]
    InvalidObservation,
    #[error("Bayesian prior {0} is not retained")]
    MissingPrior(String),
    #[error("Bayesian source document {0} is not retained")]
    MissingDocument(OccurrenceDocumentId),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum MuseLabelError {
    #[error("model label must name a model and cite source documents")]
    MissingProvenance,
    #[error("model label identity is not its canonical content-derived id")]
    IdMismatch,
    #[error("model label derivation does not match its declared model")]
    DerivationMismatch,
    #[error("model label source document {0} is not retained")]
    MissingDocument(OccurrenceDocumentId),
    #[error("model label source document {id} is invalid: {message}")]
    InvalidSource {
        id: OccurrenceDocumentId,
        message: String,
    },
    #[error("model label ontology snapshot differs from source document {0}")]
    SnapshotMismatch(OccurrenceDocumentId),
    #[error("model label document is invalid: {0}")]
    InvalidDocument(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Project-local storage backing `memory://diagnostics/<id>`.
pub fn muse_diagnostics_dir(project: &Path) -> PathBuf {
    project.join(".artist/state/muse/diagnostics")
}

/// Project-local immutable occurrence documents emitted by successful Muse
/// ingestion. The document id is semantic and source-derived, so re-ingesting
/// the same captured Artist record is idempotent without needing an actor-local
/// allocation table.
pub fn muse_documents_dir(project: &Path) -> PathBuf {
    project.join(".artist/state/muse/documents")
}

/// Project-local, inert candidate ontology proposals.
pub fn muse_candidates_dir(project: &Path) -> PathBuf {
    project.join(".artist/state/muse/candidates")
}

/// Project-local deterministic fact bases, one for each retained occurrence
/// document. They index logical memory but do not replace source provenance.
pub fn muse_facts_dir(project: &Path) -> PathBuf {
    project.join(".artist/state/muse/facts")
}

/// Project-local source-grounded rules awaiting exact formal applicability.
pub fn muse_rules_dir(project: &Path) -> PathBuf {
    project.join(".artist/state/muse/rules")
}

/// Project-local, rule-derived findings. They are retained separately from
/// source facts so callers cannot mistake a conclusion for direct evidence.
pub fn muse_findings_dir(project: &Path) -> PathBuf {
    project.join(".artist/state/muse/findings")
}

pub fn muse_labels_dir(project: &Path) -> PathBuf {
    project.join(".artist/state/muse/labels")
}

pub fn muse_bayesian_priors_dir(project: &Path) -> PathBuf {
    project.join(".artist/state/muse/bayesian/priors")
}

pub fn muse_bayesian_observations_dir(project: &Path) -> PathBuf {
    project.join(".artist/state/muse/bayesian/observations")
}

pub fn muse_bayesian_estimates_dir(project: &Path) -> PathBuf {
    project.join(".artist/state/muse/bayesian/estimates")
}

/// Project only explicit, two-referent type/relation statements into the
/// neutral reasoner representation. This is intentionally a narrow funnel:
/// modality, negation, literals, n-ary relations, questions, commands, and
/// inferred/model-generated structure never become global facts here.
#[must_use]
pub fn explicit_facts_for_muse(document: &OccurrenceDocument) -> MuseExplicitFacts {
    let mut facts = BTreeSet::new();
    for statement in document.statements.values() {
        if !matches!(statement.basis, StatementBasis::Expressed)
            || !matches!(
                statement.mode,
                PresentationMode::Record | PresentationMode::Assertion
            )
        {
            continue;
        }
        let Some(proposition) = document.propositions.get(&statement.content) else {
            continue;
        };
        match &proposition.expression {
            PropositionExpr::TypeAssertion {
                subject: Term::Referent(subject),
                r#type,
            } => {
                facts.insert(Fact::InstanceOf {
                    individual: SemanticObjectId::from(subject.as_str()),
                    concept: r#type.clone(),
                });
            }
            PropositionExpr::Relation {
                relation,
                arguments,
            } if arguments.len() == 2 => {
                let (Term::Referent(subject), Term::Referent(object)) =
                    (&arguments[0], &arguments[1])
                else {
                    continue;
                };
                facts.insert(Fact::Relation {
                    subject: SemanticObjectId::from(subject.as_str()),
                    relation: relation.clone(),
                    object: SemanticObjectId::from(object.as_str()),
                });
            }
            _ => {}
        }
    }
    MuseExplicitFacts {
        document: document.id.clone(),
        facts: KnowledgeBase { facts },
    }
}

/// Atomically retain explicit fact projections. An existing byte-identical
/// projection is idempotent; a different projection for one document id is a
/// collision rather than a silent provenance rewrite.
pub fn write_muse_explicit_facts(
    project: &Path,
    facts: &[MuseExplicitFacts],
) -> std::io::Result<Vec<PathBuf>> {
    let directory = muse_facts_dir(project);
    std::fs::create_dir_all(&directory)?;
    let mut paths = Vec::with_capacity(facts.len());
    for fact_base in facts {
        let id = fact_base.document.to_string();
        let target = directory.join(format!("{id}.json"));
        let bytes = serde_json::to_vec_pretty(fact_base).map_err(std::io::Error::other)?;
        match std::fs::read(&target) {
            Ok(existing) if existing == bytes => {}
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("Muse explicit-fact id collision: {id}"),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let temporary = directory.join(format!(".{id}.tmp"));
                std::fs::write(&temporary, bytes)?;
                std::fs::rename(temporary, &target)?;
            }
            Err(error) => return Err(error),
        }
        paths.push(target);
    }
    Ok(paths)
}

impl MuseFormalRule {
    /// Compute the deterministic identity without trusting the stored id.
    pub fn content_id(&self) -> Result<String, MuseRuleError> {
        #[derive(Serialize)]
        struct RuleContent<'a> {
            authority: &'a MuseRuleAuthority,
            source_documents: &'a BTreeSet<OccurrenceDocumentId>,
            conditions: &'a BTreeSet<Fact>,
            include_model_labels: bool,
            message: &'a str,
        }
        let bytes = serde_json::to_vec(&RuleContent {
            authority: &self.authority,
            source_documents: &self.source_documents,
            conditions: &self.conditions,
            include_model_labels: self.include_model_labels,
            message: &self.message,
        })?;
        Ok(format!("rule-{}", hex_digest(&bytes)))
    }

    /// Confirm source provenance and exact rule shape before persistence.
    pub fn validate(&self, project: &Path) -> Result<(), MuseRuleError> {
        if self.source_documents.is_empty() {
            return Err(MuseRuleError::MissingEvidence);
        }
        if self.conditions.is_empty() {
            return Err(MuseRuleError::MissingConditions);
        }
        if self.message.is_empty() || self.message.trim() != self.message {
            return Err(MuseRuleError::InvalidMessage);
        }
        if self.id != self.content_id()? {
            return Err(MuseRuleError::IdMismatch);
        }
        for fact in &self.conditions {
            fact.validate().map_err(|_| MuseRuleError::InvalidMessage)?;
        }
        validate_rule_documents(project, &self.source_documents)
    }
}

/// Atomically retain one source-grounded formal rule.
pub fn write_muse_formal_rule(
    project: &Path,
    rule: &MuseFormalRule,
) -> Result<PathBuf, MuseRuleError> {
    rule.validate(project)?;
    Ok(write_immutable_json(
        muse_rules_dir(project),
        &rule.id,
        rule,
    )?)
}

/// Evaluate all retained rules against the exact union of retained explicit
/// facts, persist every applicable conclusion, and return that conclusion set.
/// No vector search, turn count, or text resemblance participates in matching.
pub fn evaluate_muse_formal_rules(project: &Path) -> Result<Vec<MuseFormalFinding>, MuseRuleError> {
    let fact_bases = read_json_records::<MuseExplicitFacts>(muse_facts_dir(project))?;
    let mut facts = BTreeSet::new();
    let mut documents_by_fact: std::collections::BTreeMap<Fact, BTreeSet<OccurrenceDocumentId>> =
        std::collections::BTreeMap::new();
    for fact_base in fact_bases {
        for fact in fact_base.facts.facts {
            facts.insert(fact.clone());
            documents_by_fact
                .entry(fact)
                .or_default()
                .insert(fact_base.document.clone());
        }
    }
    let mut label_facts = BTreeSet::new();
    let mut label_documents: std::collections::BTreeMap<Fact, BTreeSet<OccurrenceDocumentId>> =
        std::collections::BTreeMap::new();
    for label in read_json_records::<MuseModelLabel>(muse_labels_dir(project))? {
        label
            .validate(project)
            .map_err(|error| MuseRuleError::InvalidStoredRule {
                path: muse_labels_dir(project),
                message: error.to_string(),
            })?;
        for fact in explicit_facts_for_muse(&label.document).facts.facts {
            label_facts.insert(fact.clone());
            label_documents
                .entry(fact)
                .or_default()
                .extend(label.source_documents.iter().cloned());
        }
    }
    let mut findings = Vec::new();
    for (path, rule) in read_json_records_with_paths::<MuseFormalRule>(muse_rules_dir(project))? {
        rule.validate(project)
            .map_err(|error| MuseRuleError::InvalidStoredRule {
                path: path.clone(),
                message: error.to_string(),
            })?;
        let mut applicable = facts.clone();
        let mut supporting = documents_by_fact.clone();
        if rule.include_model_labels {
            applicable.extend(label_facts.iter().cloned());
            for (fact, documents) in &label_documents {
                supporting
                    .entry(fact.clone())
                    .or_default()
                    .extend(documents.iter().cloned());
            }
        }
        if !rule.conditions.is_subset(&applicable) {
            continue;
        }
        let supporting_documents = rule
            .conditions
            .iter()
            .flat_map(|condition| supporting.get(condition).into_iter().flatten().cloned())
            .collect::<BTreeSet<_>>();
        let bytes = serde_json::to_vec(&(rule.id.as_str(), &supporting_documents))?;
        let finding = MuseFormalFinding {
            id: format!("finding-{}", hex_digest(&bytes)),
            rule: rule.id,
            message: rule.message,
            supporting_documents,
        };
        write_immutable_json(muse_findings_dir(project), &finding.id, &finding)?;
        findings.push(finding);
    }
    Ok(findings)
}

fn validate_rule_documents(
    project: &Path,
    documents: &BTreeSet<OccurrenceDocumentId>,
) -> Result<(), MuseRuleError> {
    for document_id in documents {
        let path = muse_documents_dir(project).join(format!("{document_id}.json"));
        let source = std::fs::read_to_string(&path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => MuseRuleError::MissingDocument(document_id.clone()),
            _ => MuseRuleError::Io(error),
        })?;
        let document: OccurrenceDocument =
            serde_json::from_str(&source).map_err(|error| MuseRuleError::InvalidDocument {
                id: document_id.clone(),
                message: error.to_string(),
            })?;
        document
            .validate()
            .map_err(|error| MuseRuleError::InvalidDocument {
                id: document_id.clone(),
                message: error.to_string(),
            })?;
    }
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl MuseBayesianPrior {
    pub fn content_id(&self) -> Result<String, MuseBayesianError> {
        let bytes = serde_json::to_vec(&(
            &self.proposition,
            &self.model,
            &self.prior_model_pin,
            self.support_microunits,
            self.contrary_microunits,
        ))?;
        Ok(format!("prior-{}", hex_digest(&bytes)))
    }

    pub fn validate(&self) -> Result<(), MuseBayesianError> {
        if self.proposition.is_empty()
            || self.proposition.trim() != self.proposition
            || self.model.0.is_empty()
            || self.model.0.trim() != self.model.0
            || self.prior_model_pin.is_empty()
            || self.prior_model_pin.trim() != self.prior_model_pin
        {
            return Err(MuseBayesianError::InvalidText);
        }
        if self.support_microunits == 0 || self.contrary_microunits == 0 {
            return Err(MuseBayesianError::InvalidPrior);
        }
        if self.id != self.content_id()? {
            return Err(MuseBayesianError::IdMismatch);
        }
        Ok(())
    }
}

impl MuseBayesianObservation {
    pub fn content_id(&self) -> Result<String, MuseBayesianError> {
        let bytes = serde_json::to_vec(&(
            &self.prior,
            self.supports,
            self.confidence_ppm,
            &self.source_documents,
        ))?;
        Ok(format!("observation-{}", hex_digest(&bytes)))
    }

    pub fn validate(&self, project: &Path) -> Result<(), MuseBayesianError> {
        if self.prior.is_empty() || self.prior.trim() != self.prior {
            return Err(MuseBayesianError::InvalidText);
        }
        if self.confidence_ppm == 0 || self.source_documents.is_empty() {
            return Err(MuseBayesianError::InvalidObservation);
        }
        if self.id != self.content_id()? {
            return Err(MuseBayesianError::IdMismatch);
        }
        let prior_path = muse_bayesian_priors_dir(project).join(format!("{}.json", self.prior));
        if !prior_path.is_file() {
            return Err(MuseBayesianError::MissingPrior(self.prior.clone()));
        }
        for document in &self.source_documents {
            if !muse_documents_dir(project)
                .join(format!("{document}.json"))
                .is_file()
            {
                return Err(MuseBayesianError::MissingDocument(document.clone()));
            }
        }
        Ok(())
    }
}

pub fn write_muse_bayesian_prior(
    project: &Path,
    prior: &MuseBayesianPrior,
) -> Result<PathBuf, MuseBayesianError> {
    prior.validate()?;
    Ok(write_immutable_json(
        muse_bayesian_priors_dir(project),
        &prior.id,
        prior,
    )?)
}

/// Persist an immutable evidence observation and recompute that prior's exact
/// Beta–Bernoulli posterior predictive estimate.
pub fn write_muse_bayesian_observation(
    project: &Path,
    observation: &MuseBayesianObservation,
) -> Result<MuseBayesianEstimate, MuseBayesianError> {
    observation.validate(project)?;
    write_immutable_json(
        muse_bayesian_observations_dir(project),
        &observation.id,
        observation,
    )?;
    recompute_muse_bayesian_estimate(project, &observation.prior)
}

pub fn recompute_muse_bayesian_estimate(
    project: &Path,
    prior_id: &str,
) -> Result<MuseBayesianEstimate, MuseBayesianError> {
    let prior_path = muse_bayesian_priors_dir(project).join(format!("{prior_id}.json"));
    let bytes = std::fs::read(&prior_path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => MuseBayesianError::MissingPrior(prior_id.to_owned()),
        _ => MuseBayesianError::Io(error),
    })?;
    let prior: MuseBayesianPrior = serde_json::from_slice(&bytes)?;
    prior.validate()?;
    let mut support = prior.support_microunits;
    let mut contrary = prior.contrary_microunits;
    let mut supporting_observations = BTreeSet::new();
    let mut contrary_observations = BTreeSet::new();
    let observations =
        read_json_records::<MuseBayesianObservation>(muse_bayesian_observations_dir(project))
            .map_err(|error| MuseBayesianError::Io(std::io::Error::other(error.to_string())))?;
    for observation in observations {
        if observation.prior != prior.id {
            continue;
        }
        observation.validate(project)?;
        if observation.supports {
            support = support.saturating_add(u64::from(observation.confidence_ppm));
            supporting_observations.insert(observation.id);
        } else {
            contrary = contrary.saturating_add(u64::from(observation.confidence_ppm));
            contrary_observations.insert(observation.id);
        }
    }
    let total = support.saturating_add(contrary);
    let posterior_support_ppm = if total == 0 {
        0
    } else {
        ((support.saturating_mul(1_000_000)) / total).min(1_000_000) as u32
    };
    let estimate_bytes = serde_json::to_vec(&(
        &prior.id,
        support,
        contrary,
        &supporting_observations,
        &contrary_observations,
    ))?;
    let estimate = MuseBayesianEstimate {
        id: format!("estimate-{}", hex_digest(&estimate_bytes)),
        prior: prior.id,
        model: prior.model,
        prior_model_pin: prior.prior_model_pin,
        support_microunits: support,
        contrary_microunits: contrary,
        posterior_support_ppm,
        supporting_observations,
        contrary_observations,
    };
    write_immutable_json(
        muse_bayesian_estimates_dir(project),
        &estimate.id,
        &estimate,
    )?;
    Ok(estimate)
}

impl MuseModelLabel {
    pub fn content_id(&self) -> Result<String, MuseLabelError> {
        let bytes = serde_json::to_vec(&(&self.model, &self.source_documents, &self.document))?;
        Ok(format!("label-{}", hex_digest(&bytes)))
    }

    pub fn validate(&self, project: &Path) -> Result<(), MuseLabelError> {
        if self.model.is_empty()
            || self.model.trim() != self.model
            || self.source_documents.is_empty()
        {
            return Err(MuseLabelError::MissingProvenance);
        }
        if self.id != self.content_id()? {
            return Err(MuseLabelError::IdMismatch);
        }
        if !matches!(&self.document.derivation, Derivation::ModelGenerated { model, .. } if model == &self.model)
        {
            return Err(MuseLabelError::DerivationMismatch);
        }
        self.document
            .validate()
            .map_err(|error| MuseLabelError::InvalidDocument(error.to_string()))?;
        for source_id in &self.source_documents {
            let path = muse_documents_dir(project).join(format!("{source_id}.json"));
            let bytes = std::fs::read(&path).map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => MuseLabelError::MissingDocument(source_id.clone()),
                _ => MuseLabelError::Io(error),
            })?;
            let source: OccurrenceDocument =
                serde_json::from_slice(&bytes).map_err(|error| MuseLabelError::InvalidSource {
                    id: source_id.clone(),
                    message: error.to_string(),
                })?;
            source
                .validate()
                .map_err(|error| MuseLabelError::InvalidSource {
                    id: source_id.clone(),
                    message: error.to_string(),
                })?;
            if source.ontology != self.document.ontology {
                return Err(MuseLabelError::SnapshotMismatch(source_id.clone()));
            }
        }
        Ok(())
    }
}

/// Atomically retain a structurally valid, source-pinned model label. Model
/// labels deliberately remain a separate provenance class from direct facts.
pub fn write_muse_model_label(
    project: &Path,
    label: &MuseModelLabel,
) -> Result<PathBuf, MuseLabelError> {
    label.validate(project)?;
    Ok(write_immutable_json(
        muse_labels_dir(project),
        &label.id,
        label,
    )?)
}

fn write_immutable_json<T: Serialize>(
    directory: PathBuf,
    id: &str,
    value: &T,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(&directory)?;
    let target = directory.join(format!("{id}.json"));
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    match std::fs::read(&target) {
        Ok(existing) if existing == bytes => Ok(target),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("Muse immutable record id collision: {id}"),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let temporary = directory.join(format!(".{id}.tmp"));
            std::fs::write(&temporary, bytes)?;
            std::fs::rename(temporary, &target)?;
            Ok(target)
        }
        Err(error) => Err(error),
    }
}

fn read_json_records<T: for<'de> Deserialize<'de>>(
    directory: PathBuf,
) -> Result<Vec<T>, MuseRuleError> {
    Ok(read_json_records_with_paths(directory)?
        .into_iter()
        .map(|(_, value)| value)
        .collect())
}

fn read_json_records_with_paths<T: for<'de> Deserialize<'de>>(
    directory: PathBuf,
) -> Result<Vec<(PathBuf, T)>, MuseRuleError> {
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(MuseRuleError::Io(error)),
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let bytes = std::fs::read(&path)?;
            let value = serde_json::from_slice(&bytes)?;
            Ok((path, value))
        })
        .collect()
}

impl MuseOntologyCandidate {
    /// Compute the stable content identity independently of the stored id.
    pub fn content_id(&self) -> Result<String, MuseCandidateError> {
        #[derive(Serialize)]
        struct CandidateContent<'a> {
            candidate: &'a MuseOntologyCandidateKind,
            declared_rules: &'a [Axiom],
            source_documents: &'a BTreeSet<OccurrenceDocumentId>,
            examples: &'a [String],
            tests: &'a [String],
        }
        let bytes = serde_json::to_vec(&CandidateContent {
            candidate: &self.candidate,
            declared_rules: &self.declared_rules,
            source_documents: &self.source_documents,
            examples: &self.examples,
            tests: &self.tests,
        })?;
        let digest = Sha256::digest(bytes);
        let encoded = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(format!("candidate-{encoded}"))
    }

    /// Validate candidate structure and its exact retained evidence boundary.
    pub fn validate(&self, project: &Path) -> Result<(), MuseCandidateError> {
        match &self.candidate {
            MuseOntologyCandidateKind::Concept { id, parents } => {
                id.validate().map_err(|_| MuseCandidateError::InvalidText)?;
                if parents.is_empty() {
                    return Err(MuseCandidateError::MissingParentage);
                }
                for parent in parents {
                    parent
                        .validate()
                        .map_err(|_| MuseCandidateError::InvalidText)?;
                }
            }
            MuseOntologyCandidateKind::Relation { id, parents } => {
                id.validate().map_err(|_| MuseCandidateError::InvalidText)?;
                if parents.is_empty() {
                    return Err(MuseCandidateError::MissingParentage);
                }
                for parent in parents {
                    parent
                        .validate()
                        .map_err(|_| MuseCandidateError::InvalidText)?;
                }
            }
        }
        if self.declared_rules.is_empty() {
            return Err(MuseCandidateError::MissingRules);
        }
        if self.source_documents.is_empty() {
            return Err(MuseCandidateError::MissingEvidence);
        }
        if self.examples.is_empty() {
            return Err(MuseCandidateError::MissingExamples);
        }
        if self.tests.is_empty() {
            return Err(MuseCandidateError::MissingTests);
        }
        for value in self.examples.iter().chain(&self.tests) {
            if value.is_empty() || value.trim() != value {
                return Err(MuseCandidateError::InvalidText);
            }
        }
        if self.id != self.content_id()? {
            return Err(MuseCandidateError::IdMismatch);
        }
        for document_id in &self.source_documents {
            document_id
                .validate()
                .map_err(|_| MuseCandidateError::InvalidText)?;
            let path = muse_documents_dir(project).join(format!("{document_id}.json"));
            let source = std::fs::read_to_string(&path).map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => {
                    MuseCandidateError::MissingDocument(document_id.clone())
                }
                _ => MuseCandidateError::Io(error),
            })?;
            let document: OccurrenceDocument = serde_json::from_str(&source).map_err(|error| {
                MuseCandidateError::InvalidDocument {
                    id: document_id.clone(),
                    message: error.to_string(),
                }
            })?;
            document
                .validate()
                .map_err(|error| MuseCandidateError::InvalidDocument {
                    id: document_id.clone(),
                    message: error.to_string(),
                })?;
        }
        Ok(())
    }
}

/// Atomically write an inert ontology candidate. An existing differing record
/// is a collision/error rather than an opportunity to retarget provenance.
pub fn write_muse_candidate(
    project: &Path,
    candidate: &MuseOntologyCandidate,
) -> Result<PathBuf, MuseCandidateError> {
    candidate.validate(project)?;
    let directory = muse_candidates_dir(project);
    std::fs::create_dir_all(&directory)?;
    let target = directory.join(format!("{}.json", candidate.id));
    let bytes = serde_json::to_vec_pretty(candidate)?;
    match std::fs::read(&target) {
        Ok(existing) if existing == bytes => Ok(target),
        Ok(_) => Err(MuseCandidateError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("Muse ontology candidate id collision: {}", candidate.id),
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let temporary = directory.join(format!(".{}.tmp", candidate.id));
            std::fs::write(&temporary, bytes)?;
            std::fs::rename(temporary, &target)?;
            Ok(target)
        }
        Err(error) => Err(MuseCandidateError::Io(error)),
    }
}

/// Atomically retain every successfully formalized occurrence document.
/// Existing identical bytes are accepted; a different document claiming the
/// same id is rejected rather than silently overwriting semantic evidence.
pub fn write_muse_documents(
    project: &Path,
    documents: &[OccurrenceDocument],
) -> std::io::Result<Vec<PathBuf>> {
    let directory = muse_documents_dir(project);
    std::fs::create_dir_all(&directory)?;
    let mut paths = Vec::with_capacity(documents.len());
    for document in documents {
        let id = document.id.to_string();
        let target = directory.join(format!("{id}.json"));
        let bytes = serde_json::to_vec_pretty(document).map_err(std::io::Error::other)?;
        match std::fs::read(&target) {
            Ok(existing) if existing == bytes => {}
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("Muse occurrence document id collision: {id}"),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let temporary = directory.join(format!(".{id}.tmp"));
                std::fs::write(&temporary, bytes)?;
                std::fs::rename(temporary, &target)?;
            }
            Err(error) => return Err(error),
        }
        paths.push(target);
    }
    Ok(paths)
}

/// Persist a Muse ingestion diagnostic atomically and return its opaque id.
pub fn write_muse_diagnostic(
    project: &Path,
    operation: &str,
    error: impl std::fmt::Display,
) -> std::io::Result<MuseDiagnostic> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let diagnostic = MuseDiagnostic {
        id: id.clone(),
        operation: operation.to_owned(),
        message: error.to_string(),
        recorded_at_millis: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    };
    let dir = muse_diagnostics_dir(project);
    std::fs::create_dir_all(&dir)?;
    let target = dir.join(format!("{id}.json"));
    let temporary = dir.join(format!(".{id}.tmp"));
    let bytes = serde_json::to_vec_pretty(&diagnostic).map_err(std::io::Error::other)?;
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, target)?;
    Ok(diagnostic)
}

/// Convert one canonical Artist session envelope without interpreting its
/// payload. This is intentionally a field-for-field bridge so an unknown
/// future event remains visible to Muse as residual source material.
pub fn muse_envelope(envelope: &crate::Envelope) -> Result<ArtistEnvelope, MuseCaptureError> {
    if envelope.v != ARTIST_SESSION_SCHEMA_VERSION {
        return Err(MuseCaptureError::UnsupportedSchema {
            actual: envelope.v,
            expected: ARTIST_SESSION_SCHEMA_VERSION,
        });
    }
    Ok(ArtistEnvelope {
        v: envelope.v,
        seq: envelope.seq,
        ts: envelope.ts,
        session: envelope.session.clone(),
        run: envelope.run.clone(),
        lineage: envelope.lineage.clone(),
        kind: envelope.kind.clone(),
        payload: envelope.payload.clone(),
    })
}

/// Normalize captured Artist log events for Muse.
///
/// Events remain in their recorded order. The adapter enforces monotonic
/// sequence numbers and records unknown kinds as residuals rather than
/// discarding them.
pub fn normalize_for_muse(
    events: &[crate::Envelope],
) -> Result<ArtistNormalization, MuseCaptureError> {
    let mut normalizer = ArtistNormalizer::default();
    for event in events {
        normalizer.push(muse_envelope(event)?)?;
    }
    Ok(normalizer.finish()?)
}

/// Formalize every captured Artist event against an explicit Muse registry
/// snapshot. The caller owns snapshot selection; this bridge never silently
/// changes an event's ontology basis by consulting mutable global state.
pub fn formalize_for_muse(
    events: &[crate::Envelope],
    ontology: RegistrySnapshot,
) -> Result<Vec<OccurrenceDocument>, MuseCaptureError> {
    let normalized = normalize_for_muse(events)?;
    let formalizer = ArtistEventFormalizer { ontology };
    normalized
        .structured_events
        .iter()
        .map(|event| formalizer.formalize(event).map_err(MuseCaptureError::from))
        .collect()
}

/// Formalize captured events without allowing Muse failure to fail the
/// originating Artist operation. A diagnostic write failure is also contained:
/// callers still receive an empty semantic result and can keep progressing.
pub fn ingest_for_muse(
    project: &Path,
    operation: &str,
    events: &[crate::Envelope],
    ontology: RegistrySnapshot,
) -> MuseIngestion {
    match formalize_for_muse(events, ontology) {
        Ok(documents) => match write_muse_documents(project, &documents) {
            Ok(_) => {
                let facts = documents
                    .iter()
                    .map(explicit_facts_for_muse)
                    .collect::<Vec<_>>();
                match write_muse_explicit_facts(project, &facts) {
                    Ok(_) => match evaluate_muse_formal_rules(project) {
                        Ok(findings) => MuseIngestion {
                            documents,
                            facts,
                            findings,
                            diagnostic: None,
                        },
                        Err(error) => MuseIngestion {
                            documents: Vec::new(),
                            facts: Vec::new(),
                            findings: Vec::new(),
                            diagnostic: write_muse_diagnostic(project, operation, error).ok(),
                        },
                    },
                    Err(error) => MuseIngestion {
                        documents: Vec::new(),
                        facts: Vec::new(),
                        findings: Vec::new(),
                        diagnostic: write_muse_diagnostic(project, operation, error).ok(),
                    },
                }
            }
            Err(error) => MuseIngestion {
                documents: Vec::new(),
                facts: Vec::new(),
                findings: Vec::new(),
                diagnostic: write_muse_diagnostic(project, operation, error).ok(),
            },
        },
        Err(error) => MuseIngestion {
            documents: Vec::new(),
            facts: Vec::new(),
            findings: Vec::new(),
            diagnostic: write_muse_diagnostic(project, operation, error).ok(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(seq: u64, kind: &str, payload: serde_json::Value) -> crate::Envelope {
        crate::Envelope {
            v: crate::SCHEMA_VERSION,
            seq,
            ts: seq,
            session: "session-1".into(),
            run: Some("run-1".into()),
            lineage: crate::MAIN_LINEAGE.into(),
            kind: kind.into(),
            payload,
        }
    }

    #[test]
    fn captured_events_are_normalized_without_transcript_input() {
        let events = vec![envelope(
            1,
            "turn.user",
            serde_json::json!({"content":[{"type":"text","text":"Ship it."}]}),
        )];
        let normalized = normalize_for_muse(&events).unwrap();
        assert_eq!(normalized.structured_events.len(), 1);
        assert_eq!(normalized.prose.len(), 1);
        assert_eq!(normalized.prose[0].text, "Ship it.");
    }

    #[test]
    fn unknown_captured_events_remain_residual_evidence() {
        let events = vec![envelope(1, "future.event", serde_json::json!({"value": 7}))];
        let normalized = normalize_for_muse(&events).unwrap();
        assert_eq!(normalized.residual.len(), 1);
        assert_eq!(normalized.residual[0].kind, "future.event");
    }

    #[test]
    fn unsupported_schema_is_diagnostic_not_a_reinterpretation() {
        let event = crate::Envelope {
            v: crate::SCHEMA_VERSION + 1,
            ..envelope(1, "future.event", serde_json::Value::Null)
        };
        assert!(matches!(
            muse_envelope(&event),
            Err(MuseCaptureError::UnsupportedSchema { .. })
        ));
    }

    #[test]
    fn formalization_uses_the_supplied_snapshot() {
        let events = vec![envelope(
            1,
            "turn.user",
            serde_json::json!({"content":[{"type":"text","text":"Ship it."}]}),
        )];
        let snapshot = muse_registry::PackageRegistry::new()
            .snapshot_all()
            .unwrap();
        let documents = formalize_for_muse(&events, snapshot.clone()).unwrap();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].ontology, snapshot);
    }

    #[test]
    fn explicit_fact_projection_preserves_only_supported_source_relations() {
        let events = vec![envelope(
            1,
            "turn.user",
            serde_json::json!({"content":[{"type":"text","text":"Ship it."}]}),
        )];
        let snapshot = muse_registry::PackageRegistry::new()
            .snapshot_all()
            .unwrap();
        let document = formalize_for_muse(&events, snapshot).unwrap().remove(0);
        let projected = explicit_facts_for_muse(&document);
        assert_eq!(projected.document, document.id);
        assert!(projected.facts.facts.iter().any(|fact| matches!(
            fact,
            Fact::Relation { relation, .. } if relation == &RelationId::from("artist:eventRecordSession")
        )));
        assert!(projected.facts.facts.iter().all(|fact| match fact {
            Fact::Relation {
                subject, object, ..
            } => !subject.as_str().contains("literal") && !object.as_str().contains("literal"),
            Fact::InstanceOf { .. } => true,
        }));
    }

    #[test]
    fn diagnostics_are_durable_and_do_not_require_a_session_rewrite() {
        let project = tempfile::tempdir().unwrap();
        let diagnostic = write_muse_diagnostic(project.path(), "read", "bad source").unwrap();
        let stored = muse_diagnostics_dir(project.path()).join(format!("{}.json", diagnostic.id));
        let loaded: MuseDiagnostic =
            serde_json::from_slice(&std::fs::read(stored).unwrap()).unwrap();
        assert_eq!(loaded.operation, "read");
        assert_eq!(loaded.message, "bad source");
    }

    #[test]
    fn ingestion_failure_becomes_a_diagnostic_not_an_operation_error() {
        let project = tempfile::tempdir().unwrap();
        let event = crate::Envelope {
            v: crate::SCHEMA_VERSION + 1,
            ..envelope(1, "future.event", serde_json::Value::Null)
        };
        let snapshot = muse_registry::PackageRegistry::new()
            .snapshot_all()
            .unwrap();
        let outcome = ingest_for_muse(project.path(), "read", &[event], snapshot);
        assert!(outcome.documents.is_empty());
        let diagnostic = outcome.diagnostic.expect("failure diagnostic");
        assert_eq!(diagnostic.operation, "read");
        assert!(
            muse_diagnostics_dir(project.path())
                .join(format!("{}.json", diagnostic.id))
                .is_file()
        );
    }

    #[test]
    fn successful_ingestion_documents_are_durable_and_idempotent() {
        let project = tempfile::tempdir().unwrap();
        let events = vec![envelope(
            1,
            "turn.user",
            serde_json::json!({"content":[{"type":"text","text":"Remember this."}]}),
        )];
        let snapshot = muse_registry::PackageRegistry::new()
            .snapshot_all()
            .unwrap();
        let first = ingest_for_muse(project.path(), "read", &events, snapshot.clone());
        assert!(first.diagnostic.is_none());
        assert_eq!(first.documents.len(), 1);
        assert_eq!(first.facts.len(), 1);
        let stored =
            muse_documents_dir(project.path()).join(format!("{}.json", first.documents[0].id));
        assert!(stored.is_file());
        assert!(
            muse_facts_dir(project.path())
                .join(format!("{}.json", first.documents[0].id))
                .is_file()
        );
        let second = ingest_for_muse(project.path(), "read", &events, snapshot);
        assert!(second.diagnostic.is_none());
        assert_eq!(second.documents, first.documents);
        assert_eq!(second.facts, first.facts);
    }

    #[test]
    fn ontology_candidates_are_source_grounded_inert_and_idempotent() {
        let project = tempfile::tempdir().unwrap();
        let events = vec![envelope(
            1,
            "turn.user",
            serde_json::json!({"content":[{"type":"text","text":"Ship it."}]}),
        )];
        let snapshot = muse_registry::PackageRegistry::new()
            .snapshot_all()
            .unwrap();
        let documents = formalize_for_muse(&events, snapshot).unwrap();
        write_muse_documents(project.path(), &documents).unwrap();
        let mut candidate = MuseOntologyCandidate {
            id: String::new(),
            candidate: MuseOntologyCandidateKind::Concept {
                id: ConceptId::from("artist:ReviewedEvent"),
                parents: BTreeSet::from([ConceptId::from("artist:ArtistSessionEventRecord")]),
            },
            declared_rules: vec![Axiom::Subsumption {
                child: ConceptId::from("artist:ReviewedEvent"),
                parent: ConceptId::from("artist:ArtistSessionEventRecord"),
            }],
            source_documents: BTreeSet::from([documents[0].id.clone()]),
            examples: vec!["turn.user records the user request".into()],
            tests: vec!["candidate stays outside RegistrySnapshot".into()],
        };
        candidate.id = candidate.content_id().unwrap();
        let first = write_muse_candidate(project.path(), &candidate).unwrap();
        let second = write_muse_candidate(project.path(), &candidate).unwrap();
        assert_eq!(first, second);
        assert!(first.exists());
    }

    #[test]
    fn formal_rules_fire_only_on_exact_retained_fact_applicability() {
        let project = tempfile::tempdir().unwrap();
        let events = vec![envelope(
            1,
            "turn.user",
            serde_json::json!({"content":[{"type":"text","text":"Ship it."}]}),
        )];
        let snapshot = muse_registry::PackageRegistry::new()
            .snapshot_all()
            .unwrap();
        let ingestion = ingest_for_muse(project.path(), "read", &events, snapshot);
        assert!(ingestion.diagnostic.is_none());
        let condition = ingestion.facts[0]
            .facts
            .facts
            .iter()
            .next()
            .unwrap()
            .clone();
        let mut rule = MuseFormalRule {
            id: String::new(),
            authority: MuseRuleAuthority::ProjectDocument("POLICY.md".into()),
            source_documents: BTreeSet::from([ingestion.documents[0].id.clone()]),
            conditions: BTreeSet::from([condition]),
            include_model_labels: false,
            message: "Use the recorded policy constraint.".into(),
        };
        rule.id = rule.content_id().unwrap();
        write_muse_formal_rule(project.path(), &rule).unwrap();
        let automatic = ingest_for_muse(
            project.path(),
            "read",
            &events,
            muse_registry::PackageRegistry::new()
                .snapshot_all()
                .unwrap(),
        );
        assert_eq!(automatic.findings.len(), 1);
        let findings = evaluate_muse_formal_rules(project.path()).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, rule.id);
        assert_eq!(
            findings[0].supporting_documents,
            BTreeSet::from([ingestion.documents[0].id.clone()])
        );
        assert!(
            muse_findings_dir(project.path())
                .join(format!("{}.json", findings[0].id))
                .is_file()
        );
    }

    #[test]
    fn formal_rules_do_not_fire_on_partial_or_similar_facts() {
        let project = tempfile::tempdir().unwrap();
        let events = vec![envelope(
            1,
            "turn.user",
            serde_json::json!({"content":[{"type":"text","text":"Ship it."}]}),
        )];
        let snapshot = muse_registry::PackageRegistry::new()
            .snapshot_all()
            .unwrap();
        let ingestion = ingest_for_muse(project.path(), "read", &events, snapshot);
        let actual = ingestion.facts[0]
            .facts
            .facts
            .iter()
            .next()
            .unwrap()
            .clone();
        let missing = Fact::InstanceOf {
            individual: SemanticObjectId::from("artist-event-record:missing"),
            concept: ConceptId::from("artist:ArtistSessionEventRecord"),
        };
        let mut rule = MuseFormalRule {
            id: String::new(),
            authority: MuseRuleAuthority::Profile("default".into()),
            source_documents: BTreeSet::from([ingestion.documents[0].id.clone()]),
            conditions: BTreeSet::from([actual, missing]),
            include_model_labels: false,
            message: "This must not fire from a partial match.".into(),
        };
        rule.id = rule.content_id().unwrap();
        write_muse_formal_rule(project.path(), &rule).unwrap();
        assert!(
            evaluate_muse_formal_rules(project.path())
                .unwrap()
                .is_empty()
        );
        assert!(!muse_findings_dir(project.path()).exists());
    }

    #[test]
    fn bayesian_ledger_is_pinned_source_grounded_and_updates_online() {
        let project = tempfile::tempdir().unwrap();
        let events = vec![envelope(
            1,
            "turn.user",
            serde_json::json!({"content":[{"type":"text","text":"Ship it."}]}),
        )];
        let ingestion = ingest_for_muse(
            project.path(),
            "read",
            &events,
            muse_registry::PackageRegistry::new()
                .snapshot_all()
                .unwrap(),
        );
        let mut prior = MuseBayesianPrior {
            id: String::new(),
            proposition: "the checked configuration is safe to ship".into(),
            model: ModelId("beta-bernoulli/v1".into()),
            prior_model_pin: "model:beta-bernoulli/v1+artist-runtime".into(),
            support_microunits: 1_000_000,
            contrary_microunits: 1_000_000,
        };
        prior.id = prior.content_id().unwrap();
        write_muse_bayesian_prior(project.path(), &prior).unwrap();
        let mut supporting = MuseBayesianObservation {
            id: String::new(),
            prior: prior.id.clone(),
            supports: true,
            confidence_ppm: 800_000,
            source_documents: BTreeSet::from([ingestion.documents[0].id.clone()]),
        };
        supporting.id = supporting.content_id().unwrap();
        let estimate = write_muse_bayesian_observation(project.path(), &supporting).unwrap();
        assert_eq!(estimate.model, prior.model);
        assert_eq!(estimate.prior_model_pin, prior.prior_model_pin);
        assert_eq!(estimate.posterior_support_ppm, 642_857);
        let mut contrary = MuseBayesianObservation {
            id: String::new(),
            prior: prior.id.clone(),
            supports: false,
            confidence_ppm: 800_000,
            source_documents: BTreeSet::from([ingestion.documents[0].id.clone()]),
        };
        contrary.id = contrary.content_id().unwrap();
        let estimate = write_muse_bayesian_observation(project.path(), &contrary).unwrap();
        assert_eq!(estimate.posterior_support_ppm, 500_000);
        assert!(
            muse_bayesian_estimates_dir(project.path())
                .join(format!("{}.json", estimate.id))
                .is_file()
        );
    }

    #[test]
    fn model_label_admission_requires_matching_source_snapshot_and_derivation() {
        let project = tempfile::tempdir().unwrap();
        let events = vec![envelope(
            1,
            "turn.user",
            serde_json::json!({"content":[{"type":"text","text":"Ship it."}]}),
        )];
        let ingestion = ingest_for_muse(
            project.path(),
            "read",
            &events,
            muse_registry::PackageRegistry::new()
                .snapshot_all()
                .unwrap(),
        );
        let mut document = ingestion.documents[0].clone();
        document.id = OccurrenceDocumentId::from("artist-model-label-document:session-1:1");
        document.derivation = Derivation::ModelGenerated {
            model: "labeler-v1".into(),
            protocol: "muse-runtime-label/v1".into(),
        };
        let mut label = MuseModelLabel {
            id: String::new(),
            model: "labeler-v1".into(),
            source_documents: BTreeSet::from([ingestion.documents[0].id.clone()]),
            document,
        };
        label.id = label.content_id().unwrap();
        assert!(
            write_muse_model_label(project.path(), &label)
                .unwrap()
                .is_file()
        );
        let mut invalid = label.clone();
        invalid.model = "wrong-model".into();
        invalid.id = invalid.content_id().unwrap();
        assert!(matches!(
            write_muse_model_label(project.path(), &invalid),
            Err(MuseLabelError::DerivationMismatch)
        ));
    }

    #[test]
    fn rules_only_use_model_label_facts_when_the_rule_opts_in() {
        let project = tempfile::tempdir().unwrap();
        let events = vec![envelope(
            1,
            "turn.user",
            serde_json::json!({"content":[{"type":"text","text":"Ship it."}]}),
        )];
        let ingestion = ingest_for_muse(
            project.path(),
            "read",
            &events,
            muse_registry::PackageRegistry::new()
                .snapshot_all()
                .unwrap(),
        );
        let mut document = ingestion.documents[0].clone();
        document.id = OccurrenceDocumentId::from("artist-model-label-document:session-1:2");
        document.derivation = Derivation::ModelGenerated {
            model: "labeler-v1".into(),
            protocol: "muse-runtime-label/v1".into(),
        };
        let subject = document.referents.keys().next().unwrap().clone();
        let proposition = document.propositions.keys().next().unwrap().clone();
        document
            .propositions
            .get_mut(&proposition)
            .unwrap()
            .expression = PropositionExpr::TypeAssertion {
            subject: Term::Referent(subject.clone()),
            r#type: ConceptId::from("artist:ModelReviewedEvent"),
        };
        document.statements.values_mut().next().unwrap().content = proposition;
        let mut label = MuseModelLabel {
            id: String::new(),
            model: "labeler-v1".into(),
            source_documents: BTreeSet::from([ingestion.documents[0].id.clone()]),
            document,
        };
        label.id = label.content_id().unwrap();
        write_muse_model_label(project.path(), &label).unwrap();
        let condition = Fact::InstanceOf {
            individual: SemanticObjectId::from(subject.as_str()),
            concept: ConceptId::from("artist:ModelReviewedEvent"),
        };
        let mut disabled = MuseFormalRule {
            id: String::new(),
            authority: MuseRuleAuthority::ProjectDocument("POLICY.md".into()),
            source_documents: BTreeSet::from([ingestion.documents[0].id.clone()]),
            conditions: BTreeSet::from([condition]),
            include_model_labels: false,
            message: "disabled model label condition".into(),
        };
        disabled.id = disabled.content_id().unwrap();
        write_muse_formal_rule(project.path(), &disabled).unwrap();
        assert!(
            evaluate_muse_formal_rules(project.path())
                .unwrap()
                .is_empty()
        );
        let mut enabled = disabled;
        enabled.include_model_labels = true;
        enabled.message = "enabled model label condition".into();
        enabled.id = enabled.content_id().unwrap();
        write_muse_formal_rule(project.path(), &enabled).unwrap();
        assert_eq!(evaluate_muse_formal_rules(project.path()).unwrap().len(), 1);
    }

    #[test]
    fn candidate_rejects_missing_retained_evidence() {
        let project = tempfile::tempdir().unwrap();
        let mut candidate = MuseOntologyCandidate {
            id: String::new(),
            candidate: MuseOntologyCandidateKind::Relation {
                id: RelationId::from("artist:reviewedBy"),
                parents: BTreeSet::from([RelationId::from("artist:eventRecordKind")]),
            },
            declared_rules: vec![Axiom::RelationSubsumption {
                child: RelationId::from("artist:reviewedBy"),
                parent: RelationId::from("artist:eventRecordKind"),
            }],
            source_documents: BTreeSet::from([OccurrenceDocumentId::from("missing-document")]),
            examples: vec!["a review records a reviewer".into()],
            tests: vec!["missing evidence rejects the candidate".into()],
        };
        candidate.id = candidate.content_id().unwrap();
        assert!(matches!(
            write_muse_candidate(project.path(), &candidate),
            Err(MuseCandidateError::MissingDocument(_))
        ));
    }
}
