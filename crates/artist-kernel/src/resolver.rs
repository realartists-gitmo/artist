//! Canonical resource-address semantics shared by handlers and runtimes.

use crate::{KernelError, ResourceAddress};

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
