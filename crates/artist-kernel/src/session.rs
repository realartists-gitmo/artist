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
    fn verb_definitions(&self) -> Vec<crate::VerbDefinition> {
        [
            (&self.bindings.read, "read"),
            (&self.bindings.write, "write"),
            (&self.bindings.poll, "poll"),
            (&self.bindings.abort, "abort"),
            (&self.bindings.delete, "delete"),
        ]
        .into_iter()
        .map(|(identity, function)| {
            crate::VerbDefinition::new(
                identity.clone(),
                function,
                function,
                format!("Session {function} provider"),
            )
        })
        .collect()
    }

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
                let content = if uri.path().ends_with("/inbox") {
                    session_content(&input)?
                } else {
                    String::new()
                };
                self.handler.dynamic_write(uri.clone(), content).await?
            } else if verb == &self.bindings.poll {
                self.handler.dynamic_poll(uri.clone(), input).await?
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
        let session = self
            .lookup(&ResourceAddress::uri(session_root(&uri)?))
            .await?;
        let state = session.state.lock().await;
        let text = session_read_window(&uri, &snapshot(&state, 0), None, None, None)?;
        Ok(session_text(text))
    }

    async fn dynamic_write(
        &self,
        uri: ResourceUri,
        content: String,
    ) -> Result<DynamicValue, KernelError> {
        if !uri.path().ends_with("/inbox") {
            self.create(&ResourceAddress::uri(uri.clone())).await?;
            return Ok(session_text(AnchoredText {
                uri,
                lines: Vec::new(),
            }));
        }
        let target = ResourceUri::parse(uri.to_string().trim_end_matches("/inbox"))?;
        let session = self.lookup(&ResourceAddress::uri(target)).await?;
        let mut state = session.state.lock().await;
        if state.status.terminal() {
            return Err(KernelError::InvalidRequest {
                message: "cannot write to an aborted session".to_owned(),
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

    async fn dynamic_poll(
        &self,
        uri: ResourceUri,
        input: DynamicValue,
    ) -> Result<DynamicValue, KernelError> {
        let session = self
            .lookup(&ResourceAddress::uri(session_root(&uri)?))
            .await?;
        let (pattern, timeout_ms) = poll_options(&input)?;
        let matcher = pattern
            .as_deref()
            .map(regex::Regex::new)
            .transpose()
            .map_err(|error| KernelError::InvalidPattern {
                message: error.to_string(),
            })?;
        let timeout = timeout_ms.map(std::time::Duration::from_millis);
        loop {
            let notified = session.changed.notified();
            let state = session.state.lock().await;
            let accumulated = state
                .events
                .iter()
                .map(|event| event.data.as_str())
                .collect::<String>();
            let matched = matcher
                .as_ref()
                .is_some_and(|matcher| matcher.is_match(&accumulated));
            if state.status.terminal() || matched || (matcher.is_none() && !state.events.is_empty())
            {
                let value = snapshot(&state, 0);
                let text = anchored_session_window(&uri, &value, None, None)?;
                let reason = if matched {
                    "matched"
                } else if state.status.terminal() {
                    "terminated"
                } else {
                    "changed"
                };
                return Ok(DynamicValue::Record(BTreeMap::from([
                    ("uri".to_owned(), DynamicValue::ResourceUri(uri.clone())),
                    ("text".to_owned(), session_text(text)),
                    ("reason".to_owned(), DynamicValue::String(reason.to_owned())),
                ])));
            }
            drop(state);
            if let Some(timeout) = timeout {
                if tokio::time::timeout(timeout, notified).await.is_err() {
                    let state = session.state.lock().await;
                    let text = anchored_session_window(&uri, &snapshot(&state, 0), None, None)?;
                    return Ok(DynamicValue::Record(BTreeMap::from([
                        ("uri".to_owned(), DynamicValue::ResourceUri(uri.clone())),
                        ("text".to_owned(), session_text(text)),
                        (
                            "reason".to_owned(),
                            DynamicValue::String("timeout".to_owned()),
                        ),
                    ])));
                }
            } else {
                notified.await;
            }
        }
    }

    async fn dynamic_abort(&self, uri: ResourceUri) -> Result<DynamicValue, KernelError> {
        let session = self
            .lookup(&ResourceAddress::uri(session_root(&uri)?))
            .await?;
        let mut state = session.state.lock().await;
        state.status = Status::Aborted;
        drop(state);
        session.changed.notify_waiters();
        Ok(DynamicValue::ResourceUri(uri))
    }

    async fn dynamic_delete(&self, uri: ResourceUri) -> Result<DynamicValue, KernelError> {
        let key = session_root(&uri)?.to_string();
        if self.sessions.write().await.remove(&key).is_none() {
            return Err(KernelError::NotFound { uri: key });
        }
        Ok(DynamicValue::ResourceUri(uri))
    }
}

fn session_root(uri: &ResourceUri) -> Result<ResourceUri, KernelError> {
    if uri.path().ends_with("/inbox") {
        ResourceUri::parse(uri.to_string().trim_end_matches("/inbox")).map_err(|error| {
            KernelError::InvalidUri {
                message: error.to_string(),
            }
        })
    } else {
        Ok(uri.clone())
    }
}

fn session_content(input: &DynamicValue) -> Result<String, KernelError> {
    match input {
        DynamicValue::String(value) => Ok(value.clone()),
        DynamicValue::Record(fields) => match fields.get("content").or_else(|| fields.get("value"))
        {
            Some(DynamicValue::String(value)) => Ok(value.clone()),
            _ => Err(KernelError::InvalidRequest {
                message: "session inbox requires a content string".to_owned(),
            }),
        },
        _ => Err(KernelError::InvalidRequest {
            message: "session inbox requires a content string".to_owned(),
        }),
    }
}

fn poll_options(input: &DynamicValue) -> Result<(Option<String>, Option<u64>), KernelError> {
    let DynamicValue::Record(fields) = input else {
        return Ok((None, None));
    };
    let pattern = match fields.get("match") {
        None | Some(DynamicValue::Option(None)) => None,
        Some(DynamicValue::Option(Some(value))) => match value.as_ref() {
            DynamicValue::String(value) => Some(value.clone()),
            _ => {
                return Err(KernelError::InvalidPattern {
                    message: "poll match must be a string".into(),
                });
            }
        },
        Some(DynamicValue::String(value)) => Some(value.clone()),
        _ => {
            return Err(KernelError::InvalidPattern {
                message: "poll match must be a string".into(),
            });
        }
    };
    let timeout_ms = match fields.get("timeout-ms") {
        None | Some(DynamicValue::Option(None)) => None,
        Some(DynamicValue::Option(Some(value))) => match value.as_ref() {
            DynamicValue::U64(value) => Some(*value),
            DynamicValue::S64(value) if *value >= 0 => Some(*value as u64),
            _ => {
                return Err(KernelError::InvalidRequest {
                    message: "poll timeout-ms must be u64".into(),
                });
            }
        },
        Some(DynamicValue::U64(value)) => Some(*value),
        Some(DynamicValue::S64(value)) if *value >= 0 => Some(*value as u64),
        _ => {
            return Err(KernelError::InvalidRequest {
                message: "poll timeout-ms must be u64".into(),
            });
        }
    };
    Ok((pattern, timeout_ms))
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
