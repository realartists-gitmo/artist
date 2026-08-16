//! The universal five-channel execution resource.
//!
//! A logical tool call has one typed stdin value and one lossless typed stdout
//! result. Diagnostics and model projection are deliberately separate
//! channels; status is completion state, not a second semantic error value.

use crate::{
    ClaimDecision, DynamicClaimProvider, DynamicResourceProvider, DynamicValue, DynamicVerbResult,
    KernelError, ResourceCatalogDoc, ResourceCatalogEntry, ResourceCatalogProvider, ResourceFuture,
    ResourceUri, VerbId,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, mpsc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvocationStatus {
    Running,
    Completed(i32),
    Aborted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Invocation {
    pub uri: ResourceUri,
    pub stdin: DynamicValue,
    pub stdin_history: Vec<DynamicValue>,
    pub stdout: Option<Result<DynamicValue, KernelError>>,
    pub stderr: String,
    pub stdobs: String,
    pub status: InvocationStatus,
    pub revision: u64,
    pub stdin_revision: u64,
    pub stdout_revision: u64,
    pub stderr_revision: u64,
    pub stdobs_revision: u64,
    pub channel_history: BTreeMap<String, Vec<(u64, String)>>,
}

#[derive(Clone)]
pub struct InvocationStore {
    next_id: Arc<AtomicU64>,
    values: Arc<Mutex<BTreeMap<String, Invocation>>>,
    changed: Arc<Notify>,
    per_invocation: Arc<Mutex<BTreeMap<String, Arc<Notify>>>>,
    stdin_updates: Arc<Mutex<BTreeMap<String, mpsc::UnboundedSender<DynamicValue>>>>,
}

impl Default for InvocationStore {
    fn default() -> Self {
        Self {
            next_id: Arc::new(AtomicU64::new(0)),
            values: Arc::new(Mutex::new(BTreeMap::new())),
            changed: Arc::new(Notify::new()),
            per_invocation: Arc::new(Mutex::new(BTreeMap::new())),
            stdin_updates: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
}

impl InvocationStore {
    pub fn begin(&self, stdin: DynamicValue) -> Invocation {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let uri =
            ResourceUri::parse(&format!("invocations://{id}")).expect("canonical invocation URI");
        let mut invocation = Invocation::running(uri, stdin);
        seed_channel_history(&mut invocation);
        let (sender, _receiver) = mpsc::unbounded_channel();
        self.values
            .lock()
            .expect("invocation store lock")
            .insert(id.to_string(), invocation.clone());
        self.per_invocation
            .lock()
            .expect("invocation notification store lock")
            .insert(id.to_string(), Arc::new(Notify::new()));
        self.stdin_updates
            .lock()
            .expect("invocation stdin store lock")
            .insert(id.to_string(), sender);
        self.changed.notify_waiters();
        invocation
    }

    pub fn complete(
        &self,
        uri: &ResourceUri,
        stdout: Result<DynamicValue, KernelError>,
        stdobs: impl Into<String>,
        stderr: impl Into<String>,
    ) -> Result<Invocation, KernelError> {
        let id = invocation_id(uri)?;
        let mut values = self.values.lock().map_err(|_| KernelError::Handler {
            message: "invocation store lock poisoned".into(),
        })?;
        let current = values
            .get(&id)
            .cloned()
            .ok_or_else(|| KernelError::NotFound {
                uri: uri.to_string(),
            })?;
        let stdin = current.stdin.clone();
        let stdin_history = current.stdin_history.clone();
        let stdobs = stdobs.into();
        let completed =
            Invocation::completed(current.uri.clone(), stdin, stdout, stdobs, stderr.into());
        let completed = Invocation {
            stdin_history,
            revision: current.revision + 1,
            stdout_revision: current.revision + 1,
            stderr_revision: current.revision + 1,
            stdobs_revision: current.revision + 1,
            channel_history: current.channel_history.clone(),
            ..completed
        };
        let mut completed = completed;
        record_channel_history(&mut completed);
        values.insert(id, completed.clone());
        self.notify_invocation(uri);
        self.changed.notify_waiters();
        Ok(completed)
    }

    /// Publish authoritative stdout while the logical invocation is still
    /// running. Completion later updates the same record rather than creating
    /// a second invocation or replacing the channel with a debug rendering.
    pub fn publish_stdout(
        &self,
        uri: &ResourceUri,
        stdout: Result<DynamicValue, KernelError>,
    ) -> Result<Invocation, KernelError> {
        self.publish(uri, Some(stdout), None, None)
    }

    pub fn publish_stderr(
        &self,
        uri: &ResourceUri,
        stderr: impl Into<String>,
    ) -> Result<Invocation, KernelError> {
        self.publish(uri, None, Some(stderr.into()), None)
    }

    fn publish(
        &self,
        uri: &ResourceUri,
        stdout: Option<Result<DynamicValue, KernelError>>,
        stderr: Option<String>,
        stdobs: Option<String>,
    ) -> Result<Invocation, KernelError> {
        let id = invocation_id(uri)?;
        let mut values = self.values.lock().map_err(|_| KernelError::Handler {
            message: "invocation store lock poisoned".into(),
        })?;
        let current = values
            .get(&id)
            .cloned()
            .ok_or_else(|| KernelError::NotFound {
                uri: uri.to_string(),
            })?;
        let mut updated = current.clone();
        updated.revision += 1;
        if let Some(stdout) = stdout {
            updated.stdout = Some(stdout);
            updated.stdout_revision = updated.revision;
        }
        if let Some(stderr) = stderr {
            updated.stderr = stderr;
            updated.stderr_revision = updated.revision;
        }
        if let Some(stdobs) = stdobs {
            updated.stdobs = stdobs;
            updated.stdobs_revision = updated.revision;
        }
        record_channel_history(&mut updated);
        values.insert(id, updated.clone());
        self.notify_invocation(uri);
        Ok(updated)
    }

    pub fn abort(
        &self,
        uri: &ResourceUri,
        stderr: impl Into<String>,
    ) -> Result<Invocation, KernelError> {
        let id = invocation_id(uri)?;
        let mut values = self.values.lock().map_err(|_| KernelError::Handler {
            message: "invocation store lock poisoned".into(),
        })?;
        let current = values
            .get(&id)
            .cloned()
            .ok_or_else(|| KernelError::NotFound {
                uri: uri.to_string(),
            })?;
        let stdin_history = current.stdin_history.clone();
        let aborted = Invocation {
            revision: current.revision + 1,
            stdout_revision: current.revision + 1,
            stderr_revision: current.revision + 1,
            stdobs_revision: current.revision + 1,
            channel_history: current.channel_history.clone(),
            stdin_history,
            ..Invocation::aborted(current.uri.clone(), current.stdin, stderr)
        };
        let mut aborted = aborted;
        record_channel_history(&mut aborted);
        values.insert(id, aborted.clone());
        self.notify_invocation(uri);
        self.changed.notify_waiters();
        Ok(aborted)
    }

    pub fn get(&self, uri: &ResourceUri) -> Result<Invocation, KernelError> {
        let id = invocation_id(uri)?;
        self.values
            .lock()
            .map_err(|_| KernelError::Handler {
                message: "invocation store lock poisoned".into(),
            })?
            .get(&id)
            .cloned()
            .ok_or_else(|| KernelError::NotFound {
                uri: uri.to_string(),
            })
    }

    pub fn all(&self) -> Result<Vec<Invocation>, KernelError> {
        Ok(self
            .values
            .lock()
            .map_err(|_| KernelError::Handler {
                message: "invocation store lock poisoned".into(),
            })?
            .values()
            .cloned()
            .collect())
    }

    pub fn set_stdin(
        &self,
        uri: &ResourceUri,
        stdin: DynamicValue,
    ) -> Result<Invocation, KernelError> {
        let id = invocation_id(uri)?;
        let mut values = self.values.lock().map_err(|_| KernelError::Handler {
            message: "invocation store lock poisoned".into(),
        })?;
        let current = values
            .get(&id)
            .cloned()
            .ok_or_else(|| KernelError::NotFound {
                uri: uri.to_string(),
            })?;
        let stdin_update = stdin.clone();
        let mut stdin_history = current.stdin_history;
        stdin_history.push(stdin.clone());
        let updated = Invocation {
            stdin,
            stdin_history,
            revision: current.revision + 1,
            stdin_revision: current.stdin_revision + 1,
            ..current
        };
        let mut updated = updated;
        record_channel_history(&mut updated);
        if let Ok(senders) = self.stdin_updates.lock() {
            if let Some(sender) = senders.get(&id) {
                let _ = sender.send(stdin_update);
            }
        }
        values.insert(id, updated.clone());
        self.notify_invocation(uri);
        self.changed.notify_waiters();
        Ok(updated)
    }

    pub fn delete(&self, uri: &ResourceUri) -> Result<(), KernelError> {
        let id = invocation_id(uri)?;
        self.values
            .lock()
            .map_err(|_| KernelError::Handler {
                message: "invocation store lock poisoned".into(),
            })?
            .remove(&id)
            .map(|_| ())
            .ok_or_else(|| KernelError::NotFound {
                uri: uri.to_string(),
            })?;
        if let Ok(mut notifications) = self.per_invocation.lock() {
            notifications.remove(&id);
        }
        if let Ok(mut senders) = self.stdin_updates.lock() {
            senders.remove(&id);
        }
        self.changed.notify_waiters();
        Ok(())
    }

    fn notify_invocation(&self, uri: &ResourceUri) {
        if let Ok(id) = invocation_id(uri) {
            if let Some(notify) = self
                .per_invocation
                .lock()
                .ok()
                .and_then(|items| items.get(&id).cloned())
            {
                notify.notify_waiters();
            }
        }
        self.changed.notify_waiters();
    }

    pub async fn wait_for_invocation_change(
        &self,
        uri: &ResourceUri,
        timeout: Option<std::time::Duration>,
    ) -> bool {
        let id = match invocation_id(uri) {
            Ok(id) => id,
            Err(_) => return false,
        };
        let notify = self
            .per_invocation
            .lock()
            .ok()
            .and_then(|items| items.get(&id).cloned());
        let Some(notify) = notify else {
            return false;
        };
        let wait = notify.notified();
        match timeout {
            Some(timeout) => tokio::time::timeout(timeout, wait).await.is_ok(),
            None => {
                wait.await;
                true
            }
        }
    }

    pub async fn wait_for_change(&self, timeout: Option<std::time::Duration>) -> bool {
        let notified = self.changed.notified();
        match timeout {
            Some(timeout) => tokio::time::timeout(timeout, notified).await.is_ok(),
            None => {
                notified.await;
                true
            }
        }
    }

    /// Subscribe to stdin writes for a running invocation. The logical tool
    /// executor may consume this channel while its invocation is active.
    pub fn subscribe_stdin(
        &self,
        uri: &ResourceUri,
    ) -> Result<mpsc::UnboundedReceiver<DynamicValue>, KernelError> {
        let id = invocation_id(uri)?;
        if self.get(uri).is_err() {
            return Err(KernelError::NotFound {
                uri: uri.to_string(),
            });
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        self.stdin_updates
            .lock()
            .map_err(|_| KernelError::Handler {
                message: "invocation stdin store lock poisoned".into(),
            })?
            .insert(id, sender);
        Ok(receiver)
    }
}

impl ResourceCatalogProvider for InvocationStore {
    fn resource_catalog(&self) -> Vec<ResourceCatalogEntry> {
        vec![ResourceCatalogEntry {
            name: "invocations".into(),
            description: "Addressable five-channel logical tool invocations".into(),
            docs: vec![ResourceCatalogDoc {
                uri: "invocations://<id>/{stdin|stdout|stderr|stdobs|status}".into(),
                summary: "Read, poll, write stdin, abort, or delete one invocation channel".into(),
                verbs: vec![
                    "read".into(),
                    "poll".into(),
                    "find".into(),
                    "write".into(),
                    "abort".into(),
                    "delete".into(),
                ],
                query: Vec::new(),
            }],
        }]
    }
}

fn invocation_id(uri: &ResourceUri) -> Result<String, KernelError> {
    if uri.scheme() != "invocations" {
        return Err(KernelError::UnsupportedUri {
            uri: uri.to_string(),
        });
    }
    uri.as_ref()
        .host_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| KernelError::InvalidUri {
            message: format!("invocation URI has no id: {uri}"),
        })
}

#[derive(Clone)]
pub struct InvocationResourceProvider {
    store: InvocationStore,
}

impl InvocationResourceProvider {
    pub fn new(store: InvocationStore) -> Self {
        Self { store }
    }

    fn channel(uri: &ResourceUri) -> Result<&str, KernelError> {
        match uri.path().trim_matches('/') {
            "" => Ok("root"),
            "stdin" | "stdout" | "stderr" | "stdobs" | "status" => Ok(uri.path().trim_matches('/')),
            _ => Err(KernelError::InvalidUri {
                message: format!("unknown invocation channel: {uri}"),
            }),
        }
    }

    fn output(invocation: &Invocation, channel: &str) -> DynamicValue {
        match channel {
            "stdin" => channel_text(invocation, channel, invocation.stdin.to_lossless_string()),
            "stdout" => channel_text(
                invocation,
                channel,
                invocation
                    .stdout
                    .as_ref()
                    .map(|result| match result {
                        Ok(value) => value.to_lossless_string(),
                        Err(error) => format!(
                            "{{\"type\":\"error\",\"value\":{}}}",
                            kernel_error_value(error).to_lossless_string()
                        ),
                    })
                    .unwrap_or_else(|| "{\"type\":\"pending\"}".to_owned()),
            ),
            "stderr" => channel_text(invocation, channel, invocation.stderr.clone()),
            "stdobs" => channel_text(invocation, channel, invocation.stdobs.clone()),
            "status" => channel_text(invocation, channel, status_text(&invocation.status)),
            "root" => DynamicValue::Variant(
                "entries".to_owned(),
                Some(Box::new(DynamicValue::Record(BTreeMap::from([
                    (
                        "uri".to_owned(),
                        DynamicValue::ResourceUri(invocation.uri.clone()),
                    ),
                    (
                        "entries".to_owned(),
                        DynamicValue::List(vec![DynamicValue::ResourceUri(invocation.uri.clone())]),
                    ),
                ])))),
            ),
            _ => DynamicValue::Record(BTreeMap::new()),
        }
    }

    fn poll_timeout(input: &DynamicValue) -> Option<std::time::Duration> {
        let DynamicValue::Record(fields) = input else {
            return None;
        };
        match fields.get("timeout-ms") {
            Some(DynamicValue::Option(Some(value))) => match value.as_ref() {
                DynamicValue::U64(value) => Some(std::time::Duration::from_millis(*value)),
                DynamicValue::S64(value) if *value >= 0 => {
                    Some(std::time::Duration::from_millis(*value as u64))
                }
                _ => None,
            },
            Some(DynamicValue::U64(value)) => Some(std::time::Duration::from_millis(*value)),
            Some(DynamicValue::S64(value)) if *value >= 0 => {
                Some(std::time::Duration::from_millis(*value as u64))
            }
            _ => None,
        }
    }
}

fn channel_text(invocation: &Invocation, channel: &str, text: String) -> DynamicValue {
    DynamicValue::Variant(
        "lines".to_owned(),
        Some(Box::new(channel_text_record(invocation, channel, text))),
    )
}

fn channel_text_record(invocation: &Invocation, channel: &str, text: String) -> DynamicValue {
    let channel_uri = invocation
        .channel_uri(channel)
        .unwrap_or_else(|_| invocation.uri.clone());
    DynamicValue::Record(BTreeMap::from([
        (
            "uri".to_owned(),
            DynamicValue::ResourceUri(channel_uri.clone()),
        ),
        (
            "lines".to_owned(),
            DynamicValue::List(vec![DynamicValue::Record(BTreeMap::from([
                (
                    "anchor".to_owned(),
                    DynamicValue::String(format!("#{}", channel_revision(invocation, channel))),
                ),
                ("text".to_owned(), DynamicValue::String(text)),
                ("ending".to_owned(), DynamicValue::Enum("none".to_owned())),
            ]))]),
        ),
    ]))
}

fn poll_cursor(input: &DynamicValue) -> Option<u64> {
    let DynamicValue::Record(fields) = input else {
        return None;
    };
    let DynamicValue::Option(Some(value)) = fields.get("from")? else {
        return None;
    };
    let DynamicValue::Variant(name, Some(value)) = value.as_ref() else {
        return None;
    };
    if name != "at" {
        return None;
    }
    let DynamicValue::String(anchor) = value.as_ref() else {
        return None;
    };
    anchor.trim_start_matches('#').parse().ok()
}

fn poll_output_material(
    invocation: &Invocation,
    channel: &str,
    reason: &str,
    value: String,
) -> DynamicValue {
    let channel_uri = invocation
        .channel_uri(channel)
        .unwrap_or_else(|_| invocation.uri.clone());
    DynamicValue::Record(BTreeMap::from([
        (
            "uri".to_owned(),
            DynamicValue::ResourceUri(channel_uri.clone()),
        ),
        (
            "text".to_owned(),
            DynamicValue::Record(BTreeMap::from([
                ("uri".to_owned(), DynamicValue::ResourceUri(channel_uri)),
                (
                    "lines".to_owned(),
                    DynamicValue::List(vec![DynamicValue::Record(BTreeMap::from([
                        (
                            "anchor".to_owned(),
                            DynamicValue::String(format!(
                                "#{}",
                                channel_revision(invocation, channel)
                            )),
                        ),
                        ("text".to_owned(), DynamicValue::String(value)),
                        ("ending".to_owned(), DynamicValue::Enum("none".to_owned())),
                    ]))]),
                ),
            ])),
        ),
        ("reason".to_owned(), DynamicValue::Enum(reason.to_owned())),
    ]))
}

fn status_text(status: &InvocationStatus) -> String {
    match status {
        InvocationStatus::Running => "running".to_owned(),
        InvocationStatus::Completed(code) => code.to_string(),
        InvocationStatus::Aborted => "aborted".to_owned(),
    }
}

fn channel_revision(invocation: &Invocation, channel: &str) -> u64 {
    match channel {
        "stdin" => invocation.stdin_revision,
        "stdout" => invocation.stdout_revision,
        "stderr" => invocation.stderr_revision,
        "stdobs" => invocation.stdobs_revision,
        "status" => invocation.revision,
        _ => invocation.revision,
    }
}

fn channel_content(invocation: &Invocation, channel: &str) -> String {
    match channel {
        "stdin" => invocation.stdin.to_lossless_string(),
        "stdout" => invocation
            .stdout
            .as_ref()
            .map(|result| match result {
                Ok(value) => value.to_lossless_string(),
                Err(error) => format!(
                    "{{\"type\":\"error\",\"value\":{}}}",
                    kernel_error_value(error).to_lossless_string()
                ),
            })
            .unwrap_or_else(|| "{\"type\":\"pending\"}".to_owned()),
        "stderr" => invocation.stderr.clone(),
        "stdobs" => invocation.stdobs.clone(),
        "status" => format!("{:?}", invocation.status),
        _ => String::new(),
    }
}

fn seed_channel_history(invocation: &mut Invocation) {
    for channel in ["stdin", "stdout", "stderr", "stdobs", "status"] {
        let content = channel_content(invocation, channel);
        invocation
            .channel_history
            .entry(channel.to_owned())
            .or_default()
            .push((0, content));
    }
}

fn record_channel_history(invocation: &mut Invocation) {
    for channel in ["stdin", "stdout", "stderr", "stdobs", "status"] {
        let revision = channel_revision(invocation, channel);
        let content = channel_content(invocation, channel);
        let history = invocation
            .channel_history
            .entry(channel.to_owned())
            .or_default();
        if history
            .last()
            .is_none_or(|(last, value)| *last != revision || value != &content)
        {
            history.push((revision, content));
        }
    }
}

fn channel_material_after(invocation: &Invocation, channel: &str, from: Option<u64>) -> String {
    let current = channel_content(invocation, channel);
    let Some(from) = from else {
        return current;
    };
    let Some((_, previous)) = invocation
        .channel_history
        .get(channel)
        .into_iter()
        .flat_map(|history| history.iter().rev())
        .find(|(revision, _)| *revision <= from)
    else {
        return current;
    };
    match current.strip_prefix(previous) {
        Some(delta) => delta.to_owned(),
        None => current,
    }
}

fn kernel_error_value(error: &KernelError) -> DynamicValue {
    DynamicValue::Record(BTreeMap::from([
        (
            "code".to_owned(),
            DynamicValue::Enum(error_code_name(error).to_owned()),
        ),
        (
            "uri".to_owned(),
            error_uri(error).map_or(DynamicValue::Option(None), |uri| {
                DynamicValue::Option(Some(Box::new(DynamicValue::String(uri))))
            }),
        ),
        (
            "message".to_owned(),
            DynamicValue::String(error.to_string()),
        ),
    ]))
}

fn error_code_name(error: &KernelError) -> &'static str {
    match error {
        KernelError::InvalidUri { .. } | KernelError::UnsupportedUri { .. } => "invalid-uri",
        KernelError::InvalidRequest { .. } => "invalid-input",
        KernelError::NotFound { .. } => "not-found",
        KernelError::Conflict { .. } => "conflict",
        KernelError::Aborted { .. } => "aborted",
        KernelError::PermissionDenied { .. } => "permission-denied",
        KernelError::UnsupportedVerb { .. } => "unsupported",
        _ => "internal",
    }
}

fn error_uri(error: &KernelError) -> Option<String> {
    match error {
        KernelError::InvalidUri { message }
        | KernelError::UnsupportedUri { uri: message }
        | KernelError::NotFound { uri: message }
        | KernelError::Conflict { uri: message }
        | KernelError::PermissionDenied { uri: message } => Some(message.clone()),
        KernelError::UnsupportedVerb { uri, .. } => Some(uri.clone()),
        _ => None,
    }
}

impl DynamicClaimProvider for InvocationResourceProvider {
    fn claim(&self, verb: &VerbId, uri: &ResourceUri) -> ClaimDecision {
        if uri.scheme() == "invocations"
            && verb.package() == "artist"
            && verb.interface() == "invocations"
            && matches!(
                verb.function(),
                "read" | "find" | "write" | "poll" | "grep" | "abort" | "delete"
            )
        {
            ClaimDecision::Handle
        } else {
            ClaimDecision::Pass
        }
    }
}

impl DynamicResourceProvider for InvocationResourceProvider {
    fn verb_definitions(&self) -> Vec<crate::VerbDefinition> {
        ["read", "find", "write", "poll", "grep", "abort", "delete"]
            .into_iter()
            .map(|function| {
                let identity = VerbId::new(format!("artist:invocations/{function}@1.0.0"))
                    .expect("canonical invocation verb identity");
                crate::VerbDefinition::new(
                    identity,
                    function,
                    function,
                    format!("Invocation {function} channel operation"),
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
            let channel = Self::channel(uri)?;
            let output = match verb.function() {
                "find" if channel == "root" => {
                    let entries = if uri.as_ref().host_str().is_some() {
                        let invocation = self.store.get(uri)?;
                        ["stdin", "stdout", "stderr", "stdobs", "status"]
                            .into_iter()
                            .map(|channel| {
                                invocation
                                    .channel_uri(channel)
                                    .map(DynamicValue::ResourceUri)
                            })
                            .collect::<Result<Vec<_>, _>>()?
                    } else {
                        self.store
                            .all()?
                            .into_iter()
                            .map(|invocation| DynamicValue::ResourceUri(invocation.uri))
                            .collect()
                    };
                    DynamicValue::Record(BTreeMap::from([(
                        "uris".to_owned(),
                        DynamicValue::List(entries),
                    )]))
                }
                "read" if channel == "root" => DynamicValue::Variant(
                    "entries".to_owned(),
                    Some(Box::new(DynamicValue::Record(BTreeMap::from([
                        ("uri".to_owned(), DynamicValue::ResourceUri(uri.clone())),
                        (
                            "entries".to_owned(),
                            DynamicValue::List(
                                self.store
                                    .all()?
                                    .into_iter()
                                    .map(|invocation| DynamicValue::ResourceUri(invocation.uri))
                                    .collect(),
                            ),
                        ),
                    ])))),
                ),
                "read" => Self::output(&self.store.get(uri)?, channel),
                "grep" => {
                    let pattern = match &input {
                        DynamicValue::Record(fields) => match fields.get("pattern") {
                            Some(DynamicValue::String(value)) => value.clone(),
                            _ => {
                                return Err(KernelError::InvalidRequest {
                                    message: "grep pattern must be a string".into(),
                                });
                            }
                        },
                        _ => {
                            return Err(KernelError::InvalidRequest {
                                message: "grep input must be a record".into(),
                            });
                        }
                    };
                    let invocation = self.store.get(uri)?;
                    let channel = Self::channel(uri)?;
                    let content = channel_content(&invocation, channel);
                    let matches = regex::Regex::new(&pattern)
                        .map_err(|error| KernelError::InvalidPattern {
                            message: error.to_string(),
                        })?
                        .is_match(&content);
                    DynamicValue::Record(BTreeMap::from([(
                        "matches".to_owned(),
                        DynamicValue::List(if matches {
                            vec![channel_text_record(&invocation, channel, content)]
                        } else {
                            Vec::new()
                        }),
                    )]))
                }
                "poll" => {
                    let started = std::time::Instant::now();
                    let pattern = match &input {
                        DynamicValue::Record(fields) => match fields.get("match") {
                            Some(DynamicValue::Option(Some(value))) => match value.as_ref() {
                                DynamicValue::String(value) => Some(value.clone()),
                                _ => None,
                            },
                            Some(DynamicValue::String(value)) => Some(value.clone()),
                            _ => None,
                        },
                        _ => None,
                    };
                    let matcher = pattern
                        .as_deref()
                        .map(regex::Regex::new)
                        .transpose()
                        .map_err(|error| KernelError::InvalidPattern {
                            message: error.to_string(),
                        })?;
                    let from = poll_cursor(&input);
                    loop {
                        let invocation = self.store.get(uri)?;
                        let revision = channel_revision(&invocation, channel);
                        let changed = from.is_some_and(|from| revision > from);
                        let eligible = from.is_none() || changed;
                        let content = channel_material_after(&invocation, channel, from);
                        let matched = eligible
                            && matcher
                                .as_ref()
                                .is_some_and(|matcher| matcher.is_match(&content));
                        let reason = if pattern.is_some() && matched {
                            "matched"
                        } else if matches!(invocation.status, InvocationStatus::Running) {
                            "changed"
                        } else {
                            "terminated"
                        };
                        let snapshot = poll_output_material(&invocation, channel, reason, content);
                        if changed && (pattern.is_none() || matched) {
                            break snapshot;
                        }
                        if !matches!(invocation.status, InvocationStatus::Running) {
                            break snapshot;
                        }
                        let timeout = Self::poll_timeout(&input)
                            .map(|timeout| timeout.saturating_sub(started.elapsed()));
                        if timeout.is_some_and(|timeout| timeout.is_zero())
                            || !self.store.wait_for_invocation_change(uri, timeout).await
                        {
                            let content = channel_material_after(&invocation, channel, from);
                            break poll_output_material(&invocation, channel, "timeout", content);
                        }
                        // A channel update is itself a meaningful stream
                        // event, even while the logical invocation remains
                        // running. Do not turn the channel poll into a
                        // status-only wait.
                        if changed && (pattern.is_none() || matched) && channel != "status" {
                            break snapshot;
                        }
                    }
                }
                "write" if channel == "stdin" => {
                    let content = match &input {
                        DynamicValue::Record(fields) => {
                            fields.get("content").cloned().ok_or_else(|| {
                                KernelError::InvalidRequest {
                                    message: "invocation stdin write is missing content".into(),
                                }
                            })?
                        }
                        value => value.clone(),
                    };
                    self.store.set_stdin(uri, content.clone())?;
                    DynamicValue::Record(BTreeMap::from([
                        ("uri".to_owned(), DynamicValue::ResourceUri(uri.clone())),
                        ("text".to_owned(), DynamicValue::Option(None)),
                    ]))
                }
                "abort" => {
                    self.store.abort(uri, "invocation aborted")?;
                    DynamicValue::Record(BTreeMap::from([(
                        "uri".to_owned(),
                        DynamicValue::ResourceUri(uri.clone()),
                    )]))
                }
                "delete" => {
                    self.store.delete(uri)?;
                    DynamicValue::Record(BTreeMap::from([(
                        "uri".to_owned(),
                        DynamicValue::ResourceUri(uri.clone()),
                    )]))
                }
                _ => {
                    return Err(KernelError::UnsupportedVerb {
                        verb: verb.to_string(),
                        uri: uri.to_string(),
                    });
                }
            };
            Ok(DynamicVerbResult {
                verb: verb.clone(),
                function: verb.function().to_owned(),
                output,
            })
        })
    }
}

impl Invocation {
    pub fn channel_uri(&self, channel: &str) -> Result<ResourceUri, KernelError> {
        match channel {
            "stdin" | "stdout" | "stderr" | "stdobs" | "status" => {
                ResourceUri::parse(&format!("{}/{}", self.uri, channel)).map_err(|error| {
                    KernelError::InvalidUri {
                        message: error.to_string(),
                    }
                })
            }
            _ => Err(KernelError::InvalidRequest {
                message: format!("unknown invocation channel {channel}"),
            }),
        }
    }

    pub fn running(uri: ResourceUri, stdin: DynamicValue) -> Self {
        Self {
            uri,
            stdin: stdin.clone(),
            stdin_history: vec![stdin.clone()],
            stdout: None,
            stderr: String::new(),
            stdobs: String::new(),
            status: InvocationStatus::Running,
            revision: 0,
            stdin_revision: 0,
            stdout_revision: 0,
            stderr_revision: 0,
            stdobs_revision: 0,
            channel_history: BTreeMap::new(),
        }
    }

    pub fn completed(
        uri: ResourceUri,
        stdin: DynamicValue,
        stdout: Result<DynamicValue, KernelError>,
        stdobs: String,
        stderr: String,
    ) -> Self {
        let status = if matches!(&stdout, Ok(DynamicValue::Result(Err(_)))) || stdout.is_err() {
            InvocationStatus::Completed(1)
        } else {
            InvocationStatus::Completed(0)
        };
        Self {
            uri,
            stdin: stdin.clone(),
            stdin_history: vec![stdin.clone()],
            stdout: Some(stdout),
            stderr,
            stdobs,
            status,
            revision: 0,
            stdin_revision: 0,
            stdout_revision: 0,
            stderr_revision: 0,
            stdobs_revision: 0,
            channel_history: BTreeMap::new(),
        }
    }

    pub fn aborted(uri: ResourceUri, stdin: DynamicValue, stderr: impl Into<String>) -> Self {
        Self {
            uri,
            stdin: stdin.clone(),
            stdin_history: vec![stdin.clone()],
            stdout: Some(Err(KernelError::Aborted {
                message: "invocation aborted".to_owned(),
            })),
            stderr: stderr.into(),
            stdobs: String::new(),
            status: InvocationStatus::Aborted,
            revision: 0,
            stdin_revision: 0,
            stdout_revision: 0,
            stderr_revision: 0,
            stdobs_revision: 0,
            channel_history: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_store_preserves_five_channels_and_status() {
        let store = InvocationStore::default();
        let invocation = store.begin(DynamicValue::String("request".into()));
        store
            .complete(
                &invocation.uri,
                Ok(DynamicValue::String("response".into())),
                "compact",
                "",
            )
            .unwrap();
        let saved = store.get(&invocation.uri).unwrap();
        assert_eq!(saved.stdin, DynamicValue::String("request".into()));
        assert_eq!(
            saved.stdout,
            Some(Ok(DynamicValue::String("response".into())))
        );
        assert_eq!(saved.stdobs, "compact");
        assert_eq!(saved.status, InvocationStatus::Completed(0));
        for channel in ["stdin", "stdout", "stderr", "stdobs", "status"] {
            assert!(
                saved
                    .channel_uri(channel)
                    .unwrap()
                    .to_string()
                    .contains(channel)
            );
        }
    }

    #[tokio::test]
    async fn invocation_stdin_writes_reach_active_consumers() {
        let store = InvocationStore::default();
        let invocation = store.begin(DynamicValue::String("initial".into()));
        let mut updates = store.subscribe_stdin(&invocation.uri).unwrap();

        store
            .set_stdin(&invocation.uri, DynamicValue::String("follow-up".into()))
            .unwrap();

        assert_eq!(
            updates.recv().await,
            Some(DynamicValue::String("follow-up".into()))
        );
        let saved = store.get(&invocation.uri).unwrap();
        assert_eq!(saved.stdin_history.len(), 2);
    }
}
