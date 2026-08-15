//! Direct executable-file process resources.
//!
//! This module deliberately accepts an executable path and argv separately.
//! It never invokes a shell, parses shell syntax, allocates a PTY, or exposes
//! a semantic host API beyond the process itself.

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessSnapshot {
    pub uri: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub aborted: bool,
    pub cwd: Option<String>,
    pub environment: BTreeMap<String, String>,
}

/// Dynamic contract identities used by the ordinary process resource.
/// Bindings are supplied by the installed verb packages; the process manager
/// does not contain a closed list of Artist verbs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessVerbBindings {
    pub run: VerbId,
    pub send: VerbId,
    pub read: VerbId,
    pub poll: VerbId,
    pub abort: VerbId,
    pub delete: VerbId,
}

impl ProcessVerbBindings {
    /// Metadata for the installed process verb packages.  The process
    /// provider supplies execution; these definitions supply the same typed
    /// registry identity and routing contract used by component packages.
    pub fn definitions(&self) -> Vec<VerbDefinition> {
        let string = DynamicType::String;
        let uri = DynamicType::ResourceUri;
        let option_string = || DynamicType::Option(Box::new(DynamicType::String));
        let snapshot = DynamicType::Record(BTreeMap::from([
            ("uri".to_owned(), uri.clone()),
            ("output".to_owned(), string.clone()),
            (
                "exit-code".to_owned(),
                DynamicType::Option(Box::new(DynamicType::S32)),
            ),
            ("aborted".to_owned(), DynamicType::Bool),
            ("cwd".to_owned(), option_string()),
            (
                "environment".to_owned(),
                DynamicType::List(Box::new(DynamicType::Tuple(vec![
                    string.clone(),
                    string.clone(),
                ]))),
            ),
        ]));
        let run_input = DynamicType::Record(BTreeMap::from([
            ("executable".to_owned(), string.clone()),
            ("target".to_owned(), uri.clone()),
            (
                "args".to_owned(),
                DynamicType::List(Box::new(string.clone())),
            ),
            ("cwd".to_owned(), option_string()),
            (
                "environment".to_owned(),
                DynamicType::List(Box::new(DynamicType::Tuple(vec![
                    string.clone(),
                    string.clone(),
                ]))),
            ),
        ]));
        let content_input =
            DynamicType::Record(BTreeMap::from([("content".to_owned(), string.clone())]));
        let empty_input = DynamicType::Option(Box::new(string.clone()));
        vec![
            VerbDefinition::new(
                self.run.clone(),
                "run",
                "process-run",
                "Execute a direct executable-file process",
            )
            .with_contract(run_input, uri.clone())
            .with_extractor("resource-uri")
            .with_schema_adapter("wit"),
            VerbDefinition::new(
                self.send.clone(),
                "send",
                "process-send",
                "Send bytes to a process stdin",
            )
            .with_contract(content_input, uri.clone())
            .with_extractor("resource-uri")
            .with_schema_adapter("wit"),
            VerbDefinition::new(
                self.read.clone(),
                "read",
                "process-read",
                "Read a process snapshot",
            )
            .with_contract(empty_input.clone(), snapshot.clone())
            .with_extractor("resource-uri")
            .with_schema_adapter("wit"),
            VerbDefinition::new(
                self.poll.clone(),
                "poll",
                "process-poll",
                "Poll a process snapshot",
            )
            .with_contract(empty_input.clone(), snapshot)
            .with_extractor("resource-uri")
            .with_schema_adapter("wit"),
            VerbDefinition::new(
                self.abort.clone(),
                "abort",
                "process-abort",
                "Abort a process",
            )
            .with_contract(empty_input.clone(), uri.clone())
            .with_extractor("resource-uri")
            .with_schema_adapter("wit"),
            VerbDefinition::new(
                self.delete.clone(),
                "delete",
                "process-delete",
                "Delete a process resource",
            )
            .with_contract(empty_input, uri)
            .with_extractor("resource-uri")
            .with_schema_adapter("wit"),
        ]
    }
}

#[derive(Clone)]
pub struct ProcessResourceProvider {
    manager: ProcessManager,
    bindings: ProcessVerbBindings,
    capability: String,
}

struct ProcessState {
    child: Child,
    output: Arc<Mutex<Vec<u8>>>,
    aborted: bool,
    cwd: Option<String>,
    environment: BTreeMap<String, String>,
}

fn spawn_reader<R: Read + Send + 'static>(mut stream: R, output: Arc<Mutex<Vec<u8>>>) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(size) = stream.read(&mut buffer) {
            if size == 0 {
                break;
            }
            if let Ok(mut current) = output.lock() {
                current.extend_from_slice(&buffer[..size]);
            }
        }
    });
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

    pub fn run_with_capability(
        &self,
        capability: &str,
        executable: impl AsRef<Path>,
        args: &[String],
        cwd: Option<&Path>,
        environment: &[(String, String)],
    ) -> Result<String, KernelError> {
        if capability.trim().is_empty() {
            return Err(KernelError::PermissionDenied {
                uri: "process://capability".into(),
            });
        }
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
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        command.envs(environment.iter().map(|(key, value)| (key, value)));
        let mut child = command.spawn().map_err(|error| KernelError::Handler {
            message: format!("could not execute {}: {error}", executable.display()),
        })?;
        let output = Arc::new(Mutex::new(Vec::new()));
        if let Some(stdout) = child.stdout.take() {
            spawn_reader(stdout, output.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_reader(stderr, output.clone());
        }
        let mut next_id = self.next_id.lock().map_err(|_| KernelError::Handler {
            message: "process id lock poisoned".into(),
        })?;
        *next_id += 1;
        let uri = format!("process://{}", *next_id);
        drop(next_id);
        self.processes
            .lock()
            .map_err(|_| KernelError::Handler {
                message: "process registry lock poisoned".into(),
            })?
            .insert(
                uri.clone(),
                Arc::new(Mutex::new(ProcessState {
                    child,
                    output,
                    aborted: false,
                    cwd: cwd.map(|path| path.display().to_string()),
                    environment: environment.iter().cloned().collect(),
                })),
            );
        Ok(uri)
    }

    pub fn send(&self, uri: &str, input: &[u8]) -> Result<(), KernelError> {
        let process = self.lookup(uri)?;
        let mut process = process.lock().map_err(|_| KernelError::Handler {
            message: "process lock poisoned".into(),
        })?;
        process
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| KernelError::WrongKind {
                message: "process stdin is unavailable".into(),
            })?
            .write_all(input)
            .map_err(|error| KernelError::Handler {
                message: format!("could not write process stdin: {error}"),
            })
    }

    pub fn poll(&self, uri: &str) -> Result<ProcessSnapshot, KernelError> {
        let process = self.lookup(uri)?;
        let mut process = process.lock().map_err(|_| KernelError::Handler {
            message: "process lock poisoned".into(),
        })?;
        let exit_code = process
            .child
            .try_wait()
            .map_err(|error| KernelError::Handler {
                message: format!("could not poll process: {error}"),
            })?
            .and_then(|status| status.code());
        let output =
            String::from_utf8_lossy(&process.output.lock().map_err(|_| KernelError::Handler {
                message: "process output lock poisoned".into(),
            })?)
            .into_owned();
        Ok(ProcessSnapshot {
            uri: uri.to_owned(),
            output,
            exit_code,
            aborted: process.aborted,
            cwd: process.cwd.clone(),
            environment: process.environment.clone(),
        })
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
        let process = self
            .processes
            .lock()
            .map_err(|_| KernelError::Handler {
                message: "process registry lock poisoned".into(),
            })?
            .remove(uri)
            .ok_or_else(|| KernelError::NotFound {
                uri: uri.to_owned(),
            })?;
        if let Ok(mut process) = process.lock() {
            let _ = process.child.kill();
        }
        Ok(())
    }

    fn lookup(&self, uri: &str) -> Result<Arc<Mutex<ProcessState>>, KernelError> {
        self.processes
            .lock()
            .map_err(|_| KernelError::Handler {
                message: "process registry lock poisoned".into(),
            })?
            .get(uri)
            .cloned()
            .ok_or_else(|| KernelError::NotFound {
                uri: uri.to_owned(),
            })
    }
}

impl ProcessResourceProvider {
    pub fn new(
        manager: ProcessManager,
        bindings: ProcessVerbBindings,
        capability: impl Into<String>,
    ) -> Self {
        Self {
            manager,
            bindings,
            capability: capability.into(),
        }
    }

    fn is_process_uri(uri: &ResourceUri) -> bool {
        uri.scheme() == "process"
    }

    fn string_field(input: &DynamicValue, field: &str) -> Result<String, KernelError> {
        let DynamicValue::Record(fields) = input else {
            return Err(KernelError::InvalidRequest {
                message: "process input must be a typed record".into(),
            });
        };
        match fields.get(field) {
            Some(DynamicValue::String(value)) => Ok(value.clone()),
            Some(DynamicValue::ResourceUri(value)) => Ok(value.to_string()),
            _ => Err(KernelError::InvalidRequest {
                message: format!("process input field {field} must be a string"),
            }),
        }
    }

    fn optional_string_field(
        input: &DynamicValue,
        field: &str,
    ) -> Result<Option<String>, KernelError> {
        let DynamicValue::Record(fields) = input else {
            return Err(KernelError::InvalidRequest {
                message: "process input must be a typed record".into(),
            });
        };
        match fields.get(field) {
            Some(DynamicValue::Option(None)) | None => Ok(None),
            Some(DynamicValue::Option(Some(value))) => match value.as_ref() {
                DynamicValue::String(value) => Ok(Some(value.clone())),
                DynamicValue::ResourceUri(value) => Ok(Some(value.to_string())),
                _ => Err(KernelError::InvalidRequest {
                    message: format!("process input field {field} must contain a string"),
                }),
            },
            Some(DynamicValue::String(value)) => Ok(Some(value.clone())),
            _ => Err(KernelError::InvalidRequest {
                message: format!("process input field {field} must be optional"),
            }),
        }
    }

    fn arguments(input: &DynamicValue) -> Result<Vec<String>, KernelError> {
        let DynamicValue::Record(fields) = input else {
            return Err(KernelError::InvalidRequest {
                message: "process input must be a typed record".into(),
            });
        };
        let Some(DynamicValue::List(values)) = fields.get("args") else {
            return Ok(Vec::new());
        };
        values
            .iter()
            .map(|value| match value {
                DynamicValue::String(value) => Ok(value.clone()),
                _ => Err(KernelError::InvalidRequest {
                    message: "process args must be a list of strings".into(),
                }),
            })
            .collect()
    }

    fn environment(input: &DynamicValue) -> Result<Vec<(String, String)>, KernelError> {
        let DynamicValue::Record(fields) = input else {
            return Err(KernelError::InvalidRequest {
                message: "process input must be a typed record".into(),
            });
        };
        let Some(DynamicValue::List(values)) = fields.get("environment") else {
            return Ok(Vec::new());
        };
        values
            .iter()
            .map(|value| match value {
                DynamicValue::Tuple(values) if values.len() == 2 => {
                    let (DynamicValue::String(name), DynamicValue::String(value)) =
                        (&values[0], &values[1])
                    else {
                        return Err(KernelError::InvalidRequest {
                            message: "process environment entries must be string tuples".into(),
                        });
                    };
                    Ok((name.clone(), value.clone()))
                }
                _ => Err(KernelError::InvalidRequest {
                    message: "process environment must be a list of string tuples".into(),
                }),
            })
            .collect()
    }

    fn snapshot_value(snapshot: ProcessSnapshot) -> DynamicValue {
        DynamicValue::Record(std::collections::BTreeMap::from([
            (
                "uri".into(),
                DynamicValue::ResourceUri(
                    ResourceUri::parse(&snapshot.uri).expect("process URI is authoritative"),
                ),
            ),
            ("output".into(), DynamicValue::String(snapshot.output)),
            (
                "exit-code".into(),
                DynamicValue::Option(
                    snapshot
                        .exit_code
                        .map(|value| Box::new(DynamicValue::S32(value))),
                ),
            ),
            ("aborted".into(), DynamicValue::Bool(snapshot.aborted)),
            (
                "cwd".into(),
                DynamicValue::Option(
                    snapshot
                        .cwd
                        .map(|value| Box::new(DynamicValue::String(value))),
                ),
            ),
            (
                "environment".into(),
                DynamicValue::List(
                    snapshot
                        .environment
                        .into_iter()
                        .map(|(name, value)| {
                            DynamicValue::Tuple(vec![
                                DynamicValue::String(name),
                                DynamicValue::String(value),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]))
    }
}

impl DynamicClaimProvider for ProcessResourceProvider {
    fn claim(&self, verb: &VerbId, uri: &ResourceUri) -> ClaimDecision {
        if verb == &self.bindings.run {
            return if uri.scheme() == "file" {
                ClaimDecision::Handle
            } else {
                ClaimDecision::Pass
            };
        }
        if [
            &self.bindings.send,
            &self.bindings.read,
            &self.bindings.poll,
            &self.bindings.abort,
            &self.bindings.delete,
        ]
        .contains(&verb)
            && Self::is_process_uri(uri)
        {
            ClaimDecision::Handle
        } else {
            ClaimDecision::Pass
        }
    }
}

impl DynamicResourceProvider for ProcessResourceProvider {
    fn invoke<'a>(
        &'a self,
        verb: &'a VerbId,
        uri: &'a ResourceUri,
        input: DynamicValue,
    ) -> ResourceFuture<'a> {
        let manager = self.manager.clone();
        let bindings = self.bindings.clone();
        let capability = self.capability.clone();
        Box::pin(async move {
            let output = if verb == &bindings.run {
                let executable = Self::string_field(&input, "executable")?;
                let cwd = Self::optional_string_field(&input, "cwd")?;
                let cwd = cwd.as_deref().map(Path::new);
                let args = Self::arguments(&input)?;
                let environment = Self::environment(&input)?;
                let process = manager.run_with_capability(
                    &capability,
                    executable,
                    &args,
                    cwd,
                    &environment,
                )?;
                DynamicValue::ResourceUri(ResourceUri::parse(&process)?)
            } else if verb == &bindings.send {
                let content = match &input {
                    DynamicValue::String(value) => value.clone(),
                    _ => Self::string_field(&input, "content")?,
                };
                manager.send(uri.as_ref().to_string().as_str(), content.as_bytes())?;
                DynamicValue::ResourceUri(uri.clone())
            } else if verb == &bindings.read || verb == &bindings.poll {
                Self::snapshot_value(manager.poll(uri.as_ref().to_string().as_str())?)
            } else if verb == &bindings.abort {
                manager.abort(uri.as_ref().to_string().as_str())?;
                DynamicValue::ResourceUri(uri.clone())
            } else if verb == &bindings.delete {
                manager.delete(uri.as_ref().to_string().as_str())?;
                DynamicValue::ResourceUri(uri.clone())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn bindings() -> ProcessVerbBindings {
        ProcessVerbBindings {
            run: VerbId::new("artist:process/run@1.0.0").unwrap(),
            send: VerbId::new("artist:process/send@1.0.0").unwrap(),
            read: VerbId::new("artist:process/read@1.0.0").unwrap(),
            poll: VerbId::new("artist:process/poll@1.0.0").unwrap(),
            abort: VerbId::new("artist:process/abort@1.0.0").unwrap(),
            delete: VerbId::new("artist:process/delete@1.0.0").unwrap(),
        }
    }

    #[test]
    fn process_execution_requires_a_capability_identity() {
        let manager = ProcessManager::new();
        assert!(matches!(
            manager.run_with_capability("", "/bin/true", &[], None, &[]),
            Err(KernelError::PermissionDenied { .. })
        ));
    }

    #[test]
    fn executes_direct_file_without_shell_and_supports_process_lifecycle() {
        let manager = ProcessManager::new();
        let uri = manager
            .run_with_capability("test.process", "/bin/cat", &[], None, &[])
            .expect("cat executable should be available");
        manager.send(&uri, b"direct process\n").unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert!(
            manager
                .poll(&uri)
                .unwrap()
                .output
                .contains("direct process")
        );
        manager.abort(&uri).unwrap();
        assert!(manager.poll(&uri).unwrap().aborted);
        manager.delete(&uri).unwrap();
        assert!(matches!(
            manager.poll(&uri),
            Err(KernelError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn process_lifecycle_is_available_through_dynamic_resource_provider() {
        let manager = ProcessManager::new();
        let bindings = bindings();
        let provider =
            ProcessResourceProvider::new(manager.clone(), bindings.clone(), "test.process");
        let run_uri = ResourceUri::parse("/bin/cat").unwrap();
        let input = DynamicValue::Record(std::collections::BTreeMap::from([
            ("executable".into(), DynamicValue::String("/bin/cat".into())),
            ("args".into(), DynamicValue::List(Vec::new())),
            ("cwd".into(), DynamicValue::Option(None)),
            ("environment".into(), DynamicValue::List(Vec::new())),
        ]));
        let result = provider
            .invoke(&bindings.run, &run_uri, input)
            .await
            .unwrap();
        let DynamicValue::ResourceUri(process_uri) = result.output else {
            panic!("run did not return a process URI");
        };
        provider
            .invoke(
                &bindings.send,
                &process_uri,
                DynamicValue::String("dynamic process\n".into()),
            )
            .await
            .unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let snapshot = provider
            .invoke(&bindings.read, &process_uri, DynamicValue::Option(None))
            .await
            .unwrap();
        assert!(matches!(snapshot.output, DynamicValue::Record(_)));
        provider
            .invoke(&bindings.abort, &process_uri, DynamicValue::Option(None))
            .await
            .unwrap();
        provider
            .invoke(&bindings.delete, &process_uri, DynamicValue::Option(None))
            .await
            .unwrap();
        assert!(manager.poll(&process_uri.to_string()).is_err());
    }
}
