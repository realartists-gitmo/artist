//! Legacy provider/CLI adapter requests.
//!
//! Universal component execution does not use this type. It remains available
//! only so older JSON-facing callers can be migrated without changing the
//! typed `Operation`/`OperationResult` kernel surface.

use crate::{ResourceAddress, Verb};
use serde::{Deserialize, Serialize};

/// One universal resource operation.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Request {
    pub verb: Verb,
    pub target: ResourceAddress,
    #[serde(default)]
    pub args: serde_json::Value,
}

impl Request {
    pub fn new(verb: Verb, target: impl Into<ResourceAddress>, args: serde_json::Value) -> Self {
        Self {
            verb,
            target: target.into(),
            args,
        }
    }
}

/// A batch is the ordinary request shape, not a second tool mode.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct BatchRequest {
    pub items: Vec<Request>,
}

impl FromIterator<Request> for BatchRequest {
    fn from_iter<T: IntoIterator<Item = Request>>(iter: T) -> Self {
        Self {
            items: iter.into_iter().collect(),
        }
    }
}
