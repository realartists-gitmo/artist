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
use tokio::sync::Notify;

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
    pub stdout: Option<Result<DynamicValue, KernelError>>,
    pub stderr: String,
    pub stdobs: String,
    pub status: InvocationStatus,
}

#[derive(Clone)]
pub struct InvocationStore {
    next_id: Arc<AtomicU64>,
    values: Arc<Mutex<BTreeMap<String, Invocation>>>,
    changed: Arc<Notify>,
    per_invocation: Arc<Mutex<BTreeMap<String, Arc<Notify>>>>,
}

impl Default for InvocationStore {
    fn default() -> Self {
        Self {
            next_id: Arc::new(AtomicU64::new(0)),
            values: Arc::new(Mutex::new(BTreeMap::new())),
            changed: Arc::new(Notify::new()),
            per_invocation: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
}

impl InvocationStore {
    pub fn begin(&self, stdin: DynamicValue) -> Invocation {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let uri =
            ResourceUri::parse(&format!("invocations://{id}")).expect("canonical invocation URI");
        let invocation = Invocation::running(uri, stdin);
        self.values
            .lock()
            .expect("invocation store lock")
            .insert(id.to_string(), invocation.clone());
        self.per_invocation
            .lock()
            .expect("invocation notification store lock")
            .insert(id.to_string(), Arc::new(Notify::new()));
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
        let completed = Invocation::completed(
            current.uri.clone(),
            current.stdin,
            stdout,
            stdobs.into(),
            stderr.into(),
        );
        values.insert(id, completed.clone());
        self.notify_invocation(uri);
        self.changed.notify_waiters();
        Ok(completed)
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
        let aborted = Invocation::aborted(current.uri.clone(), current.stdin, stderr);
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
        let updated = Invocation { stdin, ..current };
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
            "stdin" => invocation.stdin.clone(),
            "stdout" => invocation
                .stdout
                .clone()
                .map_or(DynamicValue::Option(None), |result| {
                    DynamicValue::Result(
                        result
                            .map(Box::new)
                            .map_err(|error| Box::new(kernel_error_value(&error))),
                    )
                }),
            "stderr" => DynamicValue::String(invocation.stderr.clone()),
            "stdobs" => DynamicValue::String(invocation.stdobs.clone()),
            "status" => {
                let (state, code) = match &invocation.status {
                    InvocationStatus::Running => ("running", None),
                    InvocationStatus::Completed(code) => ("completed", Some(*code)),
                    InvocationStatus::Aborted => ("aborted", None),
                };
                DynamicValue::Record(BTreeMap::from([
                    ("state".to_owned(), DynamicValue::Enum(state.to_owned())),
                    (
                        "code".to_owned(),
                        code.map_or(DynamicValue::Option(None), |code| {
                            DynamicValue::Option(Some(Box::new(DynamicValue::S32(code))))
                        }),
                    ),
                ]))
            }
            "root" => DynamicValue::Record(BTreeMap::from([(
                "entries".to_owned(),
                DynamicValue::List(vec![DynamicValue::ResourceUri(invocation.uri.clone())]),
            )])),
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
            && matches!(
                verb.function(),
                "read" | "find" | "write" | "poll" | "abort" | "delete"
            )
        {
            ClaimDecision::Handle
        } else {
            ClaimDecision::Pass
        }
    }
}

impl DynamicResourceProvider for InvocationResourceProvider {
    fn invoke<'a>(
        &'a self,
        verb: &'a VerbId,
        uri: &'a ResourceUri,
        input: DynamicValue,
    ) -> ResourceFuture<'a> {
        Box::pin(async move {
            let channel = Self::channel(uri)?;
            let output = match verb.function() {
                "find" if channel == "root" => DynamicValue::Record(BTreeMap::from([(
                    "entries".to_owned(),
                    DynamicValue::List(
                        self.store
                            .all()?
                            .into_iter()
                            .map(|invocation| DynamicValue::ResourceUri(invocation.uri))
                            .collect(),
                    ),
                )])),
                "read" if channel == "root" => DynamicValue::Record(BTreeMap::from([(
                    "entries".to_owned(),
                    DynamicValue::List(
                        self.store
                            .all()?
                            .into_iter()
                            .map(|invocation| DynamicValue::ResourceUri(invocation.uri))
                            .collect(),
                    ),
                )])),
                "read" => Self::output(&self.store.get(uri)?, channel),
                "poll" => {
                    let started = std::time::Instant::now();
                    loop {
                        let invocation = self.store.get(uri)?;
                        if !matches!(invocation.status, InvocationStatus::Running) {
                            break Self::output(&invocation, channel);
                        }
                        let timeout = Self::poll_timeout(&input)
                            .map(|timeout| timeout.saturating_sub(started.elapsed()));
                        if timeout.is_some_and(|timeout| timeout.is_zero())
                            || !self.store.wait_for_invocation_change(uri, timeout).await
                        {
                            break Self::output(&invocation, channel);
                        }
                    }
                }
                "write" if channel == "stdin" => {
                    self.store.set_stdin(uri, input.clone())?;
                    input
                }
                "abort" => {
                    self.store.abort(uri, "invocation aborted")?;
                    DynamicValue::Enum("aborted".into())
                }
                "delete" => {
                    self.store.delete(uri)?;
                    DynamicValue::Option(None)
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
            stdin,
            stdout: None,
            stderr: String::new(),
            stdobs: String::new(),
            status: InvocationStatus::Running,
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
            stdin,
            stdout: Some(stdout),
            stderr,
            stdobs,
            status,
        }
    }

    pub fn aborted(uri: ResourceUri, stdin: DynamicValue, stderr: impl Into<String>) -> Self {
        Self {
            uri,
            stdin,
            stdout: Some(Err(KernelError::Aborted {
                message: "invocation aborted".to_owned(),
            })),
            stderr: stderr.into(),
            stdobs: String::new(),
            status: InvocationStatus::Aborted,
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
}
