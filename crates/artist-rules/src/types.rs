//! Core rule types shared by the declarative and (future) WASM tiers.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Stable rule identifier: the declarative `name`, or `wasm:<name>` for
/// plugin rules, or `builtin:<name>` for embedded defaults.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RuleId(pub String);

impl std::fmt::Display for RuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What part of the model's streaming output a rule matches against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MatchTarget {
    /// Streamed assistant text.
    AssistantText,
    /// Streamed tool-call arguments (a match aborts BEFORE the tool runs).
    ToolArgs,
    /// Streamed reasoning summaries — earlier but fuzzier signal.
    ReasoningSummary,
}

impl MatchTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            MatchTarget::AssistantText => "assistant-text",
            MatchTarget::ToolArgs => "tool-args",
            MatchTarget::ReasoningSummary => "reasoning-summary",
        }
    }
}

/// How often a rule may fire.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FirePolicy {
    /// At most once per session (the oh-my-pi semantic; default).
    #[default]
    Once,
    /// Re-arms on every user prompt.
    PerTurn,
}

/// How long a fired rule's reminder stays active.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Persistence {
    /// Re-injected on every completion call for the rest of the session
    /// (re-applied outside ordinary history; default).
    #[default]
    Session,
    /// Delivered once with the retry prompt only.
    Message,
}

/// Which agents a rule applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuleScope {
    pub main: bool,
    pub delegate: bool,
}

impl Default for RuleScope {
    fn default() -> Self {
        Self {
            main: true,
            delegate: true,
        }
    }
}

/// A fully parsed (but not yet compiled) declarative rule.
#[derive(Clone, Debug)]
pub struct DeclarativeRule {
    pub id: RuleId,
    pub description: String,
    pub targets: Vec<MatchTarget>,
    /// Raw regex patterns (compiled in [`crate::matcher`]).
    pub patterns: Vec<String>,
    /// For `ToolArgs` targets: only these tools are matched. Empty = all.
    pub tools: Vec<String>,
    /// Matching window in bytes over the streamed text.
    pub window: usize,
    pub fire: FirePolicy,
    pub persistence: Persistence,
    pub scope: RuleScope,
    pub enabled: bool,
    /// The reminder body injected on fire.
    pub reminder: String,
    /// Where the rule came from (`builtin:` for embedded rules).
    pub source: Option<PathBuf>,
}

pub const DEFAULT_WINDOW: usize = 4096;

/// A rule firing recorded by the matcher, consumed by the retry driver.
#[derive(Clone, Debug, PartialEq)]
pub struct Firing {
    pub rule: RuleId,
    pub target: MatchTarget,
    /// The matched excerpt (bounded), for the UI and the event log.
    pub matched: String,
    pub reminder: String,
    pub persistence: Persistence,
    /// The rule's fire policy, carried so the event log can record whether a
    /// firing was per-turn and reconstruct arming state on resume.
    pub fire: FirePolicy,
}
