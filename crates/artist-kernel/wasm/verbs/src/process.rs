//! Model-facing process lifecycle verbs.
//!
//! Process ownership is supplied by the host. These types define the stable
//! resource-shaped contract without assuming how a provider launches or
//! supervises processes.

use async_trait::async_trait;

use crate::host::{VerbError, VerbTool};

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct RunRequest {
    pub executable: Option<String>,
    pub target: Option<String>,
    #[serde(default)]
    pub arguments: Vec<String>,
    pub working_directory: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct RunResponse {
    pub process: String,
    pub stdin: String,
    pub stdout: String,
    pub stderr: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct PollRequest {
    pub target: String,
    pub r#match: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct PollResponse {
    pub target: String,
    pub event: String,
    pub matched: bool,
    pub content: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct SignalRequest {
    pub process: String,
    pub signal: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessError {
    InvalidArgument(String),
    NotFound(String),
    Unsupported(String),
    PermissionDenied(String),
    Conflict(String),
    Aborted(String),
    Internal(String),
}

#[async_trait]
pub trait ProcessLauncher: Send + Sync {
    async fn launch(&self, request: RunRequest) -> Result<RunResponse, ProcessError>;
}

#[async_trait]
pub trait ProcessPoller: Send + Sync {
    async fn poll(&self, request: PollRequest) -> Result<PollResponse, ProcessError>;
}

#[async_trait]
pub trait ProcessSignaler: Send + Sync {
    async fn signal(&self, request: SignalRequest) -> Result<(), ProcessError>;
}

pub struct RunVerb<P> {
    provider: P,
}

impl<P> RunVerb<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl<P: ProcessLauncher + 'static> VerbTool<RunRequest, RunResponse> for RunVerb<P> {
    fn name(&self) -> &str {
        "run"
    }

    async fn call(&self, requests: Vec<RunRequest>) -> Vec<Result<RunResponse, VerbError>> {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            results.push(self.provider.launch(request).await.map_err(map_error));
        }
        results
    }
}

pub struct PollVerb<P> {
    provider: P,
}

impl<P> PollVerb<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl<P: ProcessPoller + 'static> VerbTool<PollRequest, PollResponse> for PollVerb<P> {
    fn name(&self) -> &str {
        "poll"
    }

    async fn call(&self, requests: Vec<PollRequest>) -> Vec<Result<PollResponse, VerbError>> {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            results.push(self.provider.poll(request).await.map_err(map_error));
        }
        results
    }
}

pub struct SignalVerb<P> {
    provider: P,
}

impl<P> SignalVerb<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl<P: ProcessSignaler + 'static> VerbTool<SignalRequest, ()> for SignalVerb<P> {
    fn name(&self) -> &str {
        "signal"
    }

    async fn call(&self, requests: Vec<SignalRequest>) -> Vec<Result<(), VerbError>> {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            results.push(self.provider.signal(request).await.map_err(map_error));
        }
        results
    }
}

fn map_error(error: ProcessError) -> VerbError {
    match error {
        ProcessError::InvalidArgument(_) => VerbError::InvalidArgument,
        ProcessError::NotFound(_) => VerbError::NotFound,
        ProcessError::Unsupported(_) => VerbError::Unsupported,
        ProcessError::PermissionDenied(_) => VerbError::PermissionDenied,
        ProcessError::Conflict(_) => VerbError::Conflict,
        ProcessError::Aborted(_) => VerbError::Aborted,
        ProcessError::Internal(_) => VerbError::Internal,
    }
}
