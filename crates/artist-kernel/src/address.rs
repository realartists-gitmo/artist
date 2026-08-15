#[cfg(test)]
use crate::KernelError;
use crate::ResourceUri;
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    path::{Path, PathBuf},
};

/// Canonical internal resource identity. Bare OS paths are converted to
/// `file://` at construction; there is no second path identity below the
/// outer adapter boundary.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct ResourceAddress(ResourceUri);

impl ResourceAddress {
    pub fn path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let absolute = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .expect("current directory is required")
                .join(path)
        };
        Self(
            ResourceUri::parse(&absolute.display().to_string())
                .expect("OS paths are valid file URIs"),
        )
    }

    pub fn uri(uri: ResourceUri) -> Self {
        Self(uri)
    }

    pub fn as_uri(&self) -> Option<&ResourceUri> {
        Some(&self.0)
    }

    pub fn uri_ref(&self) -> &ResourceUri {
        &self.0
    }
}

impl From<ResourceUri> for ResourceAddress {
    fn from(uri: ResourceUri) -> Self {
        Self::uri(uri)
    }
}

impl From<PathBuf> for ResourceAddress {
    fn from(path: PathBuf) -> Self {
        Self::path(path)
    }
}

impl From<&Path> for ResourceAddress {
    fn from(path: &Path) -> Self {
        Self::path(path)
    }
}

impl fmt::Display for ResourceAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

pub(crate) fn canonical_uri(address: &ResourceAddress) -> Result<ResourceUri, crate::KernelError> {
    Ok(address.0.clone())
}

#[cfg(test)]
pub(crate) fn uri_path(uri: &ResourceUri) -> Result<PathBuf, KernelError> {
    if uri.scheme() != "file" {
        return Err(KernelError::InvalidRequest {
            message: format!("expected file URI, got {uri}"),
        });
    }
    uri.as_ref()
        .to_file_path()
        .map_err(|_| KernelError::InvalidUri {
            message: uri.to_string(),
        })
}
