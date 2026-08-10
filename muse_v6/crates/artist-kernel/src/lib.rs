//! Small dependent-type certificate kernel for Artist cognition.
//!
//! The trusted calculus is intensional dependent type theory with predicative
//! cumulative universes, Π, Σ, identity/J, checked strictly-positive inductive
//! families, local definitions, and an append-only theory environment.
//! The kernel performs no proof search. It checks explicit terms and exposes an
//! exact, serializable, resumable machine state.

#![forbid(unsafe_code)]

mod artifact_codec;
mod certificate;
mod codec;
mod machine;
mod ontology;
mod query;
mod term;
mod theory;
mod translation;

pub use artifact_codec::{KernelArtifactCodec, KernelArtifactCodecError};
pub use certificate::{Certificate, Dependency, DependencyReport};
pub use codec::{CoreCodecError, CoreGraphCodec};
pub use machine::{CheckSession, Kernel, KernelError, KernelRequest, KernelResult, SessionStatus};
pub use ontology::{
    CompiledOntologySubmission, ElaborationRecord, ElaborationRole, OntologyCompileError,
    OntologyCompiler, OntologyNameMap, main_root,
};
pub use query::{CertifiedQueryAnswer, KernelQuery, QueryError, QueryParameter};
pub use term::{Context, Name, Term, instantiate, instantiate_under, shift, subst_at, subst_top};
pub use theory::{
    ConstructorField, Declaration, DeclarationKind, InductiveConstructor, InductiveDeclaration,
    Provenance, Theory, TheoryBuilder, TheoryError, TheoryId, Transparency,
};
pub use translation::{TheoryTranslation, TranslationError};
