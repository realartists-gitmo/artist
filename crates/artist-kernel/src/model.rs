use std::{collections::VecDeque, pin::Pin, sync::Arc};

use artist_core::{
    CallId, CompletionCallMetadata, ContentPart, FailureClass, MessageId, ModelFailure, RunId,
    SessionId, Source, TokenUsage,
};
use futures::Stream;
use thiserror::Error;
use tokio::sync::{Mutex, mpsc};

pub type ModelStream = Pin<Box<dyn Stream<Item = Result<ModelEvent, ModelError>> + Send>>;

pub trait StreamingModel: Send + Sync + 'static {
    fn stream(&self, request: ModelRequest, steering: Steering) -> ModelStream;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelRequest {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub context: String,
    pub prompt: String,
    pub history: Vec<ModelHistoryItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelHistoryItem {
    pub sequence: u64,
    pub message: ModelMessage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelMessage {
    User(String),
    Assistant(Vec<ContentPart>),
    Notification(String),
    ToolCall {
        call_id: CallId,
        name: String,
        arguments: String,
    },
    ToolResult {
        call_id: CallId,
        content: Vec<ContentPart>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelEvent {
    TextDelta(String),
    TextReset,
    ToolCallDelta {
        call_id: CallId,
        delta: String,
    },
    ToolCall {
        call_id: CallId,
        name: String,
        arguments: String,
    },
    ToolExecutionCommitted {
        call_id: CallId,
        name: String,
        arguments: String,
    },
    ToolResult {
        call_id: CallId,
        content: Vec<ContentPart>,
    },
    Content(ContentPart),
    ContextCompacted {
        through_sequence: u64,
        evicted_count: usize,
        evicted_bytes: usize,
        artifact: String,
    },
    Usage(TokenUsage),
    CompletionMetadata(Vec<CompletionCallMetadata>),
    Finished {
        output: Option<String>,
    },
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("{}", .0.message)]
pub struct ModelError(pub ModelFailure);

impl ModelError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(ModelFailure {
            message: message.into(),
            class: FailureClass::Unknown,
            retriable: false,
            provider_code: None,
            http_status: None,
            provider_request_id: None,
        })
    }
}

impl From<String> for ModelError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for ModelError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteeringNotice {
    pub message_id: MessageId,
    pub source: Source,
    pub content: String,
}

/// A gentle notification inbox shared with the active model loop.
///
/// Model adapters call [`take`](Self::take) immediately before a model request.
/// Taking notifications reports their delivery to the session; it never cancels
/// the current request.
#[derive(Clone)]
pub struct Steering {
    queued: Arc<Mutex<VecDeque<SteeringNotice>>>,
    delivered: mpsc::UnboundedSender<Vec<MessageId>>,
}

impl Steering {
    pub fn empty() -> Self {
        Self::channel().0
    }

    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<Vec<MessageId>>) {
        Self::channel_with(VecDeque::new())
    }

    pub(crate) fn channel_with(
        queued: VecDeque<SteeringNotice>,
    ) -> (Self, mpsc::UnboundedReceiver<Vec<MessageId>>) {
        let (delivered, receiver) = mpsc::unbounded_channel();
        (
            Self {
                queued: Arc::new(Mutex::new(queued)),
                delivered,
            },
            receiver,
        )
    }

    pub(crate) async fn push(&self, notice: SteeringNotice) {
        self.queued.lock().await.push_back(notice);
    }

    pub async fn take(&self) -> Vec<SteeringNotice> {
        let notices: Vec<_> = self.queued.lock().await.drain(..).collect();
        if !notices.is_empty() {
            let ids = notices
                .iter()
                .map(|notice| notice.message_id.clone())
                .collect();
            let _ = self.delivered.send(ids);
        }
        notices
    }

    pub async fn is_empty(&self) -> bool {
        self.queued.lock().await.is_empty()
    }
}
