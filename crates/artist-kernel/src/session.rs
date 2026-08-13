//! Minimal live-resource handler.
//!
//! This is intentionally not an agent, shell, or process runner. It proves
//! that a synchronized, append-only live resource can use the same kernel
//! verbs as ordinary files.

use crate::{
    Anchor, AnchoredLine, AnchoredText, BoxFuture, Handler, HandlerDescriptor, KernelError,
    KernelHandle, Operation, OperationResult, PollAtom, Request, ResourceAddress, TypedHandler,
    Verb,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::time::{Duration, timeout};

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
            verbs: vec![Verb::Send, Verb::Poll, Verb::Abort, Verb::Delete],
        }
    }

    fn claims_operation(&self, operation: &Operation) -> bool {
        let uris: Vec<&crate::ResourceUri> = match operation {
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
    ) -> BoxFuture<'a, Result<OperationResult, KernelError>> {
        Box::pin(async move {
            match operation {
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
                    let target =
                        request
                            .targets
                            .first()
                            .ok_or_else(|| KernelError::InvalidRequest {
                                message: "poll requires at least one target".to_owned(),
                            })?;
                    let mut args = json!({
                        "since": 0,
                        "timeout_ms": 0,
                        "lines": 1,
                    });
                    if let Some(until) = request.until {
                        for node in until.nodes {
                            if let crate::PollNode::Atom(atom) = node {
                                match atom {
                                    PollAtom::Changed(lines) => args["lines"] = json!(lines),
                                    PollAtom::Regex(regex) => args["match"] = json!(regex.pattern),
                                    PollAtom::Timeout(ms) => args["timeout_ms"] = json!(ms),
                                    PollAtom::Terminated(_) => {}
                                }
                            }
                        }
                    }
                    let value = self
                        .poll(&ResourceAddress::uri(target.uri.clone()), &args)
                        .await?;
                    let text = value["events"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|event| {
                            Some(AnchoredLine {
                                anchor: Anchor::from_tokens(vec![
                                    event["seq"].as_u64()?.to_string(),
                                ]),
                                text: event["data"].as_str()?.to_owned(),
                                ending: crate::LineEnding::Lf,
                            })
                        })
                        .collect::<Vec<_>>();
                    Ok(OperationResult::Poll(Ok(crate::PollResult {
                        text: vec![AnchoredText {
                            uri: target.uri.clone(),
                            lines: text,
                        }],
                        satisfied: Vec::new(),
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
        assert!(
            kernel
                .execute(request(Verb::Write, "session://local/typed", Value::Null))
                .await
                .ok
        );
        let send = kernel
            .execute_operation(crate::Operation::Send(vec![crate::SendRequest {
                uri: uri.clone(),
                content: "hello".to_owned(),
            }]))
            .await
            .unwrap();
        assert!(matches!(send, crate::OperationResult::Send(ref values) if values[0].is_ok()));
        let poll = kernel
            .execute_operation(crate::Operation::Poll(crate::PollRequest {
                targets: vec![crate::PollTarget {
                    uri: uri.clone(),
                    from_position: None,
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
