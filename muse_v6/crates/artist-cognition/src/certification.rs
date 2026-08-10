use artist_formal::{InterpretedGraph, ObjectId};
use artist_kernel::{
    Certificate, CompiledOntologySubmission, DependencyReport, ElaborationRecord, ElaborationRole,
    Kernel, KernelError, KernelRequest, KernelResult, OntologyCompileError, OntologyCompiler,
};
use artist_theories::{
    ComputationCertificate, UniversalCertificate, UniversalError, UniversalVerifier,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Optional provenance showing how an untrusted producer obtained a candidate proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "producer", rename_all = "snake_case")]
pub enum ProofProductionRecord {
    /// A universal certificate checker accepted an external proof object.
    Universal {
        verifier: UniversalVerifier,
        certificate: UniversalCertificate,
    },
    /// A universal computation produced the candidate result.
    Computation { certificate: ComputationCertificate },
    /// Opaque procedure identity retained without granting logical force.
    External { identity: String, version: String },
}

impl ProofProductionRecord {
    /// Validates any finite production trace without granting it deductive authority.
    /// The final DTT certificate must still pass independently.
    pub fn verify(&self) -> Result<(), UniversalError> {
        match self {
            Self::Universal {
                verifier,
                certificate,
            } => verifier.check(certificate),
            Self::Computation { certificate } => certificate.check(),
            Self::External { .. } => Ok(()),
        }
    }
}

/// Complete proof-carrying answer for one exactly interpreted graph proposition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertifiedGraphClaim {
    /// Exact graph proposition root.
    pub claim: ObjectId,
    /// Recomputable ontology-to-DTT elaboration.
    pub elaboration: ElaborationRecord,
    /// Kernel proof of the elaborated proposition.
    pub certificate: Certificate,
    /// Exact transitive dependency disclosure.
    pub dependencies: DependencyReport,
    /// Optional untrusted proof-production provenance.
    pub production: Option<ProofProductionRecord>,
}

/// Certified graph-claim rejection.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum CertifiedClaimError {
    /// Elaboration record failed deterministic recomputation.
    #[error("invalid graph elaboration: {0}")]
    InvalidElaboration(OntologyCompileError),
    /// Elaboration is not a proposition.
    #[error("certified graph claim is not elaborated as a proposition")]
    NotAProposition,
    /// Certificate proves another proposition or targets another theory.
    #[error("kernel certificate does not match the exact elaborated claim")]
    CertificateMismatch,
    /// Kernel rejected the proof.
    #[error("kernel rejected the graph claim: {0}")]
    Kernel(KernelError),
    /// Kernel returned an impossible success shape.
    #[error("kernel returned an unexpected result while checking a graph claim")]
    UnexpectedKernelResult,
    /// Dependency disclosure is incomplete or altered.
    #[error("graph claim dependency report does not match the certificate closure")]
    DependencyMismatch,
    /// Universal production evidence is invalid.
    #[error("invalid universal production evidence: {0}")]
    Universal(UniversalError),
}

impl CertifiedGraphClaim {
    /// Verifies the entire graph-to-DTT-to-proof chain.
    pub fn verify(
        &self,
        submission: &InterpretedGraph,
        compiled: &CompiledOntologySubmission,
        proof_theory: &artist_kernel::Theory,
    ) -> Result<(), CertifiedClaimError> {
        if self.claim != self.elaboration.object
            || self.elaboration.role != ElaborationRole::Proposition
        {
            return Err(CertifiedClaimError::NotAProposition);
        }
        OntologyCompiler::verify_record(submission, compiled, &self.elaboration)
            .map_err(CertifiedClaimError::InvalidElaboration)?;
        if !proof_theory.is_extension_of(&compiled.theory)
            || self.certificate.theory != proof_theory.id()
            || self.certificate.proposition != self.elaboration.term
        {
            return Err(CertifiedClaimError::CertificateMismatch);
        }
        match Kernel::run_to_completion(
            proof_theory.clone(),
            KernelRequest::Certificate {
                certificate: self.certificate.clone(),
            },
        ) {
            Ok(KernelResult::Certified) => {}
            Ok(_) => return Err(CertifiedClaimError::UnexpectedKernelResult),
            Err(error) => return Err(CertifiedClaimError::Kernel(error)),
        }
        if self.dependencies != self.certificate.dependencies(proof_theory) {
            return Err(CertifiedClaimError::DependencyMismatch);
        }
        if let Some(production) = &self.production {
            production
                .verify()
                .map_err(CertifiedClaimError::Universal)?;
        }
        Ok(())
    }
}
