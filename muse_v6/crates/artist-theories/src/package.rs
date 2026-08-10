use std::collections::{BTreeMap, BTreeSet};

use artist_kernel::{
    Certificate, DeclarationKind, Kernel, KernelError, KernelRequest, KernelResult, Name, Theory,
    TheoryError, TheoryId, TheoryTranslation, TranslationError,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{CheckerCompilation, Object, Symbol, TheoryPackage, UniversalError};

/// Canonical theory-package format version.
pub const THEORY_PACKAGE_FORMAT: u32 = 1;

/// Content identity of a complete package.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PackageId(pub String);

/// Exact import of another isolated package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageImport {
    /// Local import alias.
    pub alias: Symbol,
    /// Exact imported package identity.
    pub package: PackageId,
    /// Exact imported kernel identity.
    pub theory: TheoryId,
}

/// Untrusted deterministic elaboration rule description.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElaborationRule {
    /// Rule identity.
    pub name: Symbol,
    /// Exact source operator or syntactic form.
    pub source: Object,
    /// Exact target role such as proposition, term, type, or query.
    pub target_role: Symbol,
    /// Reflected implementation or specification.
    pub implementation: Object,
    /// Optional finite checker compilation validating emitted records.
    pub checker: Option<Symbol>,
}

/// Surface notation that carries no independent logical force.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotationRule {
    /// Notation identity.
    pub name: Symbol,
    /// Parser/printer description.
    pub syntax: Object,
    /// Exact elaborated constructor or operator.
    pub target: Object,
}

/// Explicit visibility policy for assumptions supplied by a package.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssumptionVisibility {
    /// Available to importers and always disclosed in dependency reports.
    Public,
    /// Usable only inside the package; exported theorems remain usable.
    PackagePrivate,
}

/// Semantic model or metatheoretic artifact carried above the kernel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticArtifact {
    /// Stable artifact identity.
    pub name: Symbol,
    /// Exact reflected model, semantics, or claim.
    pub content: Object,
    /// Optional checked theorem about the artifact.
    pub certificate: Option<artist_kernel::Certificate>,
}

/// Complete extension metadata omitted from the historical minimal package struct.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageExtensions {
    /// Exact isolated imports.
    pub imports: Vec<PackageImport>,
    /// Theory-aware elaboration rules.
    pub elaboration_rules: Vec<ElaborationRule>,
    /// Surface syntax and notation.
    pub notation: Vec<NotationRule>,
    /// Universal certificate systems.
    pub certificate_systems: BTreeMap<Symbol, CheckerCompilation>,
    /// Source-pinned translations. Full target validation requires
    /// [`TheoryPackage::validate_translation`].
    pub translations: Vec<TheoryTranslation>,
    /// Semantic models and metatheorems.
    pub semantic_artifacts: Vec<SemanticArtifact>,
    /// Explicit assumption visibility overrides.
    pub assumption_visibility: BTreeMap<Name, AssumptionVisibility>,
    /// Optional conservative-extension claims, checked as ordinary certificates.
    pub conservative_extension_claims: Vec<artist_kernel::Certificate>,
    /// Optional bridge theorems, checked as ordinary certificates.
    pub bridge_theorems: Vec<artist_kernel::Certificate>,
}

/// Theory-package format or composition failure.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum PackageError {
    /// Format version is unsupported.
    #[error("unsupported theory package format {0}")]
    UnsupportedFormat(u32),
    /// Embedded theory is invalid.
    #[error("invalid kernel theory: {0}")]
    InvalidTheory(TheoryError),
    /// Duplicate named component.
    #[error("duplicate package component {0:?}")]
    DuplicateComponent(Symbol),
    /// Import aliases collide.
    #[error("duplicate package import alias {0:?}")]
    DuplicateImportAlias(Symbol),
    /// Assumption visibility names a non-assumption or absent declaration.
    #[error("assumption visibility targets invalid declaration {0}")]
    InvalidAssumptionVisibility(Name),
    /// Universal checker is malformed.
    #[error("invalid certificate system {name:?}: {source}")]
    InvalidCertificateSystem {
        name: Symbol,
        source: UniversalError,
    },
    /// An elaboration rule names no installed checker.
    #[error("elaboration rule {rule:?} references missing checker {checker:?}")]
    MissingChecker { rule: Symbol, checker: Symbol },
    /// Embedded checked artifact is rejected by the package kernel.
    #[error("invalid package certificate {component}: {source}")]
    InvalidCertificate {
        component: String,
        source: KernelError,
    },
    /// Kernel returned an impossible result while checking an embedded certificate.
    #[error("package certificate {0} produced an unexpected kernel result")]
    UnexpectedCertificateResult(String),
    /// A stored translation does not originate in this package kernel.
    #[error("theory translation source does not match the package kernel")]
    TranslationSourceMismatch,
    /// Translation does not validate against the supplied target.
    #[error("invalid theory translation: {0}")]
    InvalidTranslation(TranslationError),
    /// Claimed extension is not exact append-only growth of its parent.
    #[error("package kernel is not an append-only extension of the parent theory")]
    NotAppendOnlyExtension,
    /// Required imported package is unavailable or has the wrong identity.
    #[error("package import {0:?} is unavailable or mismatched")]
    MissingImport(Symbol),
    /// Serialization for package identity failed.
    #[error("package identity serialization failed: {0}")]
    Identity(String),
}

impl TheoryPackage {
    /// Creates a complete empty extension package around a checked kernel theory.
    #[must_use]
    pub fn new(name: impl Into<Symbol>, version: impl Into<String>, kernel: Theory) -> Self {
        Self {
            format_version: THEORY_PACKAGE_FORMAT,
            name: name.into(),
            version: version.into(),
            kernel,
            proof_systems: Vec::new(),
            rewrite_systems: BTreeMap::new(),
            transition_systems: BTreeMap::new(),
            discovery_policies: BTreeMap::new(),
            extensions: PackageExtensions::default(),
            metadata: BTreeMap::new(),
        }
    }

    /// Canonical content identity of this complete package.
    pub fn id(&self) -> Result<PackageId, PackageError> {
        let payload =
            serde_json::to_vec(self).map_err(|error| PackageError::Identity(error.to_string()))?;
        let mut bytes = b"artist.theory-package/1".to_vec();
        bytes.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        bytes.extend_from_slice(&payload);
        Ok(PackageId(blake3::hash(&bytes).to_hex().to_string()))
    }

    /// Validates the kernel, every named component, certificate system, and policy.
    pub fn validate(&self) -> Result<(), PackageError> {
        if self.format_version != THEORY_PACKAGE_FORMAT {
            return Err(PackageError::UnsupportedFormat(self.format_version));
        }
        self.kernel
            .validate()
            .map_err(PackageError::InvalidTheory)?;

        unique_symbols(self.proof_systems.iter().map(|system| &system.name))?;
        for system in &self.proof_systems {
            unique_symbols(system.rules.iter().map(|rule| &rule.name))?;
        }
        for rules in self.rewrite_systems.values() {
            unique_symbols(rules.iter().map(|rule| &rule.name))?;
        }
        for rules in self.transition_systems.values() {
            unique_symbols(rules.iter().map(|rule| &rule.label))?;
        }
        unique_symbols(
            self.extensions
                .elaboration_rules
                .iter()
                .map(|rule| &rule.name),
        )?;
        unique_symbols(self.extensions.notation.iter().map(|rule| &rule.name))?;
        unique_symbols(
            self.extensions
                .semantic_artifacts
                .iter()
                .map(|artifact| &artifact.name),
        )?;

        let mut import_aliases = BTreeSet::new();
        for import in &self.extensions.imports {
            if !import_aliases.insert(import.alias.clone()) {
                return Err(PackageError::DuplicateImportAlias(import.alias.clone()));
            }
        }
        for (name, checker) in &self.extensions.certificate_systems {
            checker
                .validate()
                .map_err(|source| PackageError::InvalidCertificateSystem {
                    name: name.clone(),
                    source,
                })?;
            if let Some(certificate) = &checker.correctness_claim {
                check_package_certificate(
                    &self.kernel,
                    certificate,
                    &format!("certificate-system:{}", name.0),
                )?;
            }
        }
        for rule in &self.extensions.elaboration_rules {
            if let Some(checker) = &rule.checker {
                if !self.extensions.certificate_systems.contains_key(checker) {
                    return Err(PackageError::MissingChecker {
                        rule: rule.name.clone(),
                        checker: checker.clone(),
                    });
                }
            }
        }
        for artifact in &self.extensions.semantic_artifacts {
            if let Some(certificate) = &artifact.certificate {
                check_package_certificate(
                    &self.kernel,
                    certificate,
                    &format!("semantic-artifact:{}", artifact.name.0),
                )?;
            }
        }
        for (index, certificate) in self
            .extensions
            .conservative_extension_claims
            .iter()
            .enumerate()
        {
            check_package_certificate(
                &self.kernel,
                certificate,
                &format!("conservative-extension:{index}"),
            )?;
        }
        for (index, certificate) in self.extensions.bridge_theorems.iter().enumerate() {
            check_package_certificate(
                &self.kernel,
                certificate,
                &format!("bridge-theorem:{index}"),
            )?;
        }
        if self
            .extensions
            .translations
            .iter()
            .any(|translation| translation.source != self.kernel.id())
        {
            return Err(PackageError::TranslationSourceMismatch);
        }
        for name in self.extensions.assumption_visibility.keys() {
            let Some(declaration) = self.kernel.declaration(name) else {
                return Err(PackageError::InvalidAssumptionVisibility(name.clone()));
            };
            if !matches!(
                declaration.kind,
                DeclarationKind::Axiom | DeclarationKind::Oracle
            ) {
                return Err(PackageError::InvalidAssumptionVisibility(name.clone()));
            }
        }
        Ok(())
    }

    /// Checks that this package's kernel is exact append-only growth of `parent`.
    pub fn validate_extension_of(&self, parent: &TheoryPackage) -> Result<(), PackageError> {
        self.validate()?;
        parent.validate()?;
        if self.kernel.is_extension_of(&parent.kernel) {
            Ok(())
        } else {
            Err(PackageError::NotAppendOnlyExtension)
        }
    }

    /// Resolves exact isolated imports without merging namespaces or assumptions.
    pub fn validate_imports(
        &self,
        available: &BTreeMap<PackageId, TheoryPackage>,
    ) -> Result<(), PackageError> {
        self.validate()?;
        for import in &self.extensions.imports {
            let Some(package) = available.get(&import.package) else {
                return Err(PackageError::MissingImport(import.alias.clone()));
            };
            package.validate()?;
            if package.kernel.id() != import.theory || package.id()? != import.package {
                return Err(PackageError::MissingImport(import.alias.clone()));
            }
        }
        Ok(())
    }

    /// Validates one checked translation against an exact target theory.
    pub fn validate_translation(
        &self,
        translation: &TheoryTranslation,
        target: &Theory,
    ) -> Result<(), PackageError> {
        translation
            .verify(&self.kernel, target)
            .map_err(PackageError::InvalidTranslation)
    }
}

fn check_package_certificate(
    theory: &Theory,
    certificate: &Certificate,
    component: &str,
) -> Result<(), PackageError> {
    match Kernel::run_to_completion(
        theory.clone(),
        KernelRequest::Certificate {
            certificate: certificate.clone(),
        },
    ) {
        Ok(KernelResult::Certified) => Ok(()),
        Ok(_) => Err(PackageError::UnexpectedCertificateResult(
            component.to_owned(),
        )),
        Err(source) => Err(PackageError::InvalidCertificate {
            component: component.to_owned(),
            source,
        }),
    }
}

fn unique_symbols<'a>(items: impl IntoIterator<Item = &'a Symbol>) -> Result<(), PackageError> {
    let mut seen = BTreeSet::new();
    for item in items {
        if !seen.insert(item.clone()) {
            return Err(PackageError::DuplicateComponent(item.clone()));
        }
    }
    Ok(())
}
