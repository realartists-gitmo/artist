//! The typed kernel/component contract.
//!
//! JSON is an adapter concern. The kernel dispatches these values directly;
//! provider-facing JSON schemas are derived by the component layer.

use crate::{Anchor, ClaimDecision, KernelError, ResourceUri};
use serde::{Deserialize, Serialize};
use std::{
    any::Any,
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

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

/// Reconstruct the textual stream represented by anchored lines, preserving
/// each line's original terminator. Poll regexes operate on this accumulated
/// representation rather than on delivery chunks or individual lines.
pub fn accumulated_lines_text<'a>(lines: impl IntoIterator<Item = &'a AnchoredLine>) -> String {
    let mut text = String::new();
    for line in lines {
        text.push_str(&line.text);
        match line.ending {
            LineEnding::None => {}
            LineEnding::Lf => text.push('\n'),
            LineEnding::Crlf => text.push_str("\r\n"),
            LineEnding::Cr => text.push('\r'),
        }
    }
    text
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
    pub working_uri: Option<ResourceUri>,
    pub environment: Vec<EnvironmentEntry>,
    pub cancellation_token: Option<String>,
    pub deadline_ms: Option<u64>,
    pub correlation_id: Option<String>,
}

/// Runtime-only invocation state. The serializable context above remains the
/// compatibility/diagnostic metadata surface; cancellation is an actual
/// descendant-aware token and never crosses a model or WIT request boundary.
#[derive(Clone)]
pub struct InvocationScope {
    pub context: InvocationContext,
    pub cancellation: tokio_util::sync::CancellationToken,
    /// Handler-owned generation pins established during claim and consumed
    /// during invoke. The kernel does not interpret the value; it merely
    /// carries it across the two phases of one invocation.
    pub(crate) generation_pins: Arc<Mutex<HashMap<String, (String, u64)>>>,
    /// Active package generations observed by this top-level operation. This
    /// is shared by child scopes so batching and nested calls cannot observe a
    /// mid-operation hot swap.
    pub(crate) generation_snapshot: Arc<Mutex<HashMap<String, u64>>>,
    /// Handler-owned active generation objects. The kernel treats these as
    /// opaque leases; the concrete handler can recover its own generation
    /// type during nested routing even after its active registry swaps.
    generation_handles: Arc<Mutex<HashMap<String, Arc<dyn Any + Send + Sync>>>>,
    pub(crate) claim_decisions: Arc<Mutex<HashMap<String, ClaimDecision>>>,
    /// Routing frames shared by nested calls in one invocation chain. The
    /// kernel uses these to reject recursive resource re-entry before it can
    /// consume an unbounded amount of work.
    routing_stack: Arc<Mutex<Vec<String>>>,
    deadline_started: Instant,
}

/// A held routing frame. Dropping it unwinds the frame even when the nested
/// operation returns an error or is interrupted.
pub struct RoutingFrame {
    stack: Arc<Mutex<Vec<String>>>,
    key: String,
}

impl Drop for RoutingFrame {
    fn drop(&mut self) {
        let mut stack = self.stack.lock().unwrap();
        if stack.last().is_some_and(|current| current == &self.key) {
            stack.pop();
        } else if let Some(index) = stack.iter().rposition(|current| current == &self.key) {
            stack.remove(index);
        }
    }
}

impl InvocationScope {
    pub fn new(context: InvocationContext) -> Self {
        Self {
            context,
            cancellation: tokio_util::sync::CancellationToken::new(),
            generation_pins: Arc::new(Mutex::new(HashMap::new())),
            generation_snapshot: Arc::new(Mutex::new(HashMap::new())),
            generation_handles: Arc::new(Mutex::new(HashMap::new())),
            claim_decisions: Arc::new(Mutex::new(HashMap::new())),
            routing_stack: Arc::new(Mutex::new(Vec::new())),
            deadline_started: Instant::now(),
        }
    }

    pub fn with_cancellation(
        context: InvocationContext,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            context,
            cancellation,
            generation_pins: Arc::new(Mutex::new(HashMap::new())),
            generation_snapshot: Arc::new(Mutex::new(HashMap::new())),
            generation_handles: Arc::new(Mutex::new(HashMap::new())),
            claim_decisions: Arc::new(Mutex::new(HashMap::new())),
            routing_stack: Arc::new(Mutex::new(Vec::new())),
            deadline_started: Instant::now(),
        }
    }

    pub fn child(&self) -> Self {
        Self {
            context: self.context.clone(),
            cancellation: self.cancellation.child_token(),
            generation_pins: Arc::clone(&self.generation_pins),
            generation_snapshot: Arc::clone(&self.generation_snapshot),
            generation_handles: Arc::clone(&self.generation_handles),
            claim_decisions: Arc::clone(&self.claim_decisions),
            routing_stack: Arc::clone(&self.routing_stack),
            deadline_started: self.deadline_started,
        }
    }

    /// Enter a resource routing frame, rejecting a repeated frame in the
    /// current nested invocation chain.
    pub fn enter_routing_frame(&self, key: impl Into<String>) -> Result<RoutingFrame, String> {
        let key = key.into();
        let mut stack = self.routing_stack.lock().unwrap();
        if stack.iter().any(|current| current == &key) {
            let mut cycle = stack.clone();
            cycle.push(key.clone());
            return Err(cycle.join(" -> "));
        }
        stack.push(key.clone());
        Ok(RoutingFrame {
            stack: Arc::clone(&self.routing_stack),
            key,
        })
    }

    /// Return the remaining budget for this invocation chain. Child scopes
    /// share the parent's start instant, so nested calls cannot restart a
    /// deadline that has already been partly consumed.
    pub fn remaining_deadline(&self) -> Option<Duration> {
        self.context.deadline_ms.map(|milliseconds| {
            Duration::from_millis(milliseconds).saturating_sub(self.deadline_started.elapsed())
        })
    }

    /// Record a handler-owned generation selected during claim.
    pub fn pin_generation(
        &self,
        key: impl Into<String>,
        package: impl Into<String>,
        generation: u64,
    ) {
        self.generation_pins
            .lock()
            .unwrap()
            .insert(key.into(), (package.into(), generation));
    }

    /// Retrieve a generation pin for the invoke phase.
    pub fn pinned_generation(&self, key: &str) -> Option<(String, u64)> {
        self.generation_pins.lock().unwrap().get(key).cloned()
    }

    /// Record or retrieve the generation selected for a package during this
    /// top-level operation. The first observation wins for the whole scope.
    pub fn snapshot_generation(&self, package: &str, generation: u64) -> u64 {
        let mut snapshot = self.generation_snapshot.lock().unwrap();
        *snapshot.entry(package.to_owned()).or_insert(generation)
    }

    pub fn snapshotted_generation(&self, package: &str) -> Option<u64> {
        self.generation_snapshot
            .lock()
            .unwrap()
            .get(package)
            .copied()
    }

    /// Pin a handler-owned generation for this invocation chain.
    pub fn pin_generation_handle<T>(&self, key: impl Into<String>, handle: Arc<T>)
    where
        T: Any + Send + Sync,
    {
        self.generation_handles
            .lock()
            .unwrap()
            .insert(key.into(), handle);
    }

    /// Recover a previously pinned handler-owned generation.
    pub fn generation_handle<T>(&self, key: &str) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        self.generation_handles
            .lock()
            .unwrap()
            .get(key)
            .and_then(|handle| Arc::clone(handle).downcast::<T>().ok())
    }

    pub fn pin_claim(&self, key: impl Into<String>, decision: ClaimDecision) {
        self.claim_decisions
            .lock()
            .unwrap()
            .insert(key.into(), decision);
    }

    pub fn pinned_claim(&self, key: &str) -> Option<ClaimDecision> {
        self.claim_decisions.lock().unwrap().get(key).copied()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RunRequest {
    pub uri: ResourceUri,
    pub args: Vec<String>,
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

/// Apply the URI-layer fragment rules once at the kernel boundary. Providers
/// and claims only see the base path plus query; textual anchors become the
/// existing positional fields for the operations that have them.
pub fn lower_operation_fragments(operation: Operation) -> Result<Operation, KernelError> {
    fn anchor(uri: &ResourceUri) -> Anchor {
        Anchor::from_tokens(
            uri.fragment()
                .unwrap_or_default()
                .split('.')
                .map(str::to_owned)
                .collect(),
        )
    }
    fn reject(uri: &ResourceUri) -> Result<(), KernelError> {
        if uri.fragment().is_some() {
            Err(KernelError::InvalidRequest {
                message: format!("fragment is not valid for this operation: {uri}"),
            })
        } else {
            Ok(())
        }
    }
    Ok(match operation {
        Operation::Read(requests) => Operation::Read(
            requests
                .into_iter()
                .map(|mut request| {
                    if request.uri.fragment().is_some() {
                        if request.at.is_some() {
                            return Err(KernelError::InvalidRequest {
                                message: "read cannot combine a URI fragment with at".to_owned(),
                            });
                        }
                        request.at = Some(Position::At(anchor(&request.uri)));
                        request.uri = request.uri.without_fragment();
                    }
                    Ok(request)
                })
                .collect::<Result<_, _>>()?,
        ),
        Operation::Poll(mut request) => {
            for target in &mut request.targets {
                if target.uri.fragment().is_some() {
                    if target.from_position.is_some() {
                        return Err(KernelError::InvalidRequest {
                            message: "poll cannot combine a URI fragment with from-position"
                                .to_owned(),
                        });
                    }
                    target.from_position = Some(Position::At(anchor(&target.uri)));
                    target.uri = target.uri.without_fragment();
                }
            }
            Operation::Poll(request)
        }
        Operation::Write(requests) => {
            for request in &requests {
                reject(&request.uri)?;
            }
            Operation::Write(requests)
        }
        Operation::Edit(requests) => {
            for request in &requests {
                reject(&request.uri)?;
            }
            Operation::Edit(requests)
        }
        Operation::Run(requests) => {
            for request in &requests {
                reject(&request.uri)?;
            }
            Operation::Run(requests)
        }
        Operation::Send(requests) => {
            for request in &requests {
                reject(&request.uri)?;
            }
            Operation::Send(requests)
        }
        Operation::Abort(uris) => {
            for uri in &uris {
                reject(uri)?;
            }
            Operation::Abort(uris)
        }
        Operation::Delete(uris) => {
            for uri in &uris {
                reject(uri)?;
            }
            Operation::Delete(uris)
        }
        Operation::Find(request) => {
            for uri in &request.roots {
                reject(uri)?;
            }
            Operation::Find(request)
        }
        Operation::Grep(request) => {
            if let GrepSource::Resources(uris) = &request.source {
                for uri in uris {
                    reject(uri)?;
                }
            } else if let GrepSource::Text(text) = &request.source {
                for value in text {
                    reject(&value.uri)?;
                }
            }
            Operation::Grep(request)
        }
    })
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
    pub text: AnchoredText,
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

#[cfg(test)]
mod tests {
    use super::{InvocationContext, InvocationScope};

    #[test]
    fn nested_scopes_share_generation_and_routing_state_but_cancel_independently() {
        let parent = InvocationScope::new(InvocationContext::default());
        let _parent_frame = parent
            .enter_routing_frame("ast@7:read:file:///src/main.rs")
            .unwrap();
        let child = parent.child();

        assert!(
            child
                .enter_routing_frame("ast@7:read:file:///src/main.rs")
                .is_err()
        );

        let _other_frame = child
            .enter_routing_frame("ast@7:read:file:///src/lib.rs")
            .unwrap();
        drop(_other_frame);
        assert!(!child.cancellation.is_cancelled());
        parent.cancellation.cancel();
        assert!(parent.cancellation.is_cancelled());
        assert!(child.cancellation.is_cancelled());
    }

    #[test]
    fn nested_scopes_retain_handler_generation_leases() {
        let parent = InvocationScope::new(InvocationContext::default());
        let generation = std::sync::Arc::new(String::from("generation-seven"));
        parent.pin_generation_handle("ast:read:file:///src/main.rs", generation);

        let child = parent.child();
        assert_eq!(
            child
                .generation_handle::<String>("ast:read:file:///src/main.rs")
                .as_deref()
                .map(String::as_str),
            Some("generation-seven")
        );
    }

    #[test]
    fn child_scope_cannot_restart_an_expired_deadline() {
        let scope = InvocationScope::new(InvocationContext {
            deadline_ms: Some(0),
            ..InvocationContext::default()
        });
        assert_eq!(scope.remaining_deadline(), Some(std::time::Duration::ZERO));
        assert_eq!(
            scope.child().remaining_deadline(),
            Some(std::time::Duration::ZERO)
        );
    }
}
