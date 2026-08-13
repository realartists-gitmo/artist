//! Minimal live-resource handler.
//!
//! This is intentionally not an agent, shell, or process runner. It proves
//! that a synchronized, append-only live resource can use the same kernel
//! verbs as ordinary files.

use crate::{
    BoxFuture, Handler, HandlerDescriptor, KernelError, KernelHandle, Request, ResourceAddress,
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
