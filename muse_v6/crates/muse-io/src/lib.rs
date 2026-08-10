//! Canonical JSON envelopes and atomic filesystem package storage.

#![forbid(unsafe_code)]

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use muse_core::{CanonicalHashError, ContentDigest, canonical_digest};
use muse_interpretation::InterpretationBundle;
use muse_registry::{PackageRegistry, RegistryError, SemanticPackage};
use muse_validation::ValidationReport;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const FORMAT_VERSION: &str = "muse-json-1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "document_kind", content = "document", rename_all = "snake_case")]
pub enum SemanticDocument {
    Package(SemanticPackage),
    Interpretation(InterpretationBundle),
    ValidationReport(ValidationReport),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentEnvelope {
    pub format_version: String,
    pub document_digest: ContentDigest,
    pub payload: SemanticDocument,
}

impl DocumentEnvelope {
    pub fn seal(payload: SemanticDocument) -> Result<Self, IoError> {
        let payload = match payload {
            SemanticDocument::Package(package) => SemanticDocument::Package(package.seal()?),
            other => other,
        };
        let document_digest = canonical_digest(&payload)?;
        Ok(Self {
            format_version: FORMAT_VERSION.to_owned(),
            document_digest,
            payload,
        })
    }

    pub fn verify(&self) -> Result<(), IoError> {
        if self.format_version != FORMAT_VERSION {
            return Err(IoError::UnsupportedFormat(self.format_version.clone()));
        }
        let expected = canonical_digest(&self.payload)?;
        if expected != self.document_digest {
            return Err(IoError::DigestMismatch {
                expected,
                actual: self.document_digest.clone(),
            });
        }
        match &self.payload {
            SemanticDocument::Package(package) => {
                package.validate()?;
                package.verify_content_digest()?;
            }
            SemanticDocument::Interpretation(bundle) => bundle.validate()?,
            SemanticDocument::ValidationReport(_) => {}
        }
        Ok(())
    }
}

pub fn read_envelope(path: impl AsRef<Path>) -> Result<DocumentEnvelope, IoError> {
    let path = path.as_ref();
    let bytes = fs::read(path).map_err(|source| IoError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let envelope: DocumentEnvelope =
        serde_json::from_slice(&bytes).map_err(|source| IoError::Deserialize {
            path: path.to_path_buf(),
            source,
        })?;
    envelope.verify()?;
    Ok(envelope)
}

pub fn write_envelope(path: impl AsRef<Path>, envelope: &DocumentEnvelope) -> Result<(), IoError> {
    envelope.verify()?;
    let path = path.as_ref();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| IoError::CreateDirectory {
        path: parent.to_path_buf(),
        source,
    })?;
    let temporary = temporary_path(path);
    let bytes = serde_json::to_vec_pretty(envelope)?;
    fs::write(&temporary, bytes).map_err(|source| IoError::Write {
        path: temporary.clone(),
        source,
    })?;
    fs::rename(&temporary, path).map_err(|source| IoError::Rename {
        from: temporary,
        to: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

pub fn write_document(
    path: impl AsRef<Path>,
    document: SemanticDocument,
) -> Result<DocumentEnvelope, IoError> {
    let envelope = DocumentEnvelope::seal(document)?;
    write_envelope(path, &envelope)?;
    Ok(envelope)
}

pub fn load_registry_directory(path: impl AsRef<Path>) -> Result<PackageRegistry, IoError> {
    let path = path.as_ref();
    let mut files = Vec::new();
    // A release package directory may retain reproducible generator inputs under
    // `sources/`. They are not independently sealed release packages and can
    // deliberately describe an earlier bootstrap graph.
    collect_muse_files(path, &mut files, Some(&path.join("sources")))?;
    files.sort();
    let mut registry = PackageRegistry::new();
    for file in files {
        let envelope = read_envelope(&file)?;
        if let SemanticDocument::Package(package) = envelope.payload {
            registry.register(package)?;
        }
    }
    Ok(registry)
}

fn collect_muse_files(
    path: &Path,
    files: &mut Vec<PathBuf>,
    excluded: Option<&Path>,
) -> Result<(), IoError> {
    if excluded.is_some_and(|excluded| path == excluded) {
        return Ok(());
    }
    if path.is_file() {
        if is_muse_json(path) {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    let entries = fs::read_dir(path).map_err(|source| IoError::ReadDirectory {
        path: path.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| IoError::ReadDirectoryEntry {
            path: path.to_path_buf(),
            source,
        })?;
        let entry_path = entry.path();
        if entry_path.is_dir() {
            collect_muse_files(&entry_path, files, excluded)?;
        } else if is_muse_json(&entry_path) {
            files.push(entry_path);
        }
    }
    Ok(())
}

fn is_muse_json(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".muse.json"))
}

fn temporary_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document.muse.json");
    path.with_file_name(format!(".{file_name}.tmp"))
}

#[derive(Debug, Error)]
pub enum IoError {
    #[error(transparent)]
    Canonical(#[from] CanonicalHashError),
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Interpretation(#[from] muse_interpretation::InterpretationValidationError),
    #[error("unsupported document format {0}")]
    UnsupportedFormat(String),
    #[error("document digest mismatch: expected {expected}, found {actual}")]
    DigestMismatch {
        expected: ContentDigest,
        actual: ContentDigest,
    },
    #[error("failed to read {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("failed to deserialize {path}: {source}")]
    Deserialize {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("failed to create directory {path}: {source}")]
    CreateDirectory { path: PathBuf, source: io::Error },
    #[error("failed to write {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
    #[error("failed to rename {from} to {to}: {source}")]
    Rename {
        from: PathBuf,
        to: PathBuf,
        source: io::Error,
    },
    #[error("failed to read directory {path}: {source}")]
    ReadDirectory { path: PathBuf, source: io::Error },
    #[error("failed to read a directory entry in {path}: {source}")]
    ReadDirectoryEntry { path: PathBuf, source: io::Error },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_report_verifies() {
        let envelope = DocumentEnvelope::seal(SemanticDocument::ValidationReport(
            ValidationReport::default(),
        ))
        .unwrap();
        assert!(envelope.verify().is_ok());
    }
}
