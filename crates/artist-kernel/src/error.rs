use serde::{Deserialize, Serialize};
use std::fmt;

/// Errors returned by the kernel or by a resource handler.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum KernelError {
    InvalidUri { message: String },
    UnsupportedUri { uri: String },
    NoHandler { uri: String },
    UnsupportedVerb { verb: String, uri: String },
    InvalidRequest { message: String },
    InvalidAnchor { message: String },
    NotFound { uri: String },
    AlreadyExists { uri: String },
    InvalidState { message: String },
    Handler { message: String },
}

impl fmt::Display for KernelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUri { message }
            | Self::InvalidRequest { message }
            | Self::InvalidAnchor { message }
            | Self::InvalidState { message }
            | Self::Handler { message } => formatter.write_str(message),
            Self::UnsupportedUri { uri } => write!(formatter, "unsupported URI: {uri}"),
            Self::NoHandler { uri } => write!(formatter, "no handler for URI: {uri}"),
            Self::UnsupportedVerb { verb, uri } => {
                write!(formatter, "verb {verb} is unsupported for {uri}")
            }
            Self::NotFound { uri } => write!(formatter, "resource not found: {uri}"),
            Self::AlreadyExists { uri } => write!(formatter, "resource already exists: {uri}"),
        }
    }
}

impl std::error::Error for KernelError {}
