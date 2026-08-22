use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
    time::Instant,
};

use artist_core::{
    Command, EventId, InitialContext, InterruptionCause, MessageId, RunId, RunOutcome, SessionId,
    SessionRecord, Source, StreamEvent, StreamEventKind, TranscriptEntryKind,
};
use artist_store::{SessionStore, StoreError};
use futures::StreamExt;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::{
    ModelError, ModelEvent, ModelRequest, ModelStream, Steering, SteeringNotice, StreamingModel,
    project,
};

const CHANNEL_CAPACITY: usize = 64;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("session task has stopped")]
    Closed,
}

#[derive(Clone)]
pub struct SessionHandle {
    commands: mpsc::Sender<Envelope>,
    events: broadcast::Sender<StreamEvent>,
}

impl SessionHandle {
    pub async fn create(
        session_id: SessionId,
        context: InitialContext,
        store: Arc<dyn SessionStore>,
        model: Arc<dyn StreamingModel>,
    ) -> Result<Self, SessionError> {
        let record = SessionRecord::new(session_id, context);
        store.create(record.clone()).await?;
        Ok(Self::spawn(record, store, model))
    }

    pub async fn resume(
        session_id: &SessionId,
        store: Arc<dyn SessionStore>,
        model: Arc<dyn StreamingModel>,
    ) -> Result<Self, SessionError> {
        let mut record = store.load(session_id).await?;
        if let Some(run_id) = record.active_run().cloned() {
            let existing_message = record.active_message_id().cloned();
            let message_id = existing_message.clone().unwrap_or_else(|| {
                MessageId::new(format!(
                    "{}:message:{}",
                    record.session_id(),
                    record.next_sequence()
                ))
            });
            let mut kinds = Vec::new();
            if existing_message.is_none() {
                kinds.push(TranscriptEntryKind::AssistantMessage {
                    message_id: message_id.clone(),
                    run_id: run_id.clone(),
                    content: Vec::new(),
                });
            }
            kinds.push(TranscriptEntryKind::RunFinished {
                run_id,
                outcome: RunOutcome::Interrupted {
                    message_id,
                    cause: InterruptionCause::Harness {
                        reason: "resumed after an unfinished run".into(),
                    },
                },
            });
            let entries = record.entries_for(kinds);
            store
                .append(session_id, record.next_sequence(), &entries)
                .await?;
            record
                .append_batch(&entries)
                .expect("resume reconciliation is valid");
        }
        Ok(Self::spawn(record, store, model))
    }

    fn spawn(
        record: SessionRecord,
        store: Arc<dyn SessionStore>,
        model: Arc<dyn StreamingModel>,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (event_tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        let handle = Self {
            commands: command_tx,
            events: event_tx.clone(),
        };
        tokio::spawn(async move {
            Session::new(record, store, model, command_rx, event_tx)
                .run()
                .await;
        });
        handle
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StreamEvent> {
        self.events.subscribe()
    }

    pub async fn input(
        &self,
        source: Source,
        content: impl Into<String>,
    ) -> Result<(), SessionError> {
        self.send(Command::Input {
            source,
            content: content.into(),
        })
        .await
    }

    pub async fn steer(
        &self,
        source: Source,
        content: impl Into<String>,
    ) -> Result<(), SessionError> {
        self.send(Command::Steer {
            source,
            content: content.into(),
        })
        .await
    }

    pub async fn abort(&self, cause: InterruptionCause) -> Result<(), SessionError> {
        self.send(Command::Abort { cause }).await
    }

    async fn send(&self, command: Command) -> Result<(), SessionError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Envelope { command, reply })
            .await
            .map_err(|_| SessionError::Closed)?;
        response.await.map_err(|_| SessionError::Closed)?
    }
}

struct Envelope {
    command: Command,
    reply: oneshot::Sender<Result<(), SessionError>>,
}

struct PendingInput {
    message_id: MessageId,
    content: String,
}

struct ActiveRun {
    id: RunId,
    text: String,
    content: Vec<artist_core::ContentPart>,
    stream: ModelStream,
    started: Instant,
    first_token_ms: Option<u64>,
}

struct Session {
    record: SessionRecord,
    store: Arc<dyn SessionStore>,
    model: Arc<dyn StreamingModel>,
    commands: mpsc::Receiver<Envelope>,
    events: broadcast::Sender<StreamEvent>,
    event_sequence: u64,
    inputs: VecDeque<PendingInput>,
    steering: Steering,
    delivered: mpsc::UnboundedReceiver<Vec<MessageId>>,
    active: Option<ActiveRun>,
}

impl Session {
    fn new(
        record: SessionRecord,
        store: Arc<dyn SessionStore>,
        model: Arc<dyn StreamingModel>,
        commands: mpsc::Receiver<Envelope>,
        events: broadcast::Sender<StreamEvent>,
    ) -> Self {
        let started_inputs: HashSet<_> = record
            .entries()
            .iter()
            .filter_map(|entry| match &entry.kind {
                TranscriptEntryKind::RunStarted { input_id, .. } => Some(input_id.clone()),
                _ => None,
            })
            .collect();
        let delivered_steering: HashSet<_> = record
            .entries()
            .iter()
            .filter_map(|entry| match &entry.kind {
                TranscriptEntryKind::SteeringDelivered { message_ids, .. } => Some(message_ids),
                _ => None,
            })
            .flatten()
            .cloned()
            .collect();
        let inputs = record
            .entries()
            .iter()
            .filter_map(|entry| match &entry.kind {
                TranscriptEntryKind::Input {
                    message_id,
                    content,
                    ..
                } if !started_inputs.contains(message_id) => Some(PendingInput {
                    message_id: message_id.clone(),
                    content: content.clone(),
                }),
                _ => None,
            })
            .collect();
        let queued_steering = record
            .entries()
            .iter()
            .filter_map(|entry| match &entry.kind {
                TranscriptEntryKind::SteeringQueued {
                    message_id,
                    source,
                    content,
                } if !delivered_steering.contains(message_id) => Some(SteeringNotice {
                    message_id: message_id.clone(),
                    source: *source,
                    content: content.clone(),
                }),
                _ => None,
            })
            .collect();
        let (steering, delivered) = Steering::channel_with(queued_steering);
        Self {
            record,
            store,
            model,
            commands,
            events,
            event_sequence: 0,
            inputs,
            steering,
            delivered,
            active: None,
        }
    }

    async fn run(mut self) {
        loop {
            if self.active.is_none() {
                if let Some(input) = self.inputs.pop_front() {
                    if self.start(input).await.is_err() {
                        return;
                    }
                    continue;
                }
                let Some(envelope) = self.commands.recv().await else {
                    return;
                };
                self.command(envelope).await;
                continue;
            }

            enum Wake {
                Command(Option<Envelope>),
                Delivered(Option<Vec<MessageId>>),
                Model(Option<Result<ModelEvent, ModelError>>),
            }

            let wake = {
                let active = self.active.as_mut().expect("checked above");
                tokio::select! {
                    command = self.commands.recv() => Wake::Command(command),
                    delivered = self.delivered.recv() => Wake::Delivered(delivered),
                    event = active.stream.next() => Wake::Model(event),
                }
            };

            match wake {
                Wake::Command(Some(envelope)) => self.command(envelope).await,
                Wake::Command(None) => return,
                Wake::Delivered(Some(ids)) => {
                    if self.steering_delivered(ids).await.is_err() {
                        return;
                    }
                }
                Wake::Delivered(None) => return,
                Wake::Model(Some(Ok(event))) => {
                    if self.flush_delivered().await.is_err()
                        || self.model_event(event).await.is_err()
                    {
                        return;
                    }
                }
                Wake::Model(Some(Err(error))) => {
                    if self.flush_delivered().await.is_err() || self.fail(error).await.is_err() {
                        return;
                    }
                }
                Wake::Model(None) => {
                    if self.flush_delivered().await.is_err() || self.complete().await.is_err() {
                        return;
                    }
                }
            }
        }
    }

    async fn command(&mut self, envelope: Envelope) {
        let result = self.apply_command(envelope.command).await;
        let _ = envelope.reply.send(result);
    }

    async fn apply_command(&mut self, command: Command) -> Result<(), SessionError> {
        match command {
            Command::Input { source, content } => {
                let message_id = self.message_id();
                self.append(vec![TranscriptEntryKind::Input {
                    message_id: message_id.clone(),
                    source,
                    content: content.clone(),
                }])
                .await?;
                self.inputs.push_back(PendingInput {
                    message_id: message_id.clone(),
                    content,
                });
                self.emit(None, StreamEventKind::InputQueued { message_id });
            }
            Command::Steer { source, content } => {
                let message_id = self.message_id();
                self.append(vec![TranscriptEntryKind::SteeringQueued {
                    message_id: message_id.clone(),
                    source,
                    content: content.clone(),
                }])
                .await?;
                self.steering
                    .push(SteeringNotice {
                        message_id: message_id.clone(),
                        source,
                        content,
                    })
                    .await;
                self.emit(
                    self.active.as_ref().map(|run| run.id.clone()),
                    StreamEventKind::SteeringQueued { message_id },
                );
            }
            Command::Abort { cause } => {
                if self.active.is_some() {
                    self.interrupt(cause).await?;
                }
            }
        }
        Ok(())
    }

    async fn start(&mut self, input: PendingInput) -> Result<(), SessionError> {
        let run_id = RunId::new(format!(
            "{}:run:{}",
            self.record.session_id(),
            self.record.next_sequence()
        ));
        self.append(vec![TranscriptEntryKind::RunStarted {
            run_id: run_id.clone(),
            input_id: input.message_id,
        }])
        .await?;
        let (context, mut history) = project(&self.record);
        if matches!(history.last(), Some(crate::ModelHistoryItem { message: crate::ModelMessage::User(content), .. }) if content == &input.content)
        {
            history.pop();
        }
        let messages_in = history.len() + 1;
        let request = ModelRequest {
            session_id: self.record.session_id().clone(),
            run_id: run_id.clone(),
            context,
            prompt: input.content,
            history,
        };
        let stream = self.model.stream(request, self.steering.clone());
        self.active = Some(ActiveRun {
            id: run_id.clone(),
            text: String::new(),
            content: Vec::new(),
            stream,
            started: Instant::now(),
            first_token_ms: None,
        });
        self.emit(Some(run_id), StreamEventKind::RunStarted { messages_in });
        Ok(())
    }

    async fn model_event(&mut self, event: ModelEvent) -> Result<(), SessionError> {
        let run_id = self
            .active
            .as_ref()
            .expect("model event without run")
            .id
            .clone();
        match event {
            ModelEvent::TextDelta(delta) => {
                if !delta.is_empty() && self.active.as_ref().unwrap().first_token_ms.is_none() {
                    let active = self.active.as_mut().unwrap();
                    active.first_token_ms = Some(millis(active.started.elapsed()));
                }
                let active = self.active.as_mut().unwrap();
                active.text.push_str(&delta);
                active
                    .content
                    .push(artist_core::ContentPart::text(delta.clone()));
                self.emit(Some(run_id), StreamEventKind::TextDelta { delta });
            }
            ModelEvent::TextReset => {
                let active = self.active.as_mut().unwrap();
                active.text.clear();
                active.content.clear();
                self.emit(Some(run_id), StreamEventKind::TextReset);
            }
            ModelEvent::ToolCallDelta { call_id, delta } => {
                self.emit(
                    Some(run_id),
                    StreamEventKind::ToolCallDelta { call_id, delta },
                );
            }
            ModelEvent::ToolCall {
                call_id,
                name,
                arguments,
            } => {
                self.append(vec![TranscriptEntryKind::ToolCall {
                    run_id: run_id.clone(),
                    call_id: call_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                }])
                .await?;
                self.emit(
                    Some(run_id),
                    StreamEventKind::ToolCall {
                        call_id,
                        name,
                        arguments,
                    },
                );
            }
            ModelEvent::ToolExecutionCommitted {
                call_id,
                name,
                arguments,
            } => {
                self.emit(
                    Some(run_id),
                    StreamEventKind::ToolExecutionCommitted {
                        call_id,
                        name,
                        arguments,
                    },
                );
            }
            ModelEvent::ToolResult { call_id, content } => {
                self.append(vec![TranscriptEntryKind::ToolResult {
                    run_id: run_id.clone(),
                    call_id: call_id.clone(),
                    content: content.clone(),
                }])
                .await?;
                self.emit(
                    Some(run_id),
                    StreamEventKind::ToolResult { call_id, content },
                );
            }
            ModelEvent::Content(part) => {
                if matches!(
                    part,
                    artist_core::ContentPart::Reasoning { .. }
                        | artist_core::ContentPart::Image { .. }
                ) {
                    self.active.as_mut().unwrap().content.push(part.clone());
                }
                self.emit(Some(run_id), StreamEventKind::Content { part });
            }
            ModelEvent::ContextCompacted {
                through_sequence,
                evicted_count,
                evicted_bytes,
                artifact,
            } => {
                let summary_bytes = artifact.len();
                self.append(vec![TranscriptEntryKind::Compaction {
                    through_sequence,
                    artifact,
                }])
                .await?;
                self.emit(
                    Some(run_id),
                    StreamEventKind::ContextCompacted {
                        evicted_count,
                        evicted_bytes,
                        summary_bytes,
                    },
                );
            }
            ModelEvent::Usage(usage) => {
                self.emit(Some(run_id), StreamEventKind::Usage(usage));
            }
            ModelEvent::CompletionMetadata(calls) => {
                self.emit(Some(run_id), StreamEventKind::CompletionMetadata { calls });
            }
            ModelEvent::Finished { output } => {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.content.is_empty())
                    && let Some(output) = output
                {
                    let active = self.active.as_mut().unwrap();
                    active.text = output.clone();
                    active.content.push(artist_core::ContentPart::text(output));
                }
                self.complete().await?;
            }
        }
        Ok(())
    }

    async fn steering_delivered(
        &mut self,
        message_ids: Vec<MessageId>,
    ) -> Result<(), SessionError> {
        let Some(run_id) = self.active.as_ref().map(|run| run.id.clone()) else {
            return Ok(());
        };
        self.append(vec![TranscriptEntryKind::SteeringDelivered {
            run_id: run_id.clone(),
            message_ids: message_ids.clone(),
        }])
        .await?;
        self.emit(
            Some(run_id),
            StreamEventKind::SteeringDelivered { message_ids },
        );
        Ok(())
    }

    async fn flush_delivered(&mut self) -> Result<(), SessionError> {
        while let Ok(ids) = self.delivered.try_recv() {
            self.steering_delivered(ids).await?;
        }
        Ok(())
    }

    async fn complete(&mut self) -> Result<(), SessionError> {
        let Some(active) = self.active.take() else {
            return Ok(());
        };
        let duration_ms = millis(active.started.elapsed());
        let time_to_first_token_ms = active.first_token_ms;
        let message_id = self.message_id();
        self.append(vec![
            TranscriptEntryKind::AssistantMessage {
                message_id: message_id.clone(),
                run_id: active.id.clone(),
                content: active.content,
            },
            TranscriptEntryKind::RunFinished {
                run_id: active.id.clone(),
                outcome: RunOutcome::Completed {
                    message_id: message_id.clone(),
                },
            },
        ])
        .await?;
        self.emit(
            Some(active.id),
            StreamEventKind::Completed {
                message_id,
                duration_ms,
                time_to_first_token_ms,
            },
        );
        Ok(())
    }

    async fn interrupt(&mut self, cause: InterruptionCause) -> Result<(), SessionError> {
        let active = self.active.take().expect("checked by caller");
        let message_id = self.message_id();
        self.append(vec![
            TranscriptEntryKind::AssistantMessage {
                message_id: message_id.clone(),
                run_id: active.id.clone(),
                content: active.content,
            },
            TranscriptEntryKind::RunFinished {
                run_id: active.id.clone(),
                outcome: RunOutcome::Interrupted {
                    message_id,
                    cause: cause.clone(),
                },
            },
        ])
        .await?;
        self.emit(Some(active.id), StreamEventKind::Interrupted { cause });
        Ok(())
    }

    async fn fail(&mut self, error: ModelError) -> Result<(), SessionError> {
        let active = self.active.take().expect("model failure without run");
        let mut entries = Vec::new();
        let mut message_id = None;
        if !active.content.is_empty() {
            let partial_id = self.message_id();
            entries.push(TranscriptEntryKind::AssistantMessage {
                message_id: partial_id.clone(),
                run_id: active.id.clone(),
                content: active.content,
            });
            message_id = Some(partial_id);
        }
        entries.push(TranscriptEntryKind::RunFinished {
            run_id: active.id.clone(),
            outcome: RunOutcome::Failed {
                message_id,
                error: error.0.message.clone(),
            },
        });
        self.append(entries).await?;
        self.emit(
            Some(active.id),
            StreamEventKind::Failed { failure: error.0 },
        );
        Ok(())
    }

    async fn append(&mut self, kinds: Vec<TranscriptEntryKind>) -> Result<(), SessionError> {
        let expected_sequence = self.record.next_sequence();
        let entries = self.record.entries_for(kinds);
        self.store
            .append(self.record.session_id(), expected_sequence, &entries)
            .await?;
        self.record
            .append_batch(&entries)
            .expect("the store accepted an invalid kernel transition");
        Ok(())
    }

    fn message_id(&self) -> MessageId {
        MessageId::new(format!(
            "{}:message:{}",
            self.record.session_id(),
            self.record.next_sequence()
        ))
    }

    fn emit(&mut self, run_id: Option<RunId>, kind: StreamEventKind) {
        let sequence = self.event_sequence;
        self.event_sequence += 1;
        let _ = self.events.send(StreamEvent {
            event_id: EventId::new(format!("{}:stream:{sequence}", self.record.session_id())),
            session_id: self.record.session_id().clone(),
            run_id,
            sequence,
            kind,
        });
    }
}

fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use artist_core::{InitialContext, TranscriptEntryKind};
    use artist_store::MemoryStore;
    use futures::stream;

    use super::*;

    struct ControlledModel(Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Control>>>);

    struct RecordingModel {
        requests: mpsc::UnboundedSender<ModelRequest>,
        steering: mpsc::UnboundedSender<Vec<SteeringNotice>>,
    }

    enum Control {
        Event(ModelEvent),
        Error(ModelError),
        Boundary,
    }

    impl ControlledModel {
        fn new() -> (Arc<Self>, mpsc::UnboundedSender<Control>) {
            let (tx, rx) = mpsc::unbounded_channel();
            (Arc::new(Self(Arc::new(tokio::sync::Mutex::new(rx)))), tx)
        }
    }

    impl StreamingModel for ControlledModel {
        fn stream(&self, _: ModelRequest, steering: Steering) -> ModelStream {
            let receiver = self.0.clone();
            Box::pin(stream::unfold(
                (receiver, steering),
                |(receiver, steering)| async move {
                    let control = receiver.lock().await.recv().await?;
                    let event = match control {
                        Control::Event(event) => Ok(event),
                        Control::Error(error) => Err(error),
                        Control::Boundary => {
                            steering.take().await;
                            Ok(ModelEvent::TextDelta(String::new()))
                        }
                    };
                    Some((event, (receiver, steering)))
                },
            ))
        }
    }

    impl StreamingModel for RecordingModel {
        fn stream(&self, request: ModelRequest, steering: Steering) -> ModelStream {
            self.requests.send(request).unwrap();
            let observed = self.steering.clone();
            Box::pin(stream::once(async move {
                observed.send(steering.take().await).unwrap();
                Ok(ModelEvent::Finished {
                    output: Some("done".into()),
                })
            }))
        }
    }

    async fn fixture() -> (
        SessionHandle,
        Arc<MemoryStore>,
        mpsc::UnboundedSender<Control>,
    ) {
        let store = Arc::new(MemoryStore::default());
        let (model, control) = ControlledModel::new();
        let handle = SessionHandle::create(
            SessionId::from("session"),
            InitialContext { fragments: vec![] },
            store.clone(),
            model,
        )
        .await
        .unwrap();
        (handle, store, control)
    }

    async fn wait_for(
        events: &mut broadcast::Receiver<StreamEvent>,
        predicate: impl Fn(&StreamEventKind) -> bool,
    ) {
        loop {
            if predicate(&events.recv().await.unwrap().kind) {
                return;
            }
        }
    }

    #[tokio::test]
    async fn completes_streamed_content() {
        let (session, store, control) = fixture().await;
        let mut events = session.subscribe();
        session.input(Source::User, "hello").await.unwrap();
        control
            .send(Control::Event(ModelEvent::TextDelta("hel".into())))
            .unwrap();
        control
            .send(Control::Event(ModelEvent::TextDelta("lo".into())))
            .unwrap();
        control
            .send(Control::Event(ModelEvent::Finished { output: None }))
            .unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::Completed { .. })
        })
        .await;

        let record = store.load(&SessionId::from("session")).await.unwrap();
        let tail = &record.entries()[record.entries().len() - 2..];
        assert!(matches!(tail[0].kind,
            TranscriptEntryKind::AssistantMessage { ref content, .. } if content == &vec![artist_core::ContentPart::text("hel"), artist_core::ContentPart::text("lo")]));
        assert!(matches!(
            tail[1].kind,
            TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Completed { .. },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn steering_waits_for_a_boundary_and_never_interrupts() {
        let (session, store, control) = fixture().await;
        let mut events = session.subscribe();
        session.input(Source::User, "work").await.unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::RunStarted { .. })
        })
        .await;
        session.steer(Source::User, "small note").await.unwrap();

        let before = store.load(&SessionId::from("session")).await.unwrap();
        assert!(!before.entries().iter().any(|entry| matches!(
            entry.kind,
            TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Interrupted { .. },
                ..
            }
        )));
        assert!(
            !before
                .entries()
                .iter()
                .any(|entry| matches!(entry.kind, TranscriptEntryKind::SteeringDelivered { .. }))
        );

        control.send(Control::Boundary).unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::SteeringDelivered { .. })
        })
        .await;
        control
            .send(Control::Event(ModelEvent::Finished { output: None }))
            .unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::Completed { .. })
        })
        .await;

        let after = store.load(&SessionId::from("session")).await.unwrap();
        assert!(
            after
                .entries()
                .iter()
                .any(|entry| matches!(entry.kind, TranscriptEntryKind::SteeringDelivered { .. }))
        );
        assert!(!after.entries().iter().any(|entry| matches!(
            entry.kind,
            TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Interrupted { .. },
                ..
            }
        )));
    }

    #[tokio::test]
    async fn undelivered_steering_waits_for_the_next_run() {
        let (session, store, control) = fixture().await;
        let mut events = session.subscribe();
        session.input(Source::User, "first").await.unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::RunStarted { .. })
        })
        .await;
        session.steer(Source::Harness, "notice").await.unwrap();
        control
            .send(Control::Event(ModelEvent::Finished {
                output: Some("done".into()),
            }))
            .unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::Completed { .. })
        })
        .await;

        session.input(Source::User, "second").await.unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::RunStarted { .. })
        })
        .await;
        control.send(Control::Boundary).unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::SteeringDelivered { .. })
        })
        .await;

        let record = store.load(&SessionId::from("session")).await.unwrap();
        let delivered_run = record.entries().iter().find_map(|entry| match &entry.kind {
            TranscriptEntryKind::SteeringDelivered { run_id, .. } => Some(run_id),
            _ => None,
        });
        let runs: Vec<_> = record
            .entries()
            .iter()
            .filter_map(|entry| match &entry.kind {
                TranscriptEntryKind::RunStarted { run_id, .. } => Some(run_id),
                _ => None,
            })
            .collect();
        assert_eq!(delivered_run, runs.get(1).copied());
    }

    #[tokio::test]
    async fn abort_records_partial_then_interruption() {
        let (session, store, control) = fixture().await;
        let mut events = session.subscribe();
        session.input(Source::User, "work").await.unwrap();
        control
            .send(Control::Event(ModelEvent::TextDelta("partial".into())))
            .unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::TextDelta { .. })
        })
        .await;
        session.abort(InterruptionCause::User).await.unwrap();

        let record = store.load(&SessionId::from("session")).await.unwrap();
        let tail = &record.entries()[record.entries().len() - 2..];
        assert!(
            matches!(tail[0].kind, TranscriptEntryKind::AssistantMessage { ref content, .. } if content == &vec![artist_core::ContentPart::text("partial")])
        );
        assert!(matches!(
            tail[1].kind,
            TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Interrupted {
                    cause: InterruptionCause::User,
                    ..
                },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn failure_records_partial_content_and_typed_failure() {
        let (session, store, control) = fixture().await;
        let mut events = session.subscribe();
        session.input(Source::User, "work").await.unwrap();
        control
            .send(Control::Event(ModelEvent::TextDelta("partial".into())))
            .unwrap();
        control
            .send(Control::Error(ModelError::new("provider failed")))
            .unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::Failed { .. })
        })
        .await;

        let record = store.load(&SessionId::from("session")).await.unwrap();
        let tail = &record.entries()[record.entries().len() - 2..];
        assert!(matches!(
            tail[0].kind,
            TranscriptEntryKind::AssistantMessage {
                ref content,
                ..
            } if content == &vec![artist_core::ContentPart::text("partial")]
        ));
        assert!(matches!(
            tail[1].kind,
            TranscriptEntryKind::RunFinished { outcome: RunOutcome::Failed { ref error, .. }, .. } if error == "provider failed"
        ));
    }

    #[tokio::test]
    async fn failure_before_content_does_not_invent_an_assistant_message() {
        let (session, store, control) = fixture().await;
        let mut events = session.subscribe();
        session.input(Source::User, "work").await.unwrap();
        control
            .send(Control::Error(ModelError::new("no response")))
            .unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::Failed { .. })
        })
        .await;

        let record = store.load(&SessionId::from("session")).await.unwrap();
        assert!(matches!(
            record.entries().last().unwrap().kind,
            TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Failed { .. },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn busy_input_starts_after_the_current_answer() {
        let (session, store, control) = fixture().await;
        let mut events = session.subscribe();
        session.input(Source::User, "first").await.unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::RunStarted { .. })
        })
        .await;
        session.input(Source::User, "second").await.unwrap();
        control
            .send(Control::Event(ModelEvent::TextDelta("one".into())))
            .unwrap();
        control
            .send(Control::Event(ModelEvent::Finished { output: None }))
            .unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::Completed { .. })
        })
        .await;
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::RunStarted { .. })
        })
        .await;
        control
            .send(Control::Event(ModelEvent::TextDelta("two".into())))
            .unwrap();
        control
            .send(Control::Event(ModelEvent::Finished { output: None }))
            .unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::Completed { .. })
        })
        .await;

        let record = store.load(&SessionId::from("session")).await.unwrap();
        let messages: Vec<_> = project(&record)
            .1
            .into_iter()
            .map(|item| item.message)
            .collect();
        assert_eq!(
            messages,
            vec![
                crate::ModelMessage::User("first".into()),
                crate::ModelMessage::Assistant(vec![artist_core::ContentPart::text("one")]),
                crate::ModelMessage::User("second".into()),
                crate::ModelMessage::Assistant(vec![artist_core::ContentPart::text("two")]),
            ]
        );
    }

    #[tokio::test]
    async fn replacement_is_explicit_abort_then_input() {
        let (session, store, control) = fixture().await;
        let mut events = session.subscribe();
        session.input(Source::User, "old").await.unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::RunStarted { .. })
        })
        .await;
        session.abort(InterruptionCause::User).await.unwrap();
        session.input(Source::User, "new").await.unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::RunStarted { .. })
        })
        .await;
        control
            .send(Control::Event(ModelEvent::Finished {
                output: Some("new answer".into()),
            }))
            .unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::Completed { .. })
        })
        .await;

        let record = store.load(&SessionId::from("session")).await.unwrap();
        assert_eq!(
            record
                .entries()
                .iter()
                .filter(|entry| matches!(entry.kind, TranscriptEntryKind::RunStarted { .. }))
                .count(),
            2
        );
        assert_eq!(
            record
                .entries()
                .iter()
                .filter(|entry| matches!(
                    entry.kind,
                    TranscriptEntryKind::RunFinished {
                        outcome: RunOutcome::Interrupted { .. },
                        ..
                    }
                ))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn resume_closes_an_unfinished_run() {
        let store = Arc::new(MemoryStore::default());
        let mut record = SessionRecord::new(
            SessionId::from("resume"),
            InitialContext { fragments: vec![] },
        );
        let input = MessageId::from("input");
        for kind in [
            TranscriptEntryKind::Input {
                message_id: input.clone(),
                source: Source::User,
                content: "hello".into(),
            },
            TranscriptEntryKind::RunStarted {
                run_id: RunId::from("unfinished"),
                input_id: input,
            },
        ] {
            let entry = record.entry(kind);
            record.append(entry).unwrap();
        }
        store.create(record).await.unwrap();
        let (model, _) = ControlledModel::new();
        let _session = SessionHandle::resume(&SessionId::from("resume"), store.clone(), model)
            .await
            .unwrap();

        let record = store.load(&SessionId::from("resume")).await.unwrap();
        let tail = &record.entries()[record.entries().len() - 2..];
        assert!(matches!(
            tail[0].kind,
            TranscriptEntryKind::AssistantMessage { .. }
        ));
        assert!(matches!(
            tail[1].kind,
            TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Interrupted {
                    cause: InterruptionCause::Harness { .. },
                    ..
                },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn resume_reuses_an_existing_partial_message() {
        let store = Arc::new(MemoryStore::default());
        let mut record = SessionRecord::new(
            SessionId::from("resume-partial"),
            InitialContext { fragments: vec![] },
        );
        let entries = record.entries_for([
            TranscriptEntryKind::Input {
                message_id: MessageId::from("input"),
                source: Source::User,
                content: "hello".into(),
            },
            TranscriptEntryKind::RunStarted {
                run_id: RunId::from("unfinished"),
                input_id: MessageId::from("input"),
            },
            TranscriptEntryKind::AssistantMessage {
                message_id: MessageId::from("existing-partial"),
                run_id: RunId::from("unfinished"),
                content: vec![artist_core::ContentPart::text("preserved")],
            },
        ]);
        record.append_batch(&entries).unwrap();
        store.create(record).await.unwrap();
        let (model, _) = ControlledModel::new();
        let _session =
            SessionHandle::resume(&SessionId::from("resume-partial"), store.clone(), model)
                .await
                .unwrap();

        let record = store
            .load(&SessionId::from("resume-partial"))
            .await
            .unwrap();
        assert_eq!(record.entries().len(), 4);
        assert!(matches!(
            record.entries().last().unwrap().kind,
            TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Interrupted {
                    ref message_id,
                    ..
                },
                ..
            } if message_id.as_str() == "existing-partial"
        ));
    }

    #[tokio::test]
    async fn resume_reconstructs_pending_inputs_and_steering_in_order() {
        let store = Arc::new(MemoryStore::default());
        let mut record = SessionRecord::new(
            SessionId::from("resume-queues"),
            InitialContext { fragments: vec![] },
        );
        let entries = record.entries_for([
            TranscriptEntryKind::Input {
                message_id: MessageId::from("first"),
                source: Source::User,
                content: "first pending".into(),
            },
            TranscriptEntryKind::SteeringQueued {
                message_id: MessageId::from("notice-1"),
                source: Source::Harness,
                content: "first notice".into(),
            },
            TranscriptEntryKind::SteeringQueued {
                message_id: MessageId::from("notice-2"),
                source: Source::User,
                content: "second notice".into(),
            },
            TranscriptEntryKind::Input {
                message_id: MessageId::from("second"),
                source: Source::User,
                content: "second pending".into(),
            },
        ]);
        record.append_batch(&entries).unwrap();
        store.create(record).await.unwrap();

        let (requests_tx, mut requests_rx) = mpsc::unbounded_channel();
        let (steering_tx, mut steering_rx) = mpsc::unbounded_channel();
        let model = Arc::new(RecordingModel {
            requests: requests_tx,
            steering: steering_tx,
        });
        let _session =
            SessionHandle::resume(&SessionId::from("resume-queues"), store.clone(), model)
                .await
                .unwrap();

        assert_eq!(requests_rx.recv().await.unwrap().prompt, "first pending");
        let notices = steering_rx.recv().await.unwrap();
        assert_eq!(
            notices
                .iter()
                .map(|notice| notice.content.as_str())
                .collect::<Vec<_>>(),
            ["first notice", "second notice"]
        );
        assert_eq!(requests_rx.recv().await.unwrap().prompt, "second pending");
        assert!(steering_rx.recv().await.unwrap().is_empty());

        tokio::task::yield_now().await;
        let record = store.load(&SessionId::from("resume-queues")).await.unwrap();
        let started: Vec<_> = record
            .entries()
            .iter()
            .filter_map(|entry| match &entry.kind {
                TranscriptEntryKind::RunStarted { input_id, .. } => Some(input_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(started, ["first", "second"]);
        let delivered: Vec<_> = record
            .entries()
            .iter()
            .filter_map(|entry| match &entry.kind {
                TranscriptEntryKind::SteeringDelivered { message_ids, .. } => Some(message_ids),
                _ => None,
            })
            .flatten()
            .map(|id| id.as_str())
            .collect();
        assert_eq!(delivered, ["notice-1", "notice-2"]);
    }
}
