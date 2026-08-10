use std::collections::BTreeSet;

use artist_formal::InterpretedGraphRef;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Observation, ObservationId, ProcedureRun};

/// Content identity of an explicit probabilistic model.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelId(pub String);

/// Every assumption needed to interpret a Bayesian posterior.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BayesianModel {
    /// Stable model identity/version.
    pub id: ModelId,
    /// Hypotheses or latent states assigned prior measure.
    pub hypothesis_space: InterpretedGraphRef,
    /// Prior measure.
    pub prior: InterpretedGraphRef,
    /// Possible observation values.
    pub observation_space: InterpretedGraphRef,
    /// Conditional observation model.
    pub likelihood: InterpretedGraphRef,
    /// Independence, exchangeability, censoring, selection, and related assumptions.
    pub dependence_assumptions: InterpretedGraphRef,
    /// Explicit latent variables, when present.
    #[serde(default)]
    pub latent_variables: Vec<InterpretedGraphRef>,
    /// Optional causal structure. Absence means no causal interpretation is claimed.
    #[serde(default)]
    pub causal_structure: Option<InterpretedGraphRef>,
    /// Parameter space and constraints.
    #[serde(default)]
    pub parameter_space: Option<InterpretedGraphRef>,
    /// Exact observation-to-model encoding.
    #[serde(default)]
    pub observation_encoder: Option<InterpretedGraphRef>,
}

/// Exact observation set used by an empirical computation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dataset {
    /// Stable dataset/event-set identity.
    pub id: String,
    /// Observation identities in deterministic order.
    pub observations: Vec<ObservationId>,
    /// Optional selection/filter rule.
    #[serde(default)]
    pub selection: Option<InterpretedGraphRef>,
}

/// Exact or approximate status of an inference result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum InferenceMethod {
    /// Exact symbolic or finite computation.
    Exact,
    /// Approximation with an explicit deterministic error enclosure.
    BoundedApproximation { error_bound: InterpretedGraphRef },
    /// Approximation justified only by a convergence claim and diagnostics.
    Asymptotic {
        convergence_claim: InterpretedGraphRef,
        diagnostics: InterpretedGraphRef,
    },
    /// Heuristic result with no formal numerical guarantee.
    Heuristic {
        diagnostics: Option<InterpretedGraphRef>,
    },
}

/// Conditional empirical claim, never an unconditional proof of its proposition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PosteriorClaim {
    /// Proposition whose posterior measure is reported.
    pub proposition: InterpretedGraphRef,
    /// Exact model and assumptions.
    pub model: BayesianModel,
    /// Exact conditioning data.
    pub data: Dataset,
    /// Formal representation of the posterior quantity or distribution.
    pub posterior: InterpretedGraphRef,
    /// Procedure that computed or approximated it.
    pub procedure: ProcedureRun,
    /// Exact/approximate/heuristic status.
    #[serde(default = "default_inference_method")]
    pub method: InferenceMethod,
}

fn default_inference_method() -> InferenceMethod {
    InferenceMethod::Heuristic { diagnostics: None }
}

/// Explicit comparison of two models on one dataset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelComparison {
    /// Competing models.
    pub left: BayesianModel,
    pub right: BayesianModel,
    /// Exact conditioning data.
    pub data: Dataset,
    /// Comparison criterion, such as marginal likelihood or predictive score.
    pub criterion: InterpretedGraphRef,
    /// Reported comparison result.
    pub result: InterpretedGraphRef,
    /// Exact untrusted procedure.
    pub procedure: ProcedureRun,
    /// Exact/approximate/heuristic status.
    pub method: InferenceMethod,
}

/// Bayesian contract validation failure.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum BayesianError {
    /// Dataset repeats an immutable observation event.
    #[error("dataset repeats observation {0:?}")]
    DuplicateObservation(ObservationId),
    /// Supplied observations do not exactly match the dataset identity list.
    #[error("dataset observation list does not match supplied observation records")]
    DatasetMismatch,
    /// Procedure diagnostics required by the inference method are absent.
    #[error("inference method requires procedure diagnostics")]
    MissingDiagnostics,
}

impl Dataset {
    /// Validates deterministic uniqueness.
    pub fn validate(&self) -> Result<(), BayesianError> {
        let mut seen = BTreeSet::new();
        for observation in &self.observations {
            if !seen.insert(observation.clone()) {
                return Err(BayesianError::DuplicateObservation(observation.clone()));
            }
        }
        Ok(())
    }

    /// Validates exact agreement with supplied observation records.
    pub fn validate_records(&self, observations: &[Observation]) -> Result<(), BayesianError> {
        self.validate()?;
        let actual: Vec<_> = observations
            .iter()
            .map(|observation| observation.id.clone())
            .collect();
        if actual == self.observations {
            Ok(())
        } else {
            Err(BayesianError::DatasetMismatch)
        }
    }
}

impl PosteriorClaim {
    /// Validates dataset structure and approximation provenance.
    pub fn validate(&self) -> Result<(), BayesianError> {
        self.data.validate()?;
        if matches!(&self.method, InferenceMethod::Asymptotic { .. })
            && self.procedure.diagnostics.is_none()
        {
            return Err(BayesianError::MissingDiagnostics);
        }
        Ok(())
    }
}

/// Untrusted Bayesian inference implementation. Engines produce explicit artifacts;
/// certification, when available, is separately checked by `artist-kernel`.
pub trait BayesianInferenceEngine {
    /// Engine-specific failure.
    type Error;

    /// Computes an explicit posterior claim.
    fn infer(
        &mut self,
        model: &BayesianModel,
        observations: &[Observation],
        proposition: &InterpretedGraphRef,
    ) -> Result<PosteriorClaim, Self::Error>;
}

/// Untrusted model-comparison implementation.
pub trait ModelComparisonEngine {
    /// Engine-specific failure.
    type Error;

    /// Compares two explicit models under one exact criterion and dataset.
    fn compare(
        &mut self,
        left: &BayesianModel,
        right: &BayesianModel,
        observations: &[Observation],
        criterion: &InterpretedGraphRef,
    ) -> Result<ModelComparison, Self::Error>;
}

/// Induction seam. Hypothesis generation remains outside the trusted kernel.
pub trait HypothesisGenerator {
    /// Generator-specific failure.
    type Error;

    /// Produces candidate formal hypotheses with full procedure provenance.
    fn generate(
        &mut self,
        observations: &[Observation],
    ) -> Result<Vec<(InterpretedGraphRef, ProcedureRun)>, Self::Error>;
}
