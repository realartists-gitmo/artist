//! Native JSON adapter results.
//!
//! Typed component execution returns `OperationResult` directly; this module
//! is retained for the native adapter path around `Request`.

use crate::{KernelError, ResourceAddress};
use serde::{Deserialize, Serialize};

/// One result corresponding to one request item.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ItemResult {
    pub target: ResourceAddress,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<KernelError>,
}

impl ItemResult {
    pub fn success(target: ResourceAddress, value: serde_json::Value) -> Self {
        Self {
            target,
            ok: true,
            value: Some(value),
            error: None,
        }
    }

    pub fn failure(target: ResourceAddress, error: KernelError) -> Self {
        Self {
            target,
            ok: false,
            value: None,
            error: Some(error),
        }
    }
}

/// Batch results preserve request order and contain one result per item.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct BatchResult {
    pub items: Vec<ItemResult>,
}
