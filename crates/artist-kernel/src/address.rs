use crate::{KernelError, ResourceUri};
use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf};

/// An operation target may be a real OS path or a virtual resource URI.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum ResourceAddress {
    Path(PathBuf),
    Uri(ResourceUri),
}

impl ResourceAddress {
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self::Path(path.into())
    }

    pub fn uri(uri: ResourceUri) -> Self {
        Self::Uri(uri)
    }

    pub fn as_path(&self) -> Option<&PathBuf> {
        match self {
            Self::Path(path) => Some(path),
            Self::Uri(_) => None,
        }
    }

    pub fn as_uri(&self) -> Option<&ResourceUri> {
        match self {
            Self::Path(_) => None,
            Self::Uri(uri) => Some(uri),
        }
    }
}

impl From<ResourceUri> for ResourceAddress {
    fn from(uri: ResourceUri) -> Self {
        Self::Uri(uri)
    }
}

impl From<PathBuf> for ResourceAddress {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

impl From<&std::path::Path> for ResourceAddress {
    fn from(path: &std::path::Path) -> Self {
        Self::Path(path.to_owned())
    }
}

impl fmt::Display for ResourceAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path) => path.display().fmt(formatter),
            Self::Uri(uri) => uri.fmt(formatter),
        }
    }
}

pub(crate) fn require_path(address: &ResourceAddress) -> Result<&PathBuf, KernelError> {
    address
        .as_path()
        .ok_or_else(|| KernelError::InvalidRequest {
            message: format!("expected an OS path, got {address}"),
        })
}
