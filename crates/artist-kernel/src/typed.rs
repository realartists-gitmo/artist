//! The typed kernel/component contract.
//!
//! JSON is an adapter concern. The kernel dispatches these values directly;
//! provider-facing JSON schemas are derived by the component layer.

use crate::{Anchor, ClaimDecision, ResourceUri};
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
pub struct ReadInput {
    pub uri: ResourceUri,
    pub at: Option<Position>,
    pub before: Option<u32>,
    pub after: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WriteInput {
    pub uri: ResourceUri,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum InsertionPosition {
    Top,
    Bottom,
    At(Anchor),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EditInput {
    pub uri: ResourceUri,
    pub start: Anchor,
    pub end: Option<Anchor>,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InsertInput {
    pub uri: ResourceUri,
    pub at: InsertionPosition,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FindInput {
    pub root: ResourceUri,
    pub query: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct GrepInput {
    pub uri: ResourceUri,
    pub pattern: String,
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
pub struct RunInput {
    pub uri: ResourceUri,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PollInput {
    pub uri: ResourceUri,
    pub from: Option<Position>,
    pub match_pattern: Option<String>,
    pub timeout_ms: Option<u64>,
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
    pub uri: ResourceUri,
    pub text: Option<AnchoredText>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EditResult {
    pub uri: ResourceUri,
    pub changed: Vec<AnchoredText>,
    pub diff: AnchoredDiff,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InsertResult {
    pub uri: ResourceUri,
    pub changed: Vec<AnchoredText>,
    pub diff: AnchoredDiff,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FindResult {
    pub uris: Vec<ResourceUri>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct GrepResult {
    pub matches: Vec<AnchoredText>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RunResult {
    pub uri: ResourceUri,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UriInput {
    pub uri: ResourceUri,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UriResult {
    pub uri: ResourceUri,
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
pub enum PollReason {
    Changed,
    Matched,
    Terminated,
    Timeout,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PollResult {
    pub uri: ResourceUri,
    pub text: AnchoredText,
    pub reason: PollReason,
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
