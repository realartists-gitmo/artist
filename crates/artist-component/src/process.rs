//! Ephemeral component-owned process resources.
//!
//! The kernel does not launch or supervise children. This module owns the
//! short-lived process handles and exposes their I/O through the ordinary
//! resource provider and model-tool contracts. Nothing in this module is
//! persisted or reconstructed after a daemon restart.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use artist_kernel::provider::{
    ProviderAttrs, ProviderEntry, ResourceError, ResourceErrorCode, ResourceProvider,
};
use artist_kernel::{Kernel, ResourceUri};
use artist_wasm_verbs::process::{
    PollRequest, PollResponse, ProcessError, ProcessLauncher, ProcessPoller, ProcessSignaler,
    RunRequest, RunResponse, SignalRequest,
};
use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex as AsyncMutex, mpsc};

use crate::{ComponentToolRegistry, ToolComponent, ToolError};

static NEXT_PROCESS_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Default)]
pub struct ProcessRegistry {
    processes: Arc<RwLock<BTreeMap<String, Arc<ProcessEntry>>>>,
    file_root: Option<Arc<PathBuf>>,
}

struct ProcessEntry {
    child: AsyncMutex<Child>,
    stdin: AsyncMutex<Option<ChildStdin>>,
    output: AsyncMutex<OutputState>,
    incoming: AsyncMutex<mpsc::UnboundedReceiver<OutputChunk>>,
}

#[derive(Default)]
struct OutputState {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    reported_stdout: usize,
    reported_stderr: usize,
}

enum OutputChunk {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
}

#[derive(Clone, Copy)]
enum StreamKind {
    Stdout,
    Stderr,
}

impl ProcessRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind `file://` working-directory resources to the same workspace root
    /// as the host's file provider. A model-facing URI is therefore never
    /// interpreted as an arbitrary host absolute path by the process
    /// component.
    pub fn with_file_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            file_root: Some(Arc::new(std::fs::canonicalize(&root).unwrap_or(root))),
            ..Self::default()
        }
    }

    pub fn install(&self, kernel: &Arc<Kernel>) {
        kernel.register_resource_provider(self.clone());
    }

    pub fn install_tools(&self, tools: &ComponentToolRegistry) -> Result<(), ToolError> {
        for name in ["run", "poll", "signal"] {
            if !tools
                .all_names()
                .iter()
                .any(|registered| registered == name)
            {
                tools.register(ProcessTool {
                    name: name.to_owned(),
                    processes: self.clone(),
                })?;
            }
        }
        Ok(())
    }

    async fn launch_process(&self, request: RunRequest) -> Result<RunResponse, ProcessError> {
        let executable = request
            .executable
            .or(request.target)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ProcessError::InvalidArgument("run requires an executable".into()))?;

        let mut command = Command::new(&executable);
        command.args(&request.arguments);
        if let Some(directory) = request.working_directory.as_deref() {
            command.current_dir(self.working_directory(directory)?);
        }
        for (name, value) in request.environment {
            if name.trim().is_empty() {
                return Err(ProcessError::InvalidArgument(
                    "environment variable names cannot be empty".into(),
                ));
            }
            command.env(name, value);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| {
            ProcessError::Internal(format!("could not start {executable:?}: {error}"))
        })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (sender, receiver) = mpsc::unbounded_channel();
        if let Some(stdout) = stdout {
            spawn_reader(stdout, sender.clone(), StreamKind::Stdout);
        }
        if let Some(stderr) = stderr {
            spawn_reader(stderr, sender, StreamKind::Stderr);
        }

        let id = format!("p-{}", NEXT_PROCESS_ID.fetch_add(1, Ordering::Relaxed));
        let entry = Arc::new(ProcessEntry {
            child: AsyncMutex::new(child),
            stdin: AsyncMutex::new(stdin),
            output: AsyncMutex::new(OutputState::default()),
            incoming: AsyncMutex::new(receiver),
        });
        self.processes.write().unwrap().insert(id.clone(), entry);
        Ok(RunResponse {
            process: process_uri(&id),
            stdin: stream_uri(&id, "stdin"),
            stdout: stream_uri(&id, "stdout"),
            stderr: stream_uri(&id, "stderr"),
            status: "running".into(),
        })
    }

    fn working_directory(&self, value: &str) -> Result<PathBuf, ProcessError> {
        if value.trim().is_empty() {
            return Err(ProcessError::InvalidArgument(
                "working_directory cannot be empty".into(),
            ));
        }
        if let Ok(uri) = value.parse::<ResourceUri>() {
            if uri.scheme() != "file"
                || !uri.authority().is_empty()
                || uri.query().is_some()
                || uri.fragment().is_some()
            {
                return Err(ProcessError::Unsupported(
                    "process working directories require a plain file:// resource".into(),
                ));
            }
            let Some(root) = &self.file_root else {
                return Ok(PathBuf::from(uri.path()));
            };
            let mut path = root.as_ref().clone();
            for segment in uri.decoded_segments().map_err(|error| {
                ProcessError::InvalidArgument(format!("invalid working directory: {error}"))
            })? {
                path.push(segment);
            }
            return Ok(path);
        }
        let path = PathBuf::from(value);
        if let Some(root) = &self.file_root {
            if path.is_absolute() {
                return Err(ProcessError::InvalidArgument(
                    "working_directory must be a file:// resource inside the workspace".into(),
                ));
            }
            return Ok(root.join(path));
        }
        Ok(path)
    }

    fn entry(&self, id: &str) -> Result<Arc<ProcessEntry>, ProcessError> {
        self.processes
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| ProcessError::NotFound(format!("process {id:?} was not found")))
    }

    async fn drain_output(entry: &Arc<ProcessEntry>) {
        let mut incoming = entry.incoming.lock().await;
        let mut output = entry.output.lock().await;
        while let Ok(chunk) = incoming.try_recv() {
            match chunk {
                OutputChunk::Stdout(bytes) => output.stdout.extend(bytes),
                OutputChunk::Stderr(bytes) => output.stderr.extend(bytes),
            }
        }
    }

    async fn status(entry: &Arc<ProcessEntry>) -> Result<String, ProcessError> {
        let mut child = entry.child.lock().await;
        child
            .try_wait()
            .map_err(|error| ProcessError::Internal(format!("could not poll process: {error}")))
            .map(|status| match status {
                Some(status) => format!("exited:{}", status.code().unwrap_or(-1)),
                None => "running".into(),
            })
    }

    async fn read_stream(
        &self,
        id: &str,
        stream: StreamKind,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        let entry = self.entry(id).map_err(process_resource_error)?;
        Self::drain_output(&entry).await;
        let output = entry.output.lock().await;
        let bytes = match stream {
            StreamKind::Stdout => &output.stdout,
            StreamKind::Stderr => &output.stderr,
        };
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let end = start.saturating_add(size as usize).min(bytes.len());
        Ok(bytes[start..end].to_vec())
    }

    async fn stream_len(&self, id: &str, stream: StreamKind) -> Result<u64, ResourceError> {
        let entry = self.entry(id).map_err(process_resource_error)?;
        Self::drain_output(&entry).await;
        let output = entry.output.lock().await;
        Ok(match stream {
            StreamKind::Stdout => output.stdout.len(),
            StreamKind::Stderr => output.stderr.len(),
        } as u64)
    }

    async fn status_bytes(&self, id: &str) -> Result<Vec<u8>, ResourceError> {
        let entry = self.entry(id).map_err(process_resource_error)?;
        Ok(Self::status(&entry)
            .await
            .map_err(process_resource_error)?
            .into_bytes())
    }

    async fn write_stream(&self, id: &str, data: &[u8]) -> Result<u32, ResourceError> {
        let entry = self.entry(id).map_err(process_resource_error)?;
        let mut stdin = entry.stdin.lock().await;
        let Some(stdin) = stdin.as_mut() else {
            return Err(ResourceError::new(
                ResourceErrorCode::Conflict,
                "process stdin is closed",
            ));
        };
        stdin.write_all(data).await.map_err(|error| {
            ResourceError::new(
                ResourceErrorCode::Io,
                format!("could not write process stdin: {error}"),
            )
        })?;
        stdin.flush().await.map_err(|error| {
            ResourceError::new(
                ResourceErrorCode::Io,
                format!("could not flush process stdin: {error}"),
            )
        })?;
        u32::try_from(data.len()).map_err(|_| {
            ResourceError::new(
                ResourceErrorCode::InvalidAddress,
                "stdin write is too large",
            )
        })
    }

    async fn poll_process(&self, request: PollRequest) -> Result<PollResponse, ProcessError> {
        let (id, stream) = process_target(&request.target)?;
        let entry = self.entry(&id)?;
        let deadline = Instant::now() + Duration::from_millis(request.timeout_ms.min(60_000));
        loop {
            Self::drain_output(&entry).await;
            let new_output = {
                let mut output = entry.output.lock().await;
                match stream {
                    Some(StreamKind::Stdout) => {
                        let bytes = output.stdout[output.reported_stdout..].to_vec();
                        output.reported_stdout = output.stdout.len();
                        bytes
                    }
                    Some(StreamKind::Stderr) => {
                        let bytes = output.stderr[output.reported_stderr..].to_vec();
                        output.reported_stderr = output.stderr.len();
                        bytes
                    }
                    None => {
                        let stdout = output.stdout[output.reported_stdout..].to_vec();
                        let stderr = output.stderr[output.reported_stderr..].to_vec();
                        output.reported_stdout = output.stdout.len();
                        output.reported_stderr = output.stderr.len();
                        let mut combined = stdout;
                        combined.extend(stderr);
                        combined
                    }
                }
            };
            let status = Self::status(&entry).await?;
            let text = String::from_utf8_lossy(&new_output).into_owned();
            let matched = request
                .r#match
                .as_deref()
                .is_some_and(|needle| text.contains(needle));
            if matched || !new_output.is_empty() || status != "running" {
                return Ok(PollResponse {
                    target: request.target.clone(),
                    event: if matched {
                        "match"
                    } else if !new_output.is_empty() {
                        "output"
                    } else {
                        "exit"
                    }
                    .into(),
                    matched,
                    content: (!text.is_empty()).then_some(text),
                });
            }
            if Instant::now() >= deadline {
                return Ok(PollResponse {
                    target: request.target.clone(),
                    event: "timeout".into(),
                    matched: false,
                    content: None,
                });
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn signal_process(&self, request: SignalRequest) -> Result<(), ProcessError> {
        let id = process_id(&request.process)?;
        let entry = self.entry(&id)?;
        match request.signal.as_str() {
            "cancel" | "terminate" | "kill" => {
                entry.child.lock().await.start_kill().map_err(|error| {
                    ProcessError::Internal(format!("could not terminate process: {error}"))
                })?;
                Ok(())
            }
            "interrupt" => Err(ProcessError::Unsupported(
                "interrupt is not portable through this process component".into(),
            )),
            other => Err(ProcessError::Unsupported(format!(
                "unsupported process signal {other:?}"
            ))),
        }
    }

    fn kind(&self, uri: &ResourceUri) -> Result<ProcessResourceKind, ResourceError> {
        if !self.eligible(uri) {
            return Err(ResourceError::not_found(uri));
        }
        let segments: Vec<_> = uri.segments().collect();
        if uri.authority().is_empty() {
            return if segments.is_empty() {
                Ok(ProcessResourceKind::Directory)
            } else {
                Err(ResourceError::not_found(uri))
            };
        }
        if !self.processes.read().unwrap().contains_key(uri.authority()) {
            return Err(ResourceError::not_found(uri));
        }
        match segments.as_slice() {
            [] => Ok(ProcessResourceKind::Directory),
            ["stdin"] => Ok(ProcessResourceKind::Stdin),
            ["stdout"] => Ok(ProcessResourceKind::Stdout),
            ["stderr"] => Ok(ProcessResourceKind::Stderr),
            ["status"] => Ok(ProcessResourceKind::Status),
            _ => Err(ResourceError::not_found(uri)),
        }
    }
}

#[async_trait]
impl ProcessLauncher for ProcessRegistry {
    async fn launch(&self, request: RunRequest) -> Result<RunResponse, ProcessError> {
        self.launch_process(request).await
    }
}

#[async_trait]
impl ProcessPoller for ProcessRegistry {
    async fn poll(&self, request: PollRequest) -> Result<PollResponse, ProcessError> {
        self.poll_process(request).await
    }
}

#[async_trait]
impl ProcessSignaler for ProcessRegistry {
    async fn signal(&self, request: SignalRequest) -> Result<(), ProcessError> {
        self.signal_process(request).await
    }
}

#[derive(Clone, Copy)]
enum ProcessResourceKind {
    Directory,
    Stdin,
    Stdout,
    Stderr,
    Status,
}

#[async_trait]
impl ResourceProvider for ProcessRegistry {
    fn provider_name(&self) -> &str {
        "component-processes"
    }

    fn eligible(&self, uri: &ResourceUri) -> bool {
        uri.scheme() == "process"
    }

    fn priority(&self) -> u32 {
        20
    }

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        let mtime = SystemTime::now();
        match self.kind(uri)? {
            ProcessResourceKind::Directory => Ok(ProviderAttrs::directory()),
            ProcessResourceKind::Stdin => Ok(ProviderAttrs::file(0, mtime)),
            ProcessResourceKind::Stdout => Ok(ProviderAttrs::file(
                self.stream_len(uri.authority(), StreamKind::Stdout).await?,
                mtime,
            )),
            ProcessResourceKind::Stderr => Ok(ProviderAttrs::file(
                self.stream_len(uri.authority(), StreamKind::Stderr).await?,
                mtime,
            )),
            ProcessResourceKind::Status => Ok(ProviderAttrs::file(
                self.status_bytes(uri.authority()).await?.len() as u64,
                mtime,
            )),
        }
    }

    async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError> {
        match self.kind(uri)? {
            ProcessResourceKind::Directory if uri.authority().is_empty() => Ok(self
                .processes
                .read()
                .unwrap()
                .keys()
                .map(|id| ProviderEntry {
                    name: id.clone(),
                    attrs: ProviderAttrs::directory(),
                })
                .collect()),
            ProcessResourceKind::Directory => Ok(["stdin", "stdout", "stderr", "status"]
                .into_iter()
                .map(|name| ProviderEntry {
                    name: name.into(),
                    attrs: ProviderAttrs::file(0, SystemTime::now()),
                })
                .collect()),
            _ => Err(ResourceError::new(
                ResourceErrorCode::NotDir,
                "process stream is not a directory",
            )),
        }
    }

    async fn read(
        &self,
        uri: &ResourceUri,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        match self.kind(uri)? {
            ProcessResourceKind::Stdout => {
                self.read_stream(uri.authority(), StreamKind::Stdout, offset, size)
                    .await
            }
            ProcessResourceKind::Stderr => {
                self.read_stream(uri.authority(), StreamKind::Stderr, offset, size)
                    .await
            }
            ProcessResourceKind::Status => {
                let bytes = self.status_bytes(uri.authority()).await?;
                let start = usize::try_from(offset)
                    .unwrap_or(usize::MAX)
                    .min(bytes.len());
                let end = start.saturating_add(size as usize).min(bytes.len());
                Ok(bytes[start..end].to_vec())
            }
            ProcessResourceKind::Stdin | ProcessResourceKind::Directory => Err(ResourceError::new(
                ResourceErrorCode::Unsupported,
                "process stdin is write-only",
            )),
        }
    }

    async fn write(
        &self,
        uri: &ResourceUri,
        _offset: u64,
        data: &[u8],
    ) -> Result<u32, ResourceError> {
        if !matches!(self.kind(uri)?, ProcessResourceKind::Stdin) {
            return Err(ResourceError::new(
                ResourceErrorCode::Unsupported,
                "only process stdin is writable",
            ));
        }
        self.write_stream(uri.authority(), data).await
    }
}

struct ProcessTool {
    name: String,
    processes: ProcessRegistry,
}

#[async_trait]
impl ToolComponent for ProcessTool {
    fn name(&self) -> &str {
        &self.name
    }

    async fn invoke(&self, request: &[u8]) -> Result<Vec<u8>, ToolError> {
        let text = std::str::from_utf8(request).map_err(|error| {
            ToolError::InvalidArgument(format!("process request is not UTF-8: {error}"))
        })?;
        let value: Value = toon_format::decode_default(text).map_err(|error| {
            ToolError::InvalidArgument(format!("invalid process request: {error}"))
        })?;
        let output: Result<Value, ToolError> = match self.name.as_str() {
            "run" => {
                let request: RunRequest = serde_json::from_value(value)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
                let response = self
                    .processes
                    .launch(request)
                    .await
                    .map_err(process_tool_error)?;
                serde_json::to_value(response)
                    .map_err(|error| ToolError::Internal(error.to_string()))
            }
            "poll" => {
                let request: PollRequest = serde_json::from_value(value)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
                let response = self
                    .processes
                    .poll(request)
                    .await
                    .map_err(process_tool_error)?;
                serde_json::to_value(response)
                    .map_err(|error| ToolError::Internal(error.to_string()))
            }
            "signal" => {
                let request: SignalRequest = serde_json::from_value(value)
                    .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
                self.processes
                    .signal(request)
                    .await
                    .map_err(process_tool_error)?;
                Ok(Value::Object(Default::default()))
            }
            _ => Err(ToolError::NotFound(self.name.clone())),
        };
        serde_json::to_vec(&output?).map_err(|error| ToolError::Internal(error.to_string()))
    }
}

fn spawn_reader<R>(mut reader: R, sender: mpsc::UnboundedSender<OutputChunk>, stream: StreamKind)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = vec![0; 16 * 1024];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(size) => {
                    let chunk = buffer[..size].to_vec();
                    let result = match stream {
                        StreamKind::Stdout => sender.send(OutputChunk::Stdout(chunk)),
                        StreamKind::Stderr => sender.send(OutputChunk::Stderr(chunk)),
                    };
                    if result.is_err() {
                        break;
                    }
                }
            }
        }
    });
}

fn process_id(value: &str) -> Result<String, ProcessError> {
    if let Ok(uri) = value.parse::<ResourceUri>() {
        if uri.scheme() != "process"
            || uri.authority().is_empty()
            || uri.segments().next().is_some()
        {
            return Err(ProcessError::InvalidArgument(format!(
                "invalid process resource {value:?}"
            )));
        }
        return Ok(uri.authority().to_owned());
    }
    if value.trim().is_empty() || value.contains('/') || value.contains('\\') {
        return Err(ProcessError::InvalidArgument(format!(
            "invalid process handle {value:?}"
        )));
    }
    Ok(value.to_owned())
}

fn process_target(value: &str) -> Result<(String, Option<StreamKind>), ProcessError> {
    if let Ok(uri) = value.parse::<ResourceUri>() {
        if uri.scheme() != "process" || uri.authority().is_empty() {
            return Err(ProcessError::InvalidArgument(format!(
                "invalid process resource {value:?}"
            )));
        }
        let segments: Vec<_> = uri.segments().collect();
        let stream = match segments.as_slice() {
            [] | ["status"] => None,
            ["stdout"] => Some(StreamKind::Stdout),
            ["stderr"] => Some(StreamKind::Stderr),
            _ => {
                return Err(ProcessError::InvalidArgument(format!(
                    "invalid process poll resource {value:?}"
                )));
            }
        };
        return Ok((uri.authority().to_owned(), stream));
    }
    Ok((process_id(value)?, None))
}

fn process_uri(id: &str) -> String {
    format!("process://{id}")
}

fn stream_uri(id: &str, stream: &str) -> String {
    format!("process://{id}/{stream}")
}

fn process_resource_error(error: ProcessError) -> ResourceError {
    let (code, message) = match error {
        ProcessError::InvalidArgument(message) => (ResourceErrorCode::InvalidAddress, message),
        ProcessError::NotFound(message) => (ResourceErrorCode::NotFound, message),
        ProcessError::Unsupported(message) => (ResourceErrorCode::Unsupported, message),
        ProcessError::PermissionDenied(message) => (ResourceErrorCode::PermissionDenied, message),
        ProcessError::Conflict(message) => (ResourceErrorCode::Conflict, message),
        ProcessError::Aborted(message) => (ResourceErrorCode::Cancelled, message),
        ProcessError::Internal(message) => (ResourceErrorCode::Io, message),
    };
    ResourceError::new(code, message)
}

fn process_tool_error(error: ProcessError) -> ToolError {
    match error {
        ProcessError::InvalidArgument(message) => ToolError::InvalidArgument(message),
        ProcessError::NotFound(message) => ToolError::NotFound(message),
        ProcessError::Unsupported(message) => ToolError::Unsupported(message),
        ProcessError::PermissionDenied(message) => ToolError::PermissionDenied(message),
        ProcessError::Conflict(message) => ToolError::Conflict(message),
        ProcessError::Aborted(message) => ToolError::Aborted(message),
        ProcessError::Internal(message) => ToolError::Internal(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_polls_and_exposes_resource_streams() {
        let registry = ProcessRegistry::new();
        let response = registry
            .launch(RunRequest {
                executable: Some("sh".into()),
                target: None,
                arguments: vec!["-c".into(), "printf hello".into()],
                working_directory: None,
                environment: BTreeMap::new(),
            })
            .await
            .unwrap();
        let poll = registry
            .poll(PollRequest {
                target: response.process.clone(),
                r#match: Some("hello".into()),
                timeout_ms: 1_000,
            })
            .await
            .unwrap();
        assert!(poll.matched);
        assert_eq!(poll.content.as_deref(), Some("hello"));
        let stream_response = registry
            .launch(RunRequest {
                executable: Some("sh".into()),
                target: None,
                arguments: vec!["-c".into(), "printf hello".into()],
                working_directory: None,
                environment: BTreeMap::new(),
            })
            .await
            .unwrap();
        let stream_poll = registry
            .poll(PollRequest {
                target: stream_response.stdout.clone(),
                r#match: Some("hello".into()),
                timeout_ms: 1_000,
            })
            .await
            .unwrap();
        assert!(stream_poll.matched);
        assert_eq!(stream_poll.target, stream_response.stdout);
        let uri: ResourceUri = response.stdout.parse().unwrap();
        assert_eq!(registry.read(&uri, 0, 100).await.unwrap(), b"hello");
    }

    #[tokio::test]
    async fn unsupported_interrupt_is_explicit() {
        let registry = ProcessRegistry::new();
        let response = registry
            .launch(RunRequest {
                executable: Some("sh".into()),
                target: None,
                arguments: vec!["-c".into(), "sleep 1".into()],
                working_directory: None,
                environment: BTreeMap::new(),
            })
            .await
            .unwrap();
        assert!(matches!(
            registry
                .signal(SignalRequest {
                    process: response.process.clone(),
                    signal: "interrupt".into(),
                })
                .await,
            Err(ProcessError::Unsupported(_))
        ));
        registry
            .signal(SignalRequest {
                process: response.process,
                signal: "terminate".into(),
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn file_working_directories_are_resolved_inside_the_component_root() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let registry = ProcessRegistry::with_file_root(directory.path());
        let response = registry
            .launch(RunRequest {
                executable: Some("sh".into()),
                target: None,
                arguments: vec!["-c".into(), "pwd".into()],
                working_directory: Some("file:///nested".into()),
                environment: BTreeMap::new(),
            })
            .await
            .unwrap();
        let output = registry
            .poll(PollRequest {
                target: response.stdout,
                r#match: None,
                timeout_ms: 1_000,
            })
            .await
            .unwrap();
        assert_eq!(
            output.content.as_deref().map(str::trim),
            Some(nested.to_str().unwrap())
        );
    }
}
