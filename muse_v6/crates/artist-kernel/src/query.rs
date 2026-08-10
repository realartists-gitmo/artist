use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Certificate, Context, Kernel, KernelError, KernelRequest, KernelResult, Name, Term, Theory,
    TheoryId, instantiate,
};

/// One dependent query parameter. Its type may refer to earlier parameters via
/// de Bruijn indices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryParameter {
    /// Display name; it has no kernel semantics.
    pub name: Name,
    /// Expected type under the preceding query parameters.
    pub ty: Term,
}

/// An open certified goal. Parameters are existential answer positions, not local
/// assumptions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelQuery {
    /// Exact theory version.
    pub theory: TheoryId,
    /// Dependent answer telescope, oldest first.
    pub parameters: Vec<QueryParameter>,
    /// Goal under all query parameters.
    pub goal: Term,
}

/// Witnesses and proof for a kernel query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertifiedQueryAnswer {
    /// Closed witnesses ordered like the query parameters.
    pub witnesses: Vec<Term>,
    /// Proof of the fully instantiated goal.
    pub proof: Term,
}

/// Query validation errors.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum QueryError {
    /// The query targets another theory version.
    #[error("query targets theory {claimed}, loaded {loaded}")]
    TheoryMismatch { claimed: TheoryId, loaded: TheoryId },
    /// Wrong witness count.
    #[error("query requires {expected} witnesses, answer supplied {actual}")]
    WitnessCount { expected: usize, actual: usize },
    /// A parameter type is malformed.
    #[error("query parameter {index} ({name}) is not a type: {source}")]
    InvalidParameter {
        index: usize,
        name: Name,
        source: KernelError,
    },
    /// The goal is not a proposition/type.
    #[error("query goal is not a type: {0}")]
    InvalidGoal(KernelError),
    /// A witness does not inhabit its required dependent type.
    #[error("witness {index} ({name}) is invalid: {source}")]
    InvalidWitness {
        index: usize,
        name: Name,
        source: KernelError,
    },
    /// The final proof is invalid.
    #[error("query proof is invalid: {0}")]
    InvalidProof(KernelError),
    /// Kernel returned an impossible result shape.
    #[error("kernel returned an unexpected result while validating a query")]
    UnexpectedKernelResult,
}

impl KernelQuery {
    /// Creates a query pinned to a theory.
    #[must_use]
    pub fn new(theory: &Theory, parameters: Vec<QueryParameter>, goal: Term) -> Self {
        Self {
            theory: theory.id(),
            parameters,
            goal,
        }
    }

    /// Checks the dependent telescope and goal formation.
    pub fn validate(&self, theory: &Theory) -> Result<(), QueryError> {
        self.ensure_theory(theory)?;
        let mut context = Context::new();
        for (index, parameter) in self.parameters.iter().enumerate() {
            match Kernel::run_to_completion(
                theory.clone(),
                KernelRequest::EnsureUniverse {
                    context: context.clone(),
                    term: parameter.ty.clone(),
                },
            ) {
                Ok(KernelResult::Universe(_)) => {}
                Ok(_) => return Err(QueryError::UnexpectedKernelResult),
                Err(source) => {
                    return Err(QueryError::InvalidParameter {
                        index,
                        name: parameter.name.clone(),
                        source,
                    });
                }
            }
            context = context.extend(parameter.ty.clone());
        }
        match Kernel::run_to_completion(
            theory.clone(),
            KernelRequest::EnsureUniverse {
                context,
                term: self.goal.clone(),
            },
        ) {
            Ok(KernelResult::Universe(_)) => Ok(()),
            Ok(_) => Err(QueryError::UnexpectedKernelResult),
            Err(source) => Err(QueryError::InvalidGoal(source)),
        }
    }

    /// Checks every witness and returns the final accepted certificate.
    pub fn verify_answer(
        &self,
        theory: &Theory,
        answer: &CertifiedQueryAnswer,
    ) -> Result<Certificate, QueryError> {
        self.validate(theory)?;
        if answer.witnesses.len() != self.parameters.len() {
            return Err(QueryError::WitnessCount {
                expected: self.parameters.len(),
                actual: answer.witnesses.len(),
            });
        }
        let mut accepted = Vec::new();
        for (index, (parameter, witness)) in
            self.parameters.iter().zip(&answer.witnesses).enumerate()
        {
            let expected = instantiate(&parameter.ty, &accepted);
            match Kernel::run_to_completion(
                theory.clone(),
                KernelRequest::Check {
                    context: Context::new(),
                    term: witness.clone(),
                    expected,
                },
            ) {
                Ok(KernelResult::Checked) => accepted.push(witness.clone()),
                Ok(_) => return Err(QueryError::UnexpectedKernelResult),
                Err(source) => {
                    return Err(QueryError::InvalidWitness {
                        index,
                        name: parameter.name.clone(),
                        source,
                    });
                }
            }
        }
        let proposition = instantiate(&self.goal, &accepted);
        let certificate =
            Certificate::new(theory, Context::new(), proposition, answer.proof.clone());
        match Kernel::run_to_completion(
            theory.clone(),
            KernelRequest::Certificate {
                certificate: certificate.clone(),
            },
        ) {
            Ok(KernelResult::Certified) => Ok(certificate),
            Ok(_) => Err(QueryError::UnexpectedKernelResult),
            Err(source) => Err(QueryError::InvalidProof(source)),
        }
    }

    fn ensure_theory(&self, theory: &Theory) -> Result<(), QueryError> {
        let loaded = theory.id();
        if self.theory == loaded {
            Ok(())
        } else {
            Err(QueryError::TheoryMismatch {
                claimed: self.theory.clone(),
                loaded,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Provenance, TheoryBuilder};

    #[test]
    fn query_answer_binds_witness_and_proof() {
        let mut builder = TheoryBuilder::new("numbers", "1");
        builder
            .axiom("Nat", Term::universe(0), Provenance::new("fixture"))
            .unwrap();
        builder
            .axiom("zero", Term::constant("Nat"), Provenance::new("fixture"))
            .unwrap();
        let theory = builder.finish();

        // Find x : Nat and prove Nat. The goal ignores x, but the proof is still
        // checked after exact telescope instantiation.
        let query = KernelQuery::new(
            &theory,
            vec![QueryParameter {
                name: Name::from("x"),
                ty: Term::constant("Nat"),
            }],
            Term::universe(0),
        );
        let answer = CertifiedQueryAnswer {
            witnesses: vec![Term::constant("zero")],
            proof: Term::constant("Nat"),
        };
        assert!(query.verify_answer(&theory, &answer).is_ok());
    }
}
