//! Canonical resource-address semantics shared by handlers and runtimes.

use crate::{KernelError, ResourceAddress, ResourceUri};
use std::path::Path;
use url::Url;

const PROJECTIONS: &[&str] = &[
    "symbols",
    "map",
    "show",
    "implements",
    "implementations",
    "surface",
    "deps",
    "reverse-deps",
    "trace",
    "callers",
    "callees",
    "impact",
];

/// Returns whether a native path contains a resource projection suffix.
pub fn has_projection(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|name| PROJECTIONS.contains(&name))
    })
}

/// Canonicalizes a native path into the file URI form used internally by
/// projection handlers. Ordinary native paths remain paths.
pub fn normalize(address: &ResourceAddress) -> Result<ResourceAddress, KernelError> {
    let Some(path) = address.as_path() else {
        return Ok(address.clone());
    };
    if !has_projection(path) {
        return Ok(address.clone());
    }
    let url = Url::from_file_path(path).map_err(|_| KernelError::InvalidRequest {
        message: format!("invalid filesystem path: {address}"),
    })?;
    Ok(ResourceAddress::Uri(
        ResourceUri::parse(url.as_str()).map_err(|error| KernelError::InvalidRequest {
            message: format!("invalid filesystem path URI: {error}"),
        })?,
    ))
}

/// Whether an address is a local filesystem URI.
pub fn is_file_uri(address: &ResourceAddress) -> bool {
    address.as_uri().is_some_and(|uri| uri.scheme() == "file")
}
