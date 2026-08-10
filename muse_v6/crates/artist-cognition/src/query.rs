use std::collections::{BTreeMap, BTreeSet};

use artist_empirical::{BayesianError, BayesianModel, Dataset, PosteriorClaim};
use artist_formal::{
    GraphHash, InterpretationError, InterpretedGraph, InterpretedGraphHash, ObjectId,
    ObjectMeaning, OntologicalTypingDiagnostic, OntologyHash, OntologyTypeExpr,
};
use artist_kernel::{
    Certificate, CertifiedQueryAnswer, CheckSession, CompiledOntologySubmission, Context,
    DependencyReport, ElaborationRole, KernelQuery, Name, OntologyCompileError, OntologyCompiler,
    QueryError, QueryParameter, Term, Theory, TheoryId,
};
use artist_theories::UniversalError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::artifact::{ArtifactEnvelope, ArtifactError};
use crate::certification::{CertifiedClaimError, CertifiedGraphClaim, ProofProductionRecord};

/// Stable caller-selected query identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QueryId(pub String);

/// Canonical content identity of one complete query.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QueryHash(pub String);

/// One exact answer position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryVariable {
    /// Human-facing binding name. Names are unique within one query.
    pub name: String,
    /// Graph object that directly denotes the exact expected ontology type.
    pub expected_type: ObjectId,
}

/// One closed witness supplied either by exact graph identity or directly as a DTT term.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "witness", rename_all = "snake_case")]
pub enum QueryWitness {
    /// A graph object deterministically elaborated under the compiled submission.
    Graph { object: ObjectId },
    /// A closed DTT term supplied directly by a proof producer.
    Formal { term: Term },
}

/// One result tuple whose exact success predicate is proved by the fixed kernel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertifiedQueryWitness {
    /// Exact binding for every declared query variable.
    pub bindings: BTreeMap<String, QueryWitness>,
    /// Kernel certificate proving the fully instantiated result predicate.
    pub certificate: Certificate,
    /// Exact transitive dependency closure of the certificate.
    pub dependencies: DependencyReport,
    /// Optional untrusted provenance for how the candidate was produced.
    pub production: Option<ProofProductionRecord>,
}

/// Complete supported cognitive query surface.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "query", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum QueryRequest {
    /// Prove one exact proposition.
    Prove { proposition: ObjectId },
    /// Prove the explicit negation or counterclaim supplied by the harness.
    Disprove {
        proposition: ObjectId,
        counterclaim: ObjectId,
    },
    /// Find one tuple satisfying `condition`.
    ///
    /// The condition must have exact ontology type
    /// `variables[0] -> ... -> variables[n] -> Proposition`.
    Find {
        variables: Vec<QueryVariable>,
        condition: ObjectId,
        /// Optional exact proposition establishing that no witness exists.
        exhaustion_claim: Option<ObjectId>,
    },
    /// Synthesize one function satisfying an exact predicate.
    SynthesizeFunction {
        function: QueryVariable,
        /// Exact predicate of type `function.expected_type -> Proposition`.
        specification: ObjectId,
    },
    /// Enumerate certified tuples satisfying `condition`.
    Enumerate {
        variables: Vec<QueryVariable>,
        condition: ObjectId,
        /// Optional exact proposition establishing complete exhaustion.
        exhaustion_claim: Option<ObjectId>,
        /// Maximum tuples returned by one answer.
        limit: Option<u64>,
    },
    /// Produce a certified counterexample to an exact proposition.
    Counterexample {
        proposition: ObjectId,
        counterexample: QueryVariable,
        /// Exact predicate of type
        /// `Proposition -> counterexample.expected_type -> Proposition`.
        condition: ObjectId,
    },
    /// Compare two exact represented objects and certify one result.
    Compare {
        left: ObjectId,
        right: ObjectId,
        result: QueryVariable,
        /// Exact predicate of type
        /// `type(left) -> type(right) -> result.expected_type -> Proposition`.
        criterion: ObjectId,
    },
    /// Compute and certify one bound for an exact target.
    ComputeBound {
        target: ObjectId,
        bound: QueryVariable,
        /// Exact predicate of type
        /// `type(target) -> bound.expected_type -> Proposition`.
        validity: ObjectId,
    },
    /// Prove a goal under the exact ordered local assumptions.
    UnderAssumptions {
        goal: ObjectId,
        assumptions: Vec<ObjectId>,
    },
    /// Produce an empirical posterior under one exact model and dataset.
    EmpiricalPosterior {
        proposition: ObjectId,
        model: BayesianModel,
        dataset: Dataset,
    },
}

/// Resource and procedure policy. It constrains discovery, never kernel validity.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryPolicy {
    /// Optional named discovery procedure.
    pub procedure: Option<String>,
    /// Optional deterministic seed.
    pub seed: Option<String>,
    /// Maximum producer steps.
    pub step_limit: Option<u64>,
    /// Maximum returned certified tuples.
    pub result_limit: Option<u64>,
}

/// Exact cognitive query submitted above the proof kernel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CognitiveQuery {
    /// Stable request identity.
    pub id: QueryId,
    /// Complete exactly interpreted submission.
    pub submission: InterpretedGraph,
    /// Requested operation.
    pub request: QueryRequest,
    /// Optional exact target theory. Absence permits any append-only extension of
    /// the deterministically compiled submission theory.
    pub theory: Option<TheoryId>,
    /// Discovery policy.
    pub policy: QueryPolicy,
}

/// Trusted-kernel continuation pinned to one exact cognitive query.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KernelContinuation {
    /// Continuation schema version.
    pub format_version: u32,
    /// Caller-selected query identity.
    pub query: QueryId,
    /// Canonical complete query identity.
    pub query_content: QueryHash,
    /// Exact graph, ontology, and interpretation-map identity.
    pub submission: InterpretedGraphHash,
    /// Exact represented graph identity.
    pub graph: GraphHash,
    /// Exact ontology identity.
    pub ontology: OntologyHash,
    /// Exact theory embedded in the checker session.
    pub theory: TheoryId,
    /// Complete trusted checker continuation.
    pub session: CheckSession,
}

/// Discovery continuation independent of kernel-checker continuation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryContinuation {
    /// Continuation schema version.
    pub format_version: u32,
    /// Caller-selected query identity.
    pub query: QueryId,
    /// Canonical complete query identity.
    pub query_content: QueryHash,
    /// Exact graph, ontology, and interpretation-map identity.
    pub submission: InterpretedGraphHash,
    /// Exact represented graph identity.
    pub graph: GraphHash,
    /// Exact ontology identity.
    pub ontology: OntologyHash,
    /// Exact theory selected by the suspended procedure. A query with no
    /// preselected theory may choose any valid append-only extension, but the
    /// continuation must record the concrete choice once work begins.
    pub theory: TheoryId,
    /// Procedure identity and version.
    pub procedure: String,
    pub procedure_version: String,
    /// Exact opaque producer state.
    pub state: Vec<u8>,
    /// Completed logical producer steps.
    pub completed_steps: u64,
}

/// Either continuation kind, kept explicitly distinct.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "continuation", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum Continuation {
    /// Trusted kernel checker state.
    Kernel(KernelContinuation),
    /// Untrusted proof-search or computation state.
    Discovery(DiscoveryContinuation),
}

/// Successful query payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum QueryPayload {
    /// One or more result tuples, each with a kernel proof of the exact success predicate.
    CertifiedWitnesses { answers: Vec<CertifiedQueryWitness> },
    /// Explicit empirical posterior, conditional on its recorded model and data.
    EmpiricalPosterior { posterior: PosteriorClaim },
}

/// Honest terminal or resumable query result.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum QueryStatus {
    /// Kernel-certified proof of the requested proposition.
    Proved(CertifiedGraphClaim),
    /// Kernel-certified proof of the explicit counterclaim.
    Disproved(CertifiedGraphClaim),
    /// Certified witness/synthesis/comparison/bound answer or an explicit posterior.
    Answered(QueryPayload),
    /// Candidate certificate was rejected.
    RejectedCertificate { reason: String },
    /// Exact content is understood, but the requested DTT formalization is unavailable.
    FormalizationUnavailable { object: ObjectId, reason: String },
    /// Exact certified claim that the declared witness search is exhausted.
    SearchExhausted(CertifiedGraphClaim),
    /// No conclusion has been established.
    Unknown,
    /// Deliberately paused with exact continuation.
    Paused(Continuation),
    /// Cancelled without a conclusion.
    Cancelled,
    /// Resource bound reached with exact continuation.
    ResourceLimitReached(Continuation),
}

/// Complete answer envelope.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CognitiveAnswer {
    /// Exact caller-selected query identity.
    pub query: QueryId,
    /// Canonical identity of the complete query content being answered.
    pub query_content: QueryHash,
    /// Exact result.
    pub status: QueryStatus,
    /// Procedure identity, when discovery or computation was used.
    pub procedure: Option<String>,
}

/// Structural or deductive query-contract failure.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum CognitiveQueryError {
    /// Query identity is empty.
    #[error("query identity must not be empty")]
    EmptyQueryId,
    /// A declared procedure identity is empty.
    #[error("procedure identity must not be empty")]
    EmptyProcedure,
    /// A discovery continuation omits its procedure version.
    #[error("discovery procedure version must not be empty")]
    EmptyProcedureVersion,
    /// Answer or continuation procedure contradicts the query policy or envelope.
    #[error("procedure identity does not match the exact query or continuation")]
    ProcedureMismatch,
    /// Complete query content could not be canonically encoded.
    #[error("query content identity failed: {0}")]
    InvalidQueryEncoding(ArtifactError),
    /// Submission is not exactly interpreted.
    #[error("invalid interpreted query submission: {0}")]
    InvalidSubmission(InterpretationError),
    /// A semantically referenced graph application is ontologically ill typed.
    #[error("query references ontological typing errors: {0:?}")]
    IllTypedSubmission(Vec<OntologicalTypingDiagnostic>),
    /// Query references a missing graph object.
    #[error("query references missing graph object {0}")]
    MissingObject(ObjectId),
    /// A request position requires a proposition.
    #[error("query object {0} is not ontologically typed as a proposition")]
    ExpectedProposition(ObjectId),
    /// A request position requires an object denoting a type.
    #[error("query object {0} does not denote a type")]
    ExpectedType(ObjectId),
    /// A result condition has the wrong exact curried predicate type.
    #[error("query condition {object} has the wrong predicate type")]
    ExpectedPredicate { object: ObjectId },
    /// Witness-producing requests require at least one variable.
    #[error("query has no result variables")]
    EmptyVariables,
    /// Query variable names must not be empty.
    #[error("query contains an empty variable name")]
    EmptyVariableName,
    /// Query variable names must be unique within one request.
    #[error("query repeats variable name {0:?}")]
    DuplicateVariable(String),
    /// A requested result count cannot be represented by the kernel query format.
    #[error("query contains too many result variables")]
    TooManyVariables,
    /// Dataset structure is invalid.
    #[error("query contains an invalid empirical dataset: {0}")]
    InvalidDataset(BayesianError),
    /// Witness tuple does not bind exactly the declared query variables.
    #[error("witness tuple does not bind exactly the declared variables")]
    InvalidWitnessShape,
    /// Graph witness does not have the exact declared type.
    #[error("witness {object} has the wrong type for variable {variable:?}")]
    WitnessTypeMismatch { variable: String, object: ObjectId },
    /// Answer contains a graph object absent from the exact submission.
    #[error("answer references missing graph object {0}")]
    MissingAnswerObject(ObjectId),
    /// Answer repeats an identical certified result tuple.
    #[error("answer repeats a certified result tuple")]
    DuplicateWitness,
    /// Answer contains the wrong number of certified tuples.
    #[error("answer contains an incompatible number of certified result tuples")]
    InvalidResultCount,
    /// Answer exceeds an exact request or policy result limit.
    #[error("answer exceeds the declared result limit")]
    ResultLimitExceeded,
    /// Continuation targets another query or content identity.
    #[error("continuation does not belong to this exact query")]
    ContinuationMismatch,
    /// Answer targets another query.
    #[error("answer targets another query")]
    AnswerMismatch,
    /// Answer status does not apply to this request kind.
    #[error("answer status is incompatible with the query request")]
    IncompatibleAnswer,
    /// Embedded empirical posterior record is malformed.
    #[error("answer contains an invalid posterior record: {0}")]
    InvalidPosterior(BayesianError),
    /// Posterior does not match the exact query proposition, model, and dataset.
    #[error("posterior does not match the exact empirical query")]
    PosteriorMismatch,
    /// Embedded graph proof does not verify against the supplied compiled/proof theories.
    #[error("answer contains an invalid certified graph claim: {0}")]
    InvalidCertifiedClaim(CertifiedClaimError),
    /// Compiled ontology submission failed deterministic verification.
    #[error("answer verification received an invalid compiled submission: {0}")]
    InvalidCompiledSubmission(OntologyCompileError),
    /// A query object failed deterministic DTT elaboration.
    #[error("answer elaboration failed: {0}")]
    InvalidAnswerElaboration(OntologyCompileError),
    /// Supplied answer theory does not match the exact query/compiled theory contract.
    #[error("answer verification theory does not match the query or compiled submission")]
    AnswerTheoryMismatch,
    /// The generated dependent kernel query or supplied witnesses/proof are invalid.
    #[error("certified query answer failed: {0}")]
    InvalidKernelQuery(QueryError),
    /// Stored certificate differs from the certificate determined by the exact query and bindings.
    #[error("stored witness certificate does not match the exact instantiated query")]
    WitnessCertificateMismatch,
    /// Stored dependency disclosure differs from the certificate closure.
    #[error("stored witness dependency report does not match the certificate closure")]
    WitnessDependencyMismatch,
    /// Untrusted production trace is malformed.
    #[error("invalid witness production evidence: {0}")]
    InvalidProduction(UniversalError),
    /// A graph claim uses local assumptions incompatible with the exact request.
    #[error("graph claim context does not match the exact query assumptions")]
    AssumptionContextMismatch,
}

struct AnswerSpecification<'a> {
    variables: Vec<&'a QueryVariable>,
    condition: &'a ObjectId,
    fixed_arguments: Vec<&'a ObjectId>,
}

impl CognitiveQuery {
    /// Returns the canonical identity of the complete query, including its exact
    /// submission, request, theory selection, and policy.
    pub fn canonical_hash(&self) -> Result<QueryHash, CognitiveQueryError> {
        ArtifactEnvelope::digest_typed(self)
            .map(QueryHash)
            .map_err(CognitiveQueryError::InvalidQueryEncoding)
    }

    /// Validates exact input, request roles, predicate signatures, and graph references.
    pub fn validate(&self) -> Result<(), CognitiveQueryError> {
        if self.id.0.is_empty() {
            return Err(CognitiveQueryError::EmptyQueryId);
        }
        if self
            .policy
            .procedure
            .as_ref()
            .is_some_and(std::string::String::is_empty)
        {
            return Err(CognitiveQueryError::EmptyProcedure);
        }
        self.submission
            .validate()
            .map_err(CognitiveQueryError::InvalidSubmission)?;
        for object in self.referenced_objects() {
            if !self.submission.graph.nodes.contains_key(object) {
                return Err(CognitiveQueryError::MissingObject(object.clone()));
            }
        }

        let relevant = self.semantic_dependency_closure();
        let diagnostics: Vec<_> = self
            .submission
            .typing_diagnostics()
            .into_iter()
            .filter(|diagnostic| relevant.contains(diagnostic_object(diagnostic)))
            .collect();
        if !diagnostics.is_empty() {
            return Err(CognitiveQueryError::IllTypedSubmission(diagnostics));
        }

        let variables = self.answer_variables();
        if self.answer_specification().is_some() && variables.is_empty() {
            return Err(CognitiveQueryError::EmptyVariables);
        }
        let mut names = BTreeSet::new();
        for variable in &variables {
            if variable.name.is_empty() {
                return Err(CognitiveQueryError::EmptyVariableName);
            }
            if !names.insert(variable.name.clone()) {
                return Err(CognitiveQueryError::DuplicateVariable(
                    variable.name.clone(),
                ));
            }
            self.require_type(&variable.expected_type)?;
        }
        if variables.len() > u32::MAX as usize {
            return Err(CognitiveQueryError::TooManyVariables);
        }

        for proposition in self.proposition_objects() {
            self.require_proposition(proposition)?;
        }
        if let Some(specification) = self.answer_specification() {
            self.require_predicate(&specification)?;
        }
        if let QueryRequest::EmpiricalPosterior { dataset, .. } = &self.request {
            dataset
                .validate()
                .map_err(CognitiveQueryError::InvalidDataset)?;
        }
        Ok(())
    }

    /// Validates a discovery continuation against the complete query content.
    pub fn validate_discovery_continuation(
        &self,
        continuation: &DiscoveryContinuation,
    ) -> Result<(), CognitiveQueryError> {
        if continuation.procedure.is_empty() {
            return Err(CognitiveQueryError::EmptyProcedure);
        }
        if continuation.procedure_version.is_empty() {
            return Err(CognitiveQueryError::EmptyProcedureVersion);
        }
        if self
            .policy
            .procedure
            .as_ref()
            .is_some_and(|procedure| procedure != &continuation.procedure)
        {
            return Err(CognitiveQueryError::ProcedureMismatch);
        }
        if continuation.format_version == 3
            && continuation.query == self.id
            && continuation.query_content == self.canonical_hash()?
            && continuation.submission == self.submission.canonical_hash()
            && continuation.graph == self.submission.graph.canonical_hash()
            && continuation.ontology == self.submission.ontology.canonical_hash()
            && self
                .theory
                .as_ref()
                .is_none_or(|theory| theory == &continuation.theory)
        {
            Ok(())
        } else {
            Err(CognitiveQueryError::ContinuationMismatch)
        }
    }

    /// Ensures an answer belongs to this caller-selected query identity.
    pub fn validate_answer_identity(
        &self,
        answer: &CognitiveAnswer,
    ) -> Result<(), CognitiveQueryError> {
        if answer.query == self.id && answer.query_content == self.canonical_hash()? {
            Ok(())
        } else {
            Err(CognitiveQueryError::AnswerMismatch)
        }
    }

    /// Validates answer identity, request compatibility, graph references, payload
    /// shape, empirical records, and continuation pins. It does not execute DTT checks.
    pub fn validate_answer(&self, answer: &CognitiveAnswer) -> Result<(), CognitiveQueryError> {
        self.validate()?;
        self.validate_answer_identity(answer)?;
        if answer
            .procedure
            .as_ref()
            .is_some_and(std::string::String::is_empty)
        {
            return Err(CognitiveQueryError::EmptyProcedure);
        }
        if let (Some(expected), Some(actual)) = (&self.policy.procedure, &answer.procedure) {
            if expected != actual {
                return Err(CognitiveQueryError::ProcedureMismatch);
            }
        }
        match &answer.status {
            QueryStatus::Proved(claim) => {
                let expected = self
                    .provable_goal()
                    .ok_or(CognitiveQueryError::IncompatibleAnswer)?;
                if &claim.claim != expected {
                    return Err(CognitiveQueryError::IncompatibleAnswer);
                }
            }
            QueryStatus::Disproved(claim) => {
                let QueryRequest::Disprove { counterclaim, .. } = &self.request else {
                    return Err(CognitiveQueryError::IncompatibleAnswer);
                };
                if &claim.claim != counterclaim {
                    return Err(CognitiveQueryError::IncompatibleAnswer);
                }
            }
            QueryStatus::Answered(payload) => self.validate_payload(payload)?,
            QueryStatus::SearchExhausted(claim) => {
                let Some(expected) = self.exhaustion_claim() else {
                    return Err(CognitiveQueryError::IncompatibleAnswer);
                };
                if &claim.claim != expected {
                    return Err(CognitiveQueryError::IncompatibleAnswer);
                }
            }
            QueryStatus::Paused(continuation) | QueryStatus::ResourceLimitReached(continuation) => {
                self.validate_continuation(continuation)?;
                if let Continuation::Discovery(continuation) = continuation {
                    if answer.procedure.as_ref() != Some(&continuation.procedure) {
                        return Err(CognitiveQueryError::ProcedureMismatch);
                    }
                }
            }
            QueryStatus::FormalizationUnavailable { object, .. } => {
                if !self.referenced_objects().contains(&object) {
                    return Err(CognitiveQueryError::IncompatibleAnswer);
                }
            }
            QueryStatus::RejectedCertificate { .. }
            | QueryStatus::Unknown
            | QueryStatus::Cancelled => {}
        }
        Ok(())
    }

    /// Performs structural validation and verifies every embedded DTT graph claim,
    /// witness type, instantiated success predicate, certificate, dependency report,
    /// and finite production trace.
    pub fn verify_answer(
        &self,
        answer: &CognitiveAnswer,
        compiled: &CompiledOntologySubmission,
        proof_theory: &Theory,
    ) -> Result<(), CognitiveQueryError> {
        self.validate_answer(answer)?;
        OntologyCompiler::verify_compiled(&self.submission, compiled)
            .map_err(CognitiveQueryError::InvalidCompiledSubmission)?;
        let proof_theory_id = proof_theory.id();
        if !proof_theory.is_extension_of(&compiled.theory)
            || self
                .theory
                .as_ref()
                .is_some_and(|expected| expected != &proof_theory_id)
        {
            return Err(CognitiveQueryError::AnswerTheoryMismatch);
        }

        match &answer.status {
            QueryStatus::Proved(claim)
            | QueryStatus::Disproved(claim)
            | QueryStatus::SearchExhausted(claim) => {
                claim
                    .verify(&self.submission, compiled, proof_theory)
                    .map_err(CognitiveQueryError::InvalidCertifiedClaim)?;
                self.verify_graph_claim_context(claim, compiled)?;
                Ok(())
            }
            QueryStatus::Answered(QueryPayload::CertifiedWitnesses { answers }) => {
                self.verify_certified_witnesses(answers, compiled, proof_theory)
            }
            QueryStatus::Paused(continuation) | QueryStatus::ResourceLimitReached(continuation) => {
                match continuation {
                    Continuation::Kernel(continuation) => {
                        if continuation.theory != proof_theory_id {
                            return Err(CognitiveQueryError::AnswerTheoryMismatch);
                        }
                    }
                    Continuation::Discovery(continuation) => {
                        if continuation.theory != proof_theory_id {
                            return Err(CognitiveQueryError::AnswerTheoryMismatch);
                        }
                    }
                }
                Ok(())
            }
            QueryStatus::Answered(QueryPayload::EmpiricalPosterior { .. })
            | QueryStatus::RejectedCertificate { .. }
            | QueryStatus::FormalizationUnavailable { .. }
            | QueryStatus::Unknown
            | QueryStatus::Cancelled => Ok(()),
        }
    }

    fn validate_continuation(
        &self,
        continuation: &Continuation,
    ) -> Result<(), CognitiveQueryError> {
        match continuation {
            Continuation::Discovery(continuation) => {
                self.validate_discovery_continuation(continuation)
            }
            Continuation::Kernel(continuation) => {
                if continuation.format_version == 2
                    && continuation.query == self.id
                    && continuation.query_content == self.canonical_hash()?
                    && continuation.submission == self.submission.canonical_hash()
                    && continuation.graph == self.submission.graph.canonical_hash()
                    && continuation.ontology == self.submission.ontology.canonical_hash()
                    && continuation.session.theory().id() == continuation.theory
                    && self
                        .theory
                        .as_ref()
                        .is_none_or(|theory| theory == &continuation.theory)
                {
                    Ok(())
                } else {
                    Err(CognitiveQueryError::ContinuationMismatch)
                }
            }
        }
    }

    fn validate_payload(&self, payload: &QueryPayload) -> Result<(), CognitiveQueryError> {
        match payload {
            QueryPayload::CertifiedWitnesses { answers } => {
                let Some(specification) = self.answer_specification() else {
                    return Err(CognitiveQueryError::IncompatibleAnswer);
                };
                self.validate_result_count(answers.len())?;
                let expected_names: BTreeSet<_> = specification
                    .variables
                    .iter()
                    .map(|variable| variable.name.as_str())
                    .collect();
                for (index, answer) in answers.iter().enumerate() {
                    if answers[..index]
                        .iter()
                        .any(|previous| previous.bindings == answer.bindings)
                    {
                        return Err(CognitiveQueryError::DuplicateWitness);
                    }
                    let actual_names: BTreeSet<_> =
                        answer.bindings.keys().map(String::as_str).collect();
                    if actual_names != expected_names {
                        return Err(CognitiveQueryError::InvalidWitnessShape);
                    }
                    for variable in &specification.variables {
                        let QueryWitness::Graph { object } = &answer.bindings[&variable.name]
                        else {
                            continue;
                        };
                        let Some(interpretation) = self.submission.interpretations.get(object)
                        else {
                            return Err(CognitiveQueryError::MissingAnswerObject(object.clone()));
                        };
                        let expected = self.denoted_type(&variable.expected_type)?;
                        if interpretation.ty != expected {
                            return Err(CognitiveQueryError::WitnessTypeMismatch {
                                variable: variable.name.clone(),
                                object: object.clone(),
                            });
                        }
                    }
                }
                Ok(())
            }
            QueryPayload::EmpiricalPosterior { posterior } => {
                let QueryRequest::EmpiricalPosterior {
                    proposition,
                    model,
                    dataset,
                } = &self.request
                else {
                    return Err(CognitiveQueryError::IncompatibleAnswer);
                };
                posterior
                    .validate()
                    .map_err(CognitiveQueryError::InvalidPosterior)?;
                if posterior.proposition.submission != self.submission.canonical_hash()
                    || posterior.proposition.graph != self.submission.graph.canonical_hash()
                    || &posterior.proposition.root != proposition
                    || &posterior.model != model
                    || &posterior.data != dataset
                {
                    return Err(CognitiveQueryError::PosteriorMismatch);
                }
                Ok(())
            }
        }
    }

    fn validate_result_count(&self, count: usize) -> Result<(), CognitiveQueryError> {
        match &self.request {
            QueryRequest::Find { .. }
            | QueryRequest::SynthesizeFunction { .. }
            | QueryRequest::Counterexample { .. }
            | QueryRequest::Compare { .. }
            | QueryRequest::ComputeBound { .. }
                if count != 1 =>
            {
                return Err(CognitiveQueryError::InvalidResultCount);
            }
            QueryRequest::Enumerate { limit, .. } => {
                if count == 0 {
                    return Err(CognitiveQueryError::InvalidResultCount);
                }
                if limit.is_some_and(|limit| count as u128 > u128::from(limit)) {
                    return Err(CognitiveQueryError::ResultLimitExceeded);
                }
            }
            QueryRequest::Prove { .. }
            | QueryRequest::Disprove { .. }
            | QueryRequest::UnderAssumptions { .. }
            | QueryRequest::EmpiricalPosterior { .. } => {
                return Err(CognitiveQueryError::IncompatibleAnswer);
            }
            _ => {}
        }
        if self
            .policy
            .result_limit
            .is_some_and(|limit| count as u128 > u128::from(limit))
        {
            return Err(CognitiveQueryError::ResultLimitExceeded);
        }
        Ok(())
    }

    fn verify_certified_witnesses(
        &self,
        answers: &[CertifiedQueryWitness],
        compiled: &CompiledOntologySubmission,
        proof_theory: &Theory,
    ) -> Result<(), CognitiveQueryError> {
        let specification = self
            .answer_specification()
            .ok_or(CognitiveQueryError::IncompatibleAnswer)?;
        let kernel_query = self.build_kernel_query(&specification, compiled, proof_theory)?;

        for answer in answers {
            let mut witnesses = Vec::with_capacity(specification.variables.len());
            for variable in &specification.variables {
                let witness = &answer.bindings[&variable.name];
                let term = match witness {
                    QueryWitness::Graph { object } => {
                        OntologyCompiler::elaborate(
                            &self.submission,
                            compiled,
                            object,
                            ElaborationRole::Term,
                        )
                        .map_err(CognitiveQueryError::InvalidAnswerElaboration)?
                        .term
                    }
                    QueryWitness::Formal { term } => term.clone(),
                };
                witnesses.push(term);
            }
            let expected = kernel_query
                .verify_answer(
                    proof_theory,
                    &CertifiedQueryAnswer {
                        witnesses,
                        proof: answer.certificate.proof.clone(),
                    },
                )
                .map_err(CognitiveQueryError::InvalidKernelQuery)?;
            if answer.certificate != expected {
                return Err(CognitiveQueryError::WitnessCertificateMismatch);
            }
            if answer.dependencies != answer.certificate.dependencies(proof_theory) {
                return Err(CognitiveQueryError::WitnessDependencyMismatch);
            }
            if let Some(production) = &answer.production {
                production
                    .verify()
                    .map_err(CognitiveQueryError::InvalidProduction)?;
            }
        }
        Ok(())
    }

    fn build_kernel_query(
        &self,
        specification: &AnswerSpecification<'_>,
        compiled: &CompiledOntologySubmission,
        proof_theory: &Theory,
    ) -> Result<KernelQuery, CognitiveQueryError> {
        let condition = OntologyCompiler::elaborate(
            &self.submission,
            compiled,
            specification.condition,
            ElaborationRole::Function,
        )
        .map_err(CognitiveQueryError::InvalidAnswerElaboration)?;

        let mut fixed_terms = Vec::with_capacity(specification.fixed_arguments.len());
        for object in &specification.fixed_arguments {
            fixed_terms.push(
                OntologyCompiler::elaborate(
                    &self.submission,
                    compiled,
                    object,
                    ElaborationRole::Term,
                )
                .map_err(CognitiveQueryError::InvalidAnswerElaboration)?
                .term,
            );
        }

        let mut parameters = Vec::with_capacity(specification.variables.len());
        for variable in &specification.variables {
            let ty = OntologyCompiler::elaborate(
                &self.submission,
                compiled,
                &variable.expected_type,
                ElaborationRole::Type,
            )
            .map_err(CognitiveQueryError::InvalidAnswerElaboration)?
            .term;
            parameters.push(QueryParameter {
                name: Name::new(format!("artist.query/{}/{}", self.id.0, variable.name)),
                ty,
            });
        }

        let variable_count =
            u32::try_from(parameters.len()).map_err(|_| CognitiveQueryError::TooManyVariables)?;
        let variables = (0..variable_count).rev().map(Term::var);
        let goal = Term::apply_many(condition.term, fixed_terms.into_iter().chain(variables));
        let query = KernelQuery::new(proof_theory, parameters, goal);
        query
            .validate(proof_theory)
            .map_err(CognitiveQueryError::InvalidKernelQuery)?;
        Ok(query)
    }

    fn verify_graph_claim_context(
        &self,
        claim: &CertifiedGraphClaim,
        compiled: &CompiledOntologySubmission,
    ) -> Result<(), CognitiveQueryError> {
        let expected = match (&self.request, &claim.claim) {
            (QueryRequest::UnderAssumptions { assumptions, .. }, _) => {
                let mut context = Vec::with_capacity(assumptions.len());
                for assumption in assumptions {
                    context.push(
                        OntologyCompiler::elaborate(
                            &self.submission,
                            compiled,
                            assumption,
                            ElaborationRole::Proposition,
                        )
                        .map_err(CognitiveQueryError::InvalidAnswerElaboration)?
                        .term,
                    );
                }
                Context(context)
            }
            _ => Context::new(),
        };
        if claim.certificate.context == expected {
            Ok(())
        } else {
            Err(CognitiveQueryError::AssumptionContextMismatch)
        }
    }

    fn require_proposition(&self, object: &ObjectId) -> Result<(), CognitiveQueryError> {
        let expected = OntologyTypeExpr::named(self.submission.ontology.proposition_type.clone());
        if self
            .submission
            .interpretations
            .get(object)
            .is_some_and(|interpretation| interpretation.ty == expected)
        {
            Ok(())
        } else {
            Err(CognitiveQueryError::ExpectedProposition(object.clone()))
        }
    }

    fn require_type(&self, object: &ObjectId) -> Result<(), CognitiveQueryError> {
        if self
            .submission
            .interpretations
            .get(object)
            .is_some_and(|interpretation| {
                matches!(&interpretation.meaning, ObjectMeaning::Type { .. })
            })
        {
            Ok(())
        } else {
            Err(CognitiveQueryError::ExpectedType(object.clone()))
        }
    }

    fn require_predicate(
        &self,
        specification: &AnswerSpecification<'_>,
    ) -> Result<(), CognitiveQueryError> {
        let mut argument_types = Vec::new();
        for object in &specification.fixed_arguments {
            argument_types.push(self.submission.interpretations[*object].ty.clone());
        }
        for variable in &specification.variables {
            argument_types.push(self.denoted_type(&variable.expected_type)?);
        }
        let proposition =
            OntologyTypeExpr::named(self.submission.ontology.proposition_type.clone());
        let expected = argument_types
            .into_iter()
            .rev()
            .fold(proposition, |codomain, domain| {
                OntologyTypeExpr::function(domain, codomain)
            });
        if self.submission.interpretations[specification.condition].ty == expected {
            Ok(())
        } else {
            Err(CognitiveQueryError::ExpectedPredicate {
                object: specification.condition.clone(),
            })
        }
    }

    fn provable_goal(&self) -> Option<&ObjectId> {
        match &self.request {
            QueryRequest::Prove { proposition } => Some(proposition),
            QueryRequest::UnderAssumptions { goal, .. } => Some(goal),
            _ => None,
        }
    }

    fn exhaustion_claim(&self) -> Option<&ObjectId> {
        match &self.request {
            QueryRequest::Find {
                exhaustion_claim, ..
            }
            | QueryRequest::Enumerate {
                exhaustion_claim, ..
            } => exhaustion_claim.as_ref(),
            _ => None,
        }
    }

    fn denoted_type(&self, object: &ObjectId) -> Result<OntologyTypeExpr, CognitiveQueryError> {
        match &self.submission.interpretations[object].meaning {
            ObjectMeaning::Type { value } => Ok(value.clone()),
            _ => Err(CognitiveQueryError::ExpectedType(object.clone())),
        }
    }

    fn answer_specification(&self) -> Option<AnswerSpecification<'_>> {
        match &self.request {
            QueryRequest::Find {
                variables,
                condition,
                ..
            }
            | QueryRequest::Enumerate {
                variables,
                condition,
                ..
            } => Some(AnswerSpecification {
                variables: variables.iter().collect(),
                condition,
                fixed_arguments: Vec::new(),
            }),
            QueryRequest::SynthesizeFunction {
                function,
                specification,
            } => Some(AnswerSpecification {
                variables: vec![function],
                condition: specification,
                fixed_arguments: Vec::new(),
            }),
            QueryRequest::Counterexample {
                proposition,
                counterexample,
                condition,
            } => Some(AnswerSpecification {
                variables: vec![counterexample],
                condition,
                fixed_arguments: vec![proposition],
            }),
            QueryRequest::Compare {
                left,
                right,
                result,
                criterion,
            } => Some(AnswerSpecification {
                variables: vec![result],
                condition: criterion,
                fixed_arguments: vec![left, right],
            }),
            QueryRequest::ComputeBound {
                target,
                bound,
                validity,
            } => Some(AnswerSpecification {
                variables: vec![bound],
                condition: validity,
                fixed_arguments: vec![target],
            }),
            QueryRequest::Prove { .. }
            | QueryRequest::Disprove { .. }
            | QueryRequest::UnderAssumptions { .. }
            | QueryRequest::EmpiricalPosterior { .. } => None,
        }
    }

    fn answer_variables(&self) -> Vec<&QueryVariable> {
        self.answer_specification()
            .map_or_else(Vec::new, |specification| specification.variables)
    }

    fn proposition_objects(&self) -> Vec<&ObjectId> {
        let mut objects = match &self.request {
            QueryRequest::Disprove {
                proposition,
                counterclaim,
            } => vec![proposition, counterclaim],
            QueryRequest::Prove { proposition }
            | QueryRequest::Counterexample { proposition, .. }
            | QueryRequest::EmpiricalPosterior { proposition, .. } => vec![proposition],
            QueryRequest::UnderAssumptions { goal, assumptions } => {
                std::iter::once(goal).chain(assumptions).collect()
            }
            QueryRequest::Find { .. }
            | QueryRequest::SynthesizeFunction { .. }
            | QueryRequest::Enumerate { .. }
            | QueryRequest::Compare { .. }
            | QueryRequest::ComputeBound { .. } => Vec::new(),
        };
        if let Some(exhaustion) = self.exhaustion_claim() {
            objects.push(exhaustion);
        }
        objects
    }

    fn referenced_objects(&self) -> Vec<&ObjectId> {
        let mut objects = Vec::new();
        match &self.request {
            QueryRequest::Prove { proposition }
            | QueryRequest::EmpiricalPosterior { proposition, .. } => objects.push(proposition),
            QueryRequest::Disprove {
                proposition,
                counterclaim,
            } => {
                objects.push(proposition);
                objects.push(counterclaim);
            }
            QueryRequest::Find {
                variables,
                condition,
                exhaustion_claim,
            }
            | QueryRequest::Enumerate {
                variables,
                condition,
                exhaustion_claim,
                ..
            } => {
                objects.extend(variables.iter().map(|variable| &variable.expected_type));
                objects.push(condition);
                objects.extend(exhaustion_claim);
            }
            QueryRequest::SynthesizeFunction {
                function,
                specification,
            } => {
                objects.push(&function.expected_type);
                objects.push(specification);
            }
            QueryRequest::Counterexample {
                proposition,
                counterexample,
                condition,
            } => {
                objects.push(proposition);
                objects.push(&counterexample.expected_type);
                objects.push(condition);
            }
            QueryRequest::Compare {
                left,
                right,
                result,
                criterion,
            } => {
                objects.push(left);
                objects.push(right);
                objects.push(&result.expected_type);
                objects.push(criterion);
            }
            QueryRequest::ComputeBound {
                target,
                bound,
                validity,
            } => {
                objects.push(target);
                objects.push(&bound.expected_type);
                objects.push(validity);
            }
            QueryRequest::UnderAssumptions { goal, assumptions } => {
                objects.push(goal);
                objects.extend(assumptions);
            }
        }
        objects
    }

    fn semantic_dependency_closure(&self) -> BTreeSet<ObjectId> {
        let mut pending: Vec<_> = self.referenced_objects().into_iter().cloned().collect();
        let mut seen = BTreeSet::new();
        while let Some(object) = pending.pop() {
            if !seen.insert(object.clone()) {
                continue;
            }
            let Some(interpretation) = self.submission.interpretations.get(&object) else {
                continue;
            };
            let ObjectMeaning::Application { arguments } = &interpretation.meaning else {
                continue;
            };
            let Some(node) = self.submission.graph.nodes.get(&object) else {
                continue;
            };
            if let Some(operator) = &node.operator {
                pending.push(operator.clone());
            }
            for role in arguments {
                if let Some(targets) = node.edges.get(role) {
                    pending.extend(targets.iter().cloned());
                }
            }
        }
        seen
    }
}

fn diagnostic_object(diagnostic: &OntologicalTypingDiagnostic) -> &ObjectId {
    match diagnostic {
        OntologicalTypingDiagnostic::MissingOperator { object }
        | OntologicalTypingDiagnostic::EmptyApplication { object }
        | OntologicalTypingDiagnostic::DuplicateArgumentRole { object, .. }
        | OntologicalTypingDiagnostic::SymbolRoleMismatch { object, .. }
        | OntologicalTypingDiagnostic::OperatorIsNotFunction { object, .. }
        | OntologicalTypingDiagnostic::ArgumentArity { object, .. }
        | OntologicalTypingDiagnostic::UnexpectedRole { object, .. }
        | OntologicalTypingDiagnostic::ArgumentType { object, .. }
        | OntologicalTypingDiagnostic::ResultType { object, .. } => object,
    }
}
