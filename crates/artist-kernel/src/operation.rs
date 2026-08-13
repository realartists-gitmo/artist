use serde::{Deserialize, Serialize};
use std::fmt;

/// The only model-facing operations exposed by the kernel.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verb {
    Read,
    Write,
    Edit,
    Send,
    Poll,
    Abort,
    Delete,
    Find,
    Grep,
    Run,
}

impl Verb {
    pub const ALL: [Self; 10] = [
        Self::Read,
        Self::Write,
        Self::Edit,
        Self::Send,
        Self::Poll,
        Self::Abort,
        Self::Delete,
        Self::Find,
        Self::Grep,
        Self::Run,
    ];
}

impl fmt::Display for Verb {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Edit => "edit",
            Self::Send => "send",
            Self::Poll => "poll",
            Self::Abort => "abort",
            Self::Delete => "delete",
            Self::Find => "find",
            Self::Grep => "grep",
            Self::Run => "run",
        })
    }
}
