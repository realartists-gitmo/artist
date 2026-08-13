//! Minimal live-resource handler.
//!
//! This is intentionally not an agent, shell, or process runner. It proves
//! that a synchronized, append-only live resource can use the same kernel
//! verbs as ordinary files.

use crate::{
    Anchor, AnchoredLine, AnchoredText, BoxFuture, Handler, HandlerDescriptor, KernelError,
    KernelHandle, Operation, OperationResult, PollAtom, ReadResult, Request, ResourceAddress,
    TypedHandler, Verb, WriteResult,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::time::{Duration, Instant, timeout};

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

    async fn create(&self, target: &ResourceAddress) -> Result<Value, KernelError> {
        let key = target.to_string();
        let mut sessions = self.sessions.write().await;
        if sessions.contains_key(&key) {
            return Err(KernelError::AlreadyExists { uri: key });
        }
        sessions.insert(key.clone(), Arc::new(Session::new()));
        Ok(json!({"created": true, "uri": key, "status": "running"}))
    }

    async fn read(&self, target: &ResourceAddress, args: &Value) -> Result<Value, KernelError> {
        let session = self.lookup(target).await?;
        let since = args.get("since").and_then(Value::as_u64).unwrap_or(0);
        let state = session.state.lock().await;
        Ok(snapshot(&state, since))
    }

    async fn send(&self, target: &ResourceAddress, args: &Value) -> Result<Value, KernelError> {
        let session = self.lookup(target).await?;
        let input = args
            .get("value")
            .or_else(|| args.get("input"))
            .and_then(Value::as_str)
            .ok_or_else(|| KernelError::InvalidRequest {
                message: "session send requires an args.value string".to_owned(),
            })?
            .to_owned();
        let mut state = session.state.lock().await;
        if state.status.terminal() {
            return Err(KernelError::InvalidState {
                message: "cannot send to an aborted session".to_owned(),
            });
        }
        let seq = state.next_seq;
        state.next_seq += 1;
        state.events.push(Event {
            seq,
            kind: "input",
            data: input,
        });
        let next_seq = state.next_seq;
        drop(state);
        session.changed.notify_waiters();
        Ok(json!({"sent": true, "seq": seq, "next_seq": next_seq}))
    }

    async fn poll(&self, target: &ResourceAddress, args: &Value) -> Result<Value, KernelError> {
        let session = self.lookup(target).await?;
        let since = args.get("since").and_then(Value::as_u64).unwrap_or(0);
        let timeout_ms = args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(0);
        let min_lines = args.get("lines").and_then(Value::as_u64).unwrap_or(1);
        let matcher = args.get("match").and_then(Value::as_str).map(str::to_owned);

        let wait = async {
            loop {
                let notified = session.changed.notified();
                let state = session.state.lock().await;
                let available = state
                    .events
                    .iter()
                    .filter(|event| event.seq >= since)
                    .count() as u64;
                let matched = matcher.as_ref().is_some_and(|pattern| {
                    state
                        .events
                        .iter()
                        .any(|event| event.seq >= since && event.data.contains(pattern))
                });
                if available >= min_lines || matched || state.status.terminal() {
                    return Ok((
                        snapshot(&state, since),
                        if state.status.terminal() {
                            "terminal"
                        } else if matched {
                            "match"
                        } else {
                            "lines"
                        },
                    ));
                }
                drop(state);
                notified.await;
            }
        };

        match timeout(Duration::from_millis(timeout_ms), wait).await {
            Ok(result) => {
                let (mut value, condition) = result?;
                value["condition"] = Value::String(condition.to_owned());
                Ok(value)
            }
            Err(_) => Ok(timeout_snapshot(&session, since).await),
        }
    }

    async fn abort(&self, target: &ResourceAddress) -> Result<Value, KernelError> {
        let session = self.lookup(target).await?;
        let mut state = session.state.lock().await;
        state.status = Status::Aborted;
        let status = state.status.as_str();
        drop(state);
        session.changed.notify_waiters();
        Ok(json!({"aborted": true, "status": status}))
    }

    async fn delete(&self, target: &ResourceAddress) -> Result<Value, KernelError> {
        let key = target.to_string();
        let removed = self.sessions.write().await.remove(&key).is_some();
        if removed {
            Ok(json!({"deleted": true, "uri": key}))
        } else {
            Err(KernelError::NotFound { uri: key })
        }
    }

    async fn poll_cursors(&self, targets: &[crate::PollTarget]) -> Result<Vec<u64>, KernelError> {
        let mut cursors = Vec::with_capacity(targets.len());
        for target in targets {
            let session = self
                .lookup(&ResourceAddress::uri(target.uri.clone()))
                .await?;
            let state = session.state.lock().await;
            cursors.push(match target.from_position.as_ref() {
                None | Some(crate::Position::Bottom) => state.next_seq,
                Some(crate::Position::Top) => 0,
                Some(crate::Position::At(anchor)) => {
                    let token =
                        anchor
                            .tokens()
                            .first()
                            .ok_or_else(|| KernelError::StaleAnchor {
                                message: format!("poll anchor does not resolve: {anchor}"),
                            })?;
                    let seq = token.parse::<u64>().map_err(|_| KernelError::StaleAnchor {
                        message: format!("poll anchor does not resolve: {anchor}"),
                    })?;
                    seq.checked_add(1).ok_or_else(|| KernelError::StaleAnchor {
                        message: format!("poll anchor does not resolve: {anchor}"),
                    })?
                }
            });
        }
        Ok(cursors)
    }
}

impl Handler for SessionHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            name: "session".to_owned(),
            schemes: vec!["session".to_owned()],
            verbs: vec![
                Verb::Read,
                Verb::Write,
                Verb::Send,
                Verb::Poll,
                Verb::Abort,
                Verb::Delete,
            ],
        }
    }

    fn execute<'a>(
        &'a self,
        request: Request,
        _host: KernelHandle,
    ) -> BoxFuture<'a, Result<Value, KernelError>> {
        Box::pin(async move {
            match request.verb {
                Verb::Read => self.read(&request.target, &request.args).await,
                Verb::Write => self.create(&request.target).await,
                Verb::Send => self.send(&request.target, &request.args).await,
                Verb::Poll => self.poll(&request.target, &request.args).await,
                Verb::Abort => self.abort(&request.target).await,
                Verb::Delete => self.delete(&request.target).await,
                verb => Err(KernelError::UnsupportedVerb {
                    verb: verb.to_string(),
                    uri: request.target.to_string(),
                }),
            }
        })
    }
}

impl TypedHandler for SessionHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            name: "session-typed".to_owned(),
            schemes: vec!["session".to_owned()],
            verbs: vec![
                Verb::Read,
                Verb::Write,
                Verb::Send,
                Verb::Poll,
                Verb::Abort,
                Verb::Delete,
            ],
        }
    }

    fn claims_operation(&self, operation: &Operation) -> bool {
        let uris: Vec<&crate::ResourceUri> = match operation {
            Operation::Read(requests) => requests.iter().map(|request| &request.uri).collect(),
            Operation::Write(requests) => requests.iter().map(|request| &request.uri).collect(),
            Operation::Send(requests) => requests.iter().map(|request| &request.uri).collect(),
            Operation::Poll(request) => request.targets.iter().map(|target| &target.uri).collect(),
            Operation::Abort(uris) | Operation::Delete(uris) => uris.iter().collect(),
            _ => return false,
        };
        !uris.is_empty() && uris.iter().all(|uri| uri.scheme() == "session")
    }

    fn execute_typed<'a>(
        &'a self,
        operation: Operation,
        _host: KernelHandle,
        _context: crate::InvocationContext,
    ) -> BoxFuture<'a, Result<OperationResult, KernelError>> {
        Box::pin(async move {
            match operation {
                Operation::Read(requests) => {
                    let mut results = Vec::with_capacity(requests.len());
                    for request in requests {
                        let target = ResourceAddress::uri(request.uri.clone());
                        let result = match self.lookup(&target).await {
                            Ok(session) => {
                                let state = session.state.lock().await;
                                let value = snapshot(&state, 0);
                                session_read_window(
                                    &request.uri,
                                    &value,
                                    request.at.as_ref(),
                                    request.before,
                                    request.after,
                                )
                            }
                            Err(error) => Err(error),
                        };
                        results.push(result);
                    }
                    Ok(OperationResult::Read(
                        results
                            .into_iter()
                            .map(|result| result.map(ReadResult::Text))
                            .collect(),
                    ))
                }
                Operation::Write(requests) => {
                    let mut results = Vec::with_capacity(requests.len());
                    for request in requests {
                        let target = ResourceAddress::uri(request.uri.clone());
                        results.push(self.create(&target).await.map(|_| WriteResult {
                            text: AnchoredText {
                                uri: request.uri,
                                lines: Vec::new(),
                            },
                        }));
                    }
                    Ok(OperationResult::Write(results))
                }
                Operation::Send(requests) => {
                    let mut results = Vec::with_capacity(requests.len());
                    for request in requests {
                        let target = ResourceAddress::uri(request.uri.clone());
                        results.push(
                            self.send(&target, &json!({"value": request.content}))
                                .await
                                .map(|_| request.uri),
                        );
                    }
                    Ok(OperationResult::Send(results))
                }
                Operation::Abort(uris) => {
                    let mut results = Vec::with_capacity(uris.len());
                    for uri in uris {
                        results.push(
                            self.abort(&ResourceAddress::uri(uri.clone()))
                                .await
                                .map(|_| uri),
                        );
                    }
                    Ok(OperationResult::Abort(results))
                }
                Operation::Delete(uris) => {
                    let mut results = Vec::with_capacity(uris.len());
                    for uri in uris {
                        results.push(
                            self.delete(&ResourceAddress::uri(uri.clone()))
                                .await
                                .map(|_| uri),
                        );
                    }
                    Ok(OperationResult::Delete(results))
                }
                Operation::Poll(request) => {
                    if request.targets.is_empty() {
                        return Err(KernelError::InvalidRequest {
                            message: "poll requires at least one target".to_owned(),
                        });
                    }
                    let condition = request
                        .until
                        .unwrap_or_else(|| crate::default_poll_condition(request.targets.len()));
                    validate_poll_condition(&condition, request.targets.len())?;
                    let started = Instant::now();
                    let cursors = self.poll_cursors(&request.targets).await?;
                    let mut snapshots = Vec::new();
                    let mut satisfied = loop {
                        snapshots.clear();
                        for (target, since) in request.targets.iter().zip(&cursors) {
                            let value = self
                                .poll(
                                    &ResourceAddress::uri(target.uri.clone()),
                                    &json!({
                                        "since": since,
                                        "timeout_ms": 50,
                                        "lines": 1,
                                    }),
                                )
                                .await?;
                            snapshots.push(value);
                        }
                        let elapsed = started.elapsed();
                        let (ok, atoms) = evaluate_condition(&condition, &snapshots, elapsed);
                        if ok {
                            break atoms;
                        }
                    };
                    // A bounded cross-handler poll may ask this handler to
                    // wake on a short timeout. Preserve terminal state in the
                    // typed result so the kernel can still evaluate a
                    // caller's Terminated atom after combining handlers.
                    for (index, value) in snapshots.iter().enumerate() {
                        if value["status"].as_str() == Some("aborted") {
                            let atom = PollAtom::Terminated(index as u32);
                            if !satisfied.contains(&atom) {
                                satisfied.push(atom);
                            }
                        }
                    }
                    let text = request
                        .targets
                        .iter()
                        .zip(snapshots.iter())
                        .map(|(target, value)| {
                            anchored_session_window(
                                &target.uri,
                                value,
                                request.before,
                                request.after,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(OperationResult::Poll(Ok(crate::PollResult {
                        text,
                        satisfied,
                    })))
                }
                _ => Err(KernelError::UnsupportedVerb {
                    verb: "typed-session".to_owned(),
                    uri: "session://".to_owned(),
                }),
            }
        })
    }
}

fn validate_poll_condition(
    condition: &crate::PollCondition,
    targets: usize,
) -> Result<(), KernelError> {
    fn walk(condition: &crate::PollCondition, targets: usize) -> Result<(), KernelError> {
        match condition {
            crate::PollCondition::Atom(PollAtom::Changed(target))
            | crate::PollCondition::Atom(PollAtom::Terminated(target)) => {
                if (*target as usize) >= targets {
                    return Err(KernelError::InvalidRequest {
                        message: format!("poll target {target} is out of range"),
                    });
                }
            }
            crate::PollCondition::Atom(PollAtom::Regex(regex)) => {
                if (regex.target as usize) >= targets {
                    return Err(KernelError::InvalidRequest {
                        message: format!("poll target {} is out of range", regex.target),
                    });
                }
                regex::Regex::new(&regex.pattern).map_err(|error| KernelError::InvalidPattern {
                    message: error.to_string(),
                })?;
            }
            crate::PollCondition::Atom(PollAtom::Timeout(_)) => {}
            crate::PollCondition::All(children) | crate::PollCondition::Any(children) => {
                if children.is_empty() {
                    return Err(KernelError::InvalidRequest {
                        message: "poll boolean conditions cannot be empty".to_owned(),
                    });
                }
                for child in children {
                    walk(child, targets)?;
                }
            }
        }
        Ok(())
    }
    walk(condition, targets)
}

fn evaluate_condition(
    condition: &crate::PollCondition,
    snapshots: &[Value],
    elapsed: Duration,
) -> (bool, Vec<PollAtom>) {
    fn eval_atom(atom: &PollAtom, snapshots: &[Value], elapsed: Duration) -> (bool, Vec<PollAtom>) {
        let result = match atom {
            PollAtom::Changed(target) => snapshots
                .get(*target as usize)
                .and_then(|value| value["events"].as_array())
                .is_some_and(|events| !events.is_empty()),
            PollAtom::Regex(regex) => snapshots
                .get(regex.target as usize)
                .and_then(|value| value["events"].as_array())
                .is_some_and(|events| {
                    events.iter().any(|event| {
                        event["data"].as_str().is_some_and(|text| {
                            regex::Regex::new(&regex.pattern)
                                .is_ok_and(|regex| regex.is_match(text))
                        })
                    })
                }),
            PollAtom::Terminated(target) => snapshots
                .get(*target as usize)
                .and_then(|value| value["status"].as_str())
                .is_some_and(|status| status == "aborted"),
            PollAtom::Timeout(milliseconds) => elapsed >= Duration::from_millis(*milliseconds),
        };
        (
            result,
            result.then(|| vec![atom.clone()]).unwrap_or_default(),
        )
    }
    match condition {
        crate::PollCondition::Atom(atom) => eval_atom(atom, snapshots, elapsed),
        crate::PollCondition::All(children) => {
            let values = children
                .iter()
                .map(|child| evaluate_condition(child, snapshots, elapsed))
                .collect::<Vec<_>>();
            (
                values.iter().all(|(ok, _)| *ok),
                values.into_iter().flat_map(|(_, atoms)| atoms).collect(),
            )
        }
        crate::PollCondition::Any(children) => {
            let values = children
                .iter()
                .map(|child| evaluate_condition(child, snapshots, elapsed))
                .collect::<Vec<_>>();
            (
                values.iter().any(|(ok, _)| *ok),
                values
                    .into_iter()
                    .filter(|(ok, _)| *ok)
                    .flat_map(|(_, atoms)| atoms)
                    .collect(),
            )
        }
    }
}

fn anchored_session_window(
    uri: &crate::ResourceUri,
    value: &Value,
    before: Option<u32>,
    after: Option<u32>,
) -> Result<AnchoredText, KernelError> {
    let events = value["events"].as_array().cloned().unwrap_or_default();
    let lines = events
        .iter()
        .filter_map(|event| {
            Some(AnchoredLine {
                anchor: Anchor::from_tokens(vec![event["seq"].as_u64()?.to_string()]),
                text: event["data"].as_str()?.to_owned(),
                ending: crate::LineEnding::Lf,
            })
        })
        .collect::<Vec<_>>();
    let before = before.unwrap_or(u32::MAX) as usize;
    let after = after.unwrap_or(u32::MAX) as usize;
    let start = lines.len().saturating_sub(before.saturating_add(1));
    let end = (start + after + 1).min(lines.len());
    Ok(AnchoredText {
        uri: uri.clone(),
        lines: lines[start..end].to_vec(),
    })
}

const DEFAULT_READ_WINDOW: usize = 200;

fn session_read_window(
    uri: &crate::ResourceUri,
    value: &Value,
    at: Option<&crate::Position>,
    before: Option<u32>,
    after: Option<u32>,
) -> Result<AnchoredText, KernelError> {
    let events = value["events"].as_array().cloned().unwrap_or_default();
    let lines = events
        .iter()
        .filter_map(|event| {
            Some(AnchoredLine {
                anchor: Anchor::from_tokens(vec![event["seq"].as_u64()?.to_string()]),
                text: event["data"].as_str()?.to_owned(),
                ending: crate::LineEnding::Lf,
            })
        })
        .collect::<Vec<_>>();
    let at = at.unwrap_or(&crate::Position::Top);
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
    if matches!(at, crate::Position::Top) && before.is_some()
        || matches!(at, crate::Position::Bottom) && after.is_some()
    {
        return Err(KernelError::InvalidRequest {
            message: "read window is invalid for its position".to_owned(),
        });
    }
    let before = before.map_or(DEFAULT_READ_WINDOW, |value| value as usize);
    let after = after.map_or(DEFAULT_READ_WINDOW, |value| value as usize);
    let (start, end) = match at {
        crate::Position::Bottom => (index.saturating_sub(before), index),
        _ => (
            index.saturating_sub(before),
            index
                .saturating_add(after.saturating_add(1))
                .min(lines.len()),
        ),
    };
    Ok(AnchoredText {
        uri: uri.clone(),
        lines: lines[start..end].to_vec(),
    })
}

fn snapshot(state: &SessionState, since: u64) -> Value {
    json!({
        "status": state.status.as_str(),
        "next_seq": state.next_seq,
        "events": state.events.iter().filter(|event| event.seq >= since).map(|event| json!({
            "seq": event.seq,
            "kind": event.kind,
            "data": event.data,
        })).collect::<Vec<_>>(),
    })
}

async fn timeout_snapshot(session: &Session, since: u64) -> Value {
    let state = session.state.lock().await;
    let mut value = snapshot(&state, since);
    value["condition"] = Value::String("timeout".to_owned());
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kernel, ResourceUri};

    fn request(verb: Verb, target: &str, args: Value) -> Request {
        Request::new(verb, ResourceUri::parse(target).unwrap(), args)
    }

    async fn kernel() -> Kernel {
        let kernel = Kernel::new();
        kernel.register(SessionHandler::new()).await;
        kernel
    }

    #[tokio::test]
    async fn creates_sends_reads_and_polls_a_live_session() {
        let kernel = kernel().await;
        let target = "session://local/one";
        assert!(
            kernel
                .execute(request(Verb::Write, target, Value::Null))
                .await
                .ok
        );
        let poll = kernel.clone();
        let waiting = tokio::spawn(async move {
            poll.execute(request(
                Verb::Poll,
                target,
                json!({"since": 0, "lines": 1, "timeout_ms": 1000}),
            ))
            .await
        });
        tokio::task::yield_now().await;
        let sent = kernel
            .execute(request(Verb::Send, target, json!({"value": "hello"})))
            .await;
        assert!(sent.ok);
        let polled = waiting.await.unwrap();
        assert!(polled.ok);
        assert_eq!(polled.value.as_ref().unwrap()["condition"], "lines");
        assert_eq!(polled.value.as_ref().unwrap()["events"][0]["data"], "hello");

        let read = kernel
            .execute(request(Verb::Read, target, json!({"since": 1})))
            .await;
        assert!(read.ok);
        assert!(
            read.value.as_ref().unwrap()["events"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn typed_live_session_send_and_poll_return_contract_values() {
        let handler = SessionHandler::new();
        let kernel = Kernel::new();
        kernel.register(handler.clone()).await;
        kernel.register_typed(handler).await;
        let uri = ResourceUri::parse("session://local/typed").unwrap();
        let created = kernel
            .execute_operation(crate::Operation::Write(vec![crate::WriteRequest {
                uri: uri.clone(),
                content: String::new(),
            }]))
            .await
            .unwrap();
        assert!(matches!(created, crate::OperationResult::Write(ref values) if values[0].is_ok()));
        let send = kernel
            .execute_operation(crate::Operation::Send(vec![crate::SendRequest {
                uri: uri.clone(),
                content: "hello".to_owned(),
            }]))
            .await
            .unwrap();
        assert!(matches!(send, crate::OperationResult::Send(ref values) if values[0].is_ok()));
        let read = kernel
            .execute_operation(crate::Operation::Read(vec![crate::ReadRequest {
                uri: uri.clone(),
                at: Some(crate::Position::Top),
                before: None,
                after: Some(10),
            }]))
            .await
            .unwrap();
        assert!(
            matches!(read, crate::OperationResult::Read(ref values) if values[0].as_ref().is_ok_and(|value| matches!(value, crate::ReadResult::Text(text) if text.lines[0].text == "hello")))
        );
        let poll = kernel
            .execute_operation(crate::Operation::Poll(crate::PollRequest {
                targets: vec![crate::PollTarget {
                    uri: uri.clone(),
                    from_position: Some(crate::Position::Top),
                }],
                until: None,
                before: None,
                after: None,
            }))
            .await
            .unwrap();
        let crate::OperationResult::Poll(Ok(value)) = poll else {
            panic!("wrong typed poll result")
        };
        assert_eq!(value.text[0].lines[0].text, "hello");
    }

    #[test]
    fn poll_timeout_atoms_have_independent_deadlines_and_regex_semantics() {
        let condition = crate::PollCondition::All(vec![
            crate::PollCondition::Atom(PollAtom::Timeout(1_000)),
            crate::PollCondition::Atom(PollAtom::Timeout(5_000)),
        ]);
        assert!(!evaluate_condition(&condition, &[], Duration::from_millis(1_000)).0);
        assert!(evaluate_condition(&condition, &[], Duration::from_millis(5_000)).0);

        let regex = crate::PollCondition::Atom(PollAtom::Regex(crate::RegexAtom {
            target: 0,
            pattern: "^hello$".to_owned(),
        }));
        let snapshots = vec![json!({"events": [{"data": "say hello there"}]})];
        assert!(!evaluate_condition(&regex, &snapshots, Duration::ZERO).0);
        let snapshots = vec![json!({"events": [{"data": "hello"}]})];
        assert!(evaluate_condition(&regex, &snapshots, Duration::ZERO).0);
    }

    #[tokio::test]
    async fn typed_poll_bottom_is_invocation_cursor_and_anchor_is_exclusive() {
        let handler = SessionHandler::new();
        let kernel = Kernel::new();
        kernel.register_typed(handler.clone()).await;
        kernel.register(handler.clone()).await;
        let uri = ResourceUri::parse("session://local/cursors").unwrap();
        kernel
            .execute(request(Verb::Write, "session://local/cursors", Value::Null))
            .await;
        kernel
            .execute(request(
                Verb::Send,
                "session://local/cursors",
                json!({"value": "before"}),
            ))
            .await;

        let poll = kernel.clone();
        let waiting = tokio::spawn(async move {
            poll.execute_operation(crate::Operation::Poll(crate::PollRequest {
                targets: vec![crate::PollTarget {
                    uri: uri.clone(),
                    from_position: None,
                }],
                until: None,
                before: None,
                after: None,
            }))
            .await
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!waiting.is_finished());
        kernel
            .execute(request(
                Verb::Send,
                "session://local/cursors",
                json!({"value": "after"}),
            ))
            .await;
        let result = waiting.await.unwrap().unwrap();
        let crate::OperationResult::Poll(Ok(result)) = result else {
            panic!("wrong poll result")
        };
        assert_eq!(result.text[0].lines[0].text, "after");

        let first_anchor = Anchor::from_tokens(vec!["0".to_owned()]);
        let poll = kernel
            .execute_operation(crate::Operation::Poll(crate::PollRequest {
                targets: vec![crate::PollTarget {
                    uri: ResourceUri::parse("session://local/cursors").unwrap(),
                    from_position: Some(crate::Position::At(first_anchor)),
                }],
                until: Some(crate::PollCondition::Atom(PollAtom::Changed(0))),
                before: None,
                after: None,
            }))
            .await
            .unwrap();
        let crate::OperationResult::Poll(Ok(result)) = poll else {
            panic!("wrong anchored poll result")
        };
        assert!(
            result.text[0]
                .lines
                .iter()
                .all(|line| line.text != "before")
        );
    }

    #[tokio::test]
    async fn poll_times_out_and_abort_wakes_waiters() {
        let kernel = kernel().await;
        let target = "session://local/two";
        kernel
            .execute(request(Verb::Write, target, Value::Null))
            .await;
        let timeout_result = kernel
            .execute(request(Verb::Poll, target, json!({"timeout_ms": 5})))
            .await;
        assert_eq!(
            timeout_result.value.as_ref().unwrap()["condition"],
            "timeout"
        );

        let poll = kernel.clone();
        let waiting = tokio::spawn(async move {
            poll.execute(request(Verb::Poll, target, json!({"timeout_ms": 1000})))
                .await
        });
        tokio::task::yield_now().await;
        let aborted = kernel
            .execute(request(Verb::Abort, target, Value::Null))
            .await;
        assert!(aborted.ok);
        let polled = waiting.await.unwrap();
        assert_eq!(polled.value.as_ref().unwrap()["condition"], "terminal");
        assert_eq!(polled.value.as_ref().unwrap()["status"], "aborted");
        let send = kernel
            .execute(request(Verb::Send, target, json!({"value": "late"})))
            .await;
        assert!(matches!(send.error, Some(KernelError::InvalidState { .. })));
    }

    #[tokio::test]
    async fn delete_removes_the_session_record() {
        let kernel = kernel().await;
        let target = "session://local/three";
        kernel
            .execute(request(Verb::Write, target, Value::Null))
            .await;
        assert!(
            kernel
                .execute(request(Verb::Delete, target, Value::Null))
                .await
                .ok
        );
        let read = kernel
            .execute(request(Verb::Read, target, Value::Null))
            .await;
        assert!(matches!(read.error, Some(KernelError::NotFound { .. })));
    }
}
