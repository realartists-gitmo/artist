use std::collections::{BTreeMap, BTreeSet};

use artist_formal::{
    InterpretationError, InterpretedGraph, InterpretedGraphHash, ObjectId, ObjectMeaning,
    OntologicalTypingDiagnostic, OntologyHash, OntologySymbolId, OntologyTypeExpr, OntologyTypeId,
    Symbol,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Context, Declaration, DeclarationKind, Kernel, KernelError, KernelRequest, KernelResult, Name,
    Provenance, Term, Theory, TheoryBuilder, TheoryError, TheoryId, Transparency,
};

const COMPILER_VERSION: &str = "artist.ontology-dtt/3";

/// Formal role requested for deterministic ontology-to-kernel elaboration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElaborationRole {
    /// Any ordinary ontological term.
    Term,
    /// A term whose type is a universe.
    Type,
    /// A proposition, represented by a kernel type under propositions-as-types.
    Proposition,
    /// A function, relation, predicate, or other operation.
    Function,
    /// Exact quoted graph syntax/data rather than semantic application.
    QuotedObject,
}

/// Stable generated names for one compiled ontology and interpreted graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OntologyNameMap {
    /// Generated kernel constant for every ontology type.
    pub types: BTreeMap<OntologyTypeId, Name>,
    /// Generated kernel constant for every ontology symbol.
    pub symbols: BTreeMap<OntologySymbolId, Name>,
    /// Type of exact quoted graph objects.
    pub quoted_object_type: Name,
    /// Generated quoted constant for every graph object.
    pub quoted_objects: BTreeMap<ObjectId, Name>,
}

/// Checked deterministic embedding of an exact harness submission into DTT.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledOntologySubmission {
    /// Exact checked base theory extended by ontology compilation.
    pub base: Theory,
    /// Exact graph, ontology, and interpretation-map identity.
    pub submission: InterpretedGraphHash,
    /// Exact graph identity.
    pub graph: artist_formal::GraphHash,
    /// Exact ontology identity.
    pub ontology: OntologyHash,
    /// Complete checked theory containing the ontology declarations and quote constants.
    pub theory: Theory,
    /// Stable generated names.
    pub names: OntologyNameMap,
    /// Compiler contract version.
    pub compiler: String,
}

/// Recomputable record binding one exact graph object to one exact kernel term.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElaborationRecord {
    /// Exact graph, ontology, and interpretation-map identity.
    pub submission: InterpretedGraphHash,
    /// Exact graph identity.
    pub graph: artist_formal::GraphHash,
    /// Exact graph object.
    pub object: ObjectId,
    /// Exact ontology identity.
    pub ontology: OntologyHash,
    /// Exact compiled theory identity.
    pub theory: TheoryId,
    /// Requested role.
    pub role: ElaborationRole,
    /// Deterministically generated kernel term.
    pub term: Term,
    /// Deterministically generated exact kernel type.
    pub ty: Term,
    /// Graph objects recursively used by the elaboration.
    pub object_dependencies: BTreeSet<ObjectId>,
    /// Compiler contract version.
    pub compiler: String,
}

/// Deterministic ontology compilation or elaboration failure.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum OntologyCompileError {
    /// Exact input contract failed.
    #[error(transparent)]
    Interpretation(#[from] InterpretationError),
    /// One or more graph applications are ontologically ill typed.
    #[error("graph contains ontological typing errors: {0:?}")]
    IllTypedApplications(Vec<OntologicalTypingDiagnostic>),
    /// Generated theory declaration failed kernel validation.
    #[error("generated ontology theory is invalid: {0}")]
    Theory(#[from] TheoryError),
    /// A stable generated name already exists with different checked content.
    #[error("generated ontology declaration {0} conflicts with existing theory content")]
    ExistingDeclarationMismatch(Name),
    /// Requested object is absent.
    #[error("graph has no object {0}")]
    MissingObject(ObjectId),
    /// A semantic application recursively depends on itself.
    #[error("graph object {0} is cyclic as a semantic term; use its exact quoted representation")]
    CyclicSemanticTerm(ObjectId),
    /// An application has no usable declared operation.
    #[error("graph object {0} has no deterministic ontology operation")]
    InvalidApplication(ObjectId),
    /// Compiled submission is not the deterministic result for its stored base and input.
    #[error("compiled ontology submission does not match deterministic recompilation")]
    CompiledSubmissionMismatch,
    /// Elaboration record targets a different compiled submission.
    #[error("elaboration record does not target this compiled submission")]
    RecordIdentityMismatch,
    /// Term does not satisfy the requested formal role.
    #[error("graph object {object} does not satisfy requested role {role:?}")]
    RoleMismatch {
        object: ObjectId,
        role: ElaborationRole,
    },
    /// Deterministically generated term does not inhabit its recorded exact type.
    #[error("generated elaboration for graph object {object} is invalid: {source}")]
    InvalidGeneratedTerm {
        object: ObjectId,
        source: KernelError,
    },
    /// Kernel returned an impossible result shape while validating generated output.
    #[error("kernel returned an unexpected result for generated graph object {0}")]
    UnexpectedKernelResult(ObjectId),
}

/// Deterministic compiler from exact harness ontology into the fixed DTT kernel.
#[derive(Clone, Copy, Debug, Default)]
pub struct OntologyCompiler;

impl OntologyCompiler {
    /// Extends `base` with every ontology declaration and exact quoted object.
    ///
    /// No meaning is inferred. Every generated declaration is a mechanical image of
    /// the harness-supplied ontology or content-addressed graph structure.
    pub fn compile(
        base: Theory,
        submission: &InterpretedGraph,
    ) -> Result<CompiledOntologySubmission, OntologyCompileError> {
        submission.validate()?;

        let submission_hash = submission.canonical_hash();
        let ontology_hash = submission.ontology.canonical_hash();
        let graph_hash = submission.graph.canonical_hash();
        let ontology_prefix = format!("artist.ontology/{}", ontology_hash.0);
        let submission_prefix = format!("artist.submission/{}", submission_hash.0);
        let stored_base = base.clone();
        let mut builder = TheoryBuilder::from_theory(base)?;
        let mut type_names = BTreeMap::new();

        for (id, declaration) in &submission.ontology.types {
            let name = Name::new(format!("{ontology_prefix}/type/{}", id.0));
            if id == &submission.ontology.proposition_type {
                ensure_definition(
                    &mut builder,
                    name.clone(),
                    Term::universe(1),
                    Term::universe(0),
                    generated_provenance(&submission.ontology.id().0, "proposition universe"),
                )?;
            } else {
                ensure_axiom(
                    &mut builder,
                    name.clone(),
                    Term::universe(declaration.universe),
                    generated_provenance(&submission.ontology.id().0, "ontology type"),
                )?;
            }
            type_names.insert(id.clone(), name);
        }

        let quoted_object_type = Name::new(format!("{ontology_prefix}/QuotedObject"));
        ensure_axiom(
            &mut builder,
            quoted_object_type.clone(),
            Term::universe(0),
            generated_provenance(&submission.ontology.id().0, "quoted graph object type"),
        )?;

        let mut symbol_names = BTreeMap::new();
        for (id, declaration) in &submission.ontology.symbols {
            let name = Name::new(format!("{ontology_prefix}/symbol/{}", id.0));
            let ty = compile_type(
                &declaration.full_type(),
                &submission.ontology.proposition_type,
                &type_names,
                &quoted_object_type,
            );
            ensure_axiom(
                &mut builder,
                name.clone(),
                ty,
                generated_provenance(&submission.ontology.id().0, "ontology symbol"),
            )?;
            symbol_names.insert(id.clone(), name);
        }

        let mut quoted_objects = BTreeMap::new();
        for object in submission.graph.nodes.keys() {
            let name = Name::new(format!("{submission_prefix}/object/{}", object.0));
            ensure_axiom(
                &mut builder,
                name.clone(),
                Term::constant(quoted_object_type.clone()),
                Provenance {
                    source: COMPILER_VERSION.to_owned(),
                    external_ref: Some(format!("submission:{}#{}", submission_hash.0, object.0)),
                    note: Some("exact content-addressed quoted interpreted object".to_owned()),
                },
            )?;
            quoted_objects.insert(object.clone(), name);
        }

        Ok(CompiledOntologySubmission {
            base: stored_base,
            submission: submission_hash,
            graph: graph_hash,
            ontology: ontology_hash,
            theory: builder.finish(),
            names: OntologyNameMap {
                types: type_names,
                symbols: symbol_names,
                quoted_object_type,
                quoted_objects,
            },
            compiler: COMPILER_VERSION.to_owned(),
        })
    }

    /// Verifies a compiled submission by exact deterministic recompilation.
    pub fn verify_compiled(
        submission: &InterpretedGraph,
        compiled: &CompiledOntologySubmission,
    ) -> Result<(), OntologyCompileError> {
        let expected = Self::compile(compiled.base.clone(), submission)?;
        if &expected == compiled {
            Ok(())
        } else {
            Err(OntologyCompileError::CompiledSubmissionMismatch)
        }
    }

    /// Elaborates one exact object into a kernel term under the compiled submission.
    pub fn elaborate(
        submission: &InterpretedGraph,
        compiled: &CompiledOntologySubmission,
        object: &ObjectId,
        role: ElaborationRole,
    ) -> Result<ElaborationRecord, OntologyCompileError> {
        ensure_compiled_identity(submission, compiled)?;
        if !submission.graph.nodes.contains_key(object) {
            return Err(OntologyCompileError::MissingObject(object.clone()));
        }

        let mut dependencies = BTreeSet::new();
        let (term, ty) = if role == ElaborationRole::QuotedObject {
            dependencies.insert(object.clone());
            (
                Term::constant(compiled.names.quoted_objects[object].clone()),
                Term::constant(compiled.names.quoted_object_type.clone()),
            )
        } else {
            let mut visiting = BTreeSet::new();
            elaborate_semantic(
                submission,
                compiled,
                object,
                &mut visiting,
                &mut dependencies,
            )?
        };

        let interpretation = &submission.interpretations[object];
        let role_valid = match role {
            ElaborationRole::Term | ElaborationRole::QuotedObject => true,
            ElaborationRole::Type => {
                matches!(&interpretation.ty, OntologyTypeExpr::Universe { .. })
            }
            ElaborationRole::Proposition => {
                interpretation.ty
                    == OntologyTypeExpr::named(submission.ontology.proposition_type.clone())
            }
            ElaborationRole::Function => {
                matches!(&interpretation.ty, OntologyTypeExpr::Function { .. })
            }
        };
        if !role_valid {
            return Err(OntologyCompileError::RoleMismatch {
                object: object.clone(),
                role,
            });
        }

        match Kernel::run_to_completion(
            compiled.theory.clone(),
            KernelRequest::Check {
                context: Context::new(),
                term: term.clone(),
                expected: ty.clone(),
            },
        ) {
            Ok(KernelResult::Checked) => {}
            Ok(_) => {
                return Err(OntologyCompileError::UnexpectedKernelResult(object.clone()));
            }
            Err(source) => {
                return Err(OntologyCompileError::InvalidGeneratedTerm {
                    object: object.clone(),
                    source,
                });
            }
        }

        Ok(ElaborationRecord {
            submission: compiled.submission.clone(),
            graph: compiled.graph.clone(),
            object: object.clone(),
            ontology: compiled.ontology.clone(),
            theory: compiled.theory.id(),
            role,
            term,
            ty,
            object_dependencies: dependencies,
            compiler: COMPILER_VERSION.to_owned(),
        })
    }

    /// Verifies a stored elaboration record by exact deterministic recomputation.
    pub fn verify_record(
        submission: &InterpretedGraph,
        compiled: &CompiledOntologySubmission,
        record: &ElaborationRecord,
    ) -> Result<(), OntologyCompileError> {
        let expected = Self::elaborate(submission, compiled, &record.object, record.role)?;
        if &expected == record {
            Ok(())
        } else {
            Err(OntologyCompileError::RecordIdentityMismatch)
        }
    }
}

fn ensure_compiled_identity(
    submission: &InterpretedGraph,
    compiled: &CompiledOntologySubmission,
) -> Result<(), OntologyCompileError> {
    if compiled.submission != submission.canonical_hash()
        || compiled.graph != submission.graph.canonical_hash()
        || compiled.ontology != submission.ontology.canonical_hash()
        || compiled.compiler != COMPILER_VERSION
    {
        return Err(OntologyCompileError::RecordIdentityMismatch);
    }
    OntologyCompiler::verify_compiled(submission, compiled)
}

fn elaborate_semantic(
    submission: &InterpretedGraph,
    compiled: &CompiledOntologySubmission,
    object: &ObjectId,
    visiting: &mut BTreeSet<ObjectId>,
    dependencies: &mut BTreeSet<ObjectId>,
) -> Result<(Term, Term), OntologyCompileError> {
    if !visiting.insert(object.clone()) {
        return Err(OntologyCompileError::CyclicSemanticTerm(object.clone()));
    }
    dependencies.insert(object.clone());
    let object_diagnostics: Vec<_> = submission
        .typing_diagnostics()
        .into_iter()
        .filter(|diagnostic| diagnostic_object(diagnostic) == object)
        .collect();
    if !object_diagnostics.is_empty() {
        visiting.remove(object);
        return Err(OntologyCompileError::IllTypedApplications(
            object_diagnostics,
        ));
    }
    let interpretation = submission
        .interpretations
        .get(object)
        .ok_or_else(|| OntologyCompileError::MissingObject(object.clone()))?;
    let result = match &interpretation.meaning {
        ObjectMeaning::Type { value } => {
            let term = compile_type(
                value,
                &submission.ontology.proposition_type,
                &compiled.names.types,
                &compiled.names.quoted_object_type,
            );
            let ty = compile_type(
                &interpretation.ty,
                &submission.ontology.proposition_type,
                &compiled.names.types,
                &compiled.names.quoted_object_type,
            );
            Ok((term, ty))
        }
        ObjectMeaning::Symbol { id } => {
            let term = Term::constant(compiled.names.symbols[id].clone());
            let ty = compile_type(
                &interpretation.ty,
                &submission.ontology.proposition_type,
                &compiled.names.types,
                &compiled.names.quoted_object_type,
            );
            Ok((term, ty))
        }
        ObjectMeaning::Quoted => Ok((
            Term::constant(compiled.names.quoted_objects[object].clone()),
            Term::constant(compiled.names.quoted_object_type.clone()),
        )),
        ObjectMeaning::Application { arguments } => {
            let node = &submission.graph.nodes[object];
            let operator_id = node
                .operator
                .as_ref()
                .ok_or_else(|| OntologyCompileError::InvalidApplication(object.clone()))?;
            let (mut term, _) =
                elaborate_semantic(submission, compiled, operator_id, visiting, dependencies)?;
            for role in arguments {
                let targets = node
                    .edges
                    .get(role)
                    .ok_or_else(|| OntologyCompileError::InvalidApplication(object.clone()))?;
                if targets.len() != 1 {
                    return Err(OntologyCompileError::InvalidApplication(object.clone()));
                }
                let (argument, _) =
                    elaborate_semantic(submission, compiled, &targets[0], visiting, dependencies)?;
                term = Term::app(term, argument);
            }
            let ty = compile_type(
                &interpretation.ty,
                &submission.ontology.proposition_type,
                &compiled.names.types,
                &compiled.names.quoted_object_type,
            );
            Ok((term, ty))
        }
    };
    visiting.remove(object);
    result
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

fn compile_type(
    ty: &OntologyTypeExpr,
    proposition_type: &OntologyTypeId,
    names: &BTreeMap<OntologyTypeId, Name>,
    quoted_object_type: &Name,
) -> Term {
    match ty {
        OntologyTypeExpr::Universe { level } => Term::universe(*level),
        OntologyTypeExpr::QuotedObject => Term::constant(quoted_object_type.clone()),
        OntologyTypeExpr::Named { id } if id == proposition_type => Term::universe(0),
        OntologyTypeExpr::Named { id } => Term::constant(names[id].clone()),
        OntologyTypeExpr::Function { domain, codomain } => Term::pi(
            compile_type(domain, proposition_type, names, quoted_object_type),
            compile_type(codomain, proposition_type, names, quoted_object_type),
        ),
    }
}

fn ensure_axiom(
    builder: &mut TheoryBuilder,
    name: Name,
    ty: Term,
    provenance: Provenance,
) -> Result<(), OntologyCompileError> {
    let expected = Declaration {
        name: name.clone(),
        ty: ty.clone(),
        body: None,
        kind: DeclarationKind::Axiom,
        transparency: Transparency::Opaque,
        provenance: provenance.clone(),
    };
    let snapshot = builder.snapshot();
    if let Some(existing) = snapshot.declaration(&name) {
        if existing == &expected {
            return Ok(());
        }
        return Err(OntologyCompileError::ExistingDeclarationMismatch(name));
    }
    builder.axiom(name, ty, provenance)?;
    Ok(())
}

fn ensure_definition(
    builder: &mut TheoryBuilder,
    name: Name,
    ty: Term,
    body: Term,
    provenance: Provenance,
) -> Result<(), OntologyCompileError> {
    let expected = Declaration {
        name: name.clone(),
        ty: ty.clone(),
        body: Some(body.clone()),
        kind: DeclarationKind::Definition,
        transparency: Transparency::Transparent,
        provenance: provenance.clone(),
    };
    let snapshot = builder.snapshot();
    if let Some(existing) = snapshot.declaration(&name) {
        if existing == &expected {
            return Ok(());
        }
        return Err(OntologyCompileError::ExistingDeclarationMismatch(name));
    }
    builder.define(name, ty, body, provenance)?;
    Ok(())
}

fn generated_provenance(source: &str, note: &str) -> Provenance {
    Provenance {
        source: COMPILER_VERSION.to_owned(),
        external_ref: Some(source.to_owned()),
        note: Some(note.to_owned()),
    }
}

/// Canonical root role used by ontology-aware integrations.
#[must_use]
pub fn main_root() -> Symbol {
    Symbol::from("main")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use artist_formal::{
        GraphBuilder, ObjectInterpretation, ObjectNode, Ontology, OntologyTypeDeclaration,
    };

    fn malformed_submission() -> (InterpretedGraph, ObjectId) {
        let proposition = OntologyTypeId::from("Proposition");
        let syntax = OntologyTypeId::from("Syntax");
        let ontology = Ontology {
            namespace: "test.quote".into(),
            version: "1".into(),
            proposition_type: proposition.clone(),
            types: BTreeMap::from([
                (
                    proposition.clone(),
                    OntologyTypeDeclaration {
                        id: proposition,
                        universe: 1,
                        description: None,
                    },
                ),
                (
                    syntax.clone(),
                    OntologyTypeDeclaration {
                        id: syntax.clone(),
                        universe: 0,
                        description: None,
                    },
                ),
            ]),
            symbols: BTreeMap::new(),
        };
        let mut graph = GraphBuilder::new();
        let root = graph.alloc(ObjectNode::new());
        graph.root("main", root.clone());
        (
            InterpretedGraph {
                graph: graph.finish().unwrap(),
                ontology,
                interpretations: BTreeMap::from([(
                    root.clone(),
                    ObjectInterpretation {
                        ty: OntologyTypeExpr::named(syntax),
                        meaning: ObjectMeaning::Application {
                            arguments: Vec::new(),
                        },
                    },
                )]),
            },
            root,
        )
    }

    #[test]
    fn malformed_semantic_application_remains_quotable() {
        let (submission, root) = malformed_submission();
        submission.validate().unwrap();
        assert!(!submission.typing_diagnostics().is_empty());
        let compiled = OntologyCompiler::compile(Theory::empty("test", "1"), &submission).unwrap();
        assert!(
            OntologyCompiler::elaborate(
                &submission,
                &compiled,
                &root,
                ElaborationRole::QuotedObject,
            )
            .is_ok()
        );
        assert!(matches!(
            OntologyCompiler::elaborate(&submission, &compiled, &root, ElaborationRole::Term,),
            Err(OntologyCompileError::IllTypedApplications(_))
        ));
    }

    #[test]
    fn compiled_submission_is_recomputed_not_trusted() {
        let (submission, _) = malformed_submission();
        let mut compiled =
            OntologyCompiler::compile(Theory::empty("test", "1"), &submission).unwrap();
        compiled.compiler = "tampered".into();
        assert_eq!(
            OntologyCompiler::verify_compiled(&submission, &compiled),
            Err(OntologyCompileError::CompiledSubmissionMismatch)
        );
    }
}
