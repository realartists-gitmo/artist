//! Direct executable process resources.
//!
//! A process is a resource tree. Its root exposes lifecycle state; stdin,
//! stdout, stderr, and ctl are addressed child nouns. No shell parser or
//! capability string is involved.

use crate::{
    ClaimDecision, DynamicClaimProvider, DynamicResourceProvider, DynamicType, DynamicValue,
    DynamicVerbResult, KernelError, ResourceFuture, ResourceUri, VerbDefinition, VerbId,
};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
};

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessSnapshot {
    pub uri: String,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub aborted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessVerbBindings {
    pub run: VerbId,
    pub write: VerbId,
    pub read: VerbId,
    pub poll: VerbId,
    pub abort: VerbId,
    pub delete: VerbId,
    pub grep: VerbId,
}

impl ProcessVerbBindings {
    pub fn definitions(&self) -> Vec<VerbDefinition> {
        let uri = DynamicType::ResourceUri;
        let content = DynamicType::Record(BTreeMap::from([
            ("uri".to_owned(), uri.clone()),
            ("content".to_owned(), DynamicType::String),
        ]));
        let run = DynamicType::Record(BTreeMap::from([
            ("uri".to_owned(), uri.clone()),
            (
                "args".to_owned(),
                DynamicType::List(Box::new(DynamicType::String)),
            ),
        ]));
        let read_input = DynamicType::Record(BTreeMap::from([
            ("uri".to_owned(), uri.clone()),
            (
                "at".to_owned(),
                DynamicType::Option(Box::new(DynamicType::Variant(BTreeMap::from([
                    ("top".to_owned(), None),
                    ("bottom".to_owned(), None),
                    ("at".to_owned(), Some(DynamicType::String)),
                ])))),
            ),
            (
                "before".to_owned(),
                DynamicType::Option(Box::new(DynamicType::U32)),
            ),
            (
                "after".to_owned(),
                DynamicType::Option(Box::new(DynamicType::U32)),
            ),
        ]));
        let _snapshot = DynamicType::Record(BTreeMap::from([
            ("uri".to_owned(), uri.clone()),
            ("running".to_owned(), DynamicType::Bool),
            (
                "exit-code".to_owned(),
                DynamicType::Option(Box::new(DynamicType::S32)),
            ),
            ("aborted".to_owned(), DynamicType::Bool),
        ]));
        let line = DynamicType::Record(BTreeMap::from([
            ("anchor".to_owned(), DynamicType::String),
            ("text".to_owned(), DynamicType::String),
            (
                "ending".to_owned(),
                DynamicType::Enum(vec![
                    "lf".to_owned(),
                    "crlf".to_owned(),
                    "cr".to_owned(),
                    "none".to_owned(),
                ]),
            ),
        ]));
        let read_output = DynamicType::Record(BTreeMap::from([
            ("uri".to_owned(), uri.clone()),
            ("lines".to_owned(), DynamicType::List(Box::new(line))),
        ]));
        let poll_input = DynamicType::Record(BTreeMap::from([
            ("uri".to_owned(), uri.clone()),
            (
                "from".to_owned(),
                DynamicType::Option(Box::new(DynamicType::Variant(BTreeMap::from([
                    ("top".to_owned(), None),
                    ("bottom".to_owned(), None),
                    ("at".to_owned(), Some(DynamicType::String)),
                ])))),
            ),
            (
                "match".to_owned(),
                DynamicType::Option(Box::new(DynamicType::String)),
            ),
            (
                "timeout-ms".to_owned(),
                DynamicType::Option(Box::new(DynamicType::U64)),
            ),
        ]));
        let poll_output = DynamicType::Record(BTreeMap::from([
            ("uri".to_owned(), uri.clone()),
            ("text".to_owned(), read_output.clone()),
            (
                "reason".to_owned(),
                DynamicType::Enum(vec![
                    "changed".to_owned(),
                    "matched".to_owned(),
                    "terminated".to_owned(),
                    "timeout".to_owned(),
                ]),
            ),
        ]));
        let grep_input = DynamicType::Record(BTreeMap::from([
            ("uri".to_owned(), uri.clone()),
            ("pattern".to_owned(), DynamicType::String),
        ]));
        let grep_output = DynamicType::Record(BTreeMap::from([(
            "matches".to_owned(),
            DynamicType::List(Box::new(read_output.clone())),
        )]));
        vec![
            VerbDefinition::new(self.run.clone(), "run", "run", "Run a direct executable")
                .with_contract(
                    run,
                    DynamicType::Record(BTreeMap::from([("uri".to_owned(), uri.clone())])),
                )
                .with_extractor("resource-uri"),
            VerbDefinition::new(self.write.clone(), "write", "write", "Write process input")
                .with_contract(
                    content,
                    DynamicType::Record(BTreeMap::from([("uri".to_owned(), uri.clone())])),
                )
                .with_extractor("resource-uri"),
            VerbDefinition::new(
                self.read.clone(),
                "read",
                "read",
                "Read process state or output",
            )
            .with_contract(read_input, read_output.clone())
            .with_extractor("resource-uri"),
            VerbDefinition::new(self.poll.clone(), "poll", "poll", "Poll process state")
                .with_contract(poll_input, poll_output)
                .with_extractor("resource-uri"),
            VerbDefinition::new(self.abort.clone(), "abort", "abort", "Abort a process")
                .with_contract(
                    DynamicType::Record(BTreeMap::from([("uri".to_owned(), uri.clone())])),
                    DynamicType::Record(BTreeMap::from([("uri".to_owned(), uri.clone())])),
                )
                .with_extractor("resource-uri"),
            VerbDefinition::new(self.delete.clone(), "delete", "delete", "Delete a process")
                .with_contract(
                    DynamicType::Record(BTreeMap::from([("uri".to_owned(), uri.clone())])),
                    DynamicType::Record(BTreeMap::from([("uri".to_owned(), uri)])),
                )
                .with_extractor("resource-uri"),
            VerbDefinition::new(self.grep.clone(), "grep", "grep", "Search process output")
                .with_contract(grep_input, grep_output)
                .with_extractor("resource-uri"),
        ]
    }
}

struct ProcessState {
    child: Child,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    aborted: bool,
}

fn spawn_reader<R: Read + Send + 'static>(mut stream: R, output: Arc<Mutex<Vec<u8>>>) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(size) => {
                    if let Ok(mut current) = output.lock() {
                        current.extend_from_slice(&buffer[..size]);
                    }
                }
            }
        }
    });
}

#[cfg(unix)]
fn send_interrupt(child: &Child) -> Result<(), KernelError> {
    let pid = child.id() as libc::pid_t;
    // SAFETY: `pid` is obtained from the live Child handle and the call does
    // not dereference the process; it only delivers SIGINT to that process.
    let result = unsafe { libc::kill(pid, libc::SIGINT) };
    if result == 0 {
        Ok(())
    } else {
        Err(KernelError::Handler {
            message: std::io::Error::last_os_error().to_string(),
        })
    }
}

#[cfg(not(unix))]
fn send_interrupt(child: &Child) -> Result<(), KernelError> {
    child.kill().map_err(|error| KernelError::Handler {
        message: format!("could not interrupt process: {error}"),
    })
}

#[derive(Clone, Default)]
pub struct ProcessManager {
    processes: Arc<Mutex<BTreeMap<String, Arc<Mutex<ProcessState>>>>>,
    next_id: Arc<Mutex<u64>>,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn run(
        &self,
        executable: impl AsRef<Path>,
        args: &[String],
    ) -> Result<String, KernelError> {
        let executable = executable.as_ref();
        if !executable.is_file() {
            return Err(KernelError::NotFound {
                uri: executable.display().to_string(),
            });
        }
        let mut command = Command::new(executable);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| KernelError::Handler {
            message: format!("could not execute {}: {error}", executable.display()),
        })?;
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        if let Some(stream) = child.stdout.take() {
            spawn_reader(stream, stdout.clone());
        }
        if let Some(stream) = child.stderr.take() {
            spawn_reader(stream, stderr.clone());
        }
        let mut next_id = self.next_id.lock().map_err(|_| KernelError::Handler {
            message: "process id lock poisoned".to_owned(),
        })?;
        *next_id += 1;
        let uri = format!("osproc://{}", *next_id);
        self.processes
            .lock()
            .map_err(|_| KernelError::Handler {
                message: "process registry lock poisoned".to_owned(),
            })?
            .insert(
                uri.clone(),
                Arc::new(Mutex::new(ProcessState {
                    child,
                    stdout,
                    stderr,
                    aborted: false,
                })),
            );
        Ok(uri)
    }

    fn lookup(&self, uri: &str) -> Result<Arc<Mutex<ProcessState>>, KernelError> {
        let root = uri.split('/').take(3).collect::<Vec<_>>().join("/");
        self.processes
            .lock()
            .map_err(|_| KernelError::Handler {
                message: "process registry lock poisoned".to_owned(),
            })?
            .get(&root)
            .cloned()
            .ok_or_else(|| KernelError::NotFound { uri: root })
    }

    pub fn write(&self, uri: &str, content: &[u8]) -> Result<(), KernelError> {
        let process = self.lookup(uri)?;
        let mut process = process.lock().map_err(|_| KernelError::Handler {
            message: "process lock poisoned".into(),
        })?;
        if uri.ends_with("/ctl") {
            match content {
                b"interrupt" => {
                    send_interrupt(&process.child)?;
                    Ok(())
                }
                b"terminate" => {
                    process.child.kill().map_err(|error| KernelError::Handler {
                        message: format!("could not terminate process: {error}"),
                    })?;
                    Ok(())
                }
                _ => Err(KernelError::InvalidRequest {
                    message: "unknown process control command".into(),
                }),
            }
        } else {
            process
                .child
                .stdin
                .as_mut()
                .ok_or_else(|| KernelError::WrongKind {
                    message: "process stdin is unavailable".into(),
                })?
                .write_all(content)
                .map_err(|error| KernelError::Handler {
                    message: format!("could not write process stdin: {error}"),
                })
        }
    }

    pub fn snapshot(&self, uri: &str) -> Result<ProcessSnapshot, KernelError> {
        let process = self.lookup(uri)?;
        let mut process = process.lock().map_err(|_| KernelError::Handler {
            message: "process lock poisoned".into(),
        })?;
        let status = process
            .child
            .try_wait()
            .map_err(|error| KernelError::Handler {
                message: format!("could not poll process: {error}"),
            })?;
        Ok(ProcessSnapshot {
            uri: uri.to_owned(),
            running: status.is_none(),
            exit_code: status.and_then(process_exit_code),
            aborted: process.aborted,
        })
    }

    pub fn output(&self, uri: &str, stderr: bool) -> Result<String, KernelError> {
        Ok(String::from_utf8_lossy(&self.output_bytes(uri, stderr)?).into_owned())
    }

    pub fn output_bytes(&self, uri: &str, stderr: bool) -> Result<Vec<u8>, KernelError> {
        let process = self.lookup(uri)?;
        let process = process.lock().map_err(|_| KernelError::Handler {
            message: "process lock poisoned".into(),
        })?;
        let bytes = if stderr {
            process.stderr.clone()
        } else {
            process.stdout.clone()
        };
        Ok(bytes
            .lock()
            .map_err(|_| KernelError::Handler {
                message: "process output lock poisoned".into(),
            })?
            .clone())
    }

    pub fn abort(&self, uri: &str) -> Result<(), KernelError> {
        let process = self.lookup(uri)?;
        let mut process = process.lock().map_err(|_| KernelError::Handler {
            message: "process lock poisoned".into(),
        })?;
        process.child.kill().map_err(|error| KernelError::Handler {
            message: format!("could not abort process: {error}"),
        })?;
        process.aborted = true;
        Ok(())
    }

    pub fn delete(&self, uri: &str) -> Result<(), KernelError> {
        let root = uri.split('/').take(3).collect::<Vec<_>>().join("/");
        let process = self
            .processes
            .lock()
            .map_err(|_| KernelError::Handler {
                message: "process registry lock poisoned".into(),
            })?
            .remove(&root)
            .ok_or_else(|| KernelError::NotFound { uri: root.clone() })?;
        if let Ok(mut process) = process.lock() {
            let _ = process.child.kill();
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ProcessResourceProvider {
    manager: ProcessManager,
    bindings: ProcessVerbBindings,
}

impl ProcessResourceProvider {
    pub fn new(manager: ProcessManager, bindings: ProcessVerbBindings) -> Self {
        Self { manager, bindings }
    }

    fn record(input: &DynamicValue) -> Result<&BTreeMap<String, DynamicValue>, KernelError> {
        match input {
            DynamicValue::Record(fields) => Ok(fields),
            _ => Err(KernelError::InvalidRequest {
                message: "process input must be a record".into(),
            }),
        }
    }

    fn string(input: &DynamicValue, field: &str) -> Result<String, KernelError> {
        match Self::record(input)?.get(field) {
            Some(DynamicValue::String(value)) => Ok(value.clone()),
            Some(DynamicValue::ResourceUri(value)) => Ok(value.to_string()),
            _ => Err(KernelError::InvalidRequest {
                message: format!("process field {field} must be a string"),
            }),
        }
    }

    fn args(input: &DynamicValue) -> Result<Vec<String>, KernelError> {
        match Self::record(input)?.get("args") {
            Some(DynamicValue::List(values)) => values
                .iter()
                .map(|value| match value {
                    DynamicValue::String(value) => Ok(value.clone()),
                    _ => Err(KernelError::InvalidRequest {
                        message: "process args must be strings".into(),
                    }),
                })
                .collect(),
            _ => Ok(Vec::new()),
        }
    }

    fn root(uri: &ResourceUri) -> bool {
        uri.scheme() == "osproc" && uri.path().matches('/').count() <= 1
    }

    fn value(uri: ResourceUri, snapshot: ProcessSnapshot) -> DynamicValue {
        DynamicValue::Record(BTreeMap::from([
            ("uri".into(), DynamicValue::ResourceUri(uri)),
            ("running".into(), DynamicValue::Bool(snapshot.running)),
            (
                "exit-code".into(),
                DynamicValue::Option(
                    snapshot
                        .exit_code
                        .map(|value| Box::new(DynamicValue::S32(value))),
                ),
            ),
            ("aborted".into(), DynamicValue::Bool(snapshot.aborted)),
        ]))
    }
}

impl DynamicClaimProvider for ProcessResourceProvider {
    fn claim(&self, verb: &VerbId, uri: &ResourceUri) -> ClaimDecision {
        if verb == &self.bindings.run {
            return (uri.scheme() == "file")
                .then_some(ClaimDecision::Handle)
                .unwrap_or(ClaimDecision::Pass);
        }
        let child = uri.scheme() == "osproc"
            && (uri.path().ends_with("/stdin") || uri.path().ends_with("/ctl"));
        if (verb == &self.bindings.write && child)
            || ((verb == &self.bindings.read
                || verb == &self.bindings.poll
                || verb == &self.bindings.abort
                || verb == &self.bindings.delete
                || verb == &self.bindings.grep)
                && (uri.scheme() == "osproc"))
        {
            ClaimDecision::Handle
        } else {
            ClaimDecision::Pass
        }
    }
}

impl DynamicResourceProvider for ProcessResourceProvider {
    fn verb_definitions(&self) -> Vec<VerbDefinition> {
        self.bindings.definitions()
    }

    fn invoke<'a>(
        &'a self,
        verb: &'a VerbId,
        uri: &'a ResourceUri,
        input: DynamicValue,
    ) -> ResourceFuture<'a> {
        let manager = self.manager.clone();
        let bindings = self.bindings.clone();
        Box::pin(async move {
            let output = if verb == &bindings.run {
                let executable_uri = ResourceUri::parse(&Self::string(&input, "uri")?)?;
                let executable = executable_uri.as_ref().to_file_path().map_err(|_| {
                    KernelError::InvalidUri {
                        message: executable_uri.to_string(),
                    }
                })?;
                DynamicValue::Record(BTreeMap::from([(
                    "uri".into(),
                    DynamicValue::ResourceUri(ResourceUri::parse(
                        &manager.run(executable, &Self::args(&input)?)?,
                    )?),
                )]))
            } else if verb == &bindings.write {
                manager.write(
                    uri.to_string().as_str(),
                    Self::string(&input, "content")?.as_bytes(),
                )?;
                DynamicValue::Record(BTreeMap::from([(
                    "uri".into(),
                    DynamicValue::ResourceUri(uri.clone()),
                )]))
            } else if verb == &bindings.poll {
                Self::poll_value(&manager, uri, &input).await?
            } else if verb == &bindings.read {
                if uri.path().ends_with("/stdout") || uri.path().ends_with("/stderr") {
                    process_text_bytes(
                        uri,
                        &manager.output_bytes(
                            uri.to_string().as_str(),
                            uri.path().ends_with("/stderr"),
                        )?,
                        0,
                    )
                } else {
                    let snapshot = manager.snapshot(uri.to_string().as_str())?;
                    process_status_value(
                        uri,
                        format!(
                            "running={} exit-code={:?} aborted={}",
                            snapshot.running, snapshot.exit_code, snapshot.aborted
                        ),
                        if snapshot.aborted {
                            2
                        } else if snapshot.running {
                            0
                        } else {
                            1
                        },
                    )
                }
            } else if verb == &bindings.abort {
                manager.abort(uri.to_string().as_str())?;
                DynamicValue::Record(BTreeMap::from([(
                    "uri".into(),
                    DynamicValue::ResourceUri(uri.clone()),
                )]))
            } else if verb == &bindings.delete {
                manager.delete(uri.to_string().as_str())?;
                DynamicValue::Record(BTreeMap::from([(
                    "uri".into(),
                    DynamicValue::ResourceUri(uri.clone()),
                )]))
            } else if verb == &bindings.grep {
                process_grep_value(&manager, uri, &input)?
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

fn process_grep_value(
    manager: &ProcessManager,
    uri: &ResourceUri,
    input: &DynamicValue,
) -> Result<DynamicValue, KernelError> {
    let pattern = match input {
        DynamicValue::Record(fields) => match fields.get("pattern") {
            Some(DynamicValue::String(pattern)) => pattern.clone(),
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
    let regex = regex::Regex::new(&pattern).map_err(|error| KernelError::InvalidPattern {
        message: error.to_string(),
    })?;
    let bytes = if uri.path().ends_with("/stderr") {
        manager.output_bytes(&uri.to_string(), true)?
    } else if uri.path().ends_with("/stdout") {
        manager.output_bytes(&uri.to_string(), false)?
    } else {
        let snapshot = manager.snapshot(&uri.to_string())?;
        format!(
            "running={} exit-code={:?} aborted={}",
            snapshot.running, snapshot.exit_code, snapshot.aborted
        )
        .into_bytes()
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let matches = if regex.is_match(&text) {
        vec![process_text_bytes(uri, &bytes, 0)]
    } else {
        Vec::new()
    };
    Ok(DynamicValue::Record(BTreeMap::from([(
        "matches".into(),
        DynamicValue::List(matches),
    )])))
}

impl ProcessResourceProvider {
    async fn poll_value(
        manager: &ProcessManager,
        uri: &ResourceUri,
        input: &DynamicValue,
    ) -> Result<DynamicValue, KernelError> {
        let (pattern, timeout_ms, from) = process_poll_options(input)?;
        let matcher = pattern
            .as_deref()
            .map(regex::Regex::new)
            .transpose()
            .map_err(|error| KernelError::InvalidPattern {
                message: error.to_string(),
            })?;
        let timeout = timeout_ms.map(std::time::Duration::from_millis);
        let started = std::time::Instant::now();
        loop {
            let snapshot = manager.snapshot(uri.to_string().as_str())?;
            let (observed, raw, cursor, changed, status_revision) =
                if uri.path().ends_with("/stderr") {
                    let bytes = manager.output_bytes(uri.to_string().as_str(), true)?;
                    let revision = bytes.len() as u64;
                    let cursor = from.unwrap_or(0).min(revision);
                    (
                        String::from_utf8_lossy(&bytes[cursor as usize..]).into_owned(),
                        Some(bytes[cursor as usize..].to_vec()),
                        cursor,
                        from.is_some() && revision > from.unwrap_or(0),
                        None,
                    )
                } else if uri.path().ends_with("/stdout") {
                    let bytes = manager.output_bytes(uri.to_string().as_str(), false)?;
                    let revision = bytes.len() as u64;
                    let cursor = from.unwrap_or(0).min(revision);
                    (
                        String::from_utf8_lossy(&bytes[cursor as usize..]).into_owned(),
                        Some(bytes[cursor as usize..].to_vec()),
                        cursor,
                        from.is_some() && revision > from.unwrap_or(0),
                        None,
                    )
                } else {
                    let revision = if snapshot.aborted {
                        2
                    } else if snapshot.running {
                        0
                    } else {
                        1
                    };
                    (
                        format!(
                            "running={} exit-code={:?} aborted={}",
                            snapshot.running, snapshot.exit_code, snapshot.aborted
                        ),
                        None,
                        0,
                        from.is_some() && revision > from.unwrap_or(0),
                        Some(revision),
                    )
                };
            let make_result = |reason: &str| {
                raw.as_deref()
                    .map(|bytes| process_poll_result_bytes(uri, bytes, reason, cursor))
                    .unwrap_or_else(|| {
                        status_revision.map_or_else(
                            || process_poll_result(uri, observed.clone(), reason, cursor),
                            |revision| {
                                process_poll_result_status(uri, observed.clone(), reason, revision)
                            },
                        )
                    })
            };
            if matcher
                .as_ref()
                .is_some_and(|matcher| matcher.is_match(&observed))
            {
                return Ok(make_result("matched"));
            }
            if changed {
                return Ok(make_result("changed"));
            }
            if !snapshot.running {
                return Ok(make_result("terminated"));
            }
            if timeout.is_some_and(|timeout| started.elapsed() >= timeout) {
                return Ok(make_result("timeout"));
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }
}

fn process_exit_code(status: std::process::ExitStatus) -> Option<i32> {
    status.code().or_else(|| {
        #[cfg(unix)]
        {
            status.signal().map(|signal| 128 + signal)
        }
        #[cfg(not(unix))]
        {
            None
        }
    })
}

fn process_poll_result(uri: &ResourceUri, text: String, reason: &str, base: u64) -> DynamicValue {
    let lines = process_lines_at(&text, base);
    process_poll_result_with_lines(uri, lines, reason)
}

fn process_poll_result_status(
    uri: &ResourceUri,
    text: String,
    reason: &str,
    revision: u64,
) -> DynamicValue {
    process_poll_result_with_lines(
        uri,
        vec![DynamicValue::Record(BTreeMap::from([
            (
                "anchor".to_owned(),
                DynamicValue::String(format!("#{revision}")),
            ),
            ("text".to_owned(), DynamicValue::String(text)),
            ("ending".to_owned(), DynamicValue::Enum("none".to_owned())),
        ]))],
        reason,
    )
}

fn process_poll_result_bytes(
    uri: &ResourceUri,
    bytes: &[u8],
    reason: &str,
    base: u64,
) -> DynamicValue {
    process_poll_result_with_lines(uri, process_lines_at_bytes(bytes, base), reason)
}

fn process_poll_result_with_lines(
    uri: &ResourceUri,
    lines: Vec<DynamicValue>,
    reason: &str,
) -> DynamicValue {
    DynamicValue::Record(BTreeMap::from([
        ("uri".to_owned(), DynamicValue::ResourceUri(uri.clone())),
        (
            "text".to_owned(),
            DynamicValue::Record(BTreeMap::from([
                ("uri".to_owned(), DynamicValue::ResourceUri(uri.clone())),
                ("lines".to_owned(), DynamicValue::List(lines)),
            ])),
        ),
        ("reason".to_owned(), DynamicValue::Enum(reason.to_owned())),
    ]))
}

fn process_text_value(uri: &ResourceUri, text: String) -> DynamicValue {
    DynamicValue::Variant(
        "lines".to_owned(),
        Some(Box::new(process_text_bytes(uri, text.as_bytes(), 0))),
    )
}

fn process_status_value(uri: &ResourceUri, text: String, revision: u64) -> DynamicValue {
    DynamicValue::Variant(
        "lines".to_owned(),
        Some(Box::new(DynamicValue::Record(BTreeMap::from([
            ("uri".to_owned(), DynamicValue::ResourceUri(uri.clone())),
            (
                "lines".to_owned(),
                DynamicValue::List(vec![DynamicValue::Record(BTreeMap::from([
                    (
                        "anchor".to_owned(),
                        DynamicValue::String(format!("#{revision}")),
                    ),
                    ("text".to_owned(), DynamicValue::String(text)),
                    ("ending".to_owned(), DynamicValue::Enum("none".to_owned())),
                ]))]),
            ),
        ])))),
    )
}

fn process_text_bytes(uri: &ResourceUri, bytes: &[u8], base: u64) -> DynamicValue {
    DynamicValue::Record(BTreeMap::from([
        ("uri".to_owned(), DynamicValue::ResourceUri(uri.clone())),
        (
            "lines".to_owned(),
            DynamicValue::List(process_lines_at_bytes(bytes, base)),
        ),
    ]))
}

fn process_lines(text: &str) -> Vec<DynamicValue> {
    process_lines_at(text, 0)
}

fn process_lines_at(text: &str, base: u64) -> Vec<DynamicValue> {
    process_lines_at_bytes(text.as_bytes(), base)
}

fn process_lines_at_bytes(bytes: &[u8], base: u64) -> Vec<DynamicValue> {
    let mut offset = base;
    let mut lines = Vec::new();
    for chunk in bytes.split_inclusive(|byte| *byte == b'\n') {
        offset += chunk.len() as u64;
        let line = chunk.strip_suffix(&[b'\n']).unwrap_or(chunk);
        lines.push(DynamicValue::Record(BTreeMap::from([
            (
                "anchor".to_owned(),
                DynamicValue::String(format!("#{offset}")),
            ),
            (
                "text".to_owned(),
                DynamicValue::String(String::from_utf8_lossy(line).into_owned()),
            ),
            ("ending".to_owned(), DynamicValue::Enum("lf".to_owned())),
        ])));
    }
    if lines.is_empty() {
        lines.push(DynamicValue::Record(BTreeMap::from([
            (
                "anchor".to_owned(),
                DynamicValue::String(format!("#{base}")),
            ),
            ("text".to_owned(), DynamicValue::String(String::new())),
            ("ending".to_owned(), DynamicValue::Enum("none".to_owned())),
        ])));
    }
    lines
}

fn process_poll_options(
    input: &DynamicValue,
) -> Result<(Option<String>, Option<u64>, Option<u64>), KernelError> {
    let DynamicValue::Record(fields) = input else {
        return Ok((None, None, None));
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
    let timeout = match fields.get("timeout-ms") {
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
    let from = match fields.get("from") {
        None | Some(DynamicValue::Option(None)) => None,
        Some(DynamicValue::Option(Some(value))) => match value.as_ref() {
            DynamicValue::Variant(name, Some(value)) if name == "at" => match value.as_ref() {
                DynamicValue::String(anchor) => anchor.trim_start_matches('#').parse().ok(),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    };
    Ok((pattern, timeout, from))
}
