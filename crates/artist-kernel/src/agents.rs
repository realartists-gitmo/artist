//! The optional `agent://` transcript provider.
//!
//! Storage remains supplied by the caller. The kernel owns addressing,
//! resource shape, and append/close routing; it does not know the transcript
//! record schema.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::provider::{
    ProviderAttrs, ProviderEntry, ResourceError, ResourceErrorCode, ResourceProvider,
};
use crate::uri::ResourceUri;

const SCHEME: &str = "agent";
const TRANSCRIPT: &str = "transcript";
const STDIN: &str = "stdin";
const STDOUT: &str = "stdout";

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

/// The live I/O surface of a running agent.
///
/// The kernel intentionally does not prescribe how the process is hosted.
/// Implementations may be backed by pipes, channels, or an embedded runtime.
#[async_trait]
pub trait AgentProcess: Send + Sync {
    async fn write_stdin(&self, data: &[u8]) -> Result<u32, ResourceError>;
    async fn read_stdout(&self, offset: u64, size: u32) -> Result<Vec<u8>, ResourceError>;
}

#[derive(Default)]
pub struct AgentsProvider {
    transcripts: RwLock<BTreeMap<String, Arc<dyn AgentTranscript>>>,
    processes: RwLock<BTreeMap<String, Arc<dyn AgentProcess>>>,
}

impl AgentsProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T: AgentTranscript + 'static>(
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

    pub async fn append(
        &self,
        uri: &ResourceUri,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<(), ResourceError> {
        let transcript = self.transcript_for(uri)?;
        transcript.append(event_type, payload).await
    }

    pub async fn close(&self, uri: &ResourceUri) -> Result<(), ResourceError> {
        let transcript = self.transcript_for(uri)?;
        transcript.close().await
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

    fn kind(&self, uri: &ResourceUri) -> Result<ResourceKind, ResourceError> {
        if !self.eligible(uri) {
            return Err(ResourceError::not_found(uri));
        }
        let segments: Vec<_> = uri.segments().collect();
        let agent = uri.authority();
        match segments.as_slice() {
            [] if agent.is_empty() => Ok(ResourceKind::Directory),
            [] if self.has_agent(agent) => Ok(ResourceKind::Directory),
            [transcript] if *transcript == TRANSCRIPT && self.has_transcript(agent) => {
                Ok(ResourceKind::Transcript)
            }
            [stdin] if *stdin == STDIN && self.has_process(agent) => Ok(ResourceKind::Stdin),
            [stdout] if *stdout == STDOUT && self.has_process(agent) => Ok(ResourceKind::Stdout),
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
}

enum ResourceKind {
    Directory,
    Transcript,
    Stdin,
    Stdout,
}

#[async_trait]
impl ResourceProvider for AgentsProvider {
    fn provider_name(&self) -> &str {
        SCHEME
    }

    fn eligible(&self, uri: &ResourceUri) -> bool {
        uri.scheme() == SCHEME
    }

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        match self.kind(uri)? {
            ResourceKind::Directory => Ok(ProviderAttrs::directory()),
            ResourceKind::Transcript => {
                let transcript = self.transcript_for(uri)?;
                let bytes = transcript.read().await?;
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
            ResourceKind::Directory => {
                let segments: Vec<_> = uri.segments().collect();
                let mut names = BTreeMap::new();
                match segments.as_slice() {
                    [] if uri.authority().is_empty() => {
                        for agent in self.transcripts.read().unwrap().keys() {
                            names.insert(agent.clone(), ResourceKind::Directory);
                        }
                        for agent in self.processes.read().unwrap().keys() {
                            names.insert(agent.clone(), ResourceKind::Directory);
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
                };
                Ok(names
                    .into_iter()
                    .map(|(name, kind)| ProviderEntry {
                        name,
                        attrs: match kind {
                            ResourceKind::Directory => ProviderAttrs::directory(),
                            ResourceKind::Transcript => {
                                ProviderAttrs::file(0, std::time::SystemTime::now())
                            }
                            ResourceKind::Stdin | ResourceKind::Stdout => {
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
        let kind = self.kind(uri)?;
        if !matches!(kind, ResourceKind::Transcript | ResourceKind::Stdout) {
            return Err(ResourceError::new(
                ResourceErrorCode::IsDir,
                "cannot read an agents directory",
            ));
        }
        let bytes = match kind {
            ResourceKind::Transcript => self.transcript_for(uri)?.read().await?,
            ResourceKind::Stdout => {
                let agent = channel_key(uri, STDOUT)?;
                self.process_for(&agent, uri)?
                    .read_stdout(offset, size)
                    .await?
            }
            _ => unreachable!(),
        };
        if matches!(kind, ResourceKind::Stdout) {
            return Ok(bytes);
        }
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let end = start.saturating_add(size as usize).min(bytes.len());
        Ok(bytes[start..end].to_vec())
    }

    async fn write(
        &self,
        uri: &ResourceUri,
        _offset: u64,
        _data: &[u8],
    ) -> Result<u32, ResourceError> {
        if matches!(self.kind(uri)?, ResourceKind::Stdin) {
            return self.write_stdin(uri, _data).await;
        }
        let transcript = self.transcript_for(uri)?;
        if transcript.is_closed() {
            return Err(ResourceError::new(
                ResourceErrorCode::Conflict,
                "historical transcript is immutable",
            ));
        }
        Err(ResourceError::new(
            ResourceErrorCode::Unsupported,
            "transcripts require structured append through the agent runtime",
        ))
    }
}

fn channel_key(uri: &ResourceUri, channel: &str) -> Result<String, ResourceError> {
    let segments: Vec<_> = uri.segments().collect();
    match segments.as_slice() {
        [segment] if !uri.authority().is_empty() && *segment == channel => {
            Ok(uri.authority().into())
        }
        _ => Err(ResourceError::new(
            ResourceErrorCode::InvalidAddress,
            format!("not an {channel} URI: {uri}"),
        )),
    }
}

fn transcript_key(uri: &ResourceUri) -> Result<String, ResourceError> {
    let segments: Vec<_> = uri.segments().collect();
    match segments.as_slice() {
        [transcript] if !uri.authority().is_empty() && *transcript == TRANSCRIPT => {
            Ok(uri.authority().into())
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
            format!("invalid agents path segment: {value:?}"),
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MemoryTranscript {
        bytes: Mutex<Vec<u8>>,
        closed: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl AgentTranscript for MemoryTranscript {
        async fn read(&self) -> Result<Vec<u8>, ResourceError> {
            Ok(self.bytes.lock().unwrap().clone())
        }

        async fn append(
            &self,
            event_type: &str,
            payload: serde_json::Value,
        ) -> Result<(), ResourceError> {
            if self.is_closed() {
                return Err(ResourceError::new(ResourceErrorCode::Conflict, "closed"));
            }
            self.bytes
                .lock()
                .unwrap()
                .extend_from_slice(format!("{event_type}:{payload}\n").as_bytes());
            Ok(())
        }

        async fn close(&self) -> Result<(), ResourceError> {
            self.closed
                .store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        }

        fn is_closed(&self) -> bool {
            self.closed.load(std::sync::atomic::Ordering::Acquire)
        }
    }

    struct MemoryProcess {
        stdin: Mutex<Vec<u8>>,
        stdout: Vec<u8>,
    }

    #[async_trait]
    impl AgentProcess for MemoryProcess {
        async fn write_stdin(&self, data: &[u8]) -> Result<u32, ResourceError> {
            self.stdin.lock().unwrap().extend_from_slice(data);
            Ok(data.len().try_into().unwrap())
        }

        async fn read_stdout(&self, offset: u64, size: u32) -> Result<Vec<u8>, ResourceError> {
            let start = usize::try_from(offset)
                .unwrap_or(usize::MAX)
                .min(self.stdout.len());
            let end = start.saturating_add(size as usize).min(self.stdout.len());
            Ok(self.stdout[start..end].to_vec())
        }
    }

    #[tokio::test]
    async fn exposes_live_and_closed_transcripts_as_resources() {
        let kernel = crate::Kernel::with_agents();
        kernel
            .register_agent_transcript(
                "agent-1",
                MemoryTranscript {
                    bytes: Mutex::new(Vec::new()),
                    closed: std::sync::atomic::AtomicBool::new(false),
                },
            )
            .unwrap();
        let uri: ResourceUri = "agent://agent-1/transcript".parse().unwrap();

        kernel
            .append_agent_event(&uri, "agent.user_message", serde_json::json!({"text":"hi"}))
            .await
            .unwrap();
        assert_eq!(
            kernel.read_uri(&uri, 0, 4096).await.unwrap(),
            b"agent.user_message:{\"text\":\"hi\"}\n"
        );
        kernel.close_agent_transcript(&uri).await.unwrap();
        assert!(matches!(
            kernel
                .append_agent_event(&uri, "agent.late", serde_json::json!({}))
                .await,
            Err(ResourceError {
                code: ResourceErrorCode::Conflict,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn exposes_live_process_io_without_a_process_namespace() {
        let kernel = crate::Kernel::with_agents();
        kernel
            .register_agent_process(
                "agent-1",
                MemoryProcess {
                    stdin: Mutex::new(Vec::new()),
                    stdout: b"model output\n".to_vec(),
                },
            )
            .unwrap();
        let agent: ResourceUri = "agent://agent-1".parse().unwrap();
        let names = kernel.readdir_uri(&agent).await.unwrap();
        assert_eq!(
            names
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["stdin", "stdout"]
        );

        let stdin: ResourceUri = "agent://agent-1/stdin".parse().unwrap();
        assert_eq!(kernel.write_uri(&stdin, 0, b"steer\n").await.unwrap(), 6);
        let stdout: ResourceUri = "agent://agent-1/stdout".parse().unwrap();
        assert_eq!(
            kernel.read_uri(&stdout, 0, 4096).await.unwrap(),
            b"model output\n"
        );
    }
}
