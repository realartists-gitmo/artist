use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::{
    LifecycleEvent, PromptEvent, SessionHandles,
    capture::{CaptureHook, ToolMeta},
    resources::Resources,
    session_tools::{OwnedSession, OwnedState, SessionHub},
    tool_set::{self, DelegationEnv},
    ttsr::{TtsrHook, TtsrShared, reminder_message},
};
use artist_session::{
    ContentBlock, ConversationMessages, DelegateFinished, DelegateStarted, ModelTurn, RunFinished,
    RunStarted, ToolContext as SessionToolContext, ToolOutcomeRecord, ToolResultEvent,
    ToolResultImagesEvent,
};
use artist_tool_api::ArtistDynamicTool;
use artist_tools::ToolBundle;
use futures::{StreamExt, future::BoxFuture};
use llm_provider::SavedProvider;
use rig_agent::client::AgentClientExt;
use rig_agent::{agent::MultiTurnStreamItem, prelude::PromptError, streaming::StreamingChat};
use rig_core::{
    client::CompletionClient,
    completion::{Message, message::ToolResultContent},
    streaming::{StreamedAssistantContent, StreamedUserContent},
    tool::PortableTool,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

/// A run's seat on the delegation semaphore, held in a slot it can be lifted
/// out of and put back.
///
/// Delegation depth is uncapped, which only works if an ancestor chain cannot
/// consume every permit and starve its own descendants. A parent awaiting a
/// child is not doing work, so it yields its seat for the duration of the
/// nested call and takes one again when the child returns. It holds nothing
/// while re-acquiring, so this cannot deadlock: the seat it gave up is enough
/// for a serial chain of any depth to make progress.
///
/// Under concurrent fan-out a yielded seat may be taken by a sibling before the
/// child claims it, in which case the child fails with the same loud
/// "concurrency limit reached" as before. That is the existing, intended
/// behaviour of `max_concurrent`; what this removes is depth being a second,
/// undocumented limit on top of it.
#[derive(Clone)]
pub(crate) struct PermitSlot {
    permits: artist_registry::Permits,
    label: String,
    held: Arc<tokio::sync::Mutex<Option<artist_registry::Seat>>>,
}

/// How often a run waiting to retake its seat re-checks the pool.
///
/// A seat frees when a child run ends, which is a human-timescale event, so a
/// quarter second of latency is invisible and the poll costs a directory scan.
const RETAKE_POLL: std::time::Duration = std::time::Duration::from_millis(250);

impl PermitSlot {
    /// Claim a seat, or report that every one is taken. Non-blocking, so a
    /// fan-out beyond the limit fails immediately and says so rather than
    /// queueing invisibly.
    async fn claim(
        permits: artist_registry::Permits,
        label: String,
    ) -> Result<Self, DelegateError> {
        let seat = Self::acquire(&permits, &label)
            .await?
            .ok_or_else(|| DelegateError::Failed("subagent concurrency limit reached".into()))?;
        Ok(Self {
            permits,
            label,
            held: Arc::new(tokio::sync::Mutex::new(Some(seat))),
        })
    }

    /// Take a seat off the pool without blocking the runtime.
    ///
    /// The pool is a locked directory, so acquiring can block on another
    /// process's scan. That is microseconds in practice and never awaited
    /// work, but it is still a blocking syscall and does not belong on a
    /// reactor thread.
    async fn acquire(
        permits: &artist_registry::Permits,
        label: &str,
    ) -> Result<Option<artist_registry::Seat>, DelegateError> {
        let permits = permits.clone();
        let label = label.to_owned();
        tokio::task::spawn_blocking(move || permits.try_acquire(&label))
            .await
            .map_err(|error| DelegateError::Failed(format!("seat pool join failed: {error}")))?
            .map_err(|error| DelegateError::Failed(format!("seat pool unavailable: {error}")))
    }

    pub(crate) async fn yield_seat(&self) {
        *self.held.lock().await = None;
    }

    /// Waits, rather than failing: the caller holds nothing at this point, and
    /// every child eventually finishes and releases, so progress is guaranteed.
    ///
    /// Polls rather than parks because the pool now spans processes and there
    /// is no cross-process wakeup to wait on. Dropping this future — which is
    /// what cancelling the turn does — abandons the wait cleanly.
    pub(crate) async fn retake(&self) {
        loop {
            match Self::acquire(&self.permits, &self.label).await {
                Ok(Some(seat)) => {
                    *self.held.lock().await = Some(seat);
                    return;
                }
                Ok(None) => tokio::time::sleep(RETAKE_POLL).await,
                // A pool we cannot reach must not wedge the run forever. The
                // seat is lost for this run rather than the run being lost.
                Err(_) => return,
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct Delegate {
    provider: SavedProvider,
    tools: ToolBundle,
    /// The spawning context to seed a `fork=true` delegate with. Shared via
    /// `Arc` so constructing a Delegate each run/retry is a cheap refcount bump;
    /// the history is only deep-cloned if the model actually forks.
    context: Arc<Vec<Message>>,
    sessions: SessionHub,
    resources: Resources,
    handles: SessionHandles,
    disabled_tools: Vec<String>,
    profiles: crate::profiles::Profiles,
    events: UnboundedSender<PromptEvent>,
    /// Carried so a nested subagent is built from the same environment its
    /// spawner had, rather than a narrower one assembled here.
    memory: Option<crate::memory::MemoryWriter>,
    canvas: Option<Arc<artist_canvas::server::Lazy>>,
    dynamic: Vec<ArtistDynamicTool>,
    extension_runs: Vec<(String, ArtistDynamicTool)>,
    extension_manager: Option<Arc<artist_extensions::Manager>>,
    /// The spawning agent's *display* name, recorded on each child so the
    /// directory can answer "everyone under Monet". `None` where the spawner
    /// has no identity, which is only the case in tests.
    spawner_name: Option<String>,
    /// The parent's artifact store, carried so a child's yield is captured into
    /// the same durable namespace the spawner reads through `artifact://`,
    /// rather than a per-child store that dies with the subagent.
    pages: crate::pagination::PageStore,
    /// Navigation metadata is deliberately separate from the lifecycle
    /// registry's parent field, so path traversal does not infer scheduling.
    relationships: crate::relationships::RelationshipStore,
}

struct DelegateRun {
    actor: String,
    public_id: String,
    identity: crate::identity::RunIdentity,
    permit: PermitSlot,
}
struct SubagentSession {
    state: Arc<tokio::sync::RwLock<OwnedState>>,
    abort: Arc<Mutex<Option<tokio::task::AbortHandle>>>,
    sender: String,
    recipient: String,
}

impl OwnedSession for SubagentSession {
    fn state(&self) -> BoxFuture<'_, Result<OwnedState, String>> {
        Box::pin(async move { Ok(self.state.read().await.clone()) })
    }

    fn send(&self, input: Value) -> BoxFuture<'_, Result<(), String>> {
        let sender = self.sender.clone();
        let recipient = self.recipient.clone();
        Box::pin(async move {
            let body = input
                .as_str()
                .ok_or_else(|| "subagent input must be a string".to_owned())?;
            artist_registry::messages()
                .send(&artist_registry::Message {
                    id: artist_tools::short_id("m"),
                    from: sender,
                    to: recipient,
                    audience: artist_registry::Audience::Direct,
                    body: body.to_owned(),
                    expects_reply: false,
                    sent_at: artist_registry::now(),
                })
                .map_err(|error| error.to_string())
        })
    }

    fn abort(&self) -> BoxFuture<'_, Result<(), String>> {
        let abort = Arc::clone(&self.abort);
        Box::pin(async move {
            if let Some(handle) = abort
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
            {
                handle.abort();
            }
            Ok(())
        })
    }
}

impl Delegate {
    /// Built from the environment its spawner was built from, so nested subagents
    /// inherit the same profile/resource environment.
    pub(crate) fn new(env: &crate::tool_set::ToolEnv, delegation: &DelegationEnv) -> Self {
        Self {
            provider: delegation.provider.clone(),
            tools: env.bundle.clone(),
            context: Arc::clone(&delegation.context),
            sessions: env.sessions.clone(),
            resources: env.resources.clone(),
            handles: delegation.handles.clone(),
            disabled_tools: env.disabled.clone(),
            profiles: delegation.profiles.clone(),
            events: delegation.events.clone(),
            memory: env.memory.clone(),
            canvas: env.canvas.clone(),
            dynamic: env.dynamic.clone(),
            extension_runs: env.extension_runs.clone(),
            extension_manager: env.extension_manager.clone(),
            spawner_name: env.inbox.as_ref().map(|inbox| inbox.name.to_string()),
            pages: env.pages.clone(),
            relationships: env.relationships.clone(),
        }
    }

    fn emit(&self, event: PromptEvent) {
        // Background delegates can outlive the parent stream that owned the
        // receiver. Display delivery is therefore best-effort and must never
        // turn a successful background job into a tool failure.
        let _ = self.events.send(event);
    }

    fn emit_child(&self, id: &str, event: PromptEvent) {
        match &event {
            PromptEvent::ToolExecutionStart { id: tool_id, .. } => self
                .handles
                .lifecycle
                .emit(LifecycleEvent::ToolStarted(format!("{id}:{tool_id}"))),
            PromptEvent::ToolResult { id: tool_id, .. } => self
                .handles
                .lifecycle
                .emit(LifecycleEvent::ToolFinished(format!("{id}:{tool_id}"))),
            _ => {}
        }
        self.emit(PromptEvent::SubagentEvent {
            id: id.to_owned(),
            event: Box::new(event),
        });
    }

    fn finish_child(&self, id: &str, outcome: &str) {
        self.handles
            .lifecycle
            .emit(LifecycleEvent::SubagentFinished(id.to_owned()));
        self.emit(PromptEvent::SubagentFinished {
            id: id.to_owned(),
            outcome: outcome.to_owned(),
        });
    }
}

#[cfg(test)]
mod agent_creation_tests {
    use super::*;

    #[test]
    fn canonical_agent_arguments_use_brief_not_legacy_prompt() {
        let args: AgentArgs = serde_json::from_value(json!({
            "brief": "audit the retained state",
            "profile": "reviewer"
        }))
        .unwrap();
        assert_eq!(args.brief, "audit the retained state");
        assert_eq!(args.profile.as_deref(), Some("reviewer"));
        assert!(serde_json::from_value::<AgentArgs>(json!({"prompt":"legacy"})).is_err());
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DelegateArgs {
    prompt: String,
    profile: Option<String>,
}

/// The canonical path-first creation surface for a retained Artist.  `Delegate`
/// remains available as the legacy `subagent` tool while profiles and callers
/// migrate; both enter exactly the same durable roster/name lifecycle.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentArgs {
    pub(crate) brief: String,
    pub(crate) profile: Option<String>,
}

#[derive(Clone)]
pub(crate) struct AgentCreation(pub(crate) Delegate);

#[derive(Debug, thiserror::Error)]
pub(crate) enum DelegateError {
    #[error("subagent failed: {0}")]
    Failed(String),
    /// An availability failure on this candidate. The fallback loop advances
    /// to the next candidate rather than failing the run.
    #[error("subagent provider unavailable: {0}")]
    Unavailable(String),
}

impl PortableTool for Delegate {
    const NAME: &'static str = "subagent";
    type Error = DelegateError;
    type Args = DelegateArgs;
    type Output = String;

    fn description(&self) -> String {
        "Spawn a focused subagent and return its canonical agent://<artist> path immediately."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "prompt":{"type":"string","description":"Task for the subagent."},
                "profile":{"type":"string","enum":self.profiles.names(),"description":"Subagent profile; defaults to default."}
            },
            "required":["prompt"],
            "additionalProperties":false
        })
    }

    async fn call(&self, args: DelegateArgs) -> Result<String, DelegateError> {
        self.spawn(args).await
    }
}

impl PortableTool for AgentCreation {
    const NAME: &'static str = "agent";
    type Error = DelegateError;
    type Args = AgentArgs;
    type Output = String;

    fn description(&self) -> String {
        "Claim a retained Artist roster name and start it immediately; returns the canonical agent://<artist> path."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "brief":{"type":"string","description":"Focused work unit for the new Artist."},
                "profile":{"type":"string","enum":self.0.profiles.names(),"description":"Artist profile; defaults to default."}
            },
            "required":["brief"],
            "additionalProperties":false
        })
    }

    async fn call(&self, args: AgentArgs) -> Result<String, DelegateError> {
        self.0
            .spawn(DelegateArgs {
                prompt: args.brief,
                profile: args.profile,
            })
            .await
    }
}

impl Delegate {
    async fn spawn(&self, args: DelegateArgs) -> Result<String, DelegateError> {
        let profile_name = args.profile.unwrap_or_else(|| "default".into());
        let yield_profile = self
            .profiles
            .get(&profile_name)
            .map_err(DelegateError::Failed)?;

        let actor = artist_tools::short_id("a");
        let identity = crate::identity::for_run(
            &actor,
            self.tools.project_root(),
            &profile_name,
            self.spawner_name.as_deref(),
        )
        .map_err(|error| {
            DelegateError::Unavailable(format!("artist identity allocation failed: {error}"))
        })?;
        let public_id = identity.name.clone();

        if let Err(error) = self.sessions.registry().create_exact(
            &public_id,
            "subagent",
            &public_id,
            Some(self.sessions.artist()),
            json!({"profile":profile_name,"state":"running"}),
        ) {
            let _ = artist_registry::names().release(&identity.lease_key);
            return Err(DelegateError::Failed(error.to_string()));
        }
        self.sessions
            .registry()
            .set_name_lease(&public_id, identity.lease_key.clone())
            .map_err(|error| DelegateError::Failed(error.to_string()))?;
        self.relationships
            .add(
                &format!("agent://{}", self.sessions.artist()),
                "child",
                &canonical_agent_path(&public_id),
            )
            .map_err(|error| {
                DelegateError::Failed(format!("creating child relationship: {error}"))
            })?;

        let state = Arc::new(tokio::sync::RwLock::new(OwnedState::live(json!({
            "profile": profile_name,
            "state": "running"
        }))));
        let abort = Arc::new(Mutex::new(None));
        self.sessions.own(
            public_id.clone(),
            Arc::new(SubagentSession {
                state: Arc::clone(&state),
                abort: Arc::clone(&abort),
                sender: self.sessions.artist().to_owned(),
                recipient: public_id.clone(),
            }),
        );

        self.handles
            .lifecycle
            .emit(LifecycleEvent::SubagentStarted(public_id.clone()));
        self.emit(PromptEvent::SubagentStarted {
            id: public_id.clone(),
            role: profile_name.clone(),
            prompt: args.prompt.clone(),
        });

        let delegate = self.clone();
        let task_profile = profile_name.clone();
        let yield_actor = actor.clone();
        let yield_agent = public_id.clone();
        let task = tokio::spawn(async move {
            let next = match delegate
                .run_agent(args.prompt, task_profile.clone(), actor, identity)
                .await
            {
                Ok(output) => {
                    // A work-unit result is retained as structured JSON even
                    // when the model returned prose (which is then a JSON
                    // string). `agent://<artist>/yields/1` is consequently a
                    // real canonical resource, not a transcript convention.
                    let value = serde_json::from_str(&output)
                        .unwrap_or_else(|_| Value::String(output.clone()));
                    match yield_profile.validate_yield(&value) {
                        Ok(()) => {
                            // The yield is captured into the parent's artifact
                            // store automatically, so a completed work unit is
                            // durable and addressable through `artifact://` in
                            // the same namespace the spawner reads — with the
                            // profile as provenance and the exact value as the
                            // payload — without the yielder writing one by hand.
                            let artifact = capture_yield(&delegate.pages, &task_profile, &value);
                            delegate
                                .handles
                                .recorder
                                .child_lineage(&yield_actor)
                                .record(artist_session::WorkUnitYield {
                                    agent: yield_agent.clone(),
                                    sequence: 1,
                                    profile: task_profile.clone(),
                                    yield_schema: yield_profile.yield_schema.clone(),
                                    value: value.clone(),
                                    artifact: artifact.clone(),
                                });
                            OwnedState::stopped(
                                artist_registry::SessionStatus::Completed,
                                json!({
                                    "profile": task_profile,
                                    "output": output,
                                    "yields": [{
                                        "sequence": 1,
                                        "profile": task_profile,
                                        "value": value,
                                        "artifact": artifact,
                                    }],
                                }),
                            )
                        }
                        Err(error) => OwnedState::stopped(
                            artist_registry::SessionStatus::Failed,
                            json!({
                                "profile": task_profile,
                                "output": output,
                                "yieldValidationError": error,
                            }),
                        ),
                    }
                }
                Err(error) if error.to_string().contains("cancelled") => OwnedState::stopped(
                    artist_registry::SessionStatus::Cancelled,
                    json!({"profile":task_profile,"error":error.to_string()}),
                ),
                Err(error) => OwnedState::stopped(
                    artist_registry::SessionStatus::Failed,
                    json!({"profile":task_profile,"error":error.to_string()}),
                ),
            };
            *state.write().await = next;
        });
        *abort.lock().unwrap_or_else(|error| error.into_inner()) = Some(task.abort_handle());

        Ok(canonical_agent_path(&public_id))
    }

    async fn run_agent(
        &self,
        prompt: String,
        profile_name: String,
        actor: String,
        identity: crate::identity::RunIdentity,
    ) -> Result<String, DelegateError> {
        let profile = self
            .profiles
            .get(&profile_name)
            .map_err(DelegateError::Failed)?;
        let run = DelegateRun {
            public_id: identity.name.clone(),
            actor,
            identity,
            permit: PermitSlot::claim(
                self.profiles.permits.clone(),
                format!("{profile_name}:{}", self.handles.conversation_id),
            )
            .await?,
        };
        let breaker = crate::fallback::Breaker::global();
        let mut skipped = Vec::new();
        let mut last_error = None;

        for candidate in &profile.candidates {
            let label = crate::fallback::candidate_label(candidate);
            let provider = match self.resolve_provider(candidate) {
                Ok(provider) => provider,
                Err(error) => {
                    skipped.push(crate::fallback::Skipped {
                        candidate: label,
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
            let Some(model) = candidate.model_for(&provider) else {
                skipped.push(crate::fallback::Skipped {
                    candidate: label,
                    reason: "no model configured on the candidate or its account".into(),
                });
                continue;
            };
            let key = crate::fallback::CandidateKey {
                provider: provider.id.as_str().to_owned(),
                model: model.clone(),
            };
            if breaker.is_tripped(&key) {
                skipped.push(crate::fallback::Skipped {
                    candidate: label,
                    reason: "cooling down after repeated failures".into(),
                });
                continue;
            }
            let thinking = candidate.thinking.unwrap_or_default();
            match self
                .dispatch(
                    &provider,
                    &model,
                    thinking,
                    prompt.clone(),
                    &profile,
                    false,
                    &run,
                )
                .await
            {
                Ok(output) => {
                    breaker.record_success(&key);
                    return Ok(output);
                }
                Err(DelegateError::Unavailable(message)) => {
                    breaker.record_failure(&key);
                    self.emit(PromptEvent::ProviderFallback {
                        from: label,
                        reason: message.clone(),
                    });
                    last_error = Some(message);
                }
                Err(other) => return Err(other),
            }
        }
        Err(DelegateError::Failed(crate::fallback::exhausted(
            &profile.name,
            last_error,
            &skipped,
        )))
    }

    /// A profile names a provider *account*, not a provider kind. Resolving
    /// against the session's configured set is what lets a delegated profile
    /// run on a different account — and a different provider entirely — from
    /// the parent that spawned it. An unnamed provider inherits the parent's.
    fn resolve_provider(
        &self,
        candidate: &crate::profiles::Candidate,
    ) -> Result<SavedProvider, DelegateError> {
        candidate
            .resolve(&self.handles.providers, &self.provider)
            .cloned()
            .map_err(DelegateError::Failed)
    }

    /// Select the typed Rig client for the resolved account. Each arm
    /// monomorphizes `run_agent_with` separately, which is why this stays a
    /// match over provider kinds rather than a unified client type.
    #[allow(clippy::too_many_arguments)]
    /// Build the client for the resolved account and hand it to the run.
    ///
    /// Construction is centralized in `RigClient::build`, so a candidate that
    /// names a different account — or a different provider entirely — routes
    /// through exactly the same path the session root uses.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch(
        &self,
        provider: &SavedProvider,
        model: &str,
        thinking: crate::profiles::Thinking,
        prompt: String,
        profile: &crate::profiles::Profile,
        fork: bool,
        run: &DelegateRun,
    ) -> Result<String, DelegateError> {
        use crate::rig_provider::RigClient;
        let client =
            RigClient::build(provider).map_err(|error| DelegateError::Failed(error.to_string()))?;
        macro_rules! run_with {
            ($client:expr) => {
                self.run_agent_with(
                    $client, prompt, profile, provider, thinking, fork, model, run,
                )
                .await
            };
        }
        match client {
            RigClient::ArtistOpenAi(client) => {
                // Delegates have isolated provider-private lineage even when
                // their portable conversation is seeded from the parent.
                let lineage = format!("{}:delegate:{}", self.handles.conversation_id, run.actor);
                let client =
                    client.with_provider_context(lineage, self.handles.provider_context.clone());
                run_with!(client)
            }
            RigClient::Copilot(client) => run_with!(client),
            RigClient::OpenAiChat(client) => run_with!(client),
            RigClient::Anthropic(client) => run_with!(client),
            RigClient::Cohere(client) => run_with!(client),
            RigClient::Gemini(client) => run_with!(client),
            RigClient::DeepSeek(client) => run_with!(client),
            RigClient::Groq(client) => run_with!(client),
            RigClient::HuggingFace(client) => run_with!(client),
            RigClient::Hyperbolic(client) => run_with!(client),
            RigClient::Mira(client) => run_with!(client),
            RigClient::Mistral(client) => run_with!(client),
            RigClient::OpenRouter(client) => run_with!(client),
            RigClient::Perplexity(client) => run_with!(client),
            RigClient::Together(client) => run_with!(client),
            RigClient::XAi(client) => run_with!(client),
            RigClient::Azure(client) => run_with!(client),
            RigClient::Llamafile(client) => run_with!(client),
            RigClient::Ollama(client) => run_with!(client),
            RigClient::Minimax(client) => run_with!(client),
            RigClient::MinimaxAnthropic(client) => run_with!(client),
            RigClient::Moonshot(client) => run_with!(client),
            RigClient::MoonshotAnthropic(client) => run_with!(client),
            RigClient::XiaomiMiMo(client) => run_with!(client),
            RigClient::XiaomiMiMoAnthropic(client) => run_with!(client),
            RigClient::ZAi(client) => run_with!(client),
            RigClient::ZAiAnthropic(client) => run_with!(client),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_agent_with<C: CompletionClient>(
        &self,
        client: C,
        prompt: String,
        role: &crate::profiles::Profile,
        provider: &SavedProvider,
        thinking: crate::profiles::Thinking,
        fork: bool,
        model: &str,
        run: &DelegateRun,
    ) -> Result<String, DelegateError>
    where
        C::CompletionModel: 'static,
    {
        let actor = run.actor.clone();
        let display_id = run.public_id.clone();
        let identity = &run.identity;
        let child_tools = self
            .tools
            .for_actor(&actor)
            .map_err(|error| DelegateError::Failed(error.to_string()))?;
        let recorder = self.handles.recorder.child_lineage(&actor);
        recorder.record(DelegateStarted {
            prompt: prompt.clone(),
            read_only: !["bash", "edit", "write", "computer"]
                .into_iter()
                .any(|tool| role.permits(tool)),
            fork: false,
            background: true,
        });
        let child_computer = self
            .handles
            .computer
            .as_ref()
            .map(artist_computer::SurfaceRegistry::for_delegate);

        let (base, _) = crate::prompt_config::base_prompt();
        let policy = format!(
            "{base}\n\nYou are the '{}' subagent profile.\n{}\n\n{}\nCurrent working directory: {}{}",
            role.name,
            role.instructions,
            self.resources.prompt_section(role),
            self.tools.project_root().display(),
            identity.prompt_block(),
        );

        let mut seed_history = if fork {
            (*self.context).clone()
        } else {
            Vec::new()
        };
        // A forked delegate receives the main conversation as input history,
        // but that prefix belongs to the parent lineage. Only messages accepted
        // after this boundary are persisted as the child's transcript.
        let parent_history_len = seed_history.len();
        let mut seed_prompt = Message::user(&prompt);

        // Per-run abort-retry budget: this delegate's own counter, isolated
        // from the main agent and any sibling delegates.
        let retry_budget = self.handles.rules.retry_budget();
        let mut retries_used = 0u32;
        let mut overload_retry = crate::provider_retry::OverloadRetry::new(
            self.tools.project_root(),
            model,
            &format!("delegate:{actor}"),
        );
        let (output, conversation) = loop {
            let run_id = format!("r-{}", uuid::Uuid::new_v4().simple());
            let run_recorder = recorder.with_run(&run_id);
            let ttsr = TtsrShared::new(
                self.handles.rules.clone(),
                Arc::clone(&self.handles.rule_set),
                true,
                retries_used < retry_budget,
            );
            let mut builder = client.agent(model).preamble(&policy);
            // Upstream owns the ChatGPT/OpenAI request shape (fast mode,
            // context window, Responses-only policy); the profile supplies the
            // effort. Anthropic spells reasoning differently and upstream
            // returns nothing for it, so our translation covers that case.
            let effort = thinking
                .level
                .map(|level| level.as_str().to_owned())
                .or_else(|| provider.reasoning_effort.clone());
            if let Some(params) = crate::request_params(
                provider.provider,
                provider.api,
                overload_retry.cache_key(),
                effort.as_deref(),
                delegate_context_window(
                    model,
                    provider.model.as_deref(),
                    self.handles.effective_context_window,
                ),
                self.handles.fast_mode,
            )
            .or_else(|| {
                crate::thinking::request_params(
                    provider.provider,
                    overload_retry.cache_key(),
                    thinking,
                )
            }) {
                builder = builder.additional_params(params);
            }
            let tool_meta = ToolMeta::default();
            // Rebuilt per attempt from the *current* seed, so a nested child
            // spawned after a TTSR retry inherits the reminder-injected history
            // rather than the stale original turn — the same rule the session
            // root follows for its own delegates.
            let nested_context = Arc::new({
                let mut context = seed_history.clone();
                context.push(seed_prompt.clone());
                context
            });
            // A child must callback only into its own filtered surface. Sharing
            // the root registry here would let Python eval observe a parent
            // profile after the child was deliberately restricted.
            let child_tool_registry = crate::ToolRegistryHandle::default();
            let env = tool_set::ToolEnv {
                bundle: child_tools.clone(),
                // The delegate's own lineage recorder, so `artist computer log`
                // attributes a subagent's actions to the subagent.
                recorder: recorder.clone(),
                resources: self.resources.clone(),
                profiles: self.profiles.clone(),
                todos: self.handles.todos.clone(),
                // A child owns its own list and may read its parent's, but not
                // write to it: concurrent siblings sharing one list would race
                // with no obvious merge.
                todo_owner: actor.clone(),
                attachments: self.handles.attachments.clone(),
                // A subagent driving a GUI gets a display of its own, never the
                // parent's: one stage is one seat, and siblings sharing it
                // would serialize behind the input lease into uselessness.
                // Whether it may do this at all is a profile decision like any
                // other — the read-only built-ins deny it through their allow
                // list, and a project profile can grant it deliberately.
                computer: child_computer.clone(),
                memory: self.memory.clone(),
                canvas: self.canvas.clone(),
                // See `ToolEnv::handoff`: a subagent's run is its return value,
                // so there is no session beneath it to hand on.
                handoff: None,
                delegation: Some(DelegationEnv {
                    provider: provider.clone(),
                    context: nested_context,
                    handles: self.handles.clone(),
                    events: self.events.clone(),
                    profiles: self.profiles.clone(),
                }),
                // A subagent is addressable too: it has a name for the run's
                // lifetime, so a sibling or its parent can reach it while it
                // works rather than only when it returns.
                // Never in a child: a background subagent cannot block on the human.
                // It can `send` its parent a clarification request through the same
                // durable artist mailbox used for all inter-agent communication.
                ask: None,
                inbox: Some(crate::messaging::Inbox::new(identity.name.clone())),
                sessions: crate::session_tools::SessionHub::standard(
                    child_tools.project_root(),
                    identity.name.clone(),
                    Some(run.permit.clone()),
                ),
                pages: crate::pagination::PageStore::memory(),
                relationships: crate::relationships::RelationshipStore::for_project(
                    child_tools.project_root(),
                )
                .map_err(|error| {
                    DelegateError::Failed(format!("opening relationship store: {error}"))
                })?,
                tool_registry: child_tool_registry.clone(),
                dynamic: self.dynamic.clone(),
                extension_runs: self.extension_runs.clone(),
                extension_manager: self.extension_manager.clone(),
                disabled: self.disabled_tools.clone(),
            };
            let registered = tool_set::build(role, &env);
            // A delegate has an independently filtered tool surface and can
            // outlive the parent attempt. Retain exactly what this child saw,
            // never reconstruct it from the profile that happens to be live
            // during a later Muse replay.
            let mut model_tool_names = registered
                .iter()
                .map(|tool| tool.name().to_owned())
                .collect::<Vec<_>>();
            model_tool_names.sort();
            let tool_definitions = registered
                .iter()
                .map(|tool| {
                    let definition = tool.definition();
                    json!({
                        "name": definition.name.as_str(),
                        "title": definition.title.as_str(),
                        "description": definition.description.as_str(),
                        "input_schema": &definition.input_schema,
                        "output_schema": &definition.output_schema,
                        "category": definition.category,
                        "annotations": definition.annotations,
                    })
                })
                .collect::<Vec<_>>();
            let tool_definitions_digest = crate::digest_json(&tool_definitions)
                .map_err(|error| DelegateError::Failed(error.to_string()))?;
            let yield_schema_digest = role
                .yield_schema
                .as_ref()
                .map(crate::digest_json)
                .transpose()
                .map_err(|error| DelegateError::Failed(error.to_string()))?;
            let instructions_digest = crate::digest_bytes(role.instructions.as_bytes());
            let profile_digest = crate::digest_json(&json!({
                "name": role.name.as_str(),
                "description": role.description.as_str(),
                "instructions": role.instructions.as_str(),
                "yield_schema": &role.yield_schema,
                "tool_surface": &model_tool_names,
            }))
            .map_err(|error| DelegateError::Failed(error.to_string()))?;
            let extensions = self
                .extension_manager
                .as_ref()
                .map(|extensions| extensions.provenance())
                .unwrap_or_default();
            child_tool_registry.publish(registered.clone());
            let agent = builder
                .dynamic_tools(
                    registered
                        .into_iter()
                        .map(|tool| rig_agent::tool::DynamicTool::from(tool.portable()))
                        .collect(),
                )
                .add_hook(CaptureHook::new(tool_meta.clone()))
                // A subagent reads its own mail at its own turn boundaries. It
                // carries no user-steering handle: steering is the user talking
                // to the session, and a child is not the session.
                .add_hook(crate::steering::SteeringHook {
                    steering: crate::SteeringHandle::default(),
                    inbox: Some(crate::messaging::Inbox::new(identity.name.clone())),
                })
                .add_hook(TtsrHook(Arc::clone(&ttsr)))
                .add_hook(crate::profile_hook::ProfileUpdateHook::new(
                    child_tools.project_root().to_path_buf(),
                    role,
                ))
                .default_max_turns(usize::MAX)
                .build();
            run_recorder.record(RunStarted {
                provider: format!("{:?}", provider.provider).to_lowercase(),
                model: model.to_owned(),
                reasoning_effort: thinking
                    .level
                    .map(|level| level.as_str().to_owned())
                    .or_else(|| provider.reasoning_effort.clone()),
                agent: Some(identity.name.clone()),
                actor: Some(identity.actor.clone()),
                profile: Some(role.name.clone()),
            });
            run_recorder.record(SessionToolContext {
                surface_version: "artist-tool-surface-v3".into(),
                profile: role.name.clone(),
                profile_digest,
                tools: model_tool_names,
                tool_definitions_digest,
                tool_definitions,
                yield_schema_digest,
                yield_schema: role.yield_schema.clone(),
                instructions_digest,
                instructions: role.instructions.clone(),
                extensions,
            });

            let mut stream = agent
                .stream_chat(seed_prompt.clone(), seed_history.clone())
                .await;
            let mut captured_tool_calls = HashMap::<String, (String, serde_json::Value)>::new();
            // Text of the current model turn; the last turn's text is the
            // delegate's answer (matching the non-streaming `chat` output).
            let mut turn_text = String::new();
            let mut last_turn_had_text_delta = false;
            let mut final_messages = None;
            let mut retry = false;
            let mut attempt_observed = false;
            loop {
                let item = tokio::select! {
                    biased;
                    _ = self.handles.cancel.cancelled() => {
                        run_recorder.record(RunFinished::Cancelled);
                        recorder.record(DelegateFinished { outcome: "cancelled".into() });
                        self.finish_child(&display_id, "cancelled");
                        return Err(DelegateError::Failed("cancelled".into()));
                    }
                    item = stream.next() => item,
                };
                let Some(item) = item else {
                    // Match any trailing text/reasoning buffered below the
                    // coalesce threshold before the run completes.
                    if ttsr.finalize_reasoning() || ttsr.finalize_text() {
                        let firing = ttsr.take_pending().expect("finalize stashed the firing");
                        drop(stream);
                        let (committed, _) = ttsr.committed();
                        seed_history = committed;
                        crate::record_firing_events(&run_recorder, &ttsr, &firing);
                        self.emit_child(
                            &display_id,
                            PromptEvent::RuleFired {
                                rule: firing.rule.0.clone(),
                                matched: firing.matched.clone(),
                            },
                        );
                        run_recorder.record(RunFinished::Cancelled);
                        seed_prompt = reminder_message(&firing);
                        retries_used += 1;
                        retry = true;
                        break;
                    }
                    run_recorder.record(RunFinished::Completed);
                    break;
                };
                if item.is_ok() {
                    attempt_observed = true;
                }
                match item {
                    Ok(MultiTurnStreamItem::CompletionCall(call)) => {
                        last_turn_had_text_delta = !turn_text.is_empty();
                        turn_text.clear();
                        // Under the child's own lineage, so a fan-out's cache
                        // behaviour is attributable per subagent rather than
                        // summed into the parent's.
                        run_recorder.record(artist_session::RunUsage {
                            provider: format!("{:?}", provider.provider).to_lowercase(),
                            model: model.to_owned(),
                            input_tokens: call.usage.input_tokens,
                            output_tokens: call.usage.output_tokens,
                            total_tokens: call.usage.total_tokens,
                            cached_input_tokens: call.usage.cached_input_tokens,
                        });
                        self.emit_child(
                            &display_id,
                            PromptEvent::CompletionUsage {
                                total_tokens: call.usage.total_tokens,
                                cached_input_tokens: call.usage.cached_input_tokens,
                            },
                        );
                    }
                    Ok(MultiTurnStreamItem::FinalResponse(response)) => {
                        // The final response is authoritative. Some providers do
                        // not emit text deltas, which previously produced a
                        // successful subagent run with an empty output.
                        turn_text = response.output().to_owned();
                        if !last_turn_had_text_delta && !turn_text.is_empty() {
                            self.emit_child(&display_id, PromptEvent::TextDelta(turn_text.clone()));
                        }
                        final_messages = response.messages().map(ToOwned::to_owned);
                    }
                    Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::Text(text),
                    )) => {
                        turn_text.push_str(&text.text);
                        self.emit_child(&display_id, PromptEvent::TextDelta(text.text));
                    }
                    Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::ReasoningDelta {
                            // Match summary (`id: None`) and raw (`id: Some`)
                            // reasoning alike; raw deltas were dropped before,
                            // so reasoning-target rules missed them.
                            id: _,
                            reasoning,
                        },
                    )) => {
                        if ttsr.push_reasoning(&reasoning) {
                            let firing = ttsr.take_pending().expect("reasoning firing stashed");
                            drop(stream);
                            let (committed, _) = ttsr.committed();
                            seed_history = committed;
                            crate::record_firing_events(&run_recorder, &ttsr, &firing);
                            self.emit_child(
                                &display_id,
                                PromptEvent::RuleFired {
                                    rule: firing.rule.0.clone(),
                                    matched: firing.matched.clone(),
                                },
                            );
                            run_recorder.record(RunFinished::Cancelled);
                            seed_prompt = reminder_message(&firing);
                            retries_used += 1;
                            retry = true;
                            break;
                        }
                        self.emit_child(&display_id, PromptEvent::ReasoningSummaryDelta(reasoning));
                    }
                    Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::ToolCall {
                            tool_call,
                            internal_call_id,
                        },
                    )) => {
                        let name = tool_call.function.name;
                        let arguments = tool_call.function.arguments;
                        captured_tool_calls
                            .insert(internal_call_id.clone(), (name.clone(), arguments.clone()));
                        run_recorder.record(ModelTurn {
                            turn: ttsr.turn() as u32,
                            content: vec![ContentBlock::ToolCall {
                                id: internal_call_id.clone(),
                                call_id: Some(tool_call.id),
                                name: name.clone(),
                                arguments: arguments.clone(),
                                signature: None,
                            }],
                            total_tokens: 0,
                            partial: false,
                        });
                        self.emit_child(
                            &display_id,
                            PromptEvent::ToolCall {
                                id: internal_call_id,
                                name,
                                arguments,
                            },
                        )
                    }
                    Ok(MultiTurnStreamItem::ToolExecutionCommitted {
                        tool_call,
                        internal_call_id,
                    }) => self.emit_child(
                        &display_id,
                        PromptEvent::ToolExecutionStart {
                            id: internal_call_id,
                            name: tool_call.function.name,
                        },
                    ),
                    Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                        tool_result,
                        internal_call_id,
                    })) => {
                        let mut images = Vec::new();
                        let content = tool_result
                            .content
                            .into_iter()
                            .filter_map(|item| match item {
                                ToolResultContent::Text(text) => Some(text.text),
                                ToolResultContent::Image(image) => {
                                    images.extend(crate::store_result_image(
                                        &image,
                                        self.handles.attachments.as_ref(),
                                    ));
                                    None
                                }
                                ToolResultContent::Json { value } => Some(value.to_string()),
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        let meta = tool_meta.take(&internal_call_id);
                        let (outcome, duration_ms) =
                            meta.unwrap_or((ToolOutcomeRecord::Success, 0));
                        if let Some((name, arguments)) =
                            captured_tool_calls.remove(&internal_call_id)
                        {
                            let ingest_file_operation =
                                matches!(name.as_str(), "read" | "edit" | "write");
                            let operation = name.clone();
                            let presentation =
                                artist_session::presentation::ModelPresentation::literal(
                                    content.clone(),
                                );
                            let presentation = arguments
                                .get("path")
                                .and_then(serde_json::Value::as_str)
                                .filter(|path| !path.is_empty())
                                .map(|path| {
                                    presentation.clone().with_source(
                                        artist_session::presentation::PresentationSource {
                                            path: path.to_owned(),
                                            anchors: Vec::new(),
                                            occurrence: Some(format!("tool:{internal_call_id}")),
                                        },
                                    )
                                })
                                .unwrap_or(presentation);
                            run_recorder.record(ToolResultEvent {
                                internal_call_id: internal_call_id.clone(),
                                tool_call_id: Some(tool_result.id.clone()),
                                name,
                                arguments,
                                result: content.clone(),
                                presentation: Some(presentation),
                                outcome: outcome.clone(),
                                duration_ms: Some(duration_ms),
                            });
                            if !images.is_empty() {
                                run_recorder.record(ToolResultImagesEvent {
                                    internal_call_id: internal_call_id.clone(),
                                    images: images
                                        .iter()
                                        .map(|image| ContentBlock::Image {
                                            attachment: image.attachment.clone(),
                                            media_type: image.media_type.clone(),
                                        })
                                        .collect(),
                                });
                            }
                            if ingest_file_operation {
                                crate::ingest_muse_after_file_tool(
                                    &self.handles,
                                    child_tools.project_root(),
                                    &operation,
                                )
                                .await;
                            }
                        }
                        self.emit_child(
                            &display_id,
                            PromptEvent::ToolResult {
                                id: internal_call_id,
                                content,
                                outcome: Some(outcome),
                                duration_ms: Some(duration_ms),
                                images,
                            },
                        );
                    }
                    Ok(_) => {}
                    Err(error) => {
                        if let Some(firing) = ttsr.take_pending()
                            && let rig_agent::agent::StreamingError::Prompt(boxed) = &error
                            && let PromptError::PromptCancelled { chat_history, .. } =
                                boxed.as_ref()
                        {
                            seed_history = chat_history.clone();
                            crate::record_firing_events(&run_recorder, &ttsr, &firing);
                            self.emit_child(
                                &display_id,
                                PromptEvent::RuleFired {
                                    rule: firing.rule.0.clone(),
                                    matched: firing.matched.clone(),
                                },
                            );
                            run_recorder.record(RunFinished::Cancelled);
                            seed_prompt = reminder_message(&firing);
                            retries_used += 1;
                            retry = true;
                            break;
                        }
                        run_recorder.record(RunFinished::Error {
                            error: error.to_string(),
                        });
                        if !attempt_observed
                            && crate::provider_retry::is_overload(self.provider.provider, &error)
                            && let Some(delay) = overload_retry.schedule()
                        {
                            drop(stream);
                            tokio::select! {
                                _ = tokio::time::sleep(delay) => {
                                    retry = true;
                                    break;
                                }
                                _ = self.handles.cancel.cancelled() => {
                                    recorder.record(DelegateFinished { outcome: "cancelled".into() });
                                    self.finish_child(&display_id, "cancelled");
                                    return Err(DelegateError::Failed("cancelled".into()));
                                }
                            }
                        }
                        recorder.record(DelegateFinished {
                            outcome: "error".into(),
                        });
                        self.finish_child(&display_id, "error");
                        return Err(match crate::fallback::classify(&error) {
                            crate::fallback::Failure::Unavailable => {
                                DelegateError::Unavailable(error.to_string())
                            }
                            crate::fallback::Failure::Permanent => {
                                DelegateError::Failed(error.to_string())
                            }
                        });
                    }
                }
            }
            if retry {
                continue;
            }
            let conversation = final_messages.map(|messages| {
                child_conversation_messages(&seed_history, parent_history_len, messages)
            });
            break (turn_text, conversation);
        };
        if let Some(conversation) = conversation {
            recorder.record(conversation);
        }
        recorder.record(DelegateFinished {
            outcome: "completed".into(),
        });
        self.finish_child(&display_id, "completed");
        Ok(shorten(&output, 50 * 1024))
    }
}

pub(crate) fn child_conversation_messages(
    seed_history: &[Message],
    parent_history_len: usize,
    run_messages: Vec<Message>,
) -> ConversationMessages {
    let mut messages = seed_history
        .get(parent_history_len..)
        .unwrap_or_default()
        .to_vec();
    messages.extend(run_messages);
    ConversationMessages {
        messages,
        reset: true,
        display_from: 0,
    }
}

fn delegate_context_window(
    delegate_model: &str,
    main_model: Option<&str>,
    main_window: Option<u64>,
) -> Option<u64> {
    (main_model == Some(delegate_model))
        .then_some(main_window)
        .flatten()
}

fn shorten(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_owned();
    }
    let mut end = max.saturating_sub(16);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated]", &value[..end])
}

fn canonical_agent_path(artist: &str) -> String {
    format!("agent://{artist}")
}

/// Capture a completed work-unit yield into the parent's artifact store,
/// returning its id. Best-effort: a yield is never withheld because the
/// artifact write failed.
fn capture_yield(
    pages: &crate::pagination::PageStore,
    profile: &str,
    value: &Value,
) -> Option<String> {
    pages
        .capture_text(
            &format!("{profile}:yield"),
            "application/json",
            &serde_json::to_string(value).unwrap_or_default(),
        )
        .ok()
}

#[cfg(test)]
mod identity_tests {
    use super::{PermitSlot, canonical_agent_path, delegate_context_window};

    /// A seat from a pool of this test's own, so identity allocation is tested
    /// without also standing up the project-wide concurrency limit.
    async fn seat(pool: &tempfile::TempDir) -> PermitSlot {
        PermitSlot::claim(
            artist_registry::Registry::at(pool.path()).permits(1),
            "test".into(),
        )
        .await
        .expect("a fresh pool has a seat")
    }

    #[test]
    fn model_override_does_not_inherit_main_context_window() {
        assert_eq!(
            delegate_context_window("main", Some("main"), Some(100_000)),
            Some(100_000)
        );
        assert_eq!(
            delegate_context_window("small", Some("main"), Some(100_000)),
            None
        );
        assert_eq!(delegate_context_window("small", None, Some(100_000)), None);
    }

    #[test]
    fn delegated_agents_return_their_typed_noun_path() {
        assert_eq!(canonical_agent_path("monet"), "agent://monet");
    }

    /// The seat pool is the project's, not the process's: a second holder
    /// pointed at the same directory sees the first one's seat. This is the
    /// property the on-disk pool exists for, and the one an in-process
    /// semaphore silently did not have.
    #[tokio::test]
    async fn a_seat_is_visible_to_another_holder_of_the_same_project() {
        let pool = tempfile::tempdir().unwrap();
        let held = seat(&pool).await;

        let contender = artist_registry::Registry::at(pool.path()).permits(1);
        assert!(
            contender.try_acquire("other-process").unwrap().is_none(),
            "the limit is the project's"
        );

        held.yield_seat().await;
        assert!(contender.try_acquire("other-process").unwrap().is_some());
    }
}

#[cfg(test)]
mod yield_artifact_tests {
    use super::capture_yield;

    #[test]
    fn a_completed_yield_is_captured_with_profile_provenance() {
        let pages = crate::pagination::PageStore::memory();
        let id = capture_yield(
            &pages,
            "reviewer",
            &serde_json::json!({"answer": "done", "notes": "uphill both ways"}),
        )
        .expect("yield captured");
        let info = pages.artifact(&id).expect("read back").expect("exists");
        assert_eq!(info.tool, "reviewer:yield");
        assert_eq!(info.content_type, "application/json");
        assert!(info.media_type.is_none());
        // The exact yield value is the artifact payload, so the payload text is
        // addressable through the raw projection without re-entering model text.
        assert!(info.total_bytes > 0);
        assert_eq!(pages.artifact(&id).expect("read").expect("present").id, id);
    }

    #[test]
    fn repeated_identical_yields_are_occurrence_distinct_artifacts() {
        let pages = crate::pagination::PageStore::memory();
        let value = serde_json::json!({"answer": "same"});
        let first = capture_yield(&pages, "planner", &value).unwrap();
        let second = capture_yield(&pages, "planner", &value).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn distinct_profiles_do_not_collide_in_one_store() {
        let pages = crate::pagination::PageStore::memory();
        let value = serde_json::json!({"answer": "done"});
        let reviewer = capture_yield(&pages, "reviewer", &value).unwrap();
        let planner = capture_yield(&pages, "planner", &value).unwrap();
        assert_ne!(reviewer, planner);
        assert_eq!(pages.artifacts().unwrap().len(), 2);
    }
}
