//! Canonical resource-address semantics shared by handlers and runtimes.

use crate::{KernelError, ResourceAddress};
use std::path::Path;

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

/// Returns whether the URI query selects a derived projection rather than a
/// filesystem path. Query projections are owned by resource extensions.
pub fn has_query_projection(uri: &crate::ResourceUri) -> bool {
    uri.query().is_some_and(|query| {
        query.split('&').any(|item| {
            let key = item.split_once('=').map_or(item, |(key, _)| key);
            PROJECTIONS.contains(&key)
        })
    })
}

/// Canonicalizes every native path into the file URI form used internally.
pub fn normalize(address: &ResourceAddress) -> Result<ResourceAddress, KernelError> {
    Ok(ResourceAddress::uri(crate::address::canonical_uri(
        address,
    )?))
}

/// Whether an address is a local filesystem URI.
pub fn is_file_uri(address: &ResourceAddress) -> bool {
    address.as_uri().is_some_and(|uri| uri.scheme() == "file")
}
