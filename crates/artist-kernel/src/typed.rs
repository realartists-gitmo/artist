//! The typed kernel/component contract.
//!
//! JSON is an adapter concern. The kernel dispatches these values directly;
//! provider-facing JSON schemas are derived by the component layer.

use crate::{Anchor, KernelError, ResourceUri};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum LineEnding {
    None,
    Lf,
    Crlf,
    Cr,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AnchoredLine {
    pub anchor: Anchor,
    pub text: String,
    pub ending: LineEnding,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AnchoredText {
    pub uri: ResourceUri,
    pub lines: Vec<AnchoredLine>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Position {
    Top,
    Bottom,
    At(Anchor),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReadRequest {
    pub uri: ResourceUri,
    pub at: Option<Position>,
    pub before: Option<u32>,
    pub after: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WriteRequest {
    pub uri: ResourceUri,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReplaceOperation {
    pub start: Anchor,
    pub end: Option<Anchor>,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InsertOperation {
    pub at: InsertionPoint,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum InsertionPoint {
    Top,
    Bottom,
    Before(Anchor),
    After(Anchor),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum EditOperation {
    Replace(ReplaceOperation),
    Insert(InsertOperation),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EditRequest {
    pub uri: ResourceUri,
    pub operations: Vec<EditOperation>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FindRequest {
    pub roots: Vec<ResourceUri>,
    pub query: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum GrepSource {
    Resources(Vec<ResourceUri>),
    Text(Vec<AnchoredText>),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct GrepRequest {
    pub pattern: String,
    pub source: GrepSource,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EnvironmentEntry {
    pub name: String,
    pub value: String,
}

/// Invocation metadata is carried by the kernel, not encoded as tool input.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct InvocationContext {
    pub cancellation_token: Option<String>,
    pub deadline_ms: Option<u64>,
    pub correlation_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RunRequest {
    pub uri: ResourceUri,
    pub args: Vec<String>,
    pub working_uri: Option<ResourceUri>,
    pub environment: Vec<EnvironmentEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SendRequest {
    pub uri: ResourceUri,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PollTarget {
    pub uri: ResourceUri,
    pub from_position: Option<Position>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PollRequest {
    pub targets: Vec<PollTarget>,
    pub until: Option<PollCondition>,
    pub before: Option<u32>,
    pub after: Option<u32>,
}

/// A complete universal invocation. Each variant has its WIT operation shape;
/// there is no universal target or untyped argument bag here.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Operation {
    Read(Vec<ReadRequest>),
    Write(Vec<WriteRequest>),
    Edit(Vec<EditRequest>),
    Find(FindRequest),
    Grep(GrepRequest),
    Run(Vec<RunRequest>),
    Send(Vec<SendRequest>),
    Abort(Vec<ResourceUri>),
    Delete(Vec<ResourceUri>),
    Poll(PollRequest),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum OperationResult {
    Read(Vec<Result<ReadResult, KernelError>>),
    Write(Vec<Result<WriteResult, KernelError>>),
    Edit(Vec<Result<EditResult, KernelError>>),
    Find(Result<Vec<ResourceUri>, KernelError>),
    Grep(Result<Vec<AnchoredText>, KernelError>),
    Run(Vec<Result<ResourceUri, KernelError>>),
    Send(Vec<Result<ResourceUri, KernelError>>),
    Abort(Vec<Result<ResourceUri, KernelError>>),
    Delete(Vec<Result<ResourceUri, KernelError>>),
    Poll(Result<PollResult, KernelError>),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum ReadResult {
    Text(AnchoredText),
    Directory {
        uri: ResourceUri,
        entries: Vec<ResourceUri>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WriteResult {
    pub text: AnchoredText,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EditResult {
    pub uri: ResourceUri,
    pub changed: Vec<AnchoredText>,
    pub diff: AnchoredDiff,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DiffHunk {
    pub old: Vec<AnchoredLine>,
    pub new: Vec<AnchoredLine>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AnchoredDiff {
    pub uri: ResourceUri,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RegexAtom {
    pub target: u32,
    pub pattern: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum PollAtom {
    Changed(u32),
    Regex(RegexAtom),
    Terminated(u32),
    Timeout(u64),
}

/// The semantic poll condition. The component ABI may lower this recursive
/// value to an indexed arena, but the kernel must not make the lowering its
/// meaning: handlers and native callers need to evaluate the same tree.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum PollCondition {
    Atom(PollAtom),
    All(Vec<PollCondition>),
    Any(Vec<PollCondition>),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PollResult {
    pub text: Vec<AnchoredText>,
    pub satisfied: Vec<PollAtom>,
}
