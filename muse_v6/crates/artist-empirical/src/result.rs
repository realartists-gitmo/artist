use artist_formal::{InterpretedGraph, InterpretedGraphRef, ObjectMeaning};
use artist_kernel::{
    Certificate, CompiledOntologySubmission, DependencyReport, ElaborationRecord, ElaborationRole,
    Kernel, KernelError, KernelRequest, KernelResult, OntologyCompileError, OntologyCompiler,
    Theory,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{CONVERGES, COVERAGE, ENCLOSURE, EXACT};

/// Stable execution identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(pub String);

/// Complete provenance for an untrusted computation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcedureRun {
    /// Stable run identity.
    pub id: RunId,
    /// Algorithm, program, model, or workflow description.
    pub procedure: InterpretedGraphRef,
    /// Exact configuration object.
    pub configuration: InterpretedGraphRef,
    /// Optional deterministic seed.
    pub seed: Option<String>,
    /// Explicit resource limits or stopping rule.
    pub limits: InterpretedGraphRef,
    /// Diagnostics, traces, convergence checks, or residuals.
    pub diagnostics: Option<InterpretedGraphRef>,
}

/// Principled display class derived from the exact ontology constructor of a
/// kernel-certified proposition, rather than stored as editable confidence metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CertificationClass {
    /// Certificate proves an `Exact` claim.
    Exact,
    /// Certificate proves an `Enclosure` claim.
    VerifiedEnclosure,
    /// Certificate proves a finite-sample `Coverage` claim.
    CertifiedCoverage,
    /// Certificate proves a `Converges` claim.
    Asymptotic,
    /// Certificate proves another exact formal proposition.
    CertifiedOther,
    /// No certificate is attached to the reported output.
    Heuristic,
}

/// Complete graph-to-DTT certificate for one empirical claim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceCertification {
    /// Recomputable elaboration of the exact interpreted claim root.
    pub elaboration: ElaborationRecord,
    /// Kernel certificate proving that elaborated proposition.
    pub certificate: Certificate,
    /// Exact transitive dependency closure.
    pub dependencies: DependencyReport,
}

/// Output of an empirical procedure. The exact interpreted claim and run are always
/// retained; certification is optional and never inferred from confidence metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceArtifact {
    /// Reported proposition or numerical claim under one exact ontology.
    pub claim: InterpretedGraphRef,
    /// Exact untrusted procedure run.
    pub procedure: ProcedureRun,
    /// Optional complete deductive certification of `claim`.
    pub certification: Option<InferenceCertification>,
}

/// Empirical artifact validation errors.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum InferenceError {
    /// Claim reference does not identify the supplied exact interpreted submission.
    #[error("empirical claim does not belong to the supplied interpreted submission")]
    ClaimIdentityMismatch,
    /// Stored elaboration is not the deterministic elaboration of the exact claim.
    #[error("invalid empirical claim elaboration: {0}")]
    InvalidElaboration(OntologyCompileError),
    /// Certification does not elaborate the exact claim as a proposition.
    #[error("empirical certification does not target the exact claim proposition")]
    NotAProposition,
    /// Certificate proves another proposition or targets another theory.
    #[error("empirical certificate does not match the exact elaborated claim")]
    CertificateMismatch,
    /// Supplied certificate was rejected.
    #[error("empirical certificate was rejected: {0}")]
    InvalidCertificate(KernelError),
    /// Kernel returned an impossible result shape.
    #[error("kernel returned an unexpected result for an empirical certificate")]
    UnexpectedKernelResult,
    /// Stored dependency disclosure is incomplete or altered.
    #[error("empirical dependency report does not match the certificate closure")]
    DependencyMismatch,
}

impl InferenceArtifact {
    fn derived_classification(&self, submission: &InterpretedGraph) -> CertificationClass {
        if self.certification.is_none() {
            return CertificationClass::Heuristic;
        }
        let Some(interpretation) = submission.interpretations.get(&self.claim.root) else {
            return CertificationClass::CertifiedOther;
        };
        let ObjectMeaning::Application { .. } = &interpretation.meaning else {
            return CertificationClass::CertifiedOther;
        };
        let Some(node) = submission.graph.nodes.get(&self.claim.root) else {
            return CertificationClass::CertifiedOther;
        };
        let Some(operator) = node.operator.as_ref() else {
            return CertificationClass::CertifiedOther;
        };
        let Some(operator_interpretation) = submission.interpretations.get(operator) else {
            return CertificationClass::CertifiedOther;
        };
        let ObjectMeaning::Symbol { id } = &operator_interpretation.meaning else {
            return CertificationClass::CertifiedOther;
        };
        match id.0.as_str() {
            EXACT => CertificationClass::Exact,
            ENCLOSURE => CertificationClass::VerifiedEnclosure,
            COVERAGE => CertificationClass::CertifiedCoverage,
            CONVERGES => CertificationClass::Asymptotic,
            _ => CertificationClass::CertifiedOther,
        }
    }

    /// Verifies exact claim identity, deterministic elaboration, certificate, and dependencies.
    pub fn verify(
        &self,
        submission: &InterpretedGraph,
        compiled: &CompiledOntologySubmission,
        theory: &Theory,
    ) -> Result<CertificationClass, InferenceError> {
        OntologyCompiler::verify_compiled(submission, compiled)
            .map_err(InferenceError::InvalidElaboration)?;
        if self.claim.submission != submission.canonical_hash()
            || self.claim.graph != submission.graph.canonical_hash()
            || !submission.graph.nodes.contains_key(&self.claim.root)
        {
            return Err(InferenceError::ClaimIdentityMismatch);
        }
        let Some(certification) = &self.certification else {
            return Ok(CertificationClass::Heuristic);
        };
        if certification.elaboration.object != self.claim.root
            || certification.elaboration.role != ElaborationRole::Proposition
        {
            return Err(InferenceError::NotAProposition);
        }
        OntologyCompiler::verify_record(submission, compiled, &certification.elaboration)
            .map_err(InferenceError::InvalidElaboration)?;
        if !theory.is_extension_of(&compiled.theory)
            || certification.certificate.theory != theory.id()
            || certification.certificate.proposition != certification.elaboration.term
        {
            return Err(InferenceError::CertificateMismatch);
        }
        match Kernel::run_to_completion(
            theory.clone(),
            KernelRequest::Certificate {
                certificate: certification.certificate.clone(),
            },
        ) {
            Ok(KernelResult::Certified) => {}
            Ok(_) => return Err(InferenceError::UnexpectedKernelResult),
            Err(error) => return Err(InferenceError::InvalidCertificate(error)),
        }
        if certification.dependencies != certification.certificate.dependencies(theory) {
            return Err(InferenceError::DependencyMismatch);
        }
        Ok(self.derived_classification(submission))
    }
}
