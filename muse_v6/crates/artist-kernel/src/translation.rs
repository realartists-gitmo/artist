use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Certificate, ConstructorField, Context, Kernel, KernelError, KernelRequest, KernelResult, Name,
    Term, Theory, TheoryId,
};

/// Checked interpretation of every source declaration in a target theory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TheoryTranslation {
    /// Exact source theory.
    pub source: TheoryId,
    /// Exact target theory.
    pub target: TheoryId,
    /// Target term interpreting each source constant.
    pub interpretations: BTreeMap<Name, Term>,
}

/// Translation validation errors.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum TranslationError {
    /// Source theory identity mismatch.
    #[error("translation expects source {claimed}, loaded {loaded}")]
    SourceMismatch { claimed: TheoryId, loaded: TheoryId },
    /// Target theory identity mismatch.
    #[error("translation expects target {claimed}, loaded {loaded}")]
    TargetMismatch { claimed: TheoryId, loaded: TheoryId },
    /// A source constant has no interpretation.
    #[error("source declaration {0} has no target interpretation")]
    MissingInterpretation(Name),
    /// An interpretation has the wrong translated type.
    #[error("interpretation of {name} is invalid: {source}")]
    InvalidInterpretation { name: Name, source: KernelError },
    /// A transparent source definition is not preserved.
    #[error("interpretation of transparent definition {name} changes its meaning: {source}")]
    DefinitionNotPreserved { name: Name, source: KernelError },
    /// Native inductive metadata is not preserved by the declared constant mapping.
    #[error("inductive family {name} is not structurally preserved: {message}")]
    InductiveNotPreserved { name: Name, message: String },
    /// A source or target theory snapshot bypassed checked construction.
    #[error("{role} theory is invalid: {message}")]
    InvalidTheory { role: &'static str, message: String },
    /// Kernel returned an impossible result shape.
    #[error("kernel returned an unexpected result while validating translation")]
    UnexpectedKernelResult,
    /// Translated certificate was rejected.
    #[error("translated certificate is invalid: {0}")]
    InvalidCertificate(KernelError),
}

impl TheoryTranslation {
    /// Creates an empty mapping pinned to exact theory versions.
    #[must_use]
    pub fn new(source: &Theory, target: &Theory) -> Self {
        Self {
            source: source.id(),
            target: target.id(),
            interpretations: BTreeMap::new(),
        }
    }

    /// Adds an interpretation.
    #[must_use]
    pub fn with(mut self, source_name: impl Into<Name>, target_term: Term) -> Self {
        self.interpretations.insert(source_name.into(), target_term);
        self
    }

    /// Verifies declaration typing and preservation of transparent definitions.
    pub fn verify(&self, source: &Theory, target: &Theory) -> Result<(), TranslationError> {
        source
            .validate()
            .map_err(|error| TranslationError::InvalidTheory {
                role: "source",
                message: error.to_string(),
            })?;
        target
            .validate()
            .map_err(|error| TranslationError::InvalidTheory {
                role: "target",
                message: error.to_string(),
            })?;
        self.ensure_theories(source, target)?;
        for declaration in &source.declarations {
            let interpretation = self
                .interpretations
                .get(&declaration.name)
                .ok_or_else(|| TranslationError::MissingInterpretation(declaration.name.clone()))?;
            let translated_type = declaration.ty.replace_constants(&self.interpretations);
            match Kernel::run_to_completion(
                target.clone(),
                KernelRequest::Check {
                    context: Context::new(),
                    term: interpretation.clone(),
                    expected: translated_type,
                },
            ) {
                Ok(KernelResult::Checked) => {}
                Ok(_) => return Err(TranslationError::UnexpectedKernelResult),
                Err(source_error) => {
                    return Err(TranslationError::InvalidInterpretation {
                        name: declaration.name.clone(),
                        source: source_error,
                    });
                }
            }
            if let Some(body) = source.unfold(&declaration.name) {
                let translated_body = body.replace_constants(&self.interpretations);
                match Kernel::run_to_completion(
                    target.clone(),
                    KernelRequest::DefEq {
                        context: Context::new(),
                        left: interpretation.clone(),
                        right: translated_body,
                    },
                ) {
                    Ok(KernelResult::DefinitionallyEqual) => {}
                    Ok(_) => return Err(TranslationError::UnexpectedKernelResult),
                    Err(source_error) => {
                        return Err(TranslationError::DefinitionNotPreserved {
                            name: declaration.name.clone(),
                            source: source_error,
                        });
                    }
                }
            }
        }
        self.verify_inductives(source, target)?;
        Ok(())
    }

    fn verify_inductives(&self, source: &Theory, target: &Theory) -> Result<(), TranslationError> {
        for source_inductive in &source.inductives {
            let target_name = self
                .interpreted_constant_name(&source_inductive.name)
                .ok_or_else(|| TranslationError::InductiveNotPreserved {
                    name: source_inductive.name.clone(),
                    message: "family interpretation is not a target constant".to_owned(),
                })?;
            let target_inductive = target.inductive(&target_name).ok_or_else(|| {
                TranslationError::InductiveNotPreserved {
                    name: source_inductive.name.clone(),
                    message: format!("target has no inductive family {target_name}"),
                }
            })?;
            if source_inductive.universe != target_inductive.universe
                || source_inductive.parameters.len() != target_inductive.parameters.len()
                || source_inductive.indices.len() != target_inductive.indices.len()
                || source_inductive.constructors.len() != target_inductive.constructors.len()
            {
                return Err(TranslationError::InductiveNotPreserved {
                    name: source_inductive.name.clone(),
                    message: "universe or telescope/constructor arity differs".to_owned(),
                });
            }
            let translated_parameters: Vec<_> = source_inductive
                .parameters
                .iter()
                .map(|term| term.replace_constants(&self.interpretations))
                .collect();
            let translated_indices: Vec<_> = source_inductive
                .indices
                .iter()
                .map(|term| term.replace_constants(&self.interpretations))
                .collect();
            if translated_parameters != target_inductive.parameters
                || translated_indices != target_inductive.indices
            {
                return Err(TranslationError::InductiveNotPreserved {
                    name: source_inductive.name.clone(),
                    message: "translated parameter or index telescope differs".to_owned(),
                });
            }
            for (source_constructor, target_constructor) in source_inductive
                .constructors
                .iter()
                .zip(&target_inductive.constructors)
            {
                let mapped_constructor =
                    self.interpreted_constant_name(&source_constructor.name)
                        .ok_or_else(|| TranslationError::InductiveNotPreserved {
                            name: source_inductive.name.clone(),
                            message: format!(
                                "constructor {name} is not interpreted by a target constant",
                                name = source_constructor.name,
                            ),
                        })?;
                if mapped_constructor != target_constructor.name
                    || source_constructor.fields.len() != target_constructor.fields.len()
                {
                    return Err(TranslationError::InductiveNotPreserved {
                        name: source_inductive.name.clone(),
                        message: format!(
                            "constructor {source_name} does not correspond to {target_name}",
                            source_name = source_constructor.name,
                            target_name = target_constructor.name,
                        ),
                    });
                }
                for (source_field, target_field) in source_constructor
                    .fields
                    .iter()
                    .zip(&target_constructor.fields)
                {
                    if !field_translation_matches(source_field, target_field, &self.interpretations)
                    {
                        return Err(TranslationError::InductiveNotPreserved {
                            name: source_inductive.name.clone(),
                            message: format!(
                                "constructor {name} has a changed recursive shape",
                                name = source_constructor.name,
                            ),
                        });
                    }
                }
                let translated_results: Vec<_> = source_constructor
                    .result_indices
                    .iter()
                    .map(|term| term.replace_constants(&self.interpretations))
                    .collect();
                if translated_results != target_constructor.result_indices {
                    return Err(TranslationError::InductiveNotPreserved {
                        name: source_inductive.name.clone(),
                        message: format!(
                            "constructor {name} has changed result indices",
                            name = source_constructor.name,
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    fn interpreted_constant_name(&self, source: &Name) -> Option<Name> {
        match self.interpretations.get(source) {
            Some(Term::Const { name }) => Some(name.clone()),
            _ => None,
        }
    }

    /// Translates and rechecks a source certificate in the target theory.
    pub fn translate_certificate(
        &self,
        source: &Theory,
        target: &Theory,
        certificate: &Certificate,
    ) -> Result<Certificate, TranslationError> {
        self.verify(source, target)?;
        if certificate.theory != source.id() {
            return Err(TranslationError::SourceMismatch {
                claimed: certificate.theory.clone(),
                loaded: source.id(),
            });
        }
        let translated = Certificate::new(
            target,
            Context(
                certificate
                    .context
                    .0
                    .iter()
                    .map(|term| term.replace_constants(&self.interpretations))
                    .collect(),
            ),
            certificate
                .proposition
                .replace_constants(&self.interpretations),
            certificate.proof.replace_constants(&self.interpretations),
        );
        match Kernel::run_to_completion(
            target.clone(),
            KernelRequest::Certificate {
                certificate: translated.clone(),
            },
        ) {
            Ok(KernelResult::Certified) => Ok(translated),
            Ok(_) => Err(TranslationError::UnexpectedKernelResult),
            Err(error) => Err(TranslationError::InvalidCertificate(error)),
        }
    }

    fn ensure_theories(&self, source: &Theory, target: &Theory) -> Result<(), TranslationError> {
        let source_loaded = source.id();
        if self.source != source_loaded {
            return Err(TranslationError::SourceMismatch {
                claimed: self.source.clone(),
                loaded: source_loaded,
            });
        }
        let target_loaded = target.id();
        if self.target != target_loaded {
            return Err(TranslationError::TargetMismatch {
                claimed: self.target.clone(),
                loaded: target_loaded,
            });
        }
        Ok(())
    }
}

fn field_translation_matches(
    source: &ConstructorField,
    target: &ConstructorField,
    interpretations: &BTreeMap<Name, Term>,
) -> bool {
    match (source, target) {
        (ConstructorField::Plain { ty: source }, ConstructorField::Plain { ty: target }) => {
            &source.replace_constants(interpretations) == target
        }
        (
            ConstructorField::Recursive { indices: source },
            ConstructorField::Recursive { indices: target },
        ) => source
            .iter()
            .map(|term| term.replace_constants(interpretations))
            .eq(target.iter().cloned()),
        (
            ConstructorField::RecursiveFunction {
                domain: source_domain,
                indices: source_indices,
            },
            ConstructorField::RecursiveFunction {
                domain: target_domain,
                indices: target_indices,
            },
        ) => {
            &source_domain.replace_constants(interpretations) == target_domain
                && source_indices
                    .iter()
                    .map(|term| term.replace_constants(interpretations))
                    .eq(target_indices.iter().cloned())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InductiveConstructor, InductiveDeclaration, Provenance, TheoryBuilder};

    fn natural_theory(namespace: &str, family: &str, zero: &str, succ: &str) -> Theory {
        let mut builder = TheoryBuilder::new(namespace, "1");
        builder
            .inductive(InductiveDeclaration {
                name: family.into(),
                parameters: vec![],
                indices: vec![],
                universe: 0,
                constructors: vec![
                    InductiveConstructor {
                        name: zero.into(),
                        fields: vec![],
                        result_indices: vec![],
                    },
                    InductiveConstructor {
                        name: succ.into(),
                        fields: vec![ConstructorField::Recursive { indices: vec![] }],
                        result_indices: vec![],
                    },
                ],
                provenance: Provenance::new("translation fixture"),
            })
            .unwrap();
        builder.finish()
    }

    #[test]
    fn translation_preserves_native_inductive_shape() {
        let source = natural_theory("source", "Nat", "zero", "succ");
        let target = natural_theory("target", "N", "z", "s");
        let translation = TheoryTranslation::new(&source, &target)
            .with("Nat", Term::constant("N"))
            .with("zero", Term::constant("z"))
            .with("succ", Term::constant("s"));
        assert_eq!(translation.verify(&source, &target), Ok(()));
    }

    #[test]
    fn translation_cannot_erase_inductive_metadata() {
        let source = natural_theory("source", "Nat", "zero", "succ");
        let mut target_builder = TheoryBuilder::new("target", "1");
        target_builder
            .axiom("N", Term::universe(0), Provenance::new("fixture"))
            .unwrap();
        target_builder
            .axiom("z", Term::constant("N"), Provenance::new("fixture"))
            .unwrap();
        target_builder
            .axiom(
                "s",
                Term::pi(Term::constant("N"), Term::constant("N")),
                Provenance::new("fixture"),
            )
            .unwrap();
        let target = target_builder.finish();
        let translation = TheoryTranslation::new(&source, &target)
            .with("Nat", Term::constant("N"))
            .with("zero", Term::constant("z"))
            .with("succ", Term::constant("s"));
        assert!(matches!(
            translation.verify(&source, &target),
            Err(TranslationError::InductiveNotPreserved { .. })
        ));
    }
}
