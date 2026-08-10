use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{InterpretationError, InterpretedGraph, ObjectId};

/// Identity of an open query position.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HoleId(pub String);

/// One query hole. Its expected object already has exact harness-supplied ontology typing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryHole {
    /// Human-facing name.
    pub name: String,
    /// Graph object expressing the expected type or constraint.
    pub expected: ObjectId,
}

/// A theory-neutral open query in the same universal language as propositions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenQuery {
    /// Complete exactly interpreted query submission.
    pub submission: InterpretedGraph,
    /// Open positions to be supplied by an answer.
    pub holes: BTreeMap<HoleId, QueryHole>,
}

/// A substitution for every query hole. Ontological typing is supplied at the
/// interpreted-graph boundary; deductive certification is performed by the kernel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryAnswer {
    /// Hole-to-object substitutions.
    pub substitutions: BTreeMap<HoleId, ObjectId>,
}

/// Structural query errors.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum QueryError {
    /// The interpreted submission is malformed or incomplete.
    #[error(transparent)]
    Interpretation(#[from] InterpretationError),
    /// A hole points to a missing expected object.
    #[error("query hole {hole:?} expects missing object {target}")]
    MissingExpected { hole: HoleId, target: ObjectId },
    /// An answer omitted a hole.
    #[error("answer omitted query hole {0:?}")]
    MissingSubstitution(HoleId),
    /// An answer supplied a value not present in the graph.
    #[error("query hole {hole:?} was assigned missing object {target}")]
    MissingSubstitutionTarget { hole: HoleId, target: ObjectId },
    /// An answer contains a hole not declared by the query.
    #[error("answer contains undeclared query hole {0:?}")]
    UnexpectedSubstitution(HoleId),
}

impl OpenQuery {
    /// Validates the query's graph and hole references.
    pub fn validate(&self) -> Result<(), QueryError> {
        self.submission.validate()?;
        for (hole, specification) in &self.holes {
            if !self
                .submission
                .graph
                .nodes
                .contains_key(&specification.expected)
            {
                return Err(QueryError::MissingExpected {
                    hole: hole.clone(),
                    target: specification.expected.clone(),
                });
            }
        }
        Ok(())
    }

    /// Validates only structural completeness of an answer. Ontological and
    /// deductive correctness are checked by the interpreted-graph and kernel layers.
    pub fn validate_answer(&self, answer: &QueryAnswer) -> Result<(), QueryError> {
        self.validate()?;
        for hole in self.holes.keys() {
            let target = answer
                .substitutions
                .get(hole)
                .ok_or_else(|| QueryError::MissingSubstitution(hole.clone()))?;
            if !self.submission.graph.nodes.contains_key(target) {
                return Err(QueryError::MissingSubstitutionTarget {
                    hole: hole.clone(),
                    target: target.clone(),
                });
            }
        }
        for hole in answer.substitutions.keys() {
            if !self.holes.contains_key(hole) {
                return Err(QueryError::UnexpectedSubstitution(hole.clone()));
            }
        }
        Ok(())
    }
}
