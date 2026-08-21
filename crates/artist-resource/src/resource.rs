use std::{collections::BTreeSet, fmt, time::Duration};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ResourceUri;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceOperation {
    Read,
    Children,
    Write,
    Move,
    Poll,
    Edit,
}

impl fmt::Display for ResourceOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self).unwrap().as_str().unwrap()
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceRequest {
    Read {
        uri: ResourceUri,
        start_line: Option<u64>,
        line_count: Option<u64>,
    },
    Children {
        uri: ResourceUri,
    },
    Write {
        uri: ResourceUri,
        text: String,
    },
    Move {
        from: ResourceUri,
        to: Option<ResourceUri>,
    },
    Poll {
        uri: ResourceUri,
        pattern: Option<String>,
        timeout: Option<Duration>,
    },
    /// Reserved. Providers should not advertise this operation yet.
    Edit {
        uri: ResourceUri,
        instructions: String,
    },
}

impl ResourceRequest {
    pub fn operation(&self) -> ResourceOperation {
        match self {
            Self::Read { .. } => ResourceOperation::Read,
            Self::Children { .. } => ResourceOperation::Children,
            Self::Write { .. } => ResourceOperation::Write,
            Self::Move { .. } => ResourceOperation::Move,
            Self::Poll { .. } => ResourceOperation::Poll,
            Self::Edit { .. } => ResourceOperation::Edit,
        }
    }
    pub fn uri(&self) -> &ResourceUri {
        match self {
            Self::Read { uri, .. }
            | Self::Children { uri }
            | Self::Write { uri, .. }
            | Self::Poll { uri, .. }
            | Self::Edit { uri, .. } => uri,
            Self::Move { from, .. } => from,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ResourceReply {
    Text { text: String },
    Children { children: Vec<ResourceUri> },
    Written,
    Moved,
    Poll { text: String, outcome: PollOutcome },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PollOutcome {
    Matched,
    Closed,
    TimedOut,
}

#[derive(Clone, Debug, Error, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "kebab-case")]
pub enum ResourceError {
    #[error("no resource route for {operation} on {uri}")]
    NotFound {
        uri: ResourceUri,
        operation: ResourceOperation,
    },
    #[error("{operation} is unsupported on {uri}")]
    Unsupported {
        uri: ResourceUri,
        operation: ResourceOperation,
    },
    #[error("invalid resource request: {0}")]
    Invalid(String),
    #[error("resource provider failed: {0}")]
    Provider(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceRoute {
    pub base_glob: String,
    pub projection_glob: Option<String>,
    pub operations: BTreeSet<ResourceOperation>,
}

impl ResourceRoute {
    pub fn new(
        base_glob: impl Into<String>,
        projection_glob: Option<impl Into<String>>,
        operations: impl IntoIterator<Item = ResourceOperation>,
    ) -> Self {
        Self {
            base_glob: base_glob.into(),
            projection_glob: projection_glob.map(Into::into),
            operations: operations.into_iter().collect(),
        }
    }
}

#[async_trait]
pub trait ResourceProvider: Send + Sync {
    async fn handle(&self, request: ResourceRequest) -> Result<ResourceReply, ResourceError>;
}
