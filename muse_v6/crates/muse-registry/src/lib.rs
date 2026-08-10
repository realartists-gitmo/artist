//! Immutable semantic package registry and dependency snapshots.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use muse_core::{
    CanonicalHashError, ContentDigest, ImportRequirement, PackageId, PackageRef, PackageVersion,
    canonical_digest,
};
use muse_lexicon::{LexiconPackage, LexiconValidationError};
use muse_ontology::{OntologyIndex, OntologyIndexError, OntologyPackage, OntologyValidationError};
use muse_provenance::{ProvenancePackage, ProvenanceValidationError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "package", rename_all = "snake_case")]
pub enum SemanticPackage {
    Ontology(OntologyPackage),
    Lexicon(LexiconPackage),
    Provenance(ProvenancePackage),
}

impl SemanticPackage {
    #[must_use]
    pub fn package_ref(&self) -> &PackageRef {
        match self {
            Self::Ontology(package) => &package.header.package,
            Self::Lexicon(package) => &package.header.package,
            Self::Provenance(package) => &package.header.package,
        }
    }

    #[must_use]
    pub fn imports(&self) -> &[ImportRequirement] {
        match self {
            Self::Ontology(package) => &package.header.imports,
            Self::Lexicon(package) => &package.header.imports,
            Self::Provenance(package) => &package.header.imports,
        }
    }

    pub fn validate(&self) -> Result<(), RegistryError> {
        match self {
            Self::Ontology(package) => package.validate()?,
            Self::Lexicon(package) => package.validate()?,
            Self::Provenance(package) => package.validate()?,
        }
        Ok(())
    }

    fn header_mut(&mut self) -> &mut muse_core::PackageHeader {
        match self {
            Self::Ontology(package) => &mut package.header,
            Self::Lexicon(package) => &mut package.header,
            Self::Provenance(package) => &mut package.header,
        }
    }

    #[must_use]
    pub fn placeholder_digest() -> ContentDigest {
        ContentDigest::sha256("0".repeat(64))
    }

    pub fn computed_digest(&self) -> Result<ContentDigest, RegistryError> {
        let mut normalized = self.clone();
        normalized.header_mut().package.digest = Self::placeholder_digest();
        Ok(canonical_digest(&normalized)?)
    }

    pub fn verify_content_digest(&self) -> Result<(), RegistryError> {
        let expected = self.computed_digest()?;
        let actual = self.package_ref().digest.clone();
        if expected != actual {
            return Err(RegistryError::PackageDigestMismatch {
                id: self.package_ref().id.clone(),
                version: self.package_ref().version.clone(),
                expected,
                actual,
            });
        }
        Ok(())
    }

    pub fn seal(mut self) -> Result<Self, RegistryError> {
        self.header_mut().package.digest = Self::placeholder_digest();
        self.validate()?;
        let digest = self.computed_digest()?;
        self.header_mut().package.digest = digest;
        Ok(self)
    }
}

#[derive(Clone, Debug, Default)]
pub struct PackageRegistry {
    packages: BTreeMap<PackageId, BTreeMap<PackageVersion, SemanticPackage>>,
}

impl PackageRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, package: SemanticPackage) -> Result<PackageRef, RegistryError> {
        package.validate()?;
        package.verify_content_digest()?;
        let package_ref = package.package_ref().clone();
        let versions = self.packages.entry(package_ref.id.clone()).or_default();
        if let Some(existing) = versions.get(&package_ref.version) {
            if existing.package_ref().digest == package_ref.digest {
                return Ok(package_ref);
            }
            return Err(RegistryError::ConflictingPackage {
                id: package_ref.id,
                version: package_ref.version,
                existing: existing.package_ref().digest.clone(),
                incoming: package_ref.digest,
            });
        }
        versions.insert(package_ref.version.clone(), package);
        Ok(package_ref)
    }

    #[must_use]
    pub fn get(&self, package: &PackageRef) -> Option<&SemanticPackage> {
        self.packages
            .get(&package.id)
            .and_then(|versions| versions.get(&package.version))
            .filter(|candidate| candidate.package_ref().digest == package.digest)
    }

    #[must_use]
    pub fn find(&self, id: &PackageId, version: Option<&PackageVersion>) -> Vec<&SemanticPackage> {
        let Some(versions) = self.packages.get(id) else {
            return Vec::new();
        };
        match version {
            Some(version) => versions.get(version).into_iter().collect(),
            None => versions.values().collect(),
        }
    }

    #[must_use]
    pub fn all_package_refs(&self) -> BTreeSet<PackageRef> {
        self.packages
            .values()
            .flat_map(BTreeMap::values)
            .map(|package| package.package_ref().clone())
            .collect()
    }

    pub fn snapshot_all(&self) -> Result<RegistrySnapshot, RegistryError> {
        self.snapshot(self.all_package_refs())
    }

    pub fn snapshot(
        &self,
        roots: impl IntoIterator<Item = PackageRef>,
    ) -> Result<RegistrySnapshot, RegistryError> {
        let mut selected = BTreeSet::new();
        let mut pending: Vec<PackageRef> = roots.into_iter().collect();

        while let Some(package_ref) = pending.pop() {
            if !selected.insert(package_ref.clone()) {
                continue;
            }
            let package = self
                .get(&package_ref)
                .ok_or_else(|| RegistryError::UnknownPackage(package_ref.clone()))?;
            for requirement in package.imports() {
                match self.resolve_requirement(requirement)? {
                    Some(imported) => pending.push(imported),
                    None if requirement.optional => {}
                    None => {
                        return Err(RegistryError::UnsatisfiedImport {
                            importer: package_ref.clone(),
                            requirement: requirement.clone(),
                        });
                    }
                }
            }
        }

        let identity = canonical_digest(&selected)?;
        Ok(RegistrySnapshot {
            packages: selected,
            identity,
        })
    }

    fn resolve_requirement(
        &self,
        requirement: &ImportRequirement,
    ) -> Result<Option<PackageRef>, RegistryError> {
        let candidates = self.find(&requirement.id, requirement.version.as_ref());
        let mut matching: Vec<PackageRef> = candidates
            .into_iter()
            .map(|package| package.package_ref().clone())
            .filter(|package| requirement.matches(package))
            .collect();
        matching.sort();
        match matching.len() {
            0 => Ok(None),
            1 => Ok(matching.pop()),
            _ => Err(RegistryError::AmbiguousImport(requirement.clone())),
        }
    }

    pub fn validate_snapshot(&self, snapshot: &RegistrySnapshot) -> Result<(), RegistryError> {
        let expected = canonical_digest(&snapshot.packages)?;
        if expected != snapshot.identity {
            return Err(RegistryError::SnapshotIdentityMismatch {
                expected,
                actual: snapshot.identity.clone(),
            });
        }
        for package in &snapshot.packages {
            let declaration = self
                .get(package)
                .ok_or_else(|| RegistryError::UnknownPackage(package.clone()))?;
            declaration.validate()?;
            for requirement in declaration.imports() {
                let matched = snapshot
                    .packages
                    .iter()
                    .any(|candidate| requirement.matches(candidate));
                if !matched && !requirement.optional {
                    return Err(RegistryError::UnsatisfiedImport {
                        importer: package.clone(),
                        requirement: requirement.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    pub fn ontology_index(
        &self,
        snapshot: &RegistrySnapshot,
    ) -> Result<OntologyIndex, RegistryError> {
        self.validate_snapshot(snapshot)?;
        let packages: Vec<&OntologyPackage> = snapshot
            .packages
            .iter()
            .filter_map(|package_ref| match self.get(package_ref) {
                Some(SemanticPackage::Ontology(package)) => Some(package),
                _ => None,
            })
            .collect();
        Ok(OntologyIndex::build(packages)?)
    }

    pub fn lexicons<'a>(
        &'a self,
        snapshot: &RegistrySnapshot,
    ) -> Result<Vec<&'a LexiconPackage>, RegistryError> {
        self.validate_snapshot(snapshot)?;
        Ok(snapshot
            .packages
            .iter()
            .filter_map(|package_ref| match self.get(package_ref) {
                Some(SemanticPackage::Lexicon(package)) => Some(package),
                _ => None,
            })
            .collect())
    }

    pub fn provenance<'a>(
        &'a self,
        snapshot: &RegistrySnapshot,
    ) -> Result<Vec<&'a ProvenancePackage>, RegistryError> {
        self.validate_snapshot(snapshot)?;
        Ok(snapshot
            .packages
            .iter()
            .filter_map(|package_ref| match self.get(package_ref) {
                Some(SemanticPackage::Provenance(package)) => Some(package),
                _ => None,
            })
            .collect())
    }

    #[must_use]
    pub fn package_count(&self) -> usize {
        self.packages.values().map(BTreeMap::len).sum()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    pub packages: BTreeSet<PackageRef>,
    pub identity: ContentDigest,
}

impl RegistrySnapshot {
    #[must_use]
    pub fn contains(&self, package: &PackageRef) -> bool {
        self.packages.contains(package)
    }
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error(transparent)]
    OntologyValidation(#[from] OntologyValidationError),
    #[error(transparent)]
    LexiconValidation(#[from] LexiconValidationError),
    #[error(transparent)]
    ProvenanceValidation(#[from] ProvenanceValidationError),
    #[error(transparent)]
    OntologyIndex(#[from] OntologyIndexError),
    #[error(transparent)]
    CanonicalHash(#[from] CanonicalHashError),
    #[error(
        "package {id}@{version} declares digest {actual}, but its canonical content digest is {expected}"
    )]
    PackageDigestMismatch {
        id: PackageId,
        version: PackageVersion,
        expected: ContentDigest,
        actual: ContentDigest,
    },
    #[error("package {id}@{version} has conflicting digests {existing} and {incoming}")]
    ConflictingPackage {
        id: PackageId,
        version: PackageVersion,
        existing: ContentDigest,
        incoming: ContentDigest,
    },
    #[error("unknown package: {0}")]
    UnknownPackage(PackageRef),
    #[error("package {importer} has unsatisfied import {requirement:?}")]
    UnsatisfiedImport {
        importer: PackageRef,
        requirement: ImportRequirement,
    },
    #[error("import requirement resolves to multiple packages: {0:?}")]
    AmbiguousImport(ImportRequirement),
    #[error("snapshot identity mismatch: expected {expected}, found {actual}")]
    SnapshotIdentityMismatch {
        expected: ContentDigest,
        actual: ContentDigest,
    },
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use muse_core::{PackageHeader, PackageKind};
    use muse_ontology::OntologyPackage;

    use super::*;

    fn ontology(id: &str, version: &str, marker: char) -> SemanticPackage {
        SemanticPackage::Ontology(OntologyPackage {
            header: PackageHeader {
                package: PackageRef {
                    id: PackageId::from(id),
                    version: PackageVersion::from(version),
                    digest: SemanticPackage::placeholder_digest(),
                },
                kind: PackageKind::Ontology,
                title: format!("Test ontology {marker}"),
                description: "Test package".to_owned(),
                license: "CC0-1.0".to_owned(),
                imports: Vec::new(),
                evidence: Vec::new(),
            },
            concepts: BTreeMap::new(),
            relations: BTreeMap::new(),
            axioms: Vec::new(),
        })
        .seal()
        .unwrap()
    }

    #[test]
    fn rejects_same_id_and_version_with_different_content() {
        let mut registry = PackageRegistry::new();
        registry
            .register(ontology("muse.test", "0.1.0", 'a'))
            .unwrap();
        assert!(matches!(
            registry.register(ontology("muse.test", "0.1.0", 'b')),
            Err(RegistryError::ConflictingPackage { .. })
        ));
    }

    #[test]
    fn rejects_tampered_declared_digest() {
        let mut package = ontology("muse.test.tampered", "0.1.0", 'a');
        package.header_mut().package.digest = ContentDigest::sha256("f".repeat(64));
        let mut registry = PackageRegistry::new();
        assert!(matches!(
            registry.register(package),
            Err(RegistryError::PackageDigestMismatch { .. })
        ));
    }

    #[test]
    fn creates_content_bound_snapshot() {
        let mut registry = PackageRegistry::new();
        let package = registry
            .register(ontology("muse.test", "0.1.0", 'a'))
            .unwrap();
        let snapshot = registry.snapshot([package]).unwrap();
        assert!(registry.validate_snapshot(&snapshot).is_ok());
    }
}
