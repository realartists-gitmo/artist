//! The component-owned `agent://` resource implementation.
//!
//! Agent resources are ordinary URL resources.  The kernel only knows how to
//! route a `ResourceSignal` or a byte write; this component owns transcript,
//! process-channel, and lifecycle semantics.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use artist_kernel::{
    Kernel, ProviderAttrs, ProviderEntry, ResourceError, ResourceErrorCode, ResourceProvider,
    ResourceSignal, ResourceUri,
};
use artist_session::{EventLog, LogError};
use async_trait::async_trait;

const SCHEME: &str = "agent";
const TRANSCRIPT: &str = "transcript";
const STDIN: &str = "stdin";
const STDOUT: &str = "stdout";

/// URL-graph activation seam for the agent resource component. The host may
/// keep the component instance alive for registrations, but the kernel route
/// exists only while an active composition claims `agent://...`.
pub struct AgentResourceSocket;

impl AgentResourceSocket {
    pub fn sync_for_routes(
        routes: &std::collections::BTreeSet<String>,
        kernel: &Kernel,
        component: Arc<AgentResourceComponent>,
    ) {
        if routes.iter().any(|route| route.starts_with("agent://")) {
            kernel.register_shared_resource_provider(component);
        } else {
            kernel.unregister_resource_provider(SCHEME);
        }
    }
}

#[async_trait]
pub trait AgentTranscript: Send + Sync {
    async fn read(&self) -> Result<Vec<u8>, ResourceError>;
    async fn append(
        &self,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<(), ResourceError>;
    async fn close(&self) -> Result<(), ResourceError>;
    fn is_closed(&self) -> bool;
}

#[async_trait]
pub trait AgentProcess: Send + Sync {
    async fn write_stdin(&self, data: &[u8]) -> Result<u32, ResourceError>;
    async fn read_stdout(&self, offset: u64, size: u32) -> Result<Vec<u8>, ResourceError>;

    async fn signal(&self, _signal: ResourceSignal) -> Result<(), ResourceError> {
        Err(ResourceError::new(
            ResourceErrorCode::Unsupported,
            "agent process does not support this signal",
        ))
    }
}

#[derive(Default)]
pub struct AgentResourceComponent {
    transcripts: RwLock<BTreeMap<String, Arc<dyn AgentTranscript>>>,
    processes: RwLock<BTreeMap<String, Arc<dyn AgentProcess>>>,
}

impl AgentResourceComponent {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_transcript<T: AgentTranscript + 'static>(
        &self,
        agent: impl Into<String>,
        transcript: T,
    ) -> Result<(), ResourceError> {
        let agent = validate_segment(agent.into())?;
        self.transcripts
            .write()
            .unwrap()
            .insert(agent, Arc::new(transcript));
        Ok(())
    }

    pub fn register_process<T: AgentProcess + 'static>(
        &self,
        agent: impl Into<String>,
        process: T,
    ) -> Result<(), ResourceError> {
        let agent = validate_segment(agent.into())?;
        self.processes
            .write()
            .unwrap()
            .insert(agent, Arc::new(process));
        Ok(())
    }

    pub async fn append(
        &self,
        uri: &ResourceUri,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<(), ResourceError> {
        self.transcript_for(uri)?.append(event_type, payload).await
    }

    pub async fn close(&self, uri: &ResourceUri) -> Result<(), ResourceError> {
        self.transcript_for(uri)?.close().await
    }

    pub async fn write_stdin(&self, uri: &ResourceUri, data: &[u8]) -> Result<u32, ResourceError> {
        let agent = channel_key(uri, STDIN)?;
        self.process_for(&agent, uri)?.write_stdin(data).await
    }

    fn transcript_for(&self, uri: &ResourceUri) -> Result<Arc<dyn AgentTranscript>, ResourceError> {
        let agent = transcript_key(uri)?;
        self.transcripts
            .read()
            .unwrap()
            .get(&agent)
            .cloned()
            .ok_or_else(|| ResourceError::not_found(uri))
    }

    fn process_for(
        &self,
        agent: &str,
        uri: &ResourceUri,
    ) -> Result<Arc<dyn AgentProcess>, ResourceError> {
        self.processes
            .read()
            .unwrap()
            .get(agent)
            .cloned()
            .ok_or_else(|| ResourceError::not_found(uri))
    }

    fn kind(&self, uri: &ResourceUri) -> Result<ResourceKind, ResourceError> {
        if !self.eligible(uri) {
            return Err(ResourceError::not_found(uri));
        }
        let segments: Vec<_> = uri.segments().collect();
        let agent = uri.authority();
        match segments.as_slice() {
            [] if agent.is_empty() => Ok(ResourceKind::Directory),
            [] if self.has_agent(agent) => Ok(ResourceKind::AgentRoot),
            [name] if *name == TRANSCRIPT && self.has_transcript(agent) => {
                Ok(ResourceKind::Transcript)
            }
            [name] if *name == STDIN && self.has_process(agent) => Ok(ResourceKind::Stdin),
            [name] if *name == STDOUT && self.has_process(agent) => Ok(ResourceKind::Stdout),
            _ => Err(ResourceError::not_found(uri)),
        }
    }

    fn has_agent(&self, agent: &str) -> bool {
        self.has_transcript(agent) || self.has_process(agent)
    }

    fn has_transcript(&self, agent: &str) -> bool {
        self.transcripts.read().unwrap().contains_key(agent)
    }

    fn has_process(&self, agent: &str) -> bool {
        self.processes.read().unwrap().contains_key(agent)
    }

    fn root_key(uri: &ResourceUri) -> Result<String, ResourceError> {
        if uri.scheme() != SCHEME || !uri.segments().next().is_none() {
            return Err(ResourceError::new(
                ResourceErrorCode::InvalidAddress,
                format!("expected an agent root URI: {uri}"),
            ));
        }
        validate_segment(uri.authority().to_owned())
    }
}

enum ResourceKind {
    Directory,
    AgentRoot,
    Transcript,
    Stdin,
    Stdout,
}

#[async_trait]
impl ResourceProvider for AgentResourceComponent {
    fn provider_name(&self) -> &str {
        SCHEME
    }

    fn eligible(&self, uri: &ResourceUri) -> bool {
        uri.scheme() == SCHEME
    }

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        match self.kind(uri)? {
            ResourceKind::Directory | ResourceKind::AgentRoot => Ok(ProviderAttrs::directory()),
            ResourceKind::Transcript => {
                let bytes = self.transcript_for(uri)?.read().await?;
                Ok(ProviderAttrs::file(
                    bytes.len() as u64,
                    std::time::SystemTime::now(),
                ))
            }
            ResourceKind::Stdin | ResourceKind::Stdout => {
                Ok(ProviderAttrs::file(0, std::time::SystemTime::now()))
            }
        }
    }

    async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError> {
        match self.kind(uri)? {
            ResourceKind::Transcript | ResourceKind::Stdin | ResourceKind::Stdout => Err(
                ResourceError::new(ResourceErrorCode::NotDir, "agent stream is not a directory"),
            ),
            ResourceKind::Directory | ResourceKind::AgentRoot => {
                let segments: Vec<_> = uri.segments().collect();
                let mut names = BTreeMap::new();
                match segments.as_slice() {
                    [] if uri.authority().is_empty() => {
                        for agent in self.transcripts.read().unwrap().keys() {
                            names.insert(agent.clone(), ResourceKind::AgentRoot);
                        }
                        for agent in self.processes.read().unwrap().keys() {
                            names.insert(agent.clone(), ResourceKind::AgentRoot);
                        }
                    }
                    [] => {
                        if self.has_transcript(uri.authority()) {
                            names.insert(TRANSCRIPT.into(), ResourceKind::Transcript);
                        }
                        if self.has_process(uri.authority()) {
                            names.insert(STDIN.into(), ResourceKind::Stdin);
                            names.insert(STDOUT.into(), ResourceKind::Stdout);
                        }
                    }
                    _ => {}
                }
                Ok(names
                    .into_iter()
                    .map(|(name, kind)| ProviderEntry {
                        name,
                        attrs: match kind {
                            ResourceKind::Directory | ResourceKind::AgentRoot => {
                                ProviderAttrs::directory()
                            }
                            ResourceKind::Transcript
                            | ResourceKind::Stdin
                            | ResourceKind::Stdout => {
                                ProviderAttrs::file(0, std::time::SystemTime::now())
                            }
                        },
                    })
                    .collect())
            }
        }
    }

    async fn read(
        &self,
        uri: &ResourceUri,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        match self.kind(uri)? {
            ResourceKind::Transcript => {
                let bytes = self.transcript_for(uri)?.read().await?;
                let start = usize::try_from(offset)
                    .unwrap_or(usize::MAX)
                    .min(bytes.len());
                let end = start.saturating_add(size as usize).min(bytes.len());
                Ok(bytes[start..end].to_vec())
            }
            ResourceKind::Stdout => {
                let agent = channel_key(uri, STDOUT)?;
                self.process_for(&agent, uri)?
                    .read_stdout(offset, size)
                    .await
            }
            _ => Err(ResourceError::new(
                ResourceErrorCode::IsDir,
                "cannot read an agent directory or stdin stream",
            )),
        }
    }

    async fn write(
        &self,
        uri: &ResourceUri,
        _offset: u64,
        data: &[u8],
    ) -> Result<u32, ResourceError> {
        if matches!(self.kind(uri)?, ResourceKind::Stdin) {
            return self.write_stdin(uri, data).await;
        }
        Err(ResourceError::new(
            ResourceErrorCode::Unsupported,
            "agent resources only accept byte writes on stdin",
        ))
    }

    async fn signal(&self, uri: &ResourceUri, signal: ResourceSignal) -> Result<(), ResourceError> {
        match self.kind(uri)? {
            ResourceKind::Transcript => {
                if signal.name == "close" {
                    self.transcript_for(uri)?.close().await
                } else {
                    Err(ResourceError::new(
                        ResourceErrorCode::Unsupported,
                        format!("unsupported transcript signal {:?}", signal.name),
                    ))
                }
            }
            ResourceKind::AgentRoot | ResourceKind::Stdin | ResourceKind::Stdout => {
                let agent = uri.authority();
                self.process_for(agent, uri)?.signal(signal).await
            }
            ResourceKind::Directory => Err(ResourceError::new(
                ResourceErrorCode::Unsupported,
                "cannot signal the agent namespace root",
            )),
        }
    }

    async fn move_resource(
        &self,
        source: &ResourceUri,
        destination: &ResourceUri,
    ) -> Result<(), ResourceError> {
        let source = Self::root_key(source)?;
        let destination = Self::root_key(destination)?;
        if source == destination {
            return Ok(());
        }
        validate_segment(destination.clone())?;
        let mut transcripts = self.transcripts.write().unwrap();
        let mut processes = self.processes.write().unwrap();
        if !transcripts.contains_key(&source) && !processes.contains_key(&source) {
            return Err(ResourceError::not_found(
                &format!("agent://{source}").parse().unwrap(),
            ));
        }
        if transcripts.contains_key(&destination) || processes.contains_key(&destination) {
            return Err(ResourceError::new(
                ResourceErrorCode::Conflict,
                format!("agent destination already exists: {destination}"),
            ));
        }
        if let Some(value) = transcripts.remove(&source) {
            transcripts.insert(destination.clone(), value);
        }
        if let Some(value) = processes.remove(&source) {
            processes.insert(destination, value);
        }
        Ok(())
    }
}

/// Adapter that exposes an existing durable session log through the agent
/// component without making `artist-session` depend on the kernel resource
/// traits.
pub struct EventLogTranscript {
    log: Arc<EventLog>,
}

impl EventLogTranscript {
    pub fn new(log: Arc<EventLog>) -> Self {
        Self { log }
    }
}

#[async_trait]
impl AgentTranscript for EventLogTranscript {
    async fn read(&self) -> Result<Vec<u8>, ResourceError> {
        self.log.bytes().map_err(log_resource_error)
    }

    async fn append(
        &self,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<(), ResourceError> {
        self.log
            .append(event_type, payload)
            .map(|_| ())
            .map_err(log_resource_error)
    }

    async fn close(&self) -> Result<(), ResourceError> {
        self.log.close().map_err(log_resource_error)
    }

    fn is_closed(&self) -> bool {
        self.log.is_closed()
    }
}

fn log_resource_error(error: LogError) -> ResourceError {
    let code = match error {
        LogError::Closed
        | LogError::UnsupportedVersion { .. }
        | LogError::InvalidRecord { .. }
        | LogError::WrongStream { .. }
        | LogError::NonMonotonic { .. } => ResourceErrorCode::Conflict,
        _ => ResourceErrorCode::Io,
    };
    ResourceError::new(code, error.to_string())
}

fn channel_key(uri: &ResourceUri, channel: &str) -> Result<String, ResourceError> {
    match uri.segments().collect::<Vec<_>>().as_slice() {
        [segment] if !uri.authority().is_empty() && *segment == channel => {
            validate_segment(uri.authority().to_owned())
        }
        _ => Err(ResourceError::new(
            ResourceErrorCode::InvalidAddress,
            format!("not an {channel} URI: {uri}"),
        )),
    }
}

fn transcript_key(uri: &ResourceUri) -> Result<String, ResourceError> {
    match uri.segments().collect::<Vec<_>>().as_slice() {
        [segment] if !uri.authority().is_empty() && *segment == TRANSCRIPT => {
            validate_segment(uri.authority().to_owned())
        }
        _ => Err(ResourceError::new(
            ResourceErrorCode::InvalidAddress,
            format!("not a transcript URI: {uri}"),
        )),
    }
}

fn validate_segment(value: String) -> Result<String, ResourceError> {
    if value.is_empty() || value == "." || value == ".." || value.contains('/') {
        return Err(ResourceError::new(
            ResourceErrorCode::InvalidAddress,
            format!("invalid agent path segment: {value:?}"),
        ));
    }
    Ok(value)
}
