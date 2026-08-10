//! Umbrella facade for the unified Muse semantic and cognitive workspace.

#![forbid(unsafe_code)]

#[cfg(feature = "classification")]
pub use muse_classification as classification;

#[cfg(feature = "cognition")]
pub use artist_cognition as cognition;
#[cfg(feature = "empirical")]
pub use artist_empirical as empirical;
#[cfg(feature = "formal")]
pub use artist_formal as formal;
#[cfg(feature = "kernel")]
pub use artist_kernel as kernel;
#[cfg(feature = "theories")]
pub use artist_theories as theories;
#[cfg(feature = "core")]
pub use muse_core as core;
#[cfg(feature = "interpretation")]
pub use muse_interpretation as interpretation;
#[cfg(feature = "io")]
pub use muse_io as io;
#[cfg(feature = "lexicon")]
pub use muse_lexicon as lexicon;
#[cfg(feature = "occurrence")]
pub use muse_occurrence as occurrence;
#[cfg(feature = "ontology")]
pub use muse_ontology as ontology;
#[cfg(feature = "ontology")]
pub use muse_provenance as provenance;
#[cfg(feature = "reasoning")]
pub use muse_reasoning as reasoning;
#[cfg(feature = "registry")]
pub use muse_registry as registry;
#[cfg(feature = "resolution")]
pub use muse_resolution as resolution;
#[cfg(feature = "superstrate")]
pub use muse_superstrate as superstrate;
#[cfg(feature = "tooling")]
pub use muse_tooling as tooling;
#[cfg(feature = "training")]
pub use muse_training as training;
#[cfg(feature = "validation")]
pub use muse_validation as validation;

#[cfg(feature = "full")]
use muse_classification::{ClassificationRequest, OntologyClassifier, RegistryClassifier};
#[cfg(feature = "full")]
use muse_interpretation::{
    DeterministicInterpreter, InterpretationBundle, InterpretationError, InterpretationRequest,
    SemanticInterpreter,
};
#[cfg(feature = "full")]
use muse_reasoning::{
    ForwardReasoner, KnowledgeBase, ReasoningError, ReasoningOptions, ReasoningReport,
};
#[cfg(feature = "full")]
use muse_registry::{PackageRegistry, RegistryError, RegistrySnapshot, SemanticPackage};
#[cfg(feature = "full")]
use muse_resolution::{
    LexicalResolver, RegistryResolver, ResolutionError, ResolutionRequest, ResolutionResult,
};
#[cfg(feature = "full")]
use muse_validation::{
    ValidationError, ValidationOptions, ValidationReport, validate_interpretation_conformance,
    validate_registry,
};
#[cfg(feature = "full")]
use thiserror::Error;

#[cfg(feature = "full")]
#[derive(Debug, Default)]
pub struct Engine {
    registry: PackageRegistry,
}

#[cfg(feature = "full")]
impl Engine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_registry(registry: PackageRegistry) -> Self {
        Self { registry }
    }

    #[must_use]
    pub const fn registry(&self) -> &PackageRegistry {
        &self.registry
    }

    pub fn register(
        &mut self,
        package: SemanticPackage,
    ) -> Result<muse_core::PackageRef, EngineError> {
        Ok(self.registry.register(package)?)
    }

    pub fn snapshot_all(&self) -> Result<RegistrySnapshot, EngineError> {
        Ok(self.registry.snapshot_all()?)
    }

    pub fn resolve(&self, request: &ResolutionRequest) -> Result<ResolutionResult, EngineError> {
        Ok(RegistryResolver::new(&self.registry).resolve(request)?)
    }

    pub fn classify(
        &self,
        request: &ClassificationRequest,
    ) -> Result<muse_ontology::ClassificationResult, EngineError> {
        Ok(RegistryClassifier::new(&self.registry).classify(request)?)
    }

    pub fn interpret(
        &self,
        request: &InterpretationRequest,
    ) -> Result<InterpretationBundle, EngineError> {
        Ok(DeterministicInterpreter::new(&self.registry).interpret(request)?)
    }

    pub fn reason(
        &self,
        snapshot: &RegistrySnapshot,
        knowledge_base: &KnowledgeBase,
        options: &ReasoningOptions,
    ) -> Result<ReasoningReport, EngineError> {
        let index = self.registry.ontology_index(snapshot)?;
        Ok(ForwardReasoner::new(&index).reason(knowledge_base, options)?)
    }

    pub fn validate_interpretation(
        &self,
        bundle: &InterpretationBundle,
        options: &ReasoningOptions,
    ) -> Result<ReasoningReport, EngineError> {
        Ok(validate_interpretation_conformance(
            &self.registry,
            bundle,
            options,
        )?)
    }

    pub fn lower_and_kernel_check(
        &self,
        document: &muse_occurrence::OccurrenceDocument,
    ) -> Result<muse_superstrate::KernelCheckedOccurrenceDocument, EngineError> {
        Ok(muse_superstrate::lower_and_kernel_check(
            &self.registry,
            document,
        )?)
    }

    pub fn validate(
        &self,
        snapshot: &RegistrySnapshot,
        options: &ValidationOptions,
    ) -> Result<ValidationReport, EngineError> {
        Ok(validate_registry(&self.registry, snapshot, options)?)
    }
}

#[cfg(feature = "full")]
#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Classification(#[from] muse_classification::ClassificationError),
    #[error(transparent)]
    Resolution(#[from] ResolutionError),
    #[error(transparent)]
    Interpretation(#[from] InterpretationError),
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Reasoning(#[from] ReasoningError),
    #[error(transparent)]
    Superstrate(#[from] muse_superstrate::LoweringError),
}

#[cfg(feature = "full")]
pub mod prelude {
    pub use crate::{Engine, EngineError};
    pub use artist_cognition::{
        ArtifactEnvelope, CertifiedGraphClaim, CognitiveAnswer, CognitiveQuery,
    };
    pub use artist_empirical::{InferenceArtifact, Observation, PosteriorClaim};
    pub use artist_formal::{InterpretedGraph, ObjectGraph, ObjectId};
    pub use artist_kernel::{Certificate, CompiledOntologySubmission, Kernel, Term, Theory};
    pub use artist_theories::TheoryPackage;
    pub use muse_classification::{
        ClassificationPolicy, ClassificationRequest, OntologyClassifier, RegistryClassifier,
    };
    pub use muse_core::{
        ConceptId, ContentDigest, EvidenceId, InterpretationId, LanguageTag, LexicalEntryId,
        LexicalSenseId, OccurrenceDocumentId, OccurrenceId, PackageId, PackageRef, PackageVersion,
        PropositionId, ReferentId, RelationId, SemanticObjectId, SourceSpanId, StatementId,
    };
    pub use muse_interpretation::{
        DeterministicInterpreter, InterpretationBundle, InterpretationRequest, InterpretationUnit,
        SemanticInterpreter,
    };
    pub use muse_lexicon::{LexicalEntry, LexicalSense, LexiconPackage, PredicateFrame};
    pub use muse_occurrence::{OccurrenceDocument, PresentationMode, PropositionExpr};
    pub use muse_ontology::{
        ClassificationResult, ClassificationStatus, ConceptDeclaration, OntologyIndex,
        OntologyPackage, RelationDeclaration,
    };
    pub use muse_provenance::{EvidenceRecord, ProvenancePackage, SourceRecord};
    pub use muse_reasoning::{
        ConstraintViolation, Fact, ForwardReasoner, KnowledgeBase, ReasoningOptions,
        ReasoningReport, ViolationKind, WorldAssumption,
    };
    pub use muse_registry::{PackageRegistry, RegistrySnapshot, SemanticPackage};
    pub use muse_resolution::{
        LexicalResolver, RegistryResolver, ResolutionRequest, ResolutionResult,
    };
    pub use muse_superstrate::{KernelCheckedOccurrenceDocument, LoweredOccurrenceDocument};
    pub use muse_training::{
        BounceReason, LabelBounce, OpaqueFieldHint, ProseWindow, StructuredContextField,
        StructuredFieldBlock, StructuredFieldValue, StructuredWindow, TrainingContract,
        TrainingWindow,
    };
    pub use muse_validation::{
        ValidationOptions, ValidationReport, validate_interpretation_conformance, validate_registry,
    };
}
