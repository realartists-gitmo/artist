//! Facade for the Artist cognitive superstrate.
//!
//! - [`formal`] is total, open representation.
//! - [`kernel`] is the small trusted dependent-type checker.
//! - [`empirical`] records observation- and model-relative cognition.
//! - [`theories`] supplies executable, untrusted theory extensions.

#![forbid(unsafe_code)]

mod artifact;
mod certification;
mod query;

pub use artifact::{ArtifactContract, ArtifactEnvelope, ArtifactError};
pub use certification::{CertifiedClaimError, CertifiedGraphClaim, ProofProductionRecord};
pub use query::{
    CertifiedQueryWitness, CognitiveAnswer, CognitiveQuery, CognitiveQueryError, Continuation,
    DiscoveryContinuation, KernelContinuation, QueryHash, QueryId, QueryPayload, QueryPolicy,
    QueryRequest, QueryStatus, QueryVariable, QueryWitness,
};

/// Empirical/Bayesian records and extension seams.
pub use artist_empirical as empirical;
/// Universal representation layer.
pub use artist_formal as formal;
/// Deductive certification kernel.
pub use artist_kernel as kernel;
/// Executable reflected logics, rewrites, programs, time, and coinduction.
pub use artist_theories as theories;

/// Commonly used types.
pub mod prelude {
    pub use crate::{CertifiedGraphClaim, CognitiveQuery, QueryRequest, QueryStatus};
    pub use artist_empirical::{
        BayesianModel, CertificationClass, InferenceArtifact, InferenceCertification, Observation,
        PosteriorClaim, ProcedureRun,
    };
    pub use artist_formal::{
        GraphRef, InterpretedGraph, InterpretedGraphRef, ObjectGraph, ObjectId,
        ObjectInterpretation, ObjectMeaning, ObjectNode, Ontology, OntologySymbolDeclaration,
        OntologyTypeExpr, Symbol,
    };
    pub use artist_kernel::{
        Certificate, CompiledOntologySubmission, Context, ElaborationRole, Kernel, KernelRequest,
        Name, OntologyCompiler, Term, Theory, TheoryBuilder,
    };
    pub use artist_theories::{
        DiscoveryStatus, FormalAnswer, Object, ObservationAssumption, ReflectedProofSystem,
        TheoryPackage, UniversalVerifier,
    };
}
