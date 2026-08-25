use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
    time::Instant,
};

use artist_core::{
    CallId, Command, ContentPart, CorrelationId, EventId, InitialContext, InterruptionCause,
    InvocationScope, MessageId, PluginFact, ProfileSnapshot, RunId, RunOutcome, SessionId,
    SessionMetadata, SessionRecord, SlashCommandAction, SlashCommandId, SlashCommandResult, Source,
    StreamEvent, StreamEventKind, ToolControl, TranscriptEntryKind,
};
use artist_store::{SessionStore, StoreError};
use async_trait::async_trait;
use futures::StreamExt;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::{
    ExecutionExtensionError, ExecutionExtensions, FactDrain, FactEnvelope, HookPhase,
    LifecycleHookDecision, LifecycleHookEvent, ModelError, ModelEvent, ModelRequest, ModelStream,
    Steering, SteeringNotice, StreamingModel, fact_channel, no_extensions, project,
};

const CHANNEL_CAPACITY: usize = 64;

#[async_trait]
pub trait ProfileSource: Send + Sync + 'static {
    async fn load(&self, name: &str) -> Result<ProfileSnapshot, String>;
}

#[async_trait]
pub trait SlashCommandSource: Send + Sync + 'static {
    async fn invoke(&self, name: &str, arguments: &str) -> Result<SlashCommandResult, String>;
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("session task has stopped")]
    Closed,
    #[error("profile failed: {0}")]
    Profile(String),
    #[error("slash command failed: {0}")]
    SlashCommand(String),
    #[error(transparent)]
    Extension(#[from] ExecutionExtensionError),
    #[error("invalid initial context: {0}")]
    InvalidContext(String),
    #[error("lineage parent session does not exist: {0}")]
    MissingParent(SessionId),
}

#[derive(Clone)]
pub struct SessionDependencies {
    pub store: Arc<dyn SessionStore>,
    pub model: Arc<dyn StreamingModel>,
    pub profiles: Option<Arc<dyn ProfileSource>>,
    pub slash_commands: Option<Arc<dyn SlashCommandSource>>,
    pub extensions: Arc<dyn ExecutionExtensions>,
    pub resume_queued_work: bool,
}

impl SessionDependencies {
    pub fn new(store: Arc<dyn SessionStore>, model: Arc<dyn StreamingModel>) -> Self {
        Self {
            store,
            model,
            profiles: None,
            slash_commands: None,
            extensions: no_extensions(),
            resume_queued_work: true,
        }
    }
}

pub struct CreateSession {
    pub session_id: SessionId,
    pub metadata: SessionMetadata,
    pub context: InitialContext,
    pub initial_profile: Option<String>,
}

#[derive(Clone)]
pub struct SessionHandle {
    commands: mpsc::Sender<Envelope>,
    events: broadcast::Sender<StreamEvent>,
}

impl SessionHandle {
    #[cfg(test)]
    async fn create_unprofiled(
        session_id: SessionId,
        context: InitialContext,
        store: Arc<dyn SessionStore>,
        model: Arc<dyn StreamingModel>,
    ) -> Result<Self, SessionError> {
        Self::create(
            CreateSession {
                session_id,
                metadata: SessionMetadata::root(0, None),
                context,
                initial_profile: None,
            },
            SessionDependencies::new(store, model),
        )
        .await
    }

    #[cfg(test)]
    async fn resume_unprofiled(
        session_id: &SessionId,
        store: Arc<dyn SessionStore>,
        model: Arc<dyn StreamingModel>,
    ) -> Result<Self, SessionError> {
        Self::resume(session_id, SessionDependencies::new(store, model)).await
    }

    pub async fn create(
        request: CreateSession,
        dependencies: SessionDependencies,
    ) -> Result<Self, SessionError> {
        let context = dependencies
            .extensions
            .compose_initial_context(request.context)
            .await?;
        // Composed prompt fragments are validated once, before the canonical
        // record exists; malformed or oversized output never becomes durable.
        context.validate().map_err(SessionError::InvalidContext)?;
        // Creation lineage is validated at the kernel/store boundary: a child
        // session can never be created without a resolvable parent record.
        if let Some(lineage) = &request.metadata.lineage {
            dependencies
                .store
                .load(&lineage.parent_session_id)
                .await
                .map_err(|_| SessionError::MissingParent(lineage.parent_session_id.clone()))?;
        }
        let mut record =
            SessionRecord::new_with_metadata(request.session_id, request.metadata, context);
        if let Some(initial_profile) = request.initial_profile.as_deref() {
            let profiles = dependencies.profiles.as_ref().ok_or_else(|| {
                SessionError::Profile("initial profile requires a profile source".into())
            })?;
            let profile = profiles
                .load(initial_profile)
                .await
                .map_err(SessionError::Profile)?;
            let activation = record.entry(TranscriptEntryKind::ProfileActivated {
                profile,
                brief: None,
                steering_message_ids: Vec::new(),
            });
            record
                .append(activation)
                .expect("initial profile activation is valid");
        }
        dependencies.store.create(record.clone()).await?;
        Ok(Self::spawn(record, dependencies))
    }

    pub async fn resume(
        session_id: &SessionId,
        dependencies: SessionDependencies,
    ) -> Result<Self, SessionError> {
        let mut record = dependencies.store.load(session_id).await?;
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
            dependencies
                .store
                .append(session_id, record.next_sequence(), &entries)
                .await?;
            record
                .append_batch(&entries)
                .expect("resume reconciliation is valid");
        }
        if dependencies.profiles.is_some() && record.current_profile().is_none() {
            return Err(SessionError::Profile(
                "cannot profiled-resume a session without a profile activation".into(),
            ));
        }
        Ok(Self::spawn(record, dependencies))
    }

    fn spawn(record: SessionRecord, dependencies: SessionDependencies) -> Self {
        let (command_tx, command_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (event_tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        let handle = Self {
            commands: command_tx,
            events: event_tx.clone(),
        };
        tokio::spawn(async move {
            Session::new(record, dependencies, command_rx, event_tx)
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
        self.input_content(source, vec![ContentPart::text(content)])
            .await
    }

    pub async fn input_content(
        &self,
        source: Source,
        content: Vec<ContentPart>,
    ) -> Result<(), SessionError> {
        self.send(Command::Input { source, content }).await
    }

    pub async fn steer(
        &self,
        source: Source,
        content: impl Into<String>,
    ) -> Result<(), SessionError> {
        self.steer_content(source, vec![ContentPart::text(content)])
            .await
    }

    pub async fn steer_content(
        &self,
        source: Source,
        content: Vec<ContentPart>,
    ) -> Result<(), SessionError> {
        self.send(Command::Steer { source, content }).await
    }

    pub async fn abort(&self, cause: InterruptionCause) -> Result<(), SessionError> {
        self.send(Command::Abort { cause }).await
    }

    pub async fn slash(
        &self,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Result<SlashCommandResult, SessionError> {
        let response = self
            .request(Command::Slash {
                name: name.into(),
                arguments: arguments.into(),
            })
            .await?;
        response.ok_or_else(|| {
            SessionError::SlashCommand("slash invocation returned no command result".into())
        })
    }

    async fn send(&self, command: Command) -> Result<(), SessionError> {
        self.request(command).await.map(|_| ())
    }

    async fn request(&self, command: Command) -> Result<Option<SlashCommandResult>, SessionError> {
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
    reply: oneshot::Sender<Result<Option<SlashCommandResult>, SessionError>>,
}

struct PendingInput {
    message_id: MessageId,
    content: Vec<ContentPart>,
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
    profiles: Option<Arc<dyn ProfileSource>>,
    slash_commands: Option<Arc<dyn SlashCommandSource>>,
    extensions: Arc<dyn ExecutionExtensions>,
    commands: mpsc::Receiver<Envelope>,
    events: broadcast::Sender<StreamEvent>,
    event_sequence: u64,
    inputs: VecDeque<PendingInput>,
    steering: Steering,
    delivered: mpsc::UnboundedReceiver<Vec<MessageId>>,
    facts: Option<FactDrain>,
    active: Option<ActiveRun>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.extensions.unbind_fact_sink(self.record.session_id());
    }
}

impl Session {
    fn new(
        record: SessionRecord,
        dependencies: SessionDependencies,
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
                TranscriptEntryKind::SteeringDelivered { message_ids, .. }
                | TranscriptEntryKind::ProfileActivated {
                    steering_message_ids: message_ids,
                    ..
                } => Some(message_ids),
                _ => None,
            })
            .flatten()
            .cloned()
            .collect();
        let superseded_inputs: HashSet<_> = record
            .entries()
            .iter()
            .filter_map(|entry| match &entry.kind {
                TranscriptEntryKind::InputsSuperseded { message_ids } => Some(message_ids),
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
                } if !started_inputs.contains(message_id)
                    && !superseded_inputs.contains(message_id) =>
                {
                    Some(PendingInput {
                        message_id: message_id.clone(),
                        content: content.clone(),
                    })
                }
                _ => None,
            })
            .filter(|_| dependencies.resume_queued_work)
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
            .filter(|_| dependencies.resume_queued_work)
            .collect();
        let (steering, delivered) = Steering::channel_with(queued_steering);
        let (fact_sink, fact_drain) = fact_channel();
        dependencies
            .extensions
            .bind_fact_sink(record.session_id(), fact_sink);
        Self {
            record,
            store: dependencies.store,
            model: dependencies.model,
            profiles: dependencies.profiles,
            slash_commands: dependencies.slash_commands,
            extensions: dependencies.extensions,
            commands,
            events,
            event_sequence: 0,
            inputs,
            steering,
            delivered,
            facts: Some(fact_drain),
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
                Fact(Option<FactEnvelope>),
                Model(Option<Result<ModelEvent, ModelError>>),
            }

            let wake = {
                let active = self.active.as_mut().expect("checked above");
                let facts = &mut self.facts;
                tokio::select! {
                    command = self.commands.recv() => Wake::Command(command),
                    delivered = self.delivered.recv() => Wake::Delivered(delivered),
                    fact = async {
                        match facts.as_mut() {
                            Some(rx) => rx.recv().await,
                            None => std::future::pending().await,
                        }
                    } => Wake::Fact(fact),
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
                Wake::Fact(Some(envelope)) => {
                    if self.commit_facts(envelope).await.is_err() {
                        return;
                    }
                }
                Wake::Fact(None) => {
                    // Every sender dropped; stop selecting on a closed channel.
                    self.facts = None;
                }
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

    async fn apply_command(
        &mut self,
        command: Command,
    ) -> Result<Option<SlashCommandResult>, SessionError> {
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
            Command::Slash { name, arguments } => {
                validate_slash_command_name(&name).map_err(SessionError::SlashCommand)?;
                let source = self.slash_commands.as_ref().ok_or_else(|| {
                    SessionError::SlashCommand("no slash-command source is configured".into())
                })?;
                let result = source
                    .invoke(&name, &arguments)
                    .await
                    .map_err(SessionError::SlashCommand)?;
                let command_id = SlashCommandId::new(format!(
                    "{}:slash:{}",
                    self.record.session_id(),
                    self.record.next_sequence()
                ));
                self.append(vec![TranscriptEntryKind::SlashCommand {
                    command_id: command_id.clone(),
                    name: name.clone(),
                    arguments,
                    output: result.output.clone(),
                    actions: result.actions.clone(),
                }])
                .await?;
                self.emit(
                    self.active.as_ref().map(|run| run.id.clone()),
                    StreamEventKind::SlashCommandCompleted {
                        command_id,
                        name,
                        output: result.output.clone(),
                    },
                );
                for action in result.actions.clone() {
                    self.apply_slash_action(action).await?;
                }
                return Ok(Some(result));
            }
        }
        Ok(None)
    }

    async fn apply_slash_action(&mut self, action: SlashCommandAction) -> Result<(), SessionError> {
        match action {
            SlashCommandAction::Input { content } => {
                let message_id = self.message_id();
                self.append(vec![TranscriptEntryKind::Input {
                    message_id: message_id.clone(),
                    source: Source::Harness,
                    content: content.clone(),
                }])
                .await?;
                self.inputs.push_back(PendingInput {
                    message_id: message_id.clone(),
                    content,
                });
                self.emit(None, StreamEventKind::InputQueued { message_id });
            }
            SlashCommandAction::Steer { content } => {
                let message_id = self.message_id();
                self.append(vec![TranscriptEntryKind::SteeringQueued {
                    message_id: message_id.clone(),
                    source: Source::Harness,
                    content: content.clone(),
                }])
                .await?;
                self.steering
                    .push(SteeringNotice {
                        message_id: message_id.clone(),
                        source: Source::Harness,
                        content,
                    })
                    .await;
                self.emit(
                    self.active.as_ref().map(|run| run.id.clone()),
                    StreamEventKind::SteeringQueued { message_id },
                );
            }
            SlashCommandAction::Abort { reason } => {
                if self.active.is_some() {
                    self.interrupt(InterruptionCause::Harness { reason })
                        .await?;
                }
            }
            SlashCommandAction::ActivateProfile { profile, brief } => {
                self.activate_profile_from_command(profile, brief).await?;
            }
        }
        Ok(())
    }

    async fn activate_profile_from_command(
        &mut self,
        profile_name: String,
        brief: Option<String>,
    ) -> Result<(), SessionError> {
        let profiles = self.profiles.as_ref().ok_or_else(|| {
            SessionError::Profile("profile activation requires a configured profile source".into())
        })?;
        let profile = profiles
            .load(&profile_name)
            .await
            .map_err(SessionError::Profile)?;
        if self.active.is_some() {
            self.interrupt(InterruptionCause::Harness {
                reason: format!("slash command activated profile `{profile_name}`"),
            })
            .await?;
        }
        let steering = self.steering.drain_for_handoff().await;
        let steering_ids = steering
            .iter()
            .map(|notice| notice.message_id.clone())
            .collect::<Vec<_>>();
        let combined_brief = combine_brief(brief.as_deref(), &steering);
        let superseded = self
            .inputs
            .drain(..)
            .map(|input| input.message_id)
            .collect::<Vec<_>>();
        let mut entries = Vec::new();
        if !superseded.is_empty() {
            entries.push(TranscriptEntryKind::InputsSuperseded {
                message_ids: superseded,
            });
        }
        entries.push(TranscriptEntryKind::ProfileActivated {
            profile: profile.clone(),
            brief: (!combined_brief.is_empty()).then(|| combined_brief.clone()),
            steering_message_ids: steering_ids,
        });
        let input_id = (!combined_brief.is_empty()).then(|| self.message_id_at(entries.len()));
        if let Some(input_id) = &input_id {
            entries.push(TranscriptEntryKind::Input {
                message_id: input_id.clone(),
                source: Source::Harness,
                content: combined_brief.clone(),
            });
        }
        self.append(entries).await?;
        self.emit(None, StreamEventKind::ProfileActivated { profile });
        if let Some(message_id) = input_id {
            self.inputs.push_back(PendingInput {
                message_id: message_id.clone(),
                content: combined_brief,
            });
            self.emit(None, StreamEventKind::InputQueued { message_id });
        }
        Ok(())
    }

    async fn start(&mut self, input: PendingInput) -> Result<(), SessionError> {
        let run_id = RunId::new(format!(
            "{}:run:{}",
            self.record.session_id(),
            self.record.next_sequence()
        ));
        let (context, mut history) = project(&self.record);
        if matches!(history.last(), Some(crate::ModelHistoryItem { message: crate::ModelMessage::User(content), .. }) if content == &input.content)
        {
            history.pop();
        }
        let messages_in = history.len() + 1;
        let mut request = ModelRequest {
            session_id: self.record.session_id().clone(),
            run_id: run_id.clone(),
            context,
            prompt: input.content,
            history,
            profile: self.record.current_profile().cloned().map(Arc::new),
            profile_epoch: self.record.current_profile_epoch(),
            selected_model: None,
            extensions: self.extensions.clone(),
        };
        let scope = InvocationScope {
            session_id: self.record.session_id().clone(),
            run_id: Some(run_id.clone()),
            call_id: None,
            correlation_id: CorrelationId::new(format!("{run_id}:request")),
            parent_correlation_id: None,
        };
        let preparation = self.extensions.prepare_model_request(&mut request).await;
        let hook = if preparation.is_ok() {
            self.extensions
                .hook(LifecycleHookEvent {
                    phase: HookPhase::BeforeModelRequest,
                    scope,
                    payload: serde_json::json!({
                        "context": request.context,
                        "prompt": request.prompt,
                        "history_items": request.history.len(),
                    }),
                })
                .await
        } else {
            Ok(Vec::new())
        };
        let mut rejection = preparation.err().map(|error| error.to_string());
        if rejection.is_none() {
            match hook {
                Err(error) => rejection = Some(error.to_string()),
                Ok(decisions) => {
                    for decision in decisions {
                        match decision {
                            LifecycleHookDecision::Proceed => {}
                            LifecycleHookDecision::Stop { reason } => {
                                rejection = Some(reason);
                                break;
                            }
                            LifecycleHookDecision::Rewrite { value } => {
                                let Some(object) = value.as_object() else {
                                    rejection = Some(
                                        "before-model-request rewrite must be a JSON object".into(),
                                    );
                                    break;
                                };
                                if object.keys().any(|key| key != "context" && key != "prompt") {
                                    rejection = Some(
                                        "before-model-request rewrite may change only context or prompt"
                                            .into(),
                                    );
                                    break;
                                }
                                if let Some(context) = object.get("context") {
                                    let Some(context) = context.as_str() else {
                                        rejection =
                                            Some("rewritten context must be a string".into());
                                        break;
                                    };
                                    request.context = context.to_owned();
                                }
                                if let Some(prompt) = object.get("prompt") {
                                    match serde_json::from_value::<Vec<ContentPart>>(prompt.clone())
                                    {
                                        Ok(prompt) if !prompt.is_empty() => request.prompt = prompt,
                                        _ => {
                                            rejection = Some(
                                                "rewritten prompt must be non-empty ordered content"
                                                    .into(),
                                            );
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(error) = rejection {
            self.append(vec![
                TranscriptEntryKind::RunStarted {
                    run_id: run_id.clone(),
                    input_id: input.message_id,
                },
                TranscriptEntryKind::RunFinished {
                    run_id: run_id.clone(),
                    outcome: RunOutcome::Failed {
                        message_id: None,
                        error: error.clone(),
                    },
                },
            ])
            .await?;
            self.flush_plugin_facts().await?;
            self.emit(
                Some(run_id.clone()),
                StreamEventKind::RunStarted { messages_in },
            );
            self.emit(
                Some(run_id),
                StreamEventKind::Failed {
                    failure: artist_core::ModelFailure {
                        message: error,
                        class: artist_core::FailureClass::InvalidRequest,
                        retriable: false,
                        provider_code: None,
                        http_status: None,
                        provider_request_id: None,
                    },
                },
            );
            return Ok(());
        }

        self.append(vec![TranscriptEntryKind::RunStarted {
            run_id: run_id.clone(),
            input_id: input.message_id,
        }])
        .await?;
        self.flush_plugin_facts().await?;
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
        self.flush_plugin_facts().await?;
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
            ModelEvent::ToolProgress(progress) => {
                self.emit(Some(run_id), StreamEventKind::ToolProgress { progress });
            }
            ModelEvent::Content(part) => {
                if matches!(
                    part,
                    artist_core::ContentPart::Reasoning { .. }
                        | artist_core::ContentPart::Attachment { .. }
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
                let summary_bytes = serde_json::to_vec(&artifact).map_or(0, |bytes| bytes.len());
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
            ModelEvent::Control { call_id, control } => match control {
                ToolControl::Yield { payload } => self.finish_yield(call_id, payload).await?,
                ToolControl::Handoff { profile, brief } => {
                    if let Err(error) = self.finish_handoff(call_id, profile, brief).await {
                        match error {
                            SessionError::Profile(message) => {
                                self.fail(ModelError::new(format!(
                                    "handoff profile activation failed: {message}"
                                )))
                                .await?;
                            }
                            other => return Err(other),
                        }
                    }
                }
            },
        }
        Ok(())
    }

    async fn finish_yield(
        &mut self,
        call_id: CallId,
        payload: serde_json::Value,
    ) -> Result<(), SessionError> {
        let active = self.active.take().expect("control without active run");
        let mut entries = Vec::new();
        if !active.content.is_empty() {
            entries.push(TranscriptEntryKind::AssistantMessage {
                message_id: self.message_id_at(entries.len()),
                run_id: active.id.clone(),
                content: active.content,
            });
        }
        entries.push(TranscriptEntryKind::RunFinished {
            run_id: active.id.clone(),
            outcome: RunOutcome::Yielded {
                call_id: call_id.clone(),
                payload: payload.clone(),
            },
        });
        self.append(entries).await?;
        self.emit(
            Some(active.id),
            StreamEventKind::Yielded { call_id, payload },
        );
        Ok(())
    }

    async fn finish_handoff(
        &mut self,
        call_id: CallId,
        profile_name: String,
        brief: String,
    ) -> Result<(), SessionError> {
        let profiles = self.profiles.as_ref().ok_or_else(|| {
            SessionError::Profile("handoff requires a configured profile source".into())
        })?;
        let profile = profiles
            .load(&profile_name)
            .await
            .map_err(SessionError::Profile)?;
        let steering = self.steering.drain_for_handoff().await;
        let steering_ids = steering
            .iter()
            .map(|notice| notice.message_id.clone())
            .collect::<Vec<_>>();
        let combined_brief = combine_brief(Some(&brief), &steering);
        let superseded = self
            .inputs
            .drain(..)
            .map(|input| input.message_id)
            .collect::<Vec<_>>();
        let active = self.active.take().expect("control without active run");
        let mut entries = Vec::new();
        if !active.content.is_empty() {
            entries.push(TranscriptEntryKind::AssistantMessage {
                message_id: self.message_id_at(entries.len()),
                run_id: active.id.clone(),
                content: active.content,
            });
        }
        if !superseded.is_empty() {
            entries.push(TranscriptEntryKind::InputsSuperseded {
                message_ids: superseded,
            });
        }
        entries.push(TranscriptEntryKind::RunFinished {
            run_id: active.id.clone(),
            outcome: RunOutcome::HandedOff {
                call_id: call_id.clone(),
                profile: profile_name.clone(),
            },
        });
        entries.push(TranscriptEntryKind::ProfileActivated {
            profile: profile.clone(),
            brief: Some(combined_brief.clone()),
            steering_message_ids: steering_ids,
        });
        let input_id = self.message_id_at(entries.len());
        entries.push(TranscriptEntryKind::Input {
            message_id: input_id.clone(),
            source: Source::Harness,
            content: combined_brief.clone(),
        });
        self.append(entries).await?;
        self.inputs.push_back(PendingInput {
            message_id: input_id.clone(),
            content: combined_brief,
        });
        self.emit(
            Some(active.id),
            StreamEventKind::HandedOff {
                call_id,
                profile: profile_name,
            },
        );
        self.emit(None, StreamEventKind::ProfileActivated { profile });
        self.emit(
            None,
            StreamEventKind::InputQueued {
                message_id: input_id,
            },
        );
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

    async fn persist_fact(&mut self, fact: PluginFact) -> Result<(), SessionError> {
        match fact {
            PluginFact::SchemaRegistered { schema } => {
                if let Some(existing) = self.record.plugin_event_schema(&schema.schema_id) {
                    if existing != &schema {
                        return Err(SessionError::Extension(ExecutionExtensionError::new(
                            "plugin-event-schema",
                            "a schema ID changed within one canonical session",
                        )));
                    }
                    return Ok(());
                }
                self.append(vec![TranscriptEntryKind::PluginEventSchemaRegistered {
                    schema: schema.clone(),
                }])
                .await?;
                self.emit(
                    None,
                    StreamEventKind::PluginEventSchemaRegistered { schema },
                );
            }
            PluginFact::Event { event } => {
                let run_id = event.scope.run_id.clone();
                self.append(vec![TranscriptEntryKind::PluginEvent {
                    event: event.clone(),
                }])
                .await?;
                self.emit(
                    run_id,
                    StreamEventKind::PluginEvent {
                        plugin_event: event,
                    },
                );
            }
        }
        Ok(())
    }

    /// Durably append one emission batch from the bound fact sink and resolve
    /// the emitter's receipt. The emitter's success therefore implies the
    /// facts are canonical and their stream events are published.
    async fn commit_facts(&mut self, envelope: FactEnvelope) -> Result<(), SessionError> {
        let mut outcome = Ok(());
        for fact in envelope.facts {
            if let Err(error) = self.persist_fact(fact).await {
                outcome = Err(error.to_string());
                break;
            }
        }
        let _ = envelope.receipt.send(outcome.clone());
        // A failed append is a durable-store failure: the session cannot
        // continue safely after losing canonical ordering.
        if outcome.is_err() {
            return Err(SessionError::Closed);
        }
        Ok(())
    }

    async fn flush_plugin_facts(&mut self) -> Result<(), SessionError> {
        // Drain any synchronously committed envelopes that arrived between
        // extension calls first; they precede anything still sitting in the
        // legacy outbox.
        while let Some(envelope) = self.pending_fact().await {
            self.commit_facts(envelope).await?;
        }
        let facts = self
            .extensions
            .drain_plugin_facts(self.record.session_id())
            .await;
        for fact in facts {
            self.persist_fact(fact).await?;
        }
        Ok(())
    }

    async fn pending_fact(&mut self) -> Option<FactEnvelope> {
        match self.facts.as_mut() {
            Some(rx) => rx.try_recv().ok(),
            None => None,
        }
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

    fn message_id_at(&self, offset: usize) -> MessageId {
        MessageId::new(format!(
            "{}:message:{}",
            self.record.session_id(),
            self.record.next_sequence() + offset as u64
        ))
    }

    fn emit(&mut self, run_id: Option<RunId>, kind: StreamEventKind) {
        let sequence = self.event_sequence;
        self.event_sequence += 1;
        let event = StreamEvent {
            event_id: EventId::new(format!("{}:stream:{sequence}", self.record.session_id())),
            session_id: self.record.session_id().clone(),
            run_id,
            sequence,
            kind,
        };
        let _ = self.events.send(event.clone());
        self.extensions.observe_committed(event);
    }
}

fn combine_brief(brief: Option<&str>, steering: &[SteeringNotice]) -> Vec<ContentPart> {
    let mut combined = Vec::new();
    if let Some(brief) = brief.filter(|brief| !brief.is_empty()) {
        combined.push(ContentPart::text(brief));
    }
    for notice in steering {
        if !combined.is_empty() && !notice.content.is_empty() {
            combined.push(ContentPart::text("\n\n"));
        }
        combined.extend(notice.content.clone());
    }
    combined
}

fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn validate_slash_command_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.starts_with('/')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!(
            "invalid slash-command name `{name}`; use ASCII letters, digits, `-`, or `_`, without a leading slash"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use artist_core::{InitialContext, ProfilePolicy, TranscriptEntryKind, default_yield_schema};
    use artist_store::MemoryStore;
    use futures::stream;

    use super::*;

    struct ControlledModel(Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Control>>>);

    struct RecordingModel {
        requests: mpsc::UnboundedSender<ModelRequest>,
        steering: mpsc::UnboundedSender<Vec<SteeringNotice>>,
    }

    struct StaticProfiles(HashMap<String, ProfileSnapshot>);

    struct StaticSlashCommands(HashMap<String, SlashCommandResult>);

    #[async_trait]
    impl ProfileSource for StaticProfiles {
        async fn load(&self, name: &str) -> Result<ProfileSnapshot, String> {
            self.0
                .get(name)
                .cloned()
                .ok_or_else(|| format!("unknown profile `{name}`"))
        }
    }

    #[async_trait]
    impl SlashCommandSource for StaticSlashCommands {
        async fn invoke(&self, name: &str, _arguments: &str) -> Result<SlashCommandResult, String> {
            self.0
                .get(name)
                .cloned()
                .ok_or_else(|| format!("unknown slash command `{name}`"))
        }
    }

    fn text(value: &str) -> Vec<ContentPart> {
        vec![ContentPart::text(value)]
    }

    fn test_profile(name: &str, catalog: &[&str]) -> ProfileSnapshot {
        ProfileSnapshot {
            name: name.into(),
            instructions: format!("{name} instructions"),
            yield_schema: default_yield_schema(),
            policy: ProfilePolicy::default(),
            models: Vec::new(),
            catalog: catalog.iter().map(|name| (*name).into()).collect(),
        }
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

    async fn create_profiled(
        id: &str,
        profile: &str,
        profiles: Arc<StaticProfiles>,
        commands: Option<Arc<StaticSlashCommands>>,
        store: Arc<MemoryStore>,
        model: Arc<dyn StreamingModel>,
    ) -> SessionHandle {
        let mut dependencies = SessionDependencies::new(store, model);
        dependencies.profiles = Some(profiles);
        dependencies.slash_commands =
            commands.map(|commands| commands as Arc<dyn SlashCommandSource>);
        SessionHandle::create(
            CreateSession {
                session_id: SessionId::from(id),
                metadata: SessionMetadata::root(0, Some(profile.into())),
                context: InitialContext { fragments: vec![] },
                initial_profile: Some(profile.into()),
            },
            dependencies,
        )
        .await
        .unwrap()
    }

    async fn fixture() -> (
        SessionHandle,
        Arc<MemoryStore>,
        mpsc::UnboundedSender<Control>,
    ) {
        let store = Arc::new(MemoryStore::default());
        let (model, control) = ControlledModel::new();
        let handle = SessionHandle::create_unprofiled(
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

    /// Deterministic fault injection at the durable append boundary: once the
    /// failure budget is exhausted every append fails; the kernel must
    /// surface the error and never persist a partial batch.
    use artist_store::SessionStore as _Trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct FaultyStore {
        inner: Arc<MemoryStore>,
        remaining: AtomicUsize,
    }
    #[async_trait]
    impl artist_store::SessionStore for FaultyStore {
        async fn create(&self, record: SessionRecord) -> Result<(), artist_store::StoreError> {
            if self.remaining.fetch_sub(1, Ordering::AcqRel) == 0 {
                return Err(artist_store::StoreError::Corrupt("injected fault".into()));
            }
            self.inner.create(record).await
        }
        async fn append(
            &self,
            session_id: &SessionId,
            expected_sequence: u64,
            entries: &[artist_core::TranscriptEntry],
        ) -> Result<u64, artist_store::StoreError> {
            if self.remaining.fetch_sub(1, Ordering::AcqRel) == 0 {
                return Err(artist_store::StoreError::Corrupt("injected fault".into()));
            }
            self.inner
                .append(session_id, expected_sequence, entries)
                .await
        }
        async fn load(
            &self,
            session_id: &SessionId,
        ) -> Result<SessionRecord, artist_store::StoreError> {
            // Loads are reads; they keep working after the fault trips.
            self.inner.load(session_id).await
        }
    }

    #[tokio::test]
    async fn an_append_fault_aborts_the_run_without_partial_batches() {
        // Budget covers create + input + run-start only.
        let faulty = Arc::new(FaultyStore {
            inner: Arc::new(MemoryStore::default()),
            remaining: std::sync::atomic::AtomicUsize::new(3),
        });
        let healthy_inner = faulty.inner.clone();
        let (model, control) = ControlledModel::new();
        let dependencies = SessionDependencies {
            store: faulty,
            model,
            profiles: None,
            slash_commands: None,
            extensions: no_extensions(),
            resume_queued_work: true,
        };
        let session = SessionHandle::create(
            CreateSession {
                session_id: SessionId::from("session"),
                metadata: SessionMetadata::root(0, None),
                context: InitialContext { fragments: vec![] },
                initial_profile: None,
            },
            dependencies,
        )
        .await
        .unwrap();
        session.input(Source::User, "hello").await.unwrap();
        control
            .send(Control::Event(ModelEvent::TextDelta("hel".into())))
            .unwrap();
        // Completion appends AssistantMessage+RunFinished -> budget trips.
        control
            .send(Control::Event(ModelEvent::Finished {
                output: Some("hello".into()),
            }))
            .unwrap();

        // Wait for the actor to die on the faulted append, then prove the
        // session is closed by sending another command.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let error = session
            .input(Source::User, "again")
            .await
            .expect_err("session must be closed after an append fault");
        assert!(
            matches!(error, SessionError::Closed) || error.to_string().contains("closed"),
            "{error}"
        );

        // The durable record holds a prefix of valid batches: input + run
        // start exist, but neither assistant message nor run finish leaked.
        let record = healthy_inner
            .load(&SessionId::from("session"))
            .await
            .unwrap();
        let kinds = record
            .entries()
            .iter()
            .map(|e| e.kind.clone())
            .collect::<Vec<_>>();
        assert!(
            kinds
                .iter()
                .any(|k| matches!(k, TranscriptEntryKind::RunStarted { .. }))
        );
        assert!(
            !kinds
                .iter()
                .any(|k| matches!(k, TranscriptEntryKind::AssistantMessage { .. }))
        );
        assert!(
            !kinds
                .iter()
                .any(|k| matches!(k, TranscriptEntryKind::RunFinished { .. }))
        );
    }

    struct EmittingModel {
        sink: Arc<tokio::sync::Mutex<Option<crate::FactSink>>>,
    }

    impl StreamingModel for EmittingModel {
        fn stream(&self, request: ModelRequest, _steering: Steering) -> ModelStream {
            let sink = self.sink.clone();
            Box::pin(futures::stream::once(async move {
                let sink = sink.lock().await.take().expect("fact sink bound");
                let schema = artist_core::PluginEventSchema {
                    schema_id: artist_core::EventSchemaId::from("emit-v1"),
                    plugin_id: artist_core::PluginId::from("example.emitter"),
                    event_type: "example.emitter.noticed".into(),
                    version: "1".into(),
                    payload_schema: serde_json::json!({"type": "object"}),
                    presentation_schema: serde_json::json!({"type": "object"}),
                    schema_digest: String::new(),
                    presentation: serde_json::json!({}),
                };
                let mut schema = schema;
                schema.schema_digest = schema.canonical_digest().unwrap();
                let event = artist_core::PluginEvent {
                    plugin_id: schema.plugin_id.clone(),
                    schema_id: schema.schema_id.clone(),
                    event_type: schema.event_type.clone(),
                    schema_version: schema.version.clone(),
                    schema_digest: schema.schema_digest.clone(),
                    scope: artist_core::InvocationScope {
                        session_id: request.session_id.clone(),
                        run_id: Some(request.run_id.clone()),
                        call_id: None,
                        correlation_id: artist_core::CorrelationId::new("mid-run"),
                        parent_correlation_id: None,
                    },
                    payload: serde_json::json!({}),
                    presentation: serde_json::json!({}),
                };
                let (tx, rx) = tokio::sync::oneshot::channel();
                sink.send(crate::FactEnvelope {
                    session_id: request.session_id.clone(),
                    facts: vec![
                        artist_core::PluginFact::SchemaRegistered { schema },
                        artist_core::PluginFact::Event { event },
                    ],
                    receipt: tx,
                })
                .await
                .unwrap();
                // The receipt only resolves after the canonical append; a
                // success here therefore proves durable-before-output.
                rx.await.unwrap().unwrap();
                Ok(ModelEvent::Finished {
                    output: Some("done".into()),
                })
            }))
        }
    }

    struct SinkCapture(Arc<tokio::sync::Mutex<Option<crate::FactSink>>>);

    #[async_trait]
    impl ExecutionExtensions for SinkCapture {
        fn bind_fact_sink(&self, _: &SessionId, sink: crate::FactSink) {
            self.0.try_lock().unwrap().replace(sink);
        }
    }

    #[tokio::test]
    async fn emissions_commit_durably_before_the_emitter_is_released() {
        let store = Arc::new(MemoryStore::default());
        let captured = Arc::new(tokio::sync::Mutex::new(None));
        let extensions = Arc::new(SinkCapture(captured.clone()));
        let dependencies = SessionDependencies {
            store: store.clone(),
            model: Arc::new(EmittingModel {
                sink: captured.clone(),
            }),
            profiles: None,
            slash_commands: None,
            extensions,
            resume_queued_work: true,
        };
        let session = SessionHandle::create(
            CreateSession {
                session_id: SessionId::from("session"),
                metadata: SessionMetadata::root(0, None),
                context: InitialContext { fragments: vec![] },
                initial_profile: None,
            },
            dependencies,
        )
        .await
        .unwrap();
        let mut events = session.subscribe();
        session.input(Source::User, "hello").await.unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::Completed { .. })
        })
        .await;

        let record = store.load(&SessionId::from("session")).await.unwrap();
        let kinds: Vec<_> = record.entries().iter().map(|e| e.kind.clone()).collect();
        let run_started = kinds
            .iter()
            .position(|k| matches!(k, TranscriptEntryKind::RunStarted { .. }))
            .unwrap();
        let schema_at = kinds
            .iter()
            .position(|k| matches!(k, TranscriptEntryKind::PluginEventSchemaRegistered { .. }))
            .unwrap();
        let event_at = kinds
            .iter()
            .position(|k| matches!(k, TranscriptEntryKind::PluginEvent { .. }))
            .unwrap();
        let finished_at = kinds
            .iter()
            .position(|k| matches!(k, TranscriptEntryKind::RunFinished { .. }))
            .unwrap();
        assert!(run_started < schema_at && schema_at < event_at && event_at < finished_at);
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
                crate::ModelMessage::User(text("first")),
                crate::ModelMessage::Assistant(vec![artist_core::ContentPart::text("one")]),
                crate::ModelMessage::User(text("second")),
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
                content: text("hello"),
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
        let _session =
            SessionHandle::resume_unprofiled(&SessionId::from("resume"), store.clone(), model)
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
                content: text("hello"),
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
        let _session = SessionHandle::resume_unprofiled(
            &SessionId::from("resume-partial"),
            store.clone(),
            model,
        )
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
                content: text("first pending"),
            },
            TranscriptEntryKind::SteeringQueued {
                message_id: MessageId::from("notice-1"),
                source: Source::Harness,
                content: text("first notice"),
            },
            TranscriptEntryKind::SteeringQueued {
                message_id: MessageId::from("notice-2"),
                source: Source::User,
                content: text("second notice"),
            },
            TranscriptEntryKind::Input {
                message_id: MessageId::from("second"),
                source: Source::User,
                content: text("second pending"),
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
        let _session = SessionHandle::resume_unprofiled(
            &SessionId::from("resume-queues"),
            store.clone(),
            model,
        )
        .await
        .unwrap();

        assert_eq!(
            requests_rx.recv().await.unwrap().prompt,
            text("first pending")
        );
        let notices = steering_rx.recv().await.unwrap();
        assert_eq!(
            notices
                .iter()
                .map(|notice| notice.content.clone())
                .collect::<Vec<_>>(),
            [text("first notice"), text("second notice")]
        );
        assert_eq!(
            requests_rx.recv().await.unwrap().prompt,
            text("second pending")
        );
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

    #[tokio::test]
    async fn yield_is_a_typed_terminal_run_outcome() {
        let store = Arc::new(MemoryStore::default());
        let (model, control) = ControlledModel::new();
        let profiles = Arc::new(StaticProfiles(HashMap::from([(
            "worker".into(),
            test_profile("worker", &["worker"]),
        )])));
        let session = create_profiled(
            "yield-session",
            "worker",
            profiles,
            None,
            store.clone(),
            model,
        )
        .await;
        let mut events = session.subscribe();
        session.input(Source::User, "work").await.unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::RunStarted { .. })
        })
        .await;
        let call_id = CallId::from("yield-call");
        control
            .send(Control::Event(ModelEvent::ToolCall {
                call_id: call_id.clone(),
                name: "yield".into(),
                arguments: r#"{"completed":true}"#.into(),
            }))
            .unwrap();
        control
            .send(Control::Event(ModelEvent::ToolResult {
                call_id: call_id.clone(),
                content: vec![artist_core::ContentPart::Json {
                    value: serde_json::json!({"completed": true}),
                }],
            }))
            .unwrap();
        control
            .send(Control::Event(ModelEvent::Control {
                call_id: call_id.clone(),
                control: ToolControl::Yield {
                    payload: serde_json::json!({"completed": true}),
                },
            }))
            .unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::Yielded { .. })
        })
        .await;

        let record = store.load(&SessionId::from("yield-session")).await.unwrap();
        assert!(record.active_run().is_none());
        assert!(matches!(
            &record.entries().last().unwrap().kind,
            TranscriptEntryKind::RunFinished {
                outcome: RunOutcome::Yielded { call_id: stored, payload },
                ..
            } if stored == &call_id && payload == &serde_json::json!({"completed": true})
        ));
    }

    #[tokio::test]
    async fn handoff_snapshots_profile_and_replaces_projected_history_with_combined_brief() {
        let store = Arc::new(MemoryStore::default());
        let (model, control) = ControlledModel::new();
        let profiles = Arc::new(StaticProfiles(HashMap::from([
            (
                "planner".into(),
                test_profile("planner", &["planner", "worker"]),
            ),
            (
                "worker".into(),
                test_profile("worker", &["planner", "worker"]),
            ),
        ])));
        let session = create_profiled(
            "handoff-session",
            "planner",
            profiles,
            None,
            store.clone(),
            model,
        )
        .await;
        let mut events = session.subscribe();
        session.input(Source::User, "old task").await.unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::RunStarted { .. })
        })
        .await;
        session
            .input(Source::User, "obsolete queued task")
            .await
            .unwrap();
        session
            .steer(Source::User, "queued correction")
            .await
            .unwrap();

        let call_id = CallId::from("handoff-call");
        control
            .send(Control::Event(ModelEvent::ToolCall {
                call_id: call_id.clone(),
                name: "handoff".into(),
                arguments: r#"{"profile":"worker","brief":"implement it"}"#.into(),
            }))
            .unwrap();
        control
            .send(Control::Event(ModelEvent::ToolResult {
                call_id: call_id.clone(),
                content: vec![artist_core::ContentPart::text("accepted")],
            }))
            .unwrap();
        control
            .send(Control::Event(ModelEvent::Control {
                call_id,
                control: ToolControl::Handoff {
                    profile: "worker".into(),
                    brief: "implement it".into(),
                },
            }))
            .unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::ProfileActivated { profile } if profile.name == "worker")
        })
        .await;
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::RunStarted { .. })
        })
        .await;

        let record = store
            .load(&SessionId::from("handoff-session"))
            .await
            .unwrap();
        assert_eq!(record.current_profile().unwrap().name, "worker");
        assert!(record.entries().iter().any(|entry| matches!(
            &entry.kind,
            TranscriptEntryKind::InputsSuperseded { message_ids } if message_ids.len() == 1
        )));
        assert!(record.entries().iter().any(|entry| matches!(
            &entry.kind,
            TranscriptEntryKind::ProfileActivated { brief: Some(brief), steering_message_ids, .. }
                if brief == &vec![
                    ContentPart::text("implement it"),
                    ContentPart::text("\n\n"),
                    ContentPart::text("queued correction"),
                ] && steering_message_ids.len() == 1
        )));
        assert_eq!(
            project(&record).1.last().unwrap().message,
            crate::ModelMessage::User(vec![
                ContentPart::text("implement it"),
                ContentPart::text("\n\n"),
                ContentPart::text("queued correction"),
            ])
        );
    }

    #[tokio::test]
    async fn slash_output_is_durable_harness_state_and_bypasses_profile_policy() {
        let store = Arc::new(MemoryStore::default());
        let (model, _) = ControlledModel::new();
        let mut denied = test_profile("locked", &["locked"]);
        denied.policy.default = artist_core::PolicyDecision::Deny;
        let profiles = Arc::new(StaticProfiles(HashMap::from([("locked".into(), denied)])));
        let commands = Arc::new(StaticSlashCommands(HashMap::from([(
            "status".into(),
            SlashCommandResult {
                output: Some("ready".into()),
                actions: Vec::new(),
            },
        )])));
        let session = create_profiled(
            "slash-output",
            "locked",
            profiles,
            Some(commands),
            store.clone(),
            model,
        )
        .await;

        let result = session.slash("status", "verbose please").await.unwrap();
        assert_eq!(result.output.as_deref(), Some("ready"));
        let record = store.load(&SessionId::from("slash-output")).await.unwrap();
        assert!(record.entries().iter().any(|entry| matches!(
            &entry.kind,
            TranscriptEntryKind::SlashCommand { name, arguments, output: Some(output), .. }
                if name == "status" && arguments == "verbose please" && output == "ready"
        )));
        assert!(project(&record).1.is_empty());
    }

    #[tokio::test]
    async fn slash_actions_enter_the_normal_kernel_command_flow() {
        let store = Arc::new(MemoryStore::default());
        let (model, _control) = ControlledModel::new();
        let profiles = Arc::new(StaticProfiles(HashMap::from([(
            "default".into(),
            test_profile("default", &["default"]),
        )])));
        let commands = Arc::new(StaticSlashCommands(HashMap::from([(
            "ask".into(),
            SlashCommandResult {
                output: None,
                actions: vec![SlashCommandAction::Input {
                    content: text("generated prompt"),
                }],
            },
        )])));
        let session = create_profiled(
            "slash-action",
            "default",
            profiles,
            Some(commands),
            store.clone(),
            model,
        )
        .await;
        let mut events = session.subscribe();

        session.slash("ask", "").await.unwrap();
        wait_for(&mut events, |event| {
            matches!(event, StreamEventKind::RunStarted { .. })
        })
        .await;
        let record = store.load(&SessionId::from("slash-action")).await.unwrap();
        assert!(record.entries().iter().any(|entry| matches!(
            &entry.kind,
            TranscriptEntryKind::Input { source: Source::Harness, content, .. }
                if content == &text("generated prompt")
        )));
    }

    #[tokio::test]
    async fn slash_profile_activation_resets_context_and_queues_its_brief() {
        let store = Arc::new(MemoryStore::default());
        let (model, _control) = ControlledModel::new();
        let profiles = Arc::new(StaticProfiles(HashMap::from([
            (
                "planner".into(),
                test_profile("planner", &["planner", "worker"]),
            ),
            (
                "worker".into(),
                test_profile("worker", &["planner", "worker"]),
            ),
        ])));
        let commands = Arc::new(StaticSlashCommands(HashMap::from([(
            "work".into(),
            SlashCommandResult {
                output: Some("switched".into()),
                actions: vec![SlashCommandAction::ActivateProfile {
                    profile: "worker".into(),
                    brief: Some("build it".into()),
                }],
            },
        )])));
        let session = create_profiled(
            "slash-profile",
            "planner",
            profiles,
            Some(commands),
            store.clone(),
            model,
        )
        .await;

        session.slash("work", "").await.unwrap();
        let record = store.load(&SessionId::from("slash-profile")).await.unwrap();
        assert_eq!(record.current_profile().unwrap().name, "worker");
        assert_eq!(
            project(&record).1.last().unwrap().message,
            crate::ModelMessage::User(text("build it"))
        );
    }
}
