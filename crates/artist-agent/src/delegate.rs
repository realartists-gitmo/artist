use std::sync::Arc;

use crate::{
    PromptEvent, SessionHandles,
    capture::{CaptureHook, ToolMeta},
    delegate_jobs::DelegateJobs,
    resources::Resources,
    tool_set::{self, DelegationEnv},
    ttsr::{TtsrHook, TtsrShared, reminder_message},
};
use artist_session::{
    ConversationMessages, DelegateFinished, DelegateStarted, RunFinished, RunStarted,
};
use artist_tools::ToolBundle;
use futures::StreamExt;
use llm_provider::SavedProvider;
use rig_agent::client::AgentClientExt;
use rig_agent::{agent::MultiTurnStreamItem, prelude::PromptError, streaming::StreamingChat};
use rig_core::{
    client::CompletionClient,
    completion::{Message, message::ToolResultContent},
    streaming::{StreamedAssistantContent, StreamedUserContent},
    tool::{PortableDynamicTool, PortableTool},
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
    jobs: DelegateJobs,
    resources: Resources,
    handles: SessionHandles,
    disabled_tools: Vec<String>,
    profiles: crate::profiles::Profiles,
    events: UnboundedSender<PromptEvent>,
    /// Carried so a nested subagent is built from the same environment its
    /// spawner had, rather than a narrower one assembled here.
    memory: Option<crate::memory::MemoryWriter>,
    canvas: Option<Arc<artist_canvas::server::Lazy>>,
    dynamic: Vec<PortableDynamicTool>,
    /// The seat held by the run that owns this tool. `None` at the session
    /// root, which holds none.
    parent_permit: Option<PermitSlot>,
    /// Whose todo list a child spawned through this tool may read. The list
    /// belonging to the run that owns the tool — which is the conversation at
    /// the session root, and the spawning subagent's actor once nested. Taking
    /// the session id at every depth would show a grandchild the root's list
    /// instead of the one its own instructions were written against.
    spawner: String,
    /// The spawning agent's *display* name, recorded on each child so the
    /// directory can answer "everyone under Monet". `None` where the spawner
    /// has no identity, which is only the case in tests.
    spawner_name: Option<String>,
}

struct DelegateRun {
    actor: String,
    background: bool,
    permit: PermitSlot,
}

impl DelegateRun {
    fn new(task_id: Option<String>, permit: PermitSlot) -> Self {
        let background = task_id.is_some();
        Self {
            actor: task_id.unwrap_or_else(|| artist_tools::short_id("a")),
            background,
            permit,
        }
    }
}

impl Delegate {
    /// Built from the environment its spawner was built from, so a nested
    /// subagent inherits the same subsystems rather than a hand-copied subset.
    pub(crate) fn new(env: &crate::tool_set::ToolEnv, delegation: &DelegationEnv) -> Self {
        let jobs = DelegateJobs::for_project(env.bundle.project_root());
        Self {
            provider: delegation.provider.clone(),
            tools: env.bundle.clone(),
            context: Arc::clone(&delegation.context),
            jobs,
            resources: env.resources.clone(),
            handles: delegation.handles.clone(),
            disabled_tools: env.disabled.clone(),
            profiles: delegation.profiles.clone(),
            events: delegation.events.clone(),
            memory: env.memory.clone(),
            canvas: env.canvas.clone(),
            dynamic: env.dynamic.clone(),
            parent_permit: delegation.parent_permit.clone(),
            spawner: env.todo_owner.clone(),
            // The inbox name is this agent's name, so a child's parent link and
            // the address a person uses are the same string by construction
            // rather than by two lookups that could disagree.
            spawner_name: env.inbox.as_ref().map(|inbox| inbox.name.to_string()),
        }
    }

    fn emit(&self, event: PromptEvent) {
        // Background delegates can outlive the parent stream that owned the
        // receiver. Display delivery is therefore best-effort and must never
        // turn a successful background job into a tool failure.
        let _ = self.events.send(event);
    }

    fn emit_child(&self, id: &str, event: PromptEvent) {
        self.emit(PromptEvent::SubagentEvent {
            id: id.to_owned(),
            event: Box::new(event),
        });
    }

    fn finish_child(&self, id: &str, outcome: &str) {
        self.emit(PromptEvent::SubagentFinished {
            id: id.to_owned(),
            outcome: outcome.to_owned(),
        });
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DelegateArgs {
    mode: Option<String>,
    prompt: Option<String>,
    agent: Option<String>,
    fork: Option<bool>,
    background: Option<bool>,
    task_id: Option<String>,
    /// Several tasks to wait on at once. The motivating shape is a fan-out:
    /// spawn five researchers, then sleep until all five reports are in.
    task_ids: Option<Vec<String>>,
    wait_ms: Option<u64>,
}

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
        "Run a focused subagent.".into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{
            "mode":{"enum":["run","start","status","read","wait","cancel","list"],"default":"run","description":"Operation to perform."},
            "prompt":{"type":"string","description":"Task for the subagent."},
            "agent":{"type":"string","enum":self.profiles.names(),"description":"Configured subagent role."},
            "fork":{"type":"boolean","default":false,"description":"Include the full main-agent chat context."},
            "background":{"type":"boolean","default":false,"description":"Start the subagent and return immediately."},
            "taskIds":{"type":"array","items":{"type":"string"},"description":"Several task ids to wait on at once; the call returns when all of them have settled."},"taskId":{"type":"string","description":"Task identifier returned when a subagent is started; required for status, read, wait, and cancel."},
            "waitMs":{"type":"integer","minimum":1,"maximum":30000,"description":"Maximum time to wait for a background task state change."}
        },"additionalProperties":false})
    }

    async fn call(&self, args: DelegateArgs) -> Result<String, DelegateError> {
        let mode = args.mode.clone().unwrap_or_else(|| {
            if args.background.unwrap_or(false) {
                "start"
            } else {
                "run"
            }
            .to_owned()
        });
        // Only the modes that block this run on a child give up its seat. A
        // background `start` returns immediately and the child claims a seat of
        // its own, so the spawner keeps working and keeps its own.
        let yielded = match mode.as_str() {
            "run" | "wait" => self.parent_permit.clone(),
            _ => None,
        };
        if let Some(slot) = &yielded {
            slot.yield_seat().await;
        }
        let result = self.run_mode(&mode, args).await;
        if let Some(slot) = &yielded {
            slot.retake().await;
        }
        result
    }
}

impl Delegate {
    async fn run_mode(&self, mode: &str, args: DelegateArgs) -> Result<String, DelegateError> {
        match mode {
            "run" => {
                let prompt = required(args.prompt, "prompt")?;
                self.run_agent(
                    prompt,
                    args.agent.as_deref().unwrap_or("default"),
                    args.fork.unwrap_or(false),
                    None,
                )
                .await
            }
            "start" => {
                let prompt = required(args.prompt, "prompt")?;
                // A background child is not something this run waits on, so it
                // must not hold this run's seat: it claims its own, and this
                // one is freed when its own run ends rather than when the
                // detached job does.
                let mut delegate = self.clone();
                delegate.parent_permit = None;
                let task_prompt = prompt.clone();
                let role = args.agent.unwrap_or_else(|| "default".into());
                let task_role = role.clone();
                Ok(self
                    .jobs
                    .start(prompt, role, move |task_id| async move {
                        delegate
                            .run_agent(
                                task_prompt,
                                &task_role,
                                args.fork.unwrap_or(false),
                                Some(task_id),
                            )
                            .await
                            .map_err(|error| error.to_string())
                    })
                    .await)
            }
            "status" => self
                .jobs
                .status(&required(args.task_id, "taskId")?)
                .await
                .map_err(DelegateError::Failed),
            "read" => self
                .jobs
                .read(&required(args.task_id, "taskId")?)
                .await
                .map_err(DelegateError::Failed),
            "wait" => {
                // One wait primitive, whether the caller named one task or
                // twenty. Shipping a separate multi-wait would be a second
                // mechanism that has to agree with this one and eventually
                // would not — the failure this crate's tool registry exists to
                // prevent.
                let ids = match (args.task_ids, args.task_id) {
                    (Some(ids), _) if !ids.is_empty() => ids,
                    (_, Some(id)) => vec![id],
                    _ => return Err(DelegateError::Failed("taskId is required".into())),
                };
                if let [single] = ids.as_slice() {
                    return self
                        .jobs
                        .wait(single, args.wait_ms)
                        .await
                        .map_err(DelegateError::Failed);
                }
                // Concurrently, and bounded by one shared budget: waiting on
                // five tasks should take as long as the slowest, not as long as
                // the sum, and a caller that asked to wait 30s means 30s
                // overall rather than per task.
                let results = futures::future::join_all(
                    ids.iter().map(|id| self.jobs.wait(id, args.wait_ms)),
                )
                .await;
                let reports: Vec<Value> = ids
                    .iter()
                    .zip(results)
                    .map(|(id, result)| match result {
                        Ok(report) => serde_json::from_str(&report)
                            .unwrap_or_else(|_| json!({"taskId": id, "status": "unknown"})),
                        Err(error) => json!({"taskId": id, "status": "failed", "error": error}),
                    })
                    .collect();
                Ok(Value::Array(reports).to_string())
            }
            "cancel" => self
                .jobs
                .cancel(&required(args.task_id, "taskId")?)
                .await
                .map_err(DelegateError::Failed),
            "list" => Ok(self.jobs.list().await),
            other => Err(DelegateError::Failed(format!(
                "invalid delegate mode: {other}"
            ))),
        }
    }
}

impl Delegate {
    /// Drive the subagent on the streaming surface (delta hooks — and
    /// therefore stream rules — only exist there), with the same TTSR
    /// abort/inject/retry loop as the main agent. The shared `RulesHandle`
    /// makes once-per-session global across main + delegates; delegate
    /// events land in the session log under a child lineage.
    async fn run_agent(
        &self,
        prompt: String,
        profile_name: &str,
        fork: bool,
        task_id: Option<String>,
    ) -> Result<String, DelegateError> {
        let profile = self
            .profiles
            .get(profile_name)
            .map_err(DelegateError::Failed)?;
        let run = DelegateRun::new(
            task_id,
            PermitSlot::claim(
                self.profiles.permits.clone(),
                format!("{profile_name}:{}", self.handles.conversation_id),
            )
            .await?,
        );
        let breaker = crate::fallback::Breaker::global();
        let mut skipped = Vec::new();
        let mut last_error = None;

        // Ordered, not round-robin: candidate 0 is preferred while healthy.
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
                    fork,
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
                // A permanent failure reproduces on every candidate, so trying
                // the rest would only replace the real error with the last one.
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
        let child_tools = self
            .tools
            .for_actor(&actor)
            .map_err(|error| DelegateError::Failed(error.to_string()))?;
        self.emit(PromptEvent::SubagentStarted {
            id: actor.clone(),
            role: role.name.clone(),
            prompt: prompt.clone(),
        });
        let recorder = self.handles.recorder.child_lineage(&actor);
        recorder.record(DelegateStarted {
            prompt: prompt.clone(),
            // `computer` counts as write access: a subagent that can drive a
            // GUI can do anything a person at that keyboard could, so the log
            // must not record it as read-only.
            read_only: !["bash", "edit", "write", "computer"]
                .into_iter()
                .any(|tool| role.permits(tool)),
            fork,
            background: run.background,
        });
        // Budded off the parent's, not shared with it: this delegate gets its
        // own display, session bus and browser profile, and they are torn down
        // with it. Lazy, so a delegate that never touches a GUI never starts a
        // compositor — which is most of them.
        let child_computer = self
            .handles
            .computer
            .as_ref()
            .map(artist_computer::SurfaceRegistry::for_delegate);

        let (base, _) = crate::prompt_config::base_prompt();
        // Held for the life of the run: a subagent is not a resumable session,
        // so its name goes back to the roster the moment the run ends. Without
        // that a fan-out would drain the roster at the rate it spawns children.
        // Placed last in the prompt for the same reason as at the root — every
        // byte above it is shared between siblings and can cache across them.
        let identity = crate::identity::for_run(
            &run.actor,
            self.tools.project_root(),
            &role.name,
            // The spawner's display name, so `descendantOf` walks the same
            // identifiers a person addresses agents with.
            self.spawner_name.as_deref(),
        );
        let policy = format!(
            "{base}\n\nYou are the '{}' subagent profile.\n{}\n\n{}\nCurrent working directory: {}{}",
            role.name,
            role.instructions,
            self.resources.prompt_section(),
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
            let env = tool_set::ToolEnv {
                bundle: child_tools.clone(),
                // The delegate's own lineage recorder, so `artist computer log`
                // attributes a subagent's actions to the subagent.
                recorder: recorder.clone(),
                resources: self.resources.clone(),
                todos: self.handles.todos.clone(),
                // A child owns its own list and may read its parent's, but not
                // write to it: concurrent siblings sharing one list would race
                // with no obvious merge.
                todo_owner: actor.clone(),
                todo_parent: Some(self.spawner.clone()),
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
                    parent_permit: Some(run.permit.clone()),
                }),
                // A subagent is addressable too: it has a name for the run's
                // lifetime, so a sibling or its parent can reach it while it
                // works rather than only when it returns.
                // Never in a child: a background subagent blocking on a human
                // nobody is watching is the worst version of asking, and it has
                // a better option — `query`, whose target is the parent that
                // spawned it and is right there. An underspecified task comes
                // back as a blocked result for the parent to resolve.
                ask: None,
                cancel: self.handles.cancel.clone(),
                inbox: Some(crate::messaging::Inbox::new(identity.name.clone())),
                dynamic: self.dynamic.clone(),
                disabled: self.disabled_tools.clone(),
            };
            let agent = builder
                .dynamic_tools(
                    tool_set::build(role, &env)
                        .into_iter()
                        .map(rig_agent::tool::DynamicTool::from)
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

            let mut stream = agent
                .stream_chat(seed_prompt.clone(), seed_history.clone())
                .await;
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
                        self.finish_child(&actor, "cancelled");
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
                            &actor,
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
                            &actor,
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
                            self.emit_child(&actor, PromptEvent::TextDelta(turn_text.clone()));
                        }
                        final_messages = response.messages().map(ToOwned::to_owned);
                    }
                    Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::Text(text),
                    )) => {
                        turn_text.push_str(&text.text);
                        self.emit_child(&actor, PromptEvent::TextDelta(text.text));
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
                                &actor,
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
                        self.emit_child(&actor, PromptEvent::ReasoningSummaryDelta(reasoning));
                    }
                    Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::ToolCall {
                            tool_call,
                            internal_call_id,
                        },
                    )) => self.emit_child(
                        &actor,
                        PromptEvent::ToolCall {
                            id: internal_call_id,
                            name: tool_call.function.name,
                            arguments: tool_call.function.arguments,
                        },
                    ),
                    Ok(MultiTurnStreamItem::ToolExecutionCommitted {
                        tool_call,
                        internal_call_id,
                    }) => self.emit_child(
                        &actor,
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
                        self.emit_child(
                            &actor,
                            PromptEvent::ToolResult {
                                id: internal_call_id,
                                content,
                                outcome: meta.as_ref().map(|(outcome, _)| outcome.clone()),
                                duration_ms: meta.map(|(_, duration)| duration),
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
                                &actor,
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
                                    self.finish_child(&actor, "cancelled");
                                    return Err(DelegateError::Failed("cancelled".into()));
                                }
                            }
                        }
                        recorder.record(DelegateFinished {
                            outcome: "error".into(),
                        });
                        self.finish_child(&actor, "error");
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
        self.finish_child(&actor, "completed");
        Ok(
            json!({"role":role.name,"taskId":actor,"output":shorten(&output, 50 * 1024)})
                .to_string(),
        )
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

fn required<T>(value: Option<T>, name: &str) -> Result<T, DelegateError> {
    value.ok_or_else(|| DelegateError::Failed(format!("{name} is required")))
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

#[cfg(test)]
mod identity_tests {
    use super::{DelegateRun, PermitSlot, delegate_context_window};

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

    #[tokio::test]
    async fn background_run_reuses_reserved_task_id() {
        let pool = tempfile::tempdir().unwrap();
        let run = DelegateRun::new(Some("a-reserved-task".into()), seat(&pool).await);

        assert_eq!(run.actor, "a-reserved-task");
        assert!(run.background);
    }

    #[tokio::test]
    async fn foreground_run_allocates_one_actor_id() {
        let pool = tempfile::tempdir().unwrap();
        let run = DelegateRun::new(None, seat(&pool).await);

        assert!(run.actor.starts_with("a-"));
        assert!(!run.background);
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
