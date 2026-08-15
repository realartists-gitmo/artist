//! Minimal live-resource handler.
//!
//! This is intentionally not an agent, shell, or process runner. It proves
//! that a synchronized, append-only live resource can use the same kernel
//! verbs as ordinary files.

use crate::{
    Anchor, AnchoredLine, AnchoredText, ClaimDecision, DynamicClaimProvider,
    DynamicResourceProvider, DynamicValue, DynamicVerbResult, KernelError, ResourceAddress,
    ResourceFuture, ResourceUri, VerbId,
};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{Mutex, Notify, RwLock};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Status {
    Running,
    Aborted,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Aborted => "aborted",
        }
    }

    fn terminal(self) -> bool {
        matches!(self, Self::Aborted)
    }
}

#[derive(Clone, Debug)]
struct Event {
    seq: u64,
    kind: &'static str,
    data: String,
}

#[derive(Debug)]
struct SessionState {
    status: Status,
    next_seq: u64,
    events: Vec<Event>,
}

#[derive(Clone, Debug)]
struct SessionSnapshot {
    status: Status,
    next_seq: u64,
    events: Vec<Event>,
}

struct Session {
    state: Mutex<SessionState>,
    changed: Notify,
}

impl Session {
    fn new() -> Self {
        Self {
            state: Mutex::new(SessionState {
                status: Status::Running,
                next_seq: 0,
                events: Vec::new(),
            }),
            changed: Notify::new(),
        }
    }
}

/// In-memory live sessions used to establish the universal live-resource
/// contract before real agents, shells, and subprocesses are rebuilt.
#[derive(Clone, Default)]
pub struct SessionHandler {
    sessions: Arc<RwLock<BTreeMap<String, Arc<Session>>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionVerbBindings {
    pub read: VerbId,
    pub write: VerbId,
    pub send: VerbId,
    pub poll: VerbId,
    pub abort: VerbId,
    pub delete: VerbId,
}

pub struct SessionResourceProvider {
    handler: SessionHandler,
    bindings: SessionVerbBindings,
}

impl SessionResourceProvider {
    pub fn new(handler: SessionHandler, bindings: SessionVerbBindings) -> Self {
        Self { handler, bindings }
    }
}

impl SessionHandler {
    pub fn new() -> Self {
        Self::default()
    }

    async fn lookup(&self, target: &ResourceAddress) -> Result<Arc<Session>, KernelError> {
        let key = target.to_string();
        self.sessions
            .read()
            .await
            .get(&key)
            .cloned()
            .ok_or(KernelError::NotFound { uri: key })
    }

    async fn create(&self, target: &ResourceAddress) -> Result<(), KernelError> {
        let key = target.to_string();
        let mut sessions = self.sessions.write().await;
        if sessions.contains_key(&key) {
            return Err(KernelError::AlreadyExists { uri: key });
        }
        sessions.insert(key.clone(), Arc::new(Session::new()));
        Ok(())
    }
}

const DEFAULT_READ_WINDOW: usize = 200;

fn session_lines(snapshot: &SessionSnapshot) -> Vec<AnchoredLine> {
    snapshot
        .events
        .iter()
        .map(|event| AnchoredLine {
            anchor: Anchor::from_tokens(vec![event.seq.to_string()]),
            text: event.data.clone(),
            ending: crate::LineEnding::Lf,
        })
        .collect()
}

fn session_read_window(
    uri: &ResourceUri,
    snapshot: &SessionSnapshot,
    at: Option<&crate::Position>,
    before: Option<u32>,
    after: Option<u32>,
) -> Result<AnchoredText, KernelError> {
    let lines = session_lines(snapshot);
    let at = at.unwrap_or(&crate::Position::Top);
    if matches!(at, crate::Position::Top) && before.is_some()
        || matches!(at, crate::Position::Bottom) && after.is_some()
    {
        return Err(KernelError::InvalidRequest {
            message: "read window is invalid for its position".to_owned(),
        });
    }
    let index = match at {
        crate::Position::Top => 0,
        crate::Position::Bottom => lines.len(),
        crate::Position::At(anchor) => lines
            .iter()
            .position(|line| &line.anchor == anchor)
            .ok_or_else(|| KernelError::StaleAnchor {
                message: format!("read anchor does not resolve: {anchor}"),
            })?,
    };
    let before = before.map_or(DEFAULT_READ_WINDOW, |value| value as usize);
    let after = after.map_or(DEFAULT_READ_WINDOW, |value| value as usize);
    let (start, end) = match at {
        crate::Position::Bottom => (index.saturating_sub(before), index),
        _ => (
            index.saturating_sub(before),
            (index + after + 1).min(lines.len()),
        ),
    };
    Ok(AnchoredText {
        uri: uri.clone(),
        lines: lines[start..end].to_vec(),
    })
}

fn anchored_session_window(
    uri: &ResourceUri,
    snapshot: &SessionSnapshot,
    before: Option<u32>,
    after: Option<u32>,
) -> Result<AnchoredText, KernelError> {
    session_read_window(uri, snapshot, Some(&crate::Position::Bottom), before, after)
}

impl DynamicClaimProvider for SessionResourceProvider {
    fn claim(&self, verb: &VerbId, uri: &ResourceUri) -> ClaimDecision {
        let supported = [
            &self.bindings.read,
            &self.bindings.write,
            &self.bindings.send,
            &self.bindings.poll,
            &self.bindings.abort,
            &self.bindings.delete,
        ]
        .iter()
        .any(|candidate| *candidate == verb);
        if supported && uri.scheme() == "session" {
            ClaimDecision::Handle
        } else {
            ClaimDecision::Pass
        }
    }
}

impl DynamicResourceProvider for SessionResourceProvider {
    fn invoke<'a>(
        &'a self,
        verb: &'a VerbId,
        uri: &'a ResourceUri,
        input: DynamicValue,
    ) -> ResourceFuture<'a> {
        Box::pin(async move {
            let output = if verb == &self.bindings.read {
                self.handler.dynamic_read(uri.clone()).await?
            } else if verb == &self.bindings.write {
                self.handler.dynamic_write(uri.clone()).await?
            } else if verb == &self.bindings.send {
                self.handler
                    .dynamic_send(uri.clone(), session_content(&input)?)
                    .await?
            } else if verb == &self.bindings.poll {
                self.handler.dynamic_poll(uri.clone()).await?
            } else if verb == &self.bindings.abort {
                self.handler.dynamic_abort(uri.clone()).await?
            } else if verb == &self.bindings.delete {
                self.handler.dynamic_delete(uri.clone()).await?
            } else {
                return Err(KernelError::UnsupportedVerb {
                    verb: verb.to_string(),
                    uri: uri.to_string(),
                });
            };
            Ok(DynamicVerbResult {
                verb: verb.clone(),
                function: verb.function().to_owned(),
                output,
            })
        })
    }
}

impl SessionHandler {
    async fn dynamic_read(&self, uri: ResourceUri) -> Result<DynamicValue, KernelError> {
        let session = self.lookup(&ResourceAddress::uri(uri.clone())).await?;
        let state = session.state.lock().await;
        let text = session_read_window(&uri, &snapshot(&state, 0), None, None, None)?;
        Ok(session_text(text))
    }

    async fn dynamic_write(&self, uri: ResourceUri) -> Result<DynamicValue, KernelError> {
        self.create(&ResourceAddress::uri(uri.clone())).await?;
        Ok(session_text(AnchoredText {
            uri,
            lines: Vec::new(),
        }))
    }

    async fn dynamic_send(
        &self,
        uri: ResourceUri,
        content: String,
    ) -> Result<DynamicValue, KernelError> {
        let session = self.lookup(&ResourceAddress::uri(uri.clone())).await?;
        let mut state = session.state.lock().await;
        if state.status.terminal() {
            return Err(KernelError::InvalidRequest {
                message: "cannot send to an aborted session".to_owned(),
            });
        }
        let seq = state.next_seq;
        state.next_seq += 1;
        state.events.push(Event {
            seq,
            kind: "input",
            data: content,
        });
        drop(state);
        session.changed.notify_waiters();
        Ok(DynamicValue::ResourceUri(uri))
    }

    async fn dynamic_poll(&self, uri: ResourceUri) -> Result<DynamicValue, KernelError> {
        let session = self.lookup(&ResourceAddress::uri(uri.clone())).await?;
        loop {
            let notified = session.changed.notified();
            let state = session.state.lock().await;
            if state.status.terminal() || !state.events.is_empty() {
                let value = snapshot(&state, 0);
                let text = anchored_session_window(&uri, &value, None, None)?;
                let satisfied = if state.status.terminal() {
                    vec![DynamicValue::String("terminated".to_owned())]
                } else {
                    vec![DynamicValue::String("changed".to_owned())]
                };
                return Ok(DynamicValue::Record(BTreeMap::from([
                    (
                        "text".to_owned(),
                        DynamicValue::List(vec![session_text(text)]),
                    ),
                    ("satisfied".to_owned(), DynamicValue::List(satisfied)),
                ])));
            }
            drop(state);
            notified.await;
        }
    }

    async fn dynamic_abort(&self, uri: ResourceUri) -> Result<DynamicValue, KernelError> {
        let session = self.lookup(&ResourceAddress::uri(uri.clone())).await?;
        let mut state = session.state.lock().await;
        state.status = Status::Aborted;
        drop(state);
        session.changed.notify_waiters();
        Ok(DynamicValue::ResourceUri(uri))
    }

    async fn dynamic_delete(&self, uri: ResourceUri) -> Result<DynamicValue, KernelError> {
        let key = uri.to_string();
        if self.sessions.write().await.remove(&key).is_none() {
            return Err(KernelError::NotFound { uri: key });
        }
        Ok(DynamicValue::ResourceUri(uri))
    }
}

fn session_content(input: &DynamicValue) -> Result<String, KernelError> {
    match input {
        DynamicValue::String(value) => Ok(value.clone()),
        DynamicValue::Record(fields) => match fields.get("content").or_else(|| fields.get("value"))
        {
            Some(DynamicValue::String(value)) => Ok(value.clone()),
            _ => Err(KernelError::InvalidRequest {
                message: "session send requires a content string".to_owned(),
            }),
        },
        _ => Err(KernelError::InvalidRequest {
            message: "session send requires a content string".to_owned(),
        }),
    }
}

fn session_line(line: AnchoredLine) -> DynamicValue {
    DynamicValue::Record(BTreeMap::from([
        (
            "anchor".to_owned(),
            DynamicValue::List(
                line.anchor
                    .tokens()
                    .iter()
                    .cloned()
                    .map(DynamicValue::String)
                    .collect(),
            ),
        ),
        ("text".to_owned(), DynamicValue::String(line.text)),
        (
            "ending".to_owned(),
            DynamicValue::String(format!("{:?}", line.ending).to_lowercase()),
        ),
    ]))
}

fn session_text(text: AnchoredText) -> DynamicValue {
    DynamicValue::Record(BTreeMap::from([
        ("uri".to_owned(), DynamicValue::ResourceUri(text.uri)),
        (
            "lines".to_owned(),
            DynamicValue::List(text.lines.into_iter().map(session_line).collect()),
        ),
    ]))
}

fn snapshot(state: &SessionState, since: u64) -> SessionSnapshot {
    SessionSnapshot {
        status: state.status,
        next_seq: state.next_seq,
        events: state
            .events
            .iter()
            .filter(|event| event.seq >= since)
            .cloned()
            .collect(),
    }
}
