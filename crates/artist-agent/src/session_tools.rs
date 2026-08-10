//! Universal model-facing lifecycle operations for durable Artist sessions.
//!
//! The universal tools know nothing about concrete session kinds. Kinds register
//! their poll boundary, active-observation behaviour, send validation, and list
//! projection in [`SessionHub`]. Local execution handles are similarly erased
//! behind [`OwnedSession`]. This is what permits a future `debug:<slug>` kind to
//! join the surface without modifying `poll`, `abort`, `send`, or `list`.

use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
    time::Duration,
};

use artist_registry::{
    Audience, CancelDisposition, Message, SessionRecord, SessionStatus, Sessions,
};
use artist_tools::resource_path::{ResourcePath, ResourceScheme};
use dashmap::DashMap;
use futures::future::BoxFuture;
use rig_core::tool::{PortableTool, ToolExecutionError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

const CADENCE: Duration = Duration::from_millis(100);
/// A stop request is cooperative first. This duration is deliberately fixed
/// and persisted against the resource's cancellation timestamp so it means
/// the same thing across owner restarts.
const STOP_GRACE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub(crate) struct OwnedState {
    pub snapshot: Value,
    pub stopped: Option<SessionStatus>,
}

impl OwnedState {
    pub fn live(snapshot: Value) -> Self {
        Self {
            snapshot,
            stopped: None,
        }
    }

    pub fn stopped(status: SessionStatus, snapshot: Value) -> Self {
        Self {
            snapshot,
            stopped: Some(status),
        }
    }
}

/// Process-local control for a session whose durable record is owned by this
/// process. The durable record is still authoritative; this trait is only how
/// the owner performs work on its behalf.
pub(crate) trait OwnedSession: Send + Sync + 'static {
    fn state(&self) -> BoxFuture<'_, Result<OwnedState, String>>;

    /// One kind-authored observation requested by universal `poll`. Most kinds
    /// simply use their passive state. Canvas overrides this to invoke onPoll.
    fn observe(&self) -> BoxFuture<'_, Result<OwnedState, String>> {
        self.state()
    }

    fn send(&self, input: Value) -> BoxFuture<'_, Result<(), String>>;
    fn abort(&self) -> BoxFuture<'_, Result<(), String>>;
}

type Boundary = dyn Fn(&SessionRecord, Option<u64>) -> bool + Send + Sync;
type SendValidator = dyn Fn(&SessionRecord, &Value) -> Result<(), String> + Send + Sync;
type Summary = dyn Fn(&SessionRecord) -> Value + Send + Sync;

#[derive(Clone)]
pub(crate) struct KindRegistration {
    pub name: String,
    pub active_poll: bool,
    boundary: Arc<Boundary>,
    validate_send: Arc<SendValidator>,
    summary: Arc<Summary>,
}

impl KindRegistration {
    pub fn new(
        name: impl Into<String>,
        active_poll: bool,
        boundary: impl Fn(&SessionRecord, Option<u64>) -> bool + Send + Sync + 'static,
        validate_send: impl Fn(&SessionRecord, &Value) -> Result<(), String> + Send + Sync + 'static,
        summary: impl Fn(&SessionRecord) -> Value + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            active_poll,
            boundary: Arc::new(boundary),
            validate_send: Arc::new(validate_send),
            summary: Arc::new(summary),
        }
    }

    pub fn one_shot(name: &str, send: SendPolicy) -> Self {
        let kind = name.to_owned();
        Self::new(
            name,
            false,
            |record, _| !record.lifecycle.is_live(),
            move |_record, input| send.validate(input),
            move |record| json!({"kind": kind, "snapshot": record.snapshot}),
        )
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SendPolicy {
    String,
}

impl SendPolicy {
    fn validate(self, input: &Value) -> Result<(), String> {
        match self {
            Self::String if !input.is_string() => {
                Err("this session kind requires string input".into())
            }
            Self::String => Ok(()),
        }
    }
}

#[derive(Clone)]
pub(crate) struct SessionHub {
    sessions: Sessions,
    artist: Arc<str>,
    seat: Option<crate::delegate::PermitSlot>,
    kinds: Arc<RwLock<BTreeMap<String, KindRegistration>>>,
    local: Arc<DashMap<String, Arc<dyn OwnedSession>>>,
}

impl SessionHub {
    pub fn new(
        project: &std::path::Path,
        artist: impl Into<String>,
        seat: Option<crate::delegate::PermitSlot>,
    ) -> Self {
        Self {
            sessions: artist_registry::Registry::for_project(project).sessions(),
            artist: Arc::from(artist.into()),
            seat,
            kinds: Arc::new(RwLock::new(BTreeMap::new())),
            local: Arc::new(DashMap::new()),
        }
    }

    pub fn registry(&self) -> &Sessions {
        &self.sessions
    }

    pub fn standard(
        project: &std::path::Path,
        artist: impl Into<String>,
        seat: Option<crate::delegate::PermitSlot>,
    ) -> Self {
        let hub = Self::new(project, artist, seat);
        hub.register_standard_kinds();
        hub
    }

    pub fn artist(&self) -> &str {
        &self.artist
    }

    pub fn register_kind(&self, registration: KindRegistration) {
        self.kinds
            .write()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(registration.name.clone(), registration);
    }

    pub fn register_standard_kinds(&self) {
        self.register_kind(KindRegistration::one_shot("subagent", SendPolicy::String));
        self.register_kind(KindRegistration::new(
            "bash",
            false,
            |record, _| {
                !record.lifecycle.is_live()
                    || record
                        .snapshot
                        .get("readiness")
                        .and_then(Value::as_str)
                        .is_some_and(|state| state == "ready")
            },
            |record, input| {
                if record
                    .snapshot
                    .get("interactive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    SendPolicy::String.validate(input)
                } else {
                    Err("send is unsupported for non-interactive bash".into())
                }
            },
            |record| json!({"kind":"bash", "snapshot": record.snapshot}),
        ));
        self.register_kind(KindRegistration::new(
            "ask",
            false,
            |record, _| !record.lifecycle.is_live(),
            |_record, _| Err("model-to-ask input is unsupported".into()),
            |record| json!({"kind":"ask", "snapshot": crate::ask_tool::model_snapshot(&record.snapshot)}),
        ));
        self.register_kind(KindRegistration::new(
            "canvas",
            true,
            |record, requested| {
                !record.lifecycle.is_live()
                    || requested.is_some_and(|sequence| record.poll_completed >= sequence)
            },
            |record, _input| {
                if record
                    .snapshot
                    .get("sendSupported")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    Ok(())
                } else {
                    Err("canvas has no artist.onSend handler registered".into())
                }
            },
            |record| json!({"kind":"canvas", "snapshot": record.snapshot}),
        ));
        self.register_kind(KindRegistration::new(
            "computer",
            false,
            |_record, _| true,
            |_record, _| Err("generic send is unsupported for computer sessions; use the computer tool".into()),
            |record| {
                json!({
                    "kind":"computer",
                    "snapshot": {
                        "control": record.snapshot.get("control").cloned().unwrap_or_else(|| Value::String("live".into())),
                        "rung": record.snapshot.get("rung").cloned().unwrap_or(Value::Null)
                    }
                })
            },
        ));
    }

    pub fn own(&self, id: impl Into<String>, session: Arc<dyn OwnedSession>) {
        let id = id.into();
        self.local.insert(id.clone(), Arc::clone(&session));
        let hub = self.clone();
        tokio::spawn(async move {
            hub.drive_owned(id, session).await;
        });
    }

    async fn drive_owned(&self, id: String, session: Arc<dyn OwnedSession>) {
        loop {
            let record = match self.sessions.get(&id) {
                Ok(Some(record)) => record,
                _ => break,
            };
            if !record.lifecycle.is_live() {
                break;
            }
            if record.cancel_requested
                && record.cancel_requested_at.is_some_and(|requested_at| {
                    artist_registry::now().saturating_sub(requested_at)
                        >= STOP_GRACE.as_millis() as u64
                })
            {
                match session.abort().await {
                    Ok(()) => {
                        let state = session.state().await.ok();
                        let snapshot = state.map(|state| state.snapshot);
                        let _ = self
                            .sessions
                            .finish(&id, SessionStatus::Cancelled, snapshot);
                        break;
                    }
                    Err(error) => {
                        let mut snapshot = record.snapshot.clone();
                        if let Some(object) = snapshot.as_object_mut() {
                            object.insert("cancelError".into(), Value::String(error));
                        }
                        let _ = self.sessions.set_snapshot(&id, snapshot);
                    }
                }
            }

            // Once a graceful stop is requested, no new work is admitted; the
            // owned runtime may still settle naturally during the grace period.
            if !record.cancel_requested {
                match self.sessions.drain_inputs(&id) {
                    Ok(inputs) => {
                        for input in inputs {
                            if let Err(error) = session.send(input.input).await {
                                let mut snapshot = self
                                    .sessions
                                    .get(&id)
                                    .ok()
                                    .flatten()
                                    .map(|record| record.snapshot)
                                    .unwrap_or(Value::Null);
                                if let Some(object) = snapshot.as_object_mut() {
                                    object.insert("sendError".into(), Value::String(error));
                                }
                                let _ = self.sessions.set_snapshot(&id, snapshot);
                            }
                        }
                    }
                    Err(_) => break,
                }
            }

            let current = match self.sessions.get(&id) {
                Ok(Some(record)) => record,
                _ => break,
            };
            if current.poll_requested > current.poll_completed {
                let sequence = current.poll_requested;
                match session.observe().await {
                    Ok(state) => {
                        let _ = self
                            .sessions
                            .complete_poll(&id, sequence, state.snapshot.clone());
                        if let Some(status) = state.stopped {
                            let _ = self.sessions.finish(&id, status, Some(state.snapshot));
                            break;
                        }
                    }
                    Err(error) => {
                        let snapshot = json!({
                            "harnessFailure": {
                                "kind": "poll",
                                "message": error
                            }
                        });
                        let _ = self.sessions.complete_poll(&id, sequence, snapshot);
                    }
                }
            }

            match session.state().await {
                Ok(state) => {
                    if let Some(status) = state.stopped {
                        let _ = self.sessions.finish(&id, status, Some(state.snapshot));
                        break;
                    }
                    let _ = self.sessions.set_snapshot(&id, state.snapshot);
                }
                Err(error) => {
                    let _ = self.sessions.finish(
                        &id,
                        SessionStatus::Failed,
                        Some(json!({"harnessFailure":{"kind":"owner","message":error}})),
                    );
                    break;
                }
            }

            // A SIGUSR1 nudge sets the process-wide flag; the cadence remains the
            // correctness path if signals coalesce or are lost.
            if !artist_registry::take_wake() {
                tokio::time::sleep(CADENCE).await;
            }
        }
        self.local.remove(&id);
    }

    fn kind(&self, kind: &str) -> Result<KindRegistration, SessionError> {
        if let Some(registered) = self
            .kinds
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(kind)
            .cloned()
        {
            return Ok(registered);
        }
        let summary_kind = kind.to_owned();
        let send_kind = kind.to_owned();
        Ok(KindRegistration::new(
            kind,
            false,
            |record, _| !record.lifecycle.is_live(),
            move |_record, _input| {
                Err(format!(
                    "generic send is unsupported for session kind `{send_kind}`"
                ))
            },
            move |record| json!({"kind": summary_kind, "snapshot": record.snapshot}),
        ))
    }

    fn target_ids(
        session: Option<String>,
        sessions: Option<Vec<String>>,
    ) -> Result<Vec<String>, SessionError> {
        match (session, sessions) {
            (Some(session), None) if !session.is_empty() => {
                Ok(vec![canonical_session_id(&session)?])
            }
            (None, Some(sessions)) if !sessions.is_empty() => sessions
                .iter()
                .map(|session| canonical_session_id(session))
                .collect(),
            (Some(_), Some(_)) => Err(SessionError(
                "exactly one of `session` or `sessions` is allowed".into(),
            )),
            _ => Err(SessionError(
                "one `session` or a non-empty `sessions` list is required".into(),
            )),
        }
    }

    async fn poll(&self, args: PollArgs) -> Result<String, SessionError> {
        let ids = Self::target_ids(args.session, args.sessions)?;
        let matcher = args
            .match_pattern
            .as_deref()
            .map(regex::Regex::new)
            .transpose()
            .map_err(|error| SessionError(format!("invalid poll match regex: {error}")))?;
        let immediate = args.timeout_ms == Some(0);
        let mut active_sequences = BTreeMap::new();
        if !immediate {
            for id in &ids {
                let record = self.record(id)?;
                let kind = self.kind(&record.kind)?;
                if kind.active_poll && record.lifecycle.is_live() {
                    active_sequences.insert(
                        id.clone(),
                        self.sessions.request_poll(id).map_err(registry_error)?,
                    );
                    if record.owner != artist_registry::Owner::current() {
                        let _ = record.owner.wake();
                    }
                }
            }
        }

        let deadline = args
            .timeout_ms
            .filter(|timeout| *timeout > 0)
            .map(|timeout| tokio::time::Instant::now() + Duration::from_millis(timeout));
        let mut yielded = false;
        let result = loop {
            let records = ids
                .iter()
                .map(|id| self.record(id))
                .collect::<Result<Vec<_>, _>>()?;
            if immediate
                || records.iter().all(|record| {
                    if !record.lifecycle.is_live() {
                        return true;
                    }
                    if let Some(matcher) = &matcher {
                        return poll_text(record).is_some_and(|text| matcher.is_match(&text));
                    }
                    self.kind(&record.kind).is_ok_and(|kind| {
                        (kind.boundary)(record, active_sequences.get(&record.id).copied())
                    })
                })
                || deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
            {
                break self.render_poll(&records, matcher.as_ref())?;
            }
            if !yielded {
                if let Some(seat) = &self.seat {
                    seat.yield_seat().await;
                }
                yielded = true;
            }
            tokio::time::sleep(CADENCE).await;
        };
        if yielded {
            if let Some(seat) = &self.seat {
                seat.retake().await;
            }
        }
        Ok(result)
    }

    fn render_poll(
        &self,
        records: &[SessionRecord],
        matcher: Option<&regex::Regex>,
    ) -> Result<String, SessionError> {
        let values = records
            .iter()
            .map(|record| {
                let kind = self.kind(&record.kind)?;
                let mut value = json!({
                    "session": record.id,
                    "kind": record.kind,
                    "lifecycle": record.lifecycle,
                    "cancelRequested": record.cancel_requested,
                    "snapshot": (kind.summary)(record)
                        .get("snapshot")
                        .cloned()
                        .unwrap_or_else(|| record.snapshot.clone())
                });
                if let Some(matcher) = matcher
                    && let Some(continuation) = transcript_continuation(record, matcher)
                {
                    value["continuation"] = continuation;
                }
                Ok(value)
            })
            .collect::<Result<Vec<Value>, SessionError>>()?;
        let rendered = if values.len() == 1 {
            values.into_iter().next().unwrap_or(Value::Null)
        } else {
            Value::Array(values)
        };
        serde_json::to_string_pretty(&rendered).map_err(|error| SessionError(error.to_string()))
    }

    async fn abort(&self, args: AbortArgs) -> Result<String, SessionError> {
        let ids = Self::target_ids(args.session, args.sessions)?;
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            let result = match self.sessions.request_cancel(&id) {
                Ok((CancelDisposition::LocalOwner, record)) => {
                    if let Some(local) = self.local.get(&id).map(|entry| Arc::clone(entry.value()))
                    {
                        match local.abort().await {
                            Ok(()) => {
                                match self.sessions.finish(&id, SessionStatus::Cancelled, None) {
                                    Ok(record) => json!({"session":id,"state":record.lifecycle}),
                                    Err(error) => json!({"session":id,"error":error.to_string()}),
                                }
                            }
                            Err(error) => {
                                json!({"session":id,"cancelRequested":true,"error":error})
                            }
                        }
                    } else {
                        json!({"session":id,"cancelRequested":true,"state":record.lifecycle})
                    }
                }
                Ok((CancelDisposition::ForeignOwner(owner), record)) => match owner.wake() {
                    Ok(()) => json!({"session":id,"cancelRequested":true,"state":record.lifecycle}),
                    Err(_) => match self.sessions.get(&id) {
                        Ok(Some(record)) => {
                            json!({"session":id,"cancelRequested":record.cancel_requested,"state":record.lifecycle})
                        }
                        _ => {
                            json!({"session":id,"error":"owner disappeared while cancellation was requested"})
                        }
                    },
                },
                Ok((CancelDisposition::AlreadyStopped, record)) => {
                    json!({"session":id,"state":record.lifecycle})
                }
                Ok((CancelDisposition::Abandoned, record)) => {
                    json!({"session":id,"state":record.lifecycle})
                }
                Err(error) => json!({"session":id,"error":error.to_string()}),
            };
            results.push(result);
        }
        let rendered = if results.len() == 1 {
            results.into_iter().next().unwrap_or(Value::Null)
        } else {
            Value::Array(results)
        };
        serde_json::to_string_pretty(&rendered).map_err(|error| SessionError(error.to_string()))
    }

    async fn stop(&self, args: AbortArgs) -> Result<String, SessionError> {
        let ids = Self::target_ids(args.session, args.sessions)?;
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            let result = match self.sessions.request_cancel(&id) {
                Ok((CancelDisposition::LocalOwner, record)) => json!({
                    "session": id,
                    "cancelRequested": true,
                    "state": record.lifecycle,
                    "forceAfterMs": STOP_GRACE.as_millis(),
                }),
                Ok((CancelDisposition::ForeignOwner(owner), record)) => {
                    let _ = owner.wake();
                    json!({
                        "session": id,
                        "cancelRequested": true,
                        "state": record.lifecycle,
                        "forceAfterMs": STOP_GRACE.as_millis(),
                    })
                }
                Ok((CancelDisposition::AlreadyStopped | CancelDisposition::Abandoned, record)) => {
                    json!({"session": id, "state": record.lifecycle})
                }
                Err(error) => json!({"session": id, "error": error.to_string()}),
            };
            results.push(result);
        }
        let rendered = if results.len() == 1 {
            results.into_iter().next().unwrap_or(Value::Null)
        } else {
            Value::Array(results)
        };
        serde_json::to_string_pretty(&rendered).map_err(|error| SessionError(error.to_string()))
    }

    fn delete(&self, args: DeleteArgs) -> Result<String, SessionError> {
        let ids = Self::target_ids(args.session, args.sessions)?;
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            match self.sessions.delete_stopped(&id) {
                Ok(record) => {
                    self.local.remove(&id);
                    results.push(json!({"session": id, "deleted": true, "kind": record.kind}));
                }
                Err(error) => results.push(json!({
                    "session": id,
                    "deleted": false,
                    "error": error.to_string(),
                    "recovery": "stop the session, poll until it settles, then retry delete"
                })),
            }
        }
        let rendered = if results.len() == 1 {
            results.into_iter().next().unwrap_or(Value::Null)
        } else {
            Value::Array(results)
        };
        serde_json::to_string_pretty(&rendered).map_err(|error| SessionError(error.to_string()))
    }

    async fn send(&self, args: SendArgs) -> Result<String, SessionError> {
        let ids = Self::target_ids(args.session, args.sessions)?;
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            let result = match self.sessions.get(&id).map_err(registry_error)? {
                Some(record) => {
                    if !record.lifecycle.is_live() {
                        json!({"session":id,"error":"session is stopped"})
                    } else if record.cancel_requested {
                        json!({"session":id,"error":"session is stopping; no new input is accepted"})
                    } else {
                        let kind = self.kind(&record.kind)?;
                        if let Err(error) = (kind.validate_send)(&record, &args.input) {
                            json!({"session":id,"error":error})
                        } else {
                            self.sessions
                                .enqueue_input(&id, args.input.clone())
                                .map_err(registry_error)?;
                            if record.owner != artist_registry::Owner::current() {
                                let _ = record.owner.wake();
                            }
                            json!({"session":id,"delivered":true})
                        }
                    }
                }
                None => {
                    let target = artist_registry::names()
                        .resolve(&id)
                        .map_err(registry_error)?;
                    if target.is_none() {
                        json!({"session":id,"error":"unknown session or artist identity"})
                    } else {
                        let body = args
                            .input
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| args.input.to_string());
                        artist_registry::messages()
                            .send(&Message {
                                id: artist_tools::short_id("m"),
                                from: self.artist.to_string(),
                                to: id.clone(),
                                audience: Audience::Direct,
                                body,
                                expects_reply: false,
                                sent_at: artist_registry::now(),
                            })
                            .map_err(registry_error)?;
                        json!({"session":id,"delivered":true})
                    }
                }
            };
            results.push(result);
        }
        let rendered = if results.len() == 1 {
            results.into_iter().next().unwrap_or(Value::Null)
        } else {
            Value::Array(results)
        };
        serde_json::to_string_pretty(&rendered).map_err(|error| SessionError(error.to_string()))
    }

    fn list(&self, args: ListArgs) -> Result<String, SessionError> {
        let scope = args.scope.as_deref().unwrap_or("mine");
        if !matches!(scope, "mine" | "project") {
            return Err(SessionError("scope must be `mine` or `project`".into()));
        }
        let kind_filter = args.kind.as_deref().filter(|kind| *kind != "all");
        let values = self
            .sessions
            .list()
            .map_err(registry_error)?
            .into_iter()
            .filter(|record| record.lifecycle.is_live())
            .filter(|record| {
                scope == "project"
                    || record.artist == &*self.artist
                    || record.parent_artist.as_deref() == Some(&*self.artist)
            })
            .filter(|record| kind_filter.is_none_or(|kind| record.kind == kind))
            .map(|record| {
                let summary = self
                    .kind(&record.kind)
                    .map(|kind| (kind.summary)(&record))
                    .unwrap_or_else(|_| record.snapshot.clone());
                json!({
                    "session": record.id,
                    "kind": record.kind,
                    "owner": record.artist,
                    "createdAt": record.created_at,
                    "state": record.lifecycle,
                    "summary": summary
                })
            })
            .collect::<Vec<_>>();
        serde_json::to_string_pretty(&values).map_err(|error| SessionError(error.to_string()))
    }

    fn record(&self, id: &str) -> Result<SessionRecord, SessionError> {
        self.sessions
            .get(id)
            .map_err(registry_error)?
            .ok_or_else(|| SessionError(format!("unknown session `{id}`")))
    }
}

/// Lifecycle verbs accept the canonical path returned by `read` as well as a
/// legacy durable id. A child projection (for example `agent://goethe/todo`)
/// is deliberately not a lifecycle resource and cannot be mistaken for one.
fn canonical_session_id(target: &str) -> Result<String, SessionError> {
    match ResourcePath::parse(target).map_err(|error| SessionError(error.to_string()))? {
        ResourcePath::Real(_) => Ok(target.to_owned()),
        ResourcePath::Virtual { scheme, segments }
            if matches!(
                scheme,
                ResourceScheme::Agent
                    | ResourceScheme::Bash
                    | ResourceScheme::Ask
                    | ResourceScheme::Canvas
                    | ResourceScheme::Computer
            ) && segments.len() == 1 =>
        {
            Ok(segments.into_iter().next().expect("one segment"))
        }
        ResourcePath::Virtual { scheme, .. } => Err(SessionError(format!(
            "{target} is not a runnable session resource; use a direct {scheme}://<id> path or the resource's owning tool"
        ))),
    }
}

/// Canonical model text examined by a match-aware poll. Bash uses its semantic
/// transcript; every other current resource uses its canonical JSON snapshot.
fn poll_text(record: &SessionRecord) -> Option<String> {
    if record.kind == "bash" {
        return record
            .snapshot
            .get("screen")
            .and_then(Value::as_str)
            .or_else(|| record.snapshot.get("output").and_then(Value::as_str))
            .map(str::to_owned);
    }
    serde_json::to_string(&record.snapshot).ok()
}

/// A selector is emitted only when a terminal match has an anchored text view.
/// It names the final matching line; callers resume strictly after that anchor.
fn transcript_continuation(record: &SessionRecord, matcher: &regex::Regex) -> Option<Value> {
    if record.kind != "bash" {
        return None;
    }
    // `poll(bash://...)` evaluates the emulator screen. Do not manufacture a
    // transcript cursor merely because stale scrollback happens to match when
    // the visible terminal does not.
    if record
        .snapshot
        .get("screen")
        .and_then(Value::as_str)
        .is_some_and(|screen| !matcher.is_match(screen))
    {
        return None;
    }
    let transcript = record.snapshot.get("output")?.as_str()?;
    let line_index = transcript.lines().position(|line| matcher.is_match(line))?;
    let anchored = artist_tools::resource_path::render_anchored_virtual_text(transcript);
    let anchor = anchored.lines().nth(line_index)?.split_once(": ")?.0;
    Some(json!({
        "path": format!("bash://{}", record.id),
        "after": anchor,
        "revision": artist_tools::resource_path::virtual_text_revision(transcript),
    }))
}

fn registry_error(error: artist_registry::Error) -> SessionError {
    SessionError(error.to_string())
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct SessionError(String);

impl From<SessionError> for ToolExecutionError {
    fn from(value: SessionError) -> Self {
        ToolExecutionError::other(value.to_string()).with_code("session_error")
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PollArgs {
    session: Option<String>,
    sessions: Option<Vec<String>>,
    #[serde(rename = "match")]
    match_pattern: Option<String>,
    timeout_ms: Option<u64>,
}

#[derive(Clone)]
pub(crate) struct PollTool(pub SessionHub);

impl PortableTool for PollTool {
    const NAME: &'static str = "poll";
    type Error = SessionError;
    type Args = PollArgs;
    type Output = String;

    fn description(&self) -> String {
        "Return an idempotent bounded snapshot for one session or an ordered list. `match` is a regex evaluated atomically against current and future canonical output; on a terminal match the continuation begins strictly after its TECA anchor. Omit timeoutMs to wait indefinitely; 0 returns immediately.".into()
    }

    fn parameters(&self) -> Value {
        target_schema(Some(json!({
            "match":{"type":"string","description":"Optional regex evaluated against the resource's canonical output."},
            "timeoutMs":{"type":"integer","minimum":0}
        })))
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.0.poll(args).await
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AbortArgs {
    session: Option<String>,
    sessions: Option<Vec<String>>,
}

#[derive(Clone)]
pub(crate) struct AbortTool(pub SessionHub);

impl PortableTool for AbortTool {
    const NAME: &'static str = "abort";
    type Error = SessionError;
    type Args = AbortArgs;
    type Output = String;

    fn description(&self) -> String {
        "Request that one or more sessions stop. Foreign cancellation is durable and asynchronous; use poll to observe settlement.".into()
    }

    fn parameters(&self) -> Value {
        target_schema(None)
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.0.abort(args).await
    }
}

/// The canonical lifecycle verb. `AbortTool` remains an internal compatibility
/// adapter while profiles and callers migrate from the old spelling.
#[derive(Clone)]
pub(crate) struct StopTool(pub SessionHub);

impl PortableTool for StopTool {
    const NAME: &'static str = "stop";
    type Error = SessionError;
    type Args = AbortArgs;
    type Output = String;

    fn description(&self) -> String {
        "Request graceful cancellation for one or more sessions. Foreign cancellation is durable and asynchronous; use poll to observe settlement before delete.".into()
    }

    fn parameters(&self) -> Value {
        target_schema(None)
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.0.stop(args).await
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteArgs {
    session: Option<String>,
    sessions: Option<Vec<String>>,
}

#[derive(Clone)]
pub(crate) struct DeleteTool(pub SessionHub);

impl PortableTool for DeleteTool {
    const NAME: &'static str = "delete";
    type Error = SessionError;
    type Args = DeleteArgs;
    type Output = String;

    fn description(&self) -> String {
        "Permanently delete one or more stopped session resources, their queued input, and any retained artist-name reservation. Live sessions are never deleted; stop and poll them first.".into()
    }

    fn parameters(&self) -> Value {
        target_schema(None)
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.0.delete(args)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SendArgs {
    session: Option<String>,
    sessions: Option<Vec<String>>,
    input: Value,
}

#[derive(Clone)]
pub(crate) struct SendTool {
    hub: SessionHub,
    computer: Option<crate::computer_tool::ComputerTool>,
}

impl SendTool {
    pub(crate) fn new(
        hub: SessionHub,
        computer: Option<crate::computer_tool::ComputerTool>,
    ) -> Self {
        Self { hub, computer }
    }
}

impl PortableTool for SendTool {
    const NAME: &'static str = "send";
    type Error = SessionError;
    type Args = SendArgs;
    type Output = String;

    fn description(&self) -> String {
        "Durably deliver the same input to one or more live sessions or retained artist identities. For one live computer session, `{intent, args?}` selects and invokes a permitted `computer://use/...` capability; otherwise sending is fire-and-forget and effects are observed with poll.".into()
    }

    fn parameters(&self) -> Value {
        target_schema(Some(json!({"input":{}})))
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if let (Some(computer), Some(session)) = (&self.computer, args.session.as_deref())
            && args.sessions.is_none()
            && args.input.get("intent").is_some()
        {
            let record = self.hub.record(session)?;
            if record.kind == "computer" {
                return computer
                    .dispatch_intent(session, args.input)
                    .await
                    .map(|output| output.render())
                    .map_err(|error| SessionError(error.to_string()));
            }
        }
        self.hub.send(args).await
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListArgs {
    scope: Option<String>,
    kind: Option<String>,
}

#[derive(Clone)]
pub(crate) struct ListTool(pub SessionHub);

impl PortableTool for ListTool {
    const NAME: &'static str = "list";
    type Error = SessionError;
    type Args = ListArgs;
    type Output = String;

    fn description(&self) -> String {
        "List live/current sessions. Defaults to sessions owned by this artist; scope=project includes the shared project registry. kind is an open session-kind string.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "scope":{"enum":["mine","project"],"default":"mine"},
                "kind":{"type":"string","default":"all"}
            },
            "additionalProperties":false
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.0.list(args)
    }
}

fn target_schema(extra: Option<Value>) -> Value {
    let mut properties = serde_json::Map::from_iter([
        ("session".into(), json!({"type":"string"})),
        (
            "sessions".into(),
            json!({"type":"array","items":{"type":"string"},"minItems":1}),
        ),
    ]);
    if let Some(Value::Object(extra)) = extra {
        properties.extend(extra);
    }
    json!({
        "type":"object",
        "properties":properties,
        "oneOf":[
            {"required":["session"],"not":{"required":["sessions"]}},
            {"required":["sessions"],"not":{"required":["session"]}}
        ],
        "additionalProperties":false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct StubbornSession {
        aborted: Arc<AtomicBool>,
    }

    impl OwnedSession for StubbornSession {
        fn state(&self) -> BoxFuture<'_, Result<OwnedState, String>> {
            Box::pin(async { Ok(OwnedState::live(json!({"state":"running"}))) })
        }

        fn send(&self, _: Value) -> BoxFuture<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }

        fn abort(&self) -> BoxFuture<'_, Result<(), String>> {
            let aborted = Arc::clone(&self.aborted);
            Box::pin(async move {
                aborted.store(true, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    #[test]
    fn target_schema_requires_exactly_one_target_shape() {
        let schema = PollTool(SessionHub::new(
            std::path::Path::new("/tmp"),
            "Goethe",
            None,
        ))
        .parameters();
        assert_eq!(schema["oneOf"].as_array().unwrap().len(), 2);
        assert!(schema["properties"].get("maxBytes").is_none());
    }

    #[tokio::test]
    async fn send_routes_computer_intents_to_the_capability_router() {
        let root = tempfile::tempdir().unwrap();
        let hub = SessionHub::standard(root.path(), "Goethe", None);
        hub.registry()
            .create_exact(
                "computer:surface",
                "computer",
                "Goethe",
                None,
                json!({"surface":"missing"}),
            )
            .unwrap();
        let router = crate::computer_tool::ComputerTool::new(
            artist_computer::SurfaceRegistry::new(),
            artist_session::Recorder::noop(),
            None,
            hub.clone(),
            root.path(),
        );
        let send = SendTool::new(hub, Some(router));
        let error = send
            .call(SendArgs {
                session: Some("computer:surface".into()),
                sessions: None,
                input: json!({"intent":"not-a-capability"}),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("no permitted computer capability matches"));
        assert!(error.contains("computer://use/observe"));
    }

    #[test]
    fn send_schema_accepts_exactly_one_singular_or_plural_target() {
        let schema = SendTool::new(
            SessionHub::new(std::path::Path::new("/tmp"), "Goethe", None),
            None,
        )
        .parameters();
        assert_eq!(schema["oneOf"].as_array().unwrap().len(), 2);
        assert!(schema["properties"].get("session").is_some());
        assert!(schema["properties"].get("sessions").is_some());
        assert!(schema["properties"].get("input").is_some());
        assert!(schema["properties"].get("group").is_none());
    }

    #[tokio::test]
    async fn unknown_future_kinds_remain_pollable_without_an_enum_arm() {
        let root = tempfile::tempdir().unwrap();
        let hub = SessionHub::new(root.path(), "Goethe", None);
        let record = hub
            .registry()
            .create_exact(
                "debugger:future",
                "debugger",
                "Goethe",
                None,
                json!({"phase":"attached"}),
            )
            .unwrap();
        let snapshot = hub
            .poll(PollArgs {
                session: Some(record.id),
                sessions: None,
                match_pattern: None,
                timeout_ms: Some(0),
            })
            .await
            .unwrap();
        assert!(snapshot.contains("debugger"));
        assert!(snapshot.contains("attached"));
    }

    #[tokio::test]
    async fn stopped_records_are_pollable_but_not_listed() {
        let root = tempfile::tempdir().unwrap();
        let hub = SessionHub::new(root.path(), "Goethe", None);
        hub.register_standard_kinds();
        let record = hub
            .registry()
            .create_exact("Goethe", "subagent", "Goethe", None, Value::Null)
            .unwrap();
        hub.registry()
            .finish(
                &record.id,
                SessionStatus::Completed,
                Some(json!({"output":"done"})),
            )
            .unwrap();
        let listed = hub
            .list(ListArgs {
                scope: None,
                kind: None,
            })
            .unwrap();
        assert_eq!(listed, "[]");
        let polled = hub
            .poll(PollArgs {
                session: Some("Goethe".into()),
                sessions: None,
                match_pattern: None,
                timeout_ms: Some(0),
            })
            .await
            .unwrap();
        assert!(polled.contains("completed"));
    }

    #[tokio::test]
    async fn poll_match_observes_current_terminal_output_and_returns_a_cursor() {
        let root = tempfile::tempdir().unwrap();
        let hub = SessionHub::standard(root.path(), "Goethe", None);
        let record = hub
            .registry()
            .create_exact(
                "bash:log",
                "bash",
                "Goethe",
                None,
                json!({"output":"before\\nneedle\\nafter"}),
            )
            .unwrap();
        hub.registry()
            .finish(&record.id, SessionStatus::Completed, None)
            .unwrap();
        let polled = hub
            .poll(PollArgs {
                session: Some(record.id),
                sessions: None,
                match_pattern: Some("needle".into()),
                timeout_ms: None,
            })
            .await
            .unwrap();
        assert!(polled.contains("bash://bash:log"), "{polled}");
        assert!(polled.contains("\"after\": \"#"), "{polled}");
    }

    #[tokio::test]
    async fn bash_poll_uses_visible_screen_not_hidden_transcript() {
        let root = tempfile::tempdir().unwrap();
        let hub = SessionHub::standard(root.path(), "Goethe", None);
        let record = hub
            .registry()
            .create_exact(
                "bash:screen",
                "bash",
                "Goethe",
                None,
                json!({"output":"needle", "screen":"visible"}),
            )
            .unwrap();
        hub.registry()
            .finish(&record.id, SessionStatus::Completed, None)
            .unwrap();
        let polled = hub
            .poll(PollArgs {
                session: Some(record.id),
                sessions: None,
                match_pattern: Some("needle".into()),
                timeout_ms: Some(0),
            })
            .await
            .unwrap();
        assert!(!polled.contains("\"continuation\""), "{polled}");
    }

    #[tokio::test]
    async fn lifecycle_tools_accept_canonical_session_paths() {
        let root = tempfile::tempdir().unwrap();
        let hub = SessionHub::standard(root.path(), "Goethe", None);
        let record = hub
            .registry()
            .create_exact("bash:log", "bash", "Goethe", None, json!({"output":"done"}))
            .unwrap();
        hub.registry()
            .finish(&record.id, SessionStatus::Completed, None)
            .unwrap();
        let polled = hub
            .poll(PollArgs {
                session: Some("bash://bash:log".into()),
                sessions: None,
                match_pattern: None,
                timeout_ms: Some(0),
            })
            .await
            .unwrap();
        assert!(polled.contains("bash:log"), "{polled}");
        let deleted = hub
            .delete(DeleteArgs {
                session: Some("bash://bash:log".into()),
                sessions: None,
            })
            .unwrap();
        assert!(deleted.contains("\"deleted\": true"), "{deleted}");
    }

    #[tokio::test]
    async fn delete_requires_stop_then_removes_the_record() {
        let root = tempfile::tempdir().unwrap();
        let hub = SessionHub::new(root.path(), "Goethe", None);
        let record = hub
            .registry()
            .create_exact("bash:one", "bash", "Goethe", None, Value::Null)
            .unwrap();
        let live = hub
            .delete(DeleteArgs {
                session: Some(record.id.clone()),
                sessions: None,
            })
            .unwrap();
        assert!(live.contains("stop the session"));
        hub.registry()
            .finish(&record.id, SessionStatus::Cancelled, None)
            .unwrap();
        let deleted = hub
            .delete(DeleteArgs {
                session: Some(record.id.clone()),
                sessions: None,
            })
            .unwrap();
        assert!(deleted.contains("\"deleted\": true"));
        assert!(hub.registry().get(&record.id).unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_agent_removes_its_retained_yield_snapshot_but_not_muse_records() {
        let root = tempfile::tempdir().unwrap();
        let hub = SessionHub::new(root.path(), "Goethe", None);
        let record = hub
            .registry()
            .create_exact(
                "goethe",
                "subagent",
                "Goethe",
                None,
                json!({"yields":[{"sequence":1,"value":{"answer":"done"}}]}),
            )
            .unwrap();
        hub.registry()
            .finish(&record.id, SessionStatus::Completed, None)
            .unwrap();
        let muse = artist_session::muse_documents_dir(root.path()).join("retained.json");
        std::fs::create_dir_all(muse.parent().unwrap()).unwrap();
        std::fs::write(&muse, "{}\n").unwrap();

        let deleted = hub
            .delete(DeleteArgs {
                session: Some(record.id.clone()),
                sessions: None,
            })
            .unwrap();
        assert!(deleted.contains("\"deleted\": true"));
        assert!(hub.registry().get(&record.id).unwrap().is_none());
        assert!(
            muse.is_file(),
            "Muse-derived records outlive Artist resources"
        );
    }

    #[tokio::test]
    async fn stop_persists_a_graceful_request_without_immediately_finishing() {
        let root = tempfile::tempdir().unwrap();
        let hub = SessionHub::standard(root.path(), "Goethe", None);
        let record = hub
            .registry()
            .create_exact("bash:grace", "bash", "Goethe", None, Value::Null)
            .unwrap();

        let result = hub
            .stop(AbortArgs {
                session: Some(record.id.clone()),
                sessions: None,
            })
            .await
            .unwrap();
        let retained = hub.registry().get(&record.id).unwrap().unwrap();

        assert!(result.contains("forceAfterMs"));
        assert!(retained.lifecycle.is_live());
        assert!(retained.cancel_requested);
        assert!(retained.cancel_requested_at.is_some());
    }

    #[tokio::test]
    async fn owner_forces_a_stubborn_session_only_after_the_durable_grace_deadline() {
        let root = tempfile::tempdir().unwrap();
        let hub = SessionHub::standard(root.path(), "Goethe", None);
        let record = hub
            .registry()
            .create_exact("bash:stubborn", "bash", "Goethe", None, Value::Null)
            .unwrap();
        let aborted = Arc::new(AtomicBool::new(false));
        hub.own(
            record.id.clone(),
            Arc::new(StubbornSession {
                aborted: Arc::clone(&aborted),
            }),
        );

        hub.stop(AbortArgs {
            session: Some(record.id.clone()),
            sessions: None,
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!aborted.load(Ordering::SeqCst));

        hub.registry()
            .mutate(&record.id, |stored| {
                stored.cancel_requested_at =
                    Some(artist_registry::now().saturating_sub(STOP_GRACE.as_millis() as u64 + 1));
                Ok(())
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(aborted.load(Ordering::SeqCst));
        assert!(
            !hub.registry()
                .get(&record.id)
                .unwrap()
                .unwrap()
                .lifecycle
                .is_live()
        );
    }
}
