use std::{collections::BTreeSet, fmt, time::Duration};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use artist_core::ContentPart;

use crate::ResourceUri;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceOperation {
    Read,
    Children,
    Write,
    Edit,
    Move,
    Run,
    Signal,
    Poll,
}

impl ResourceOperation {
    pub const ALL: [Self; 8] = [
        Self::Read,
        Self::Children,
        Self::Write,
        Self::Edit,
        Self::Move,
        Self::Run,
        Self::Signal,
        Self::Poll,
    ];
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentEntry {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TextReplacement {
    pub start_byte: u64,
    pub end_byte: u64,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AnchoredEditOperation {
    Replace {
        start: String,
        end: Option<String>,
        content: String,
    },
    Delete {
        start: String,
        end: Option<String>,
    },
    InsertBefore {
        anchor: String,
        content: String,
    },
    InsertAfter {
        anchor: String,
        content: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AnchoredEditRequest {
    pub uri: String,
    pub operations: Vec<AnchoredEditOperation>,
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
    Edit {
        uri: ResourceUri,
        expected_sha256: String,
        replacements: Vec<TextReplacement>,
    },
    Move {
        from: ResourceUri,
        to: Option<ResourceUri>,
    },
    Run {
        target: ResourceUri,
        input: String,
        cwd: Option<ResourceUri>,
        env: Vec<EnvironmentEntry>,
        timeout: Option<Duration>,
    },
    Signal {
        uri: ResourceUri,
        name: String,
        payload: Option<String>,
    },
    Poll {
        uri: ResourceUri,
        pattern: Option<String>,
        timeout: Option<Duration>,
        /// Opaque provider continuation returned by the preceding poll.
        cursor: Option<String>,
    },
}

impl ResourceRequest {
    pub fn operation(&self) -> ResourceOperation {
        match self {
            Self::Read { .. } => ResourceOperation::Read,
            Self::Children { .. } => ResourceOperation::Children,
            Self::Write { .. } => ResourceOperation::Write,
            Self::Edit { .. } => ResourceOperation::Edit,
            Self::Move { .. } => ResourceOperation::Move,
            Self::Run { .. } => ResourceOperation::Run,
            Self::Signal { .. } => ResourceOperation::Signal,
            Self::Poll { .. } => ResourceOperation::Poll,
        }
    }

    pub fn uri(&self) -> &ResourceUri {
        match self {
            Self::Read { uri, .. }
            | Self::Children { uri }
            | Self::Write { uri, .. }
            | Self::Edit { uri, .. }
            | Self::Signal { uri, .. }
            | Self::Poll { uri, .. } => uri,
            Self::Move { from, .. } => from,
            Self::Run { target, .. } => target,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ResourceReply {
    Text {
        text: String,
    },
    Content {
        content: Vec<ContentPart>,
    },
    Children {
        children: Vec<ResourceUri>,
    },
    Written,
    Edited {
        revision: String,
    },
    Moved,
    Started {
        uri: ResourceUri,
    },
    Signaled,
    Poll {
        text: String,
        outcome: PollOutcome,
        next_cursor: String,
    },
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
    #[error("resource conflict on {uri}; current revision is {current_revision}")]
    Conflict {
        uri: ResourceUri,
        current_revision: String,
    },
    #[error("invalid resource request: {0}")]
    Invalid(String),
    #[error("resource provider failed: {0}")]
    Provider(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SignalDefinition {
    pub name: String,
    pub description: String,
    pub payload_schema: Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceMetadata {
    pub uri: ResourceUri,
    pub operations: Vec<ResourceOperation>,
    pub signals: Vec<SignalDefinition>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceRoute {
    pub base_glob: String,
    pub projection_glob: Option<String>,
    pub operations: BTreeSet<ResourceOperation>,
    pub signals: Vec<SignalDefinition>,
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
            signals: Vec::new(),
        }
    }

    pub fn with_signals(mut self, signals: Vec<SignalDefinition>) -> Self {
        self.signals = signals;
        self
    }
}

#[async_trait]
pub trait ResourceProvider: Send + Sync {
    async fn handle(&self, request: ResourceRequest) -> Result<ResourceReply, ResourceError>;
}
