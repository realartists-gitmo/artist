//! Shared value types.

use serde::{Deserialize, Serialize};

/// Which store a fact belongs to. Global facts follow the user between repos;
/// project facts stay with one checkout.
///
/// These are separate databases rather than one relation with a `scope` column,
/// because a query-time HNSW `filter:` is only a post-filter over the `ef`
/// candidate pool — partitioning by filter would silently drop results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    Global,
    Project,
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Project => "project",
        }
    }
}

/// A fact awaiting insertion.
#[derive(Clone, Debug)]
pub struct NewFact {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    /// The prose actually shown to the model.
    pub text: String,
    pub embedding: Vec<f32>,
    pub source_session: String,
    pub source_seq: i64,
    /// Which write point produced this — `tool`, `correction`, `output`, `commit`.
    pub origin: String,
}

/// A stored fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub id: i64,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub text: String,
    pub origin: String,
}

/// A retrieval result.
#[derive(Clone, Debug)]
pub struct Hit {
    pub id: i64,
    pub score: f64,
    pub text: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub origin: String,
    /// Unix epoch seconds, as `now()` recorded it at write time.
    ///
    /// Carried all the way to the model: when two live facts disagree, the
    /// only thing that lets it prefer one is knowing which came later.
    pub created_at: f64,
    pub scope: Scope,
}

/// A retrieval result from the code index.
#[derive(Clone, Debug)]
pub struct CodeHit {
    pub score: f64,
    pub path: String,
    pub lang: String,
    pub start_line: u32,
    pub end_line: u32,
    pub body: String,
}

/// One indexable unit of source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub path: String,
    pub lang: String,
    /// 1-indexed, inclusive.
    pub start_line: u32,
    pub end_line: u32,
    pub body: String,
    pub content_hash: String,
}
