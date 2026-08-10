//! Empirical cognition records layered over the deductive kernel.
//!
//! Observations are provenance-bearing events, never logical axioms by default.
//! Bayesian claims expose their model, prior/likelihood objects, dependency
//! assumptions, data, procedure, and certification status. This crate defines no
//! privileged inference algorithm.

#![forbid(unsafe_code)]

mod bayes;
mod observation;
mod result;
mod vocabulary;

pub use bayes::{
    BayesianError, BayesianInferenceEngine, BayesianModel, Dataset, HypothesisGenerator,
    InferenceMethod, ModelComparison, ModelComparisonEngine, ModelId, PosteriorClaim,
};
pub use observation::{
    Acquisition, ClockId, ClockRelation, ClockRelationError, EpistemicStatus, Observation,
    ObservationError, ObservationId, ObservationLedgerError, Source, TimeInterval, Timestamp,
    validate_observation_ledger,
};
pub use result::{
    CertificationClass, InferenceArtifact, InferenceCertification, InferenceError, ProcedureRun,
    RunId,
};
pub use vocabulary::{
    CONVERGES, COVERAGE, DATASET, ENCLOSURE, ESTIMATE, EXACT, MODEL, POSTERIOR, PROBABILITY,
    PROPOSITION, QUANTITY, SEQUENCE, VALUE, install_empirical_vocabulary,
};
