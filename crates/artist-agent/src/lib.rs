//! The Artist agent loop, built on Rig.

mod ask_tool;
pub mod canvas;
mod capture;
mod code_search;
pub mod compaction;
mod computer_tool;
mod contracts;
mod conversation;
mod delegate;
#[cfg(test)]
mod delegate_tests;
mod fallback;
pub mod gemini_cache;
pub mod handoff;
mod identity;
mod lifecycle;
pub mod mcp;
pub mod memory;
mod messaging;
pub mod openai_responses;
pub mod pagination;
pub mod prefix;
pub mod profiles;
mod prompt_config;
mod provider_retry;
mod resources;
mod rig_provider;
mod ttsr;
#[cfg(test)]
mod ttsr_tests;

pub use lifecycle::{LifecycleEmitter, LifecycleEvent};
pub use resources::AvailableSkill;
mod bash_tool;
mod session_tools;
mod statefulness;

mod steering;

mod thinking;
pub mod todo;
mod tool_prompt;
pub mod tool_registry;
pub mod tool_set;

pub use statefulness::Statefulness;
pub use steering::SteeringHandle;
pub use tool_registry::ToolRegistryHandle;

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use artist_rules::matcher::RuleSet;
use artist_rules::state::RulesHandle;
use artist_rules::types::Firing;
use artist_session::{
    Recorder, RuleFired, RuleInjection, RunFinished, RunStarted, ToolOutcomeRecord,
};
use artist_tools::ToolBundle;
use base64::Engine;
use futures::StreamExt;
use llm_provider::SavedProvider;
use rig_agent::client::AgentClientExt;
use rig_agent::{agent::MultiTurnStreamItem, prelude::PromptError, streaming::StreamingPrompt};
use rig_core::{
    OneOrMany,
    client::CompletionClient,
    completion::message::{
        DocumentSourceKind, Image, ImageMediaType, Message, ToolResultContent, UserContent,
    },
    memory::{ConversationMemory, InMemoryConversationMemory},
    streaming::{StreamedAssistantContent, StreamedUserContent},
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use capture::{CaptureHook, ToolMeta};
use ttsr::{TtsrHook, TtsrShared, reminder_message};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum PromptEvent {
    ReasoningSummaryDelta(String),
    TextDelta(String),
    /// A delegated agent began running. Its subsequent stream events are
    /// wrapped in [`PromptEvent::SubagentEvent`] with the same id.
    SubagentStarted {
        id: String,
        role: String,
        prompt: String,
    },
    /// One event from a delegated agent's own streaming tool loop.
    SubagentEvent {
        id: String,
        event: Box<PromptEvent>,
    },
    /// A delegated agent stopped. `outcome` mirrors the persisted
    /// `delegate.finished` lifecycle value.
    SubagentFinished {
        id: String,
        outcome: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    ToolExecutionStart {
        id: String,
        name: String,
    },
    ToolResult {
        id: String,
        content: String,
        /// Structured outcome from the capture hook, when recording is on.
        outcome: Option<ToolOutcomeRecord>,
        duration_ms: Option<u64>,
        /// Image content items in the result. Each is already stored in the
        /// session attachment store, so the display stream carries only a
        /// digest and the payload rides the log.
        images: Vec<ToolImage>,
    },
    CompletionUsage {
        total_tokens: u64,
        /// Input tokens the provider served from its prompt cache. Zero also
        /// means "the provider told us nothing", so a display should read it as
        /// absence of evidence rather than a measured miss.
        cached_input_tokens: u64,
    },
    /// A stream rule matched: the run aborted, the reminder was injected,
    /// and the run is retrying from the same point. The UI should clear any
    /// partial streaming output and show the rule card.
    RuleFired {
        rule: String,
        matched: String,
    },
    /// A routing candidate failed and the run moved to the next one. Surfaced
    /// because a silent move to a weaker model is discovered hours later.
    ProviderFallback {
        from: String,
        reason: String,
    },
    /// The session changed profile. The transcript above stays on screen; the
    /// model's context does not.
    HandedOff {
        from: String,
        to: String,
    },
}

/// How a `stream_chat` run ended (errors surface via `Result`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Cancelled,
    /// The agent handed the session to another profile. Terminal for this run:
    /// the caller clears the conversation and starts the target profile with
    /// [`handoff::Handoff::seed`] as its opening task.
    HandedOff {
        from: String,
        handoff: handoff::Handoff,
    },
}

/// Everything a run needs beyond the prompt: shared handles owned by the
/// CLI session. `Default` gives inert handles (no recording, no rules, no
/// cancellation) — the configuration tests and simple embedders want.
#[derive(Clone)]
pub struct SessionHandles {
    pub steering: SteeringHandle,
    pub rules: RulesHandle,
    pub rule_set: Arc<RuleSet>,
    pub recorder: Recorder,
    pub memory: Arc<dyn ConversationMemory>,
    pub conversation_id: String,
    /// Identity already recorded for a resumed lineage. New sessions leave
    /// this empty and claim an identity normally; resumed child agents reuse
    /// their durable actor and display name instead of becoming a new agent.
    pub recorded_identity: Option<RecordedIdentity>,
    /// Provider-private opaque context; currently only consumed by the opt-in OpenAI adapter.
    pub provider_context: artist_session::ProviderContextHandle,
    /// Effective model context window from the normalized catalog. Unknown
    /// models use the adapter's conservative fallback.
    pub effective_context_window: Option<u64>,
    /// Request OpenAI priority processing for this session.
    pub fast_mode: bool,
    /// Non-display lifecycle notifications remain live for background delegates.
    pub lifecycle: LifecycleEmitter,
    pub cancel: CancellationToken,
    /// Blob store for tool-result image payloads. `None` for inert handles,
    /// where nothing is recorded and so nothing needs storing.
    pub attachments: Option<artist_session::AttachmentStore>,
    /// The configured provider accounts, so a profile naming a `provider:` can
    /// be resolved to an account other than the one running the session.
    /// Empty for inert handles, where every profile inherits the parent.
    pub providers: llm_provider::ProviderSet,
    /// Harness-owned todo lists, keyed by owner. Lives outside the model
    /// context so it survives compaction and handoff intact.
    pub todos: todo::TodoStore,
    /// Durable cross-session memory. Distinct from `memory` above, which is
    /// rig's conversation history for this session: this one outlives it.
    /// `None` disables the whole subsystem, including the tool.
    pub durable_memory: Option<memory::MemoryHandle>,
    /// Driveable surfaces for computer use, and the display they run on.
    /// `None` disables the subsystem, including the tool.
    ///
    /// Lives here rather than in [`ToolContext`] because delegates need it too:
    /// a subagent permitted to drive a GUI gets its own stage budded off this
    /// one, and `ToolContext` is a per-turn borrow the main agent alone sees.
    pub computer: Option<artist_computer::SurfaceRegistry>,
    /// Handoffs already performed in this session, so a resumed session keeps
    /// the provider-private lineage its current profile was running on.
    pub handoff_depth: usize,
    /// The tools registered for the current attempt, published so surfaces
    /// outside the loop — a canvas today — invoke exactly what the model can.
    pub tools: ToolRegistryHandle,
    /// Where a question to the user is posted, shared with every surface that
    /// can render or answer one. `None` for a run with no user attached, which
    /// is what makes the `ask` tool absent there rather than hanging.
    pub ask: Option<artist_session::AskRegistry>,
    /// The session's frozen system prompt, held here because prompt-cache
    /// stability is a property of the session rather than of any one turn.
    /// See [`prefix`] for why this is a store rather than a convention.
    pub prefix: prefix::PrefixFreezer,
    /// Where the provider-side conversation stands, when one is being held for
    /// us. Session-scoped because a provider client is rebuilt every turn, and
    /// a chain that reset each turn would never save anything.
    pub chain: artist_session::ChainState,
    /// Which provider-side handles this session may use. Every one is off by
    /// default; see [`statefulness`] for why this is passed in rather than read
    /// from the environment.
    pub statefulness: Statefulness,
    /// Capabilities this endpoint has already refused, so a missing files or
    /// prompts endpoint costs one probe per session rather than one per turn.
    pub capabilities: artist_session::ProviderCapabilities,
}

impl Default for SessionHandles {
    fn default() -> Self {
        Self {
            steering: SteeringHandle::default(),
            rules: RulesHandle::default(),
            rule_set: Arc::new(RuleSet::compile(Vec::new())),
            recorder: Recorder::noop(),
            memory: Arc::new(InMemoryConversationMemory::new()),
            conversation_id: "default".to_owned(),
            recorded_identity: None,
            provider_context: artist_session::ProviderContextHandle::noop(),
            effective_context_window: None,
            fast_mode: false,
            // Inert handles have no user attached, so there is nobody to ask
            // and the tool is correctly absent.
            ask: None,
            lifecycle: LifecycleEmitter::default(),
            cancel: CancellationToken::new(),
            attachments: None,
            providers: llm_provider::ProviderSet::default(),
            todos: todo::TodoStore::default(),
            durable_memory: None,
            computer: None,
            handoff_depth: 0,
            tools: ToolRegistryHandle::new(),
            prefix: prefix::PrefixFreezer::for_session(),
            chain: artist_session::ChainState::new(),
            capabilities: artist_session::ProviderCapabilities::for_session(),
            statefulness: Statefulness::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedIdentity {
    pub name: String,
    pub actor: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

impl ChatMessage {
    /// Text-only rig message (legacy sessions and simple callers).
    pub fn to_rig(&self) -> Message {
        match self.role {
            ChatRole::User => Message::user(&self.content),
            ChatRole::Assistant => Message::assistant(&self.content),
        }
    }
}

/// One image a tool returned, after it has been stored.
///
/// The display stream carries the digest rather than the bytes: a screenshot is
/// megabytes, the TUI only ever needs to name it, and the blob is already
/// durable in the session attachment store by the time this is emitted.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolImage {
    /// Content-addressed id in the session attachment store.
    pub attachment: String,
    pub media_type: Option<String>,
    pub bytes: usize,
}

#[cfg(test)]
mod prompt_event_wire_tests {
    use super::PromptEvent;

    #[test]
    fn prompt_events_round_trip_for_frontend_streams() {
        let events = [
            PromptEvent::TextDelta("hello".into()),
            PromptEvent::ReasoningSummaryDelta("thinking".into()),
            PromptEvent::ToolCall {
                id: "call-1".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "src/main.rs"}),
            },
            PromptEvent::CompletionUsage {
                total_tokens: 42,
                cached_input_tokens: 7,
            },
        ];
        for event in events {
            let encoded = serde_json::to_string(&event).unwrap();
            assert_eq!(
                serde_json::from_str::<PromptEvent>(&encoded).unwrap(),
                event
            );
        }
    }
}

/// Store a tool-result image, returning how to name it in the display stream.
///
/// Returns `None` when there is no attachment store (an unrecorded run) or the
/// image is not storable, in which case the caller keeps counting it but has no
/// digest to show.
pub(crate) fn store_result_image(
    image: &rig_core::completion::message::Image,
    attachments: Option<&artist_session::AttachmentStore>,
) -> Option<ToolImage> {
    use rig_core::completion::message::DocumentSourceKind;

    let bytes = match &image.data {
        // Computed rather than decoded: this is a display label, and decoding a
        // multi-megabyte screenshot twice to produce it is not worth it.
        DocumentSourceKind::Base64(data) => {
            let padding = data.bytes().rev().take_while(|byte| *byte == b'=').count();
            (data.len() / 4 * 3).saturating_sub(padding)
        }
        DocumentSourceKind::Raw(raw) => raw.len(),
        _ => 0,
    };
    match artist_session::store_tool_image(image, attachments?)? {
        artist_session::ContentBlock::Image {
            attachment,
            media_type,
        } => Some(ToolImage {
            attachment,
            media_type,
            bytes,
        }),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub struct ImageAttachment {
    pub data: Vec<u8>,
    pub media_type: ImageMediaType,
}

impl ImageAttachment {
    pub fn png(data: Vec<u8>) -> Self {
        Self {
            data,
            media_type: ImageMediaType::PNG,
        }
    }
    pub fn jpeg(data: Vec<u8>) -> Self {
        Self {
            data,
            media_type: ImageMediaType::JPEG,
        }
    }
    pub fn gif(data: Vec<u8>) -> Self {
        Self {
            data,
            media_type: ImageMediaType::GIF,
        }
    }
    pub fn webp(data: Vec<u8>) -> Self {
        Self {
            data,
            media_type: ImageMediaType::WEBP,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChatInput {
    pub text: String,
    pub images: Vec<ImageAttachment>,
}

impl From<String> for ChatInput {
    fn from(text: String) -> Self {
        Self {
            text,
            images: Vec::new(),
        }
    }
}

/// The tool surfaces available to a run: native tools, MCP proxies,
/// extension-provided tools, and the user's disabled-tool list.
#[derive(Clone, Copy)]
pub struct ToolContext<'a> {
    pub native: &'a ToolBundle,
    pub mcp: &'a mcp::McpManager,
    pub extensions: Option<&'a artist_extensions::Manager>,
    pub disabled: &'a [String],
    /// The canvas server's handle. It has not necessarily bound anything: the
    /// server starts on the first call that needs one. Absent in one-shot and
    /// headless paths, where the tool is simply not registered.
    pub canvas: Option<&'a std::sync::Arc<artist_canvas::server::Lazy>>,
}

/// Sends the small completion used by provider health checks through the same
/// typed Rig client dispatch as normal and delegated runs.
pub async fn provider_health_check(provider: &SavedProvider, model: &str) -> Result<String> {
    provider_health_check_with_device_flow(provider, model, false).await
}

/// Interactive health check; only this explicit CLI path may start device OAuth.
pub async fn provider_health_check_with_device_flow(
    provider: &SavedProvider,
    model: &str,
    allow_device_flow: bool,
) -> Result<String> {
    rig_provider::RigClient::build_with_device_flow(provider, allow_device_flow)?
        .prompt(
            model,
            "Reply with exactly OK and nothing else.",
            "Reply with exactly OK.",
            16,
        )
        .await
}

/// Runs a turn on the session's default profile.
pub async fn stream_chat(
    provider: &SavedProvider,
    input: &ChatInput,
    tool_context: ToolContext<'_>,
    handles: SessionHandles,
    on_event: impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome> {
    stream_chat_as(provider, "default", input, tool_context, handles, on_event).await
}

/// Runs a turn with the session root instantiated from a named profile.
///
/// The profile determines the system prompt, the tool surface, and the routing:
/// `provider` is only the account the session was launched with, and a profile
/// naming its own account resolves against [`SessionHandles::providers`]
/// instead. This is the same object and the same resolution a delegated
/// subagent uses — the session root is one of its three instantiation modes.
pub async fn stream_chat_as(
    provider: &SavedProvider,
    profile_name: &str,
    input: &ChatInput,
    tool_context: ToolContext<'_>,
    handles: SessionHandles,
    mut on_event: impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome> {
    let mut chain = vec![profile_name.to_owned()];
    let mut current = profile_name.to_owned();
    let mut seeded: Option<ChatInput> = None;
    let mut depth = handles.handoff_depth;
    loop {
        let turn = seeded.as_ref().unwrap_or(input);
        let outcome = run_profile(
            provider,
            &current,
            provider_lineage(&handles.conversation_id, depth),
            turn,
            tool_context,
            handles.clone(),
            &mut on_event,
        )
        .await?;
        let RunOutcome::HandedOff { from, handoff } = outcome else {
            return Ok(outcome);
        };
        chain.push(handoff.to.clone());
        // Functionally `/clear` followed by a fresh session on the target
        // profile: the conversation reset is the same event compaction emits,
        // and the payload becomes the incoming profile's opening task.
        handles.recorder.record(artist_session::HandoffPerformed {
            from: from.clone(),
            to: handoff.to.clone(),
            brief: handoff.brief.clone(),
            chain: chain.clone(),
            read_files: Vec::new(),
            modified_files: Vec::new(),
            jobs: Vec::new(),
            todos: handles.todos.get(&handles.conversation_id),
        });
        handles
            .memory
            .clear(&handles.conversation_id)
            .await
            .context("clear conversation for handoff")?;
        on_event(PromptEvent::HandedOff {
            from,
            to: handoff.to.clone(),
        })?;
        depth += 1;
        seeded = Some(ChatInput {
            text: handoff.seed(
                &current,
                &input.text,
                &chain,
                &todo::render(&handles.todos.get(&handles.conversation_id)),
            ),
            images: Vec::new(),
        });
        current = handoff.to.clone();
    }
}

/// One profile's turn, including its candidate fallback.
async fn run_profile(
    provider: &SavedProvider,
    profile_name: &str,
    lineage: String,
    input: &ChatInput,
    tool_context: ToolContext<'_>,
    handles: SessionHandles,
    on_event: &mut impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome> {
    let profiles = profiles::Profiles::discover(tool_context.native.project_root());
    let profile = profiles.get(profile_name).map_err(|error| anyhow!(error))?;
    let breaker = fallback::Breaker::global();
    let mut skipped = Vec::new();
    let mut last_error = None;

    // Ordered, not round-robin: candidate 0 is preferred while healthy.
    for candidate in &profile.candidates {
        let label = fallback::candidate_label(candidate);
        let resolved = match candidate.resolve(&handles.providers, provider) {
            Ok(resolved) => resolved.clone(),
            Err(error) => {
                skipped.push(fallback::Skipped {
                    candidate: label,
                    reason: error,
                });
                continue;
            }
        };
        let Some(model) = candidate.model_for(&resolved) else {
            skipped.push(fallback::Skipped {
                candidate: label,
                reason: "no model configured on the candidate or its account".into(),
            });
            continue;
        };
        let key = fallback::CandidateKey {
            provider: resolved.id.as_str().to_owned(),
            model: model.clone(),
        };
        if breaker.is_tripped(&key) {
            skipped.push(fallback::Skipped {
                candidate: label,
                reason: "cooling down after repeated failures".into(),
            });
            continue;
        }
        let run = RootRun {
            profiles: profiles.clone(),
            profile: profile.clone(),
            thinking: candidate.thinking.unwrap_or_default(),
            model,
        };
        match attempt(
            &resolved,
            &run,
            &lineage,
            input,
            tool_context,
            handles.clone(),
            on_event,
        )
        .await
        {
            Ok(outcome) => {
                breaker.record_success(&key);
                return Ok(outcome);
            }
            Err(error) if error.downcast_ref::<fallback::Unavailable>().is_some() => {
                breaker.record_failure(&key);
                let reason = error.to_string();
                on_event(PromptEvent::ProviderFallback {
                    from: label,
                    reason: reason.clone(),
                })?;
                last_error = Some(reason);
            }
            // A permanent failure reproduces on every candidate, so trying the
            // rest would only replace the real error with the last one.
            Err(error) => return Err(error),
        }
    }
    Err(anyhow!(fallback::exhausted(
        &profile.name,
        last_error,
        &skipped
    )))
}

/// One attempt on one resolved candidate.
///
/// Each arm monomorphizes `stream_chat_with` separately, which is why this
/// stays a match over provider kinds rather than a unified client type.
/// One attempt on one resolved candidate.
///
/// Construction is centralized in `RigClient::build`, so a profile naming a
/// different account routes through exactly the same path as the session's own.
#[allow(clippy::too_many_arguments)]
async fn attempt(
    resolved: &SavedProvider,
    run: &RootRun,
    lineage: &str,
    input: &ChatInput,
    tool_context: ToolContext<'_>,
    handles: SessionHandles,
    on_event: &mut impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome> {
    use rig_provider::RigClient;
    macro_rules! run_with {
        ($client:expr) => {
            stream_chat_with(
                $client,
                resolved,
                run,
                input,
                tool_context,
                handles,
                on_event,
            )
            .await
        };
    }
    match RigClient::build(resolved)? {
        RigClient::ArtistOpenAi(client) => {
            let mut client = client
                .with_provider_context(lineage.to_owned(), handles.provider_context.clone())
                .with_effective_context_window(handles.effective_context_window);
            // Beside the session's attachments, because it is the same content
            // under the same address — one says where the bytes are locally,
            // the other says what the provider calls them. Session-scoped for
            // now; a project-scoped ledger would additionally spare re-uploads
            // of an image that recurs across sessions.
            //
            // Absent for inert handles, which record nothing and so have
            // nowhere to keep this; those sessions simply keep inlining.
            if let Some(attachments) = &handles.attachments {
                client = client.with_handles(artist_session::HandleLedger::new(
                    attachments.dir().with_file_name("file-handles"),
                ));
            }
            let client = client.with_session_state(
                handles.chain.clone(),
                handles.capabilities.clone(),
                handles.statefulness,
            );
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

/// The provider-private lineage for a hop.
///
/// Hop 0 keeps the bare conversation id so existing sessions are unaffected;
/// later hops get their own namespace. The depth comes from the event log, so
/// this is stable across a resume and a rewind past a boundary reuses the
/// earlier hop's lineage.
/// Bind a tool bundle to the conversation that is using it.
///
/// Refuses the placeholder rather than passing it through. `SessionHandles`
/// defaults `conversation_id` to `"default"`, and a path that reaches a real
/// turn still holding it would put every such session back on one shared
/// actor — silently, which is the failure this whole change exists to remove.
/// Loud here beats a collision nobody can see.
fn tools_for_conversation(tools: &ToolBundle, conversation_id: &str) -> Result<ToolBundle> {
    if conversation_id.trim().is_empty() || conversation_id == "default" {
        anyhow::bail!(
            "a run needs a real conversation id to scope its anchor state; got \
             `{conversation_id}`. Sharing one actor across sessions makes them overwrite each \
             other's anchors."
        );
    }
    tools.for_actor(conversation_id)
}

fn provider_lineage(conversation_id: &str, depth: usize) -> String {
    if depth == 0 {
        conversation_id.to_owned()
    } else {
        format!("{conversation_id}:handoff:{depth}")
    }
}

/// How many times one turn may be regenerated after the model calls a tool
/// that does not exist.
const UNKNOWN_TOOL_RETRIES: u32 = 2;

/// The resolved profile context for a session-root run.
struct RootRun {
    profiles: profiles::Profiles,
    profile: profiles::Profile,
    thinking: profiles::Thinking,
    model: String,
}

#[allow(clippy::too_many_arguments)]
async fn stream_chat_with<C: CompletionClient>(
    client: C,
    provider: &SavedProvider,
    run: &RootRun,
    input: &ChatInput,
    tool_context: ToolContext<'_>,
    handles: SessionHandles,
    on_event: &mut impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome>
where
    C::CompletionModel: 'static,
{
    // Anchor state is keyed by actor, and the bundle is built before any
    // conversation exists — so every session would otherwise share one actor,
    // and with it one row. Concurrent sessions overwrite each other's anchors
    // on every edit, and a resumed one can load the other's table.
    //
    // The conversation is the right key, and the one the per-agent scoping was
    // built for: a resumed session reclaims its own anchors, two live ones
    // never touch. Re-actoring is a clone that shares the coordinator behind
    // it, which is the same thing delegates already do for subagents.
    let tools = &tools_for_conversation(tool_context.native, &handles.conversation_id)?;
    let mcp = tool_context.mcp;
    let model = run.model.as_str();
    let profile = &run.profile;
    let profiles = &run.profiles;

    let resources = resources::Resources::discover(tools.project_root());
    handles.rules.note_user_turn();

    let mut seed_history = handles
        .memory
        .load(&handles.conversation_id)
        .await
        .context("load conversation memory")?;
    let durable_history_len = seed_history.len();

    // Built once per turn and hoisted out of the retry loop below, because it
    // is the prompt-cache prefix: rebuilding it per attempt gave a TTSR retry
    // the chance to change it, and rebuilding it per turn gave that chance to
    // anyone editing an AGENTS.md mid-session.
    //
    // Every profile is composed on the shared prompt: the body says what is
    // different about this profile, not what is true of every agent.
    let (base, base_diagnostics) = prompt_config::base_prompt();
    let persona = if profile.instructions.trim().is_empty() {
        base
    } else {
        format!("{base}\n\n{}", profile.instructions)
    };
    let prompt_diagnostics = profiles
        .diagnostics()
        .iter()
        .chain(base_diagnostics.iter())
        .chain(resources.diagnostics().iter())
        .map(|d| format!("<diagnostic>{}</diagnostic>", d))
        .collect::<String>();
    // Frozen, not merely computed once: hoisting stops today's churn, but only
    // the freezer stops a future edit from reintroducing it. See `prefix`.
    // Identity is true of every agent in every mode, so it belongs in the
    // system prompt rather than a user turn — and it goes *last*, after every
    // byte that is shared between agents. Prompt caching is a prefix match, so
    // a name placed any earlier gives each agent its own prefix and none of
    // them ever share a cached prompt. That costs most in exactly the case we
    // care about: a fan-out of subagents spawned together off one base prompt.
    let identity = if let Some(recorded) = handles.recorded_identity.as_ref() {
        identity::recorded(&recorded.name, &recorded.actor)
    } else {
        identity::session(
            &handles.conversation_id,
            &handles.conversation_id,
            tools.project_root(),
            &profile.name,
        )
        .map_err(|error| anyhow::anyhow!("artist identity allocation failed: {error}"))?
    };
    let frozen_prompt = handles.prefix.freeze(
        &prefix::key(&handles.conversation_id, &profile.name),
        format!(
            "{}\n\n{}{}<available_profiles>{}</available_profiles>\nCurrent working directory: {}{}",
            persona,
            prompt_diagnostics,
            resources.prompt_section(&profile),
            profiles.catalog(),
            tools.project_root().display(),
            identity.prompt_block(),
        ),
    );

    let mut seed_prompt = user_message(input);
    // Recalled memory rides the user turn for the same reason skills do: it is
    // conditioned on what was just typed, so folding it into the preamble would
    // break the stable prompt-cache prefix. Bounded so a cold embedding model
    // can never stall a turn — anything slower than this still reaches the
    // model through the hook's injection channel on a later completion call.
    if let Some(durable) = &handles.durable_memory {
        let recalled = tokio::time::timeout(
            std::time::Duration::from_millis(750),
            durable.recall(&input.text),
        )
        .await
        .unwrap_or_default();
        let section = artist_memory::render(&recalled);
        if !section.is_empty()
            && let Message::User { content } = &mut seed_prompt
        {
            content.insert(0, UserContent::text(section));
        }
    }
    // Files can move while nobody is looking: the user edits in their own
    // editor between turns, another session in this worktree lands a change, a
    // branch gets switched. The per-tool check cannot see any of that, because
    // by definition no tool ran. Riding the user turn for the same reason
    // skills and memory do — it is conditioned on what just happened, so
    // folding it into the preamble would break the stable cache prefix.
    if let Some(report) = tools.edit.0.drift_watch().report().await
        && let Message::User { content } = &mut seed_prompt
    {
        content.insert(0, UserContent::text(report));
    }
    // The system prompt is frozen for the session, so an instruction file
    // edited mid-session cannot reach the model through the preamble. It rides
    // the user turn instead — the same trade the three injections above make,
    // and the reason freezing costs nothing: the content still arrives, just on
    // the channel where arriving is free.
    if let Some(note) = frozen_prompt.superseded_note()
        && let Message::User { content } = &mut seed_prompt
    {
        content.insert(0, UserContent::text(note));
    }
    // The shape of the project, once, on the turn that opens the session.
    //
    // Rides the user turn rather than the preamble for the reason everything
    // else here does — but with a second benefit specific to this: a message
    // already in the history cannot be rewritten, so "frozen for the session"
    // stops being a rule someone has to remember and becomes a property of
    // where it lives. Architectural drift is reported afterwards as a delta
    // rather than by reissuing this.
    //
    // `seed_history` is empty only on the first turn, which makes that the
    // trigger without any separate bookkeeping.
    //
    // Bounded like memory recall is: on a large repository this may have to
    // build the dependency graph from cold, and orientation is never worth
    // stalling the first thing the user asked for.
    if seed_history.is_empty() {
        let root = tools.project_root().to_path_buf();
        // The request is in hand here, so a project too large to show whole
        // shows the parts it is about rather than an arbitrary top slice.
        let focus = input.text.clone();
        let built = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio::task::spawn_blocking(move || {
                artist_tools::skeleton::build_for(
                    &root,
                    artist_tools::skeleton::DEFAULT_BUDGET,
                    &focus,
                )
                .map(|skeleton| skeleton.render())
            }),
        )
        .await;
        if let Ok(Ok(Some(shape))) = built
            && !shape.is_empty()
            && let Message::User { content } = &mut seed_prompt
        {
            content.insert(0, UserContent::text(shape));
        }
    }
    // Observes the model's own output: schedules recall on a stated decision,
    // injects what has landed on the next completion call, and captures a fact
    // when a commit succeeds. Inert when memory is disabled.
    let memory_writer = handles
        .durable_memory
        .as_ref()
        .map(|handle| handle.writer(handles.recorder.clone(), handles.conversation_id.clone()));
    let memory_hook = memory::MemoryHook::new(memory_writer.clone(), true);
    // Write trigger 1. A correction is the highest-signal moment for memory —
    // a durable preference stated out loud — so the user's own words are stored
    // verbatim rather than paraphrased through the model, which is both more
    // faithful and free.
    if let Some(writer) = memory_writer.clone() {
        memory::capture_correction(writer, &input.text);
    }
    // Delegate tools execute inside `stream.next()`. A separate channel lets
    // their events wake this outer driver while that future is still pending,
    // instead of buffering the entire child transcript until the tool returns.
    let (subagent_events_tx, mut subagent_events_rx) =
        tokio::sync::mpsc::unbounded_channel::<PromptEvent>();
    let visible_steering = handles.steering.clone();
    let result_attachments = handles.attachments.as_ref();
    let tool_meta = ToolMeta::default();
    let mcp_tools = mcp.tools().await;
    // Where a fired handoff lands. Shared with the tool for the whole turn so a
    // TTSR retry rebuilds the tool against the same slot.
    let pending_handoff = handoff::HandoffShared::default();

    // Per-run abort-retry budget: spans this turn's retries but is isolated
    // from concurrent delegate runs (each has its own counter).
    let retry_budget = handles.rules.retry_budget();
    let mut retries_used = 0u32;
    let mut unknown_tool_retries = 0u32;
    // Keep cache affinity within a conversation rather than pinning every main
    // agent and delegate in the project to the same provider route.
    let mut overload_retry = provider_retry::OverloadRetry::new(
        tools.project_root(),
        model,
        &format!("main:{}", handles.conversation_id),
    );
    'retry: loop {
        let run_id = format!("r-{}", uuid::Uuid::new_v4().simple());
        let run_recorder = handles.recorder.with_run(&run_id);
        let ttsr = TtsrShared::new(
            handles.rules.clone(),
            Arc::clone(&handles.rule_set),
            false,
            retries_used < retry_budget,
        );
        // Rebuilt per attempt from the *current* seed, so a `fork=true` delegate
        // spawned after a TTSR retry inherits the reminder-injected history
        // rather than the stale original turn.
        let fork_context = Arc::new({
            let mut context = seed_history.clone();
            context.push(seed_prompt.clone());
            context
        });
        // A display-callback failure ends the run like any other error, so it
        // must record a terminal event first. Propagating the error directly
        // would leave `run.started` with no `run.finished`, which replay reads
        // as a run still in flight.
        macro_rules! emit {
            ($event:expr) => {
                if let Err(error) = on_event($event) {
                    run_recorder.record(RunFinished::Error {
                        error: error.to_string(),
                    });
                    return Err(error);
                }
            };
        }

        // One client for the out-of-band cache calls; the provider client rig
        // builds does not expose its own.
        let gemini_http = reqwest::Client::new();
        // Gemini has no conversation chaining, but it can hold the preamble.
        // Referencing it by name takes the largest resent item out of the
        // request entirely — and with it, any way for the preamble to differ
        // between turns. Falls back to sending it inline whenever a cache
        // cannot be had.
        let cached_context = if matches!(provider.provider, llm_provider::ProviderKind::Gemini) {
            let api_key = match &provider.credentials {
                llm_provider::Credentials::ApiKey { api_key } => api_key.expose(),
                _ => "",
            };
            let ledger = handles.attachments.as_ref().map(|attachments| {
                artist_session::HandleLedger::new(attachments.dir().with_file_name("file-handles"))
            });
            gemini_cache::GeminiCache {
                http: &gemini_http,
                base_url: provider.base_url.as_str(),
                api_key,
                ledger: ledger.as_ref(),
                capabilities: &handles.capabilities,
            }
            .ensure(
                model,
                frozen_prompt.as_str(),
                handles.statefulness.gemini_cache,
            )
            .await
        } else {
            None
        };

        let mut builder = client.agent(model);
        // These fields belong to the ChatGPT subscription transport. Keep them
        // off OpenAI Responses and Chat Completions requests, whose accepted
        // parameter shapes differ.
        if let Some(params) = request_params(
            provider.provider,
            provider.api,
            overload_retry.cache_key(),
            provider.reasoning_effort.as_deref(),
            handles.effective_context_window,
            handles.fast_mode,
        ) {
            builder = builder.additional_params(params);
        }
        if let Some(name) = &cached_context {
            builder = builder.additional_params(gemini_cache::reference(name));
        }
        // One environment, one policy pass — see [`tool_set`]. The subagent path
        // builds its own `ToolEnv` and calls the same function, which is what
        // stops the two surfaces from drifting apart again.
        let env = tool_set::ToolEnv {
            bundle: tools.clone(),
            recorder: handles.recorder.clone(),
            resources: resources.clone(),
            todos: handles.todos.clone(),
            todo_owner: handles.conversation_id.clone(),
            attachments: handles.attachments.clone(),
            computer: handles.computer.clone(),
            memory: memory_writer.clone(),
            // The canvas server does not exist until the model asks for
            // something that needs it, so a session that never touches one
            // binds no port; absent entirely in one-shot and headless paths.
            canvas: tool_context.canvas.cloned(),
            handoff: Some(tool_set::HandoffEnv {
                pending: pending_handoff.clone(),
                profiles: profiles.clone(),
                current: profile.name.clone(),
            }),
            delegation: Some(tool_set::DelegationEnv {
                provider: provider.clone(),
                context: Arc::clone(&fork_context),
                handles: handles.clone(),
                events: subagent_events_tx.clone(),
                profiles: profiles.clone(),
                // The session root holds no seat on the delegation semaphore:
                // it is not itself a delegate, so it has none to yield.
            }),
            ask: handles.ask.clone(),
            inbox: Some(messaging::Inbox::new(identity.name.clone())),
            sessions: session_tools::SessionHub::standard(
                tools.project_root(),
                identity.name.clone(),
                None,
            ),
            pages: pagination::PageStore::memory(),
            dynamic: mcp_tools
                .iter()
                .cloned()
                .chain(
                    tool_context
                        .extensions
                        .map(artist_extensions::Manager::tools)
                        .unwrap_or_default(),
                )
                .collect(),
            disabled: tool_context.disabled.to_vec(),
        };
        let registered = tool_set::build(profile, &env);
        // Publish what the model actually got, so a canvas cannot reach a tool
        // the profile denied nor miss one it allowed.
        handles.tools.publish(registered.clone());
        let persistence = conversation::PersistenceStatus::default();
        let attempt_memory = conversation::AttemptMemory::new(
            Arc::clone(&handles.memory),
            handles.conversation_id.clone(),
            seed_history.clone(),
            durable_history_len,
            persistence.clone(),
        );
        let agent = builder
            // Gemini rejects a request carrying both a cached prefix and a
            // system instruction, and sending it anyway would forfeit the
            // saving the cache exists for.
            .preamble(if cached_context.is_some() {
                ""
            } else {
                frozen_prompt.as_str()
            })
            .memory(attempt_memory)
            .conversation(handles.conversation_id.clone())
            .dynamic_tools(
                registered
                    .into_iter()
                    .map(|tool| rig_agent::tool::DynamicTool::from(tool.portable()))
                    .collect(),
            )
            .add_hook(steering::SteeringHook {
                steering: handles.steering.clone(),
                inbox: Some(messaging::Inbox::new(identity.name.clone())),
            })
            .add_hook(CaptureHook::new(tool_meta.clone()))
            .add_hook(TtsrHook(Arc::clone(&ttsr)))
            .add_hook(memory_hook.clone())
            .default_max_turns(usize::MAX)
            .build();

        run_recorder.record(RunStarted {
            provider: format!("{:?}", provider.provider).to_lowercase(),
            model: model.to_owned(),
            reasoning_effort: run
                .thinking
                .level
                .map(|level| level.as_str().to_owned())
                .or_else(|| provider.reasoning_effort.clone()),
            agent: Some(identity.name.clone()),
            actor: Some(identity.actor.clone()),
            profile: Some(profile.name.clone()),
        });

        let mut stream = agent.stream_prompt(seed_prompt.clone()).await;
        let mut streamed_assistant_text = String::new();
        let mut streamed_turn = ttsr.turn();
        // Retrying after any visible model or tool event could duplicate output
        // or side effects. Provider overloads are retried only while pristine.
        let mut attempt_observed = false;
        loop {
            // A tool round trip can advance Rig to a new model turn before
            // that turn emits text. Never retain the preceding turn's text as
            // though it were a partial answer from the failed turn.
            let current_turn = ttsr.turn();
            if current_turn != streamed_turn {
                streamed_assistant_text.clear();
                streamed_turn = current_turn;
            }
            let item = tokio::select! {
                biased;
                event = subagent_events_rx.recv() => {
                    if let Some(event) = event {
                        attempt_observed = true;
                        on_event(event)?;
                    }
                    continue;
                }
                item = stream.next() => item,
                _ = handles.cancel.cancelled() => {
                    drop(stream);
                    let (committed, _) = ttsr.committed();
                    let mut cancelled_delta = if committed.is_empty() {
                        let mut fallback = seed_history.clone();
                        fallback.push(seed_prompt.clone());
                        fallback
                    } else {
                        committed
                    };
                    cancelled_delta =
                        conversation::delta_after(&cancelled_delta, durable_history_len);
                    conversation::retain_cancelled_turn(
                        handles.memory.as_ref(),
                        &handles.conversation_id,
                        cancelled_delta,
                        streamed_assistant_text,
                    )
                    .await
                    .context("retain cancelled turn in conversation memory")?;
                    run_recorder.record(RunFinished::Cancelled);
                    return Ok(RunOutcome::Cancelled);
                }
            };
            let Some(item) = item else {
                let error = anyhow!("Rig stream ended without a final response");
                run_recorder.record(RunFinished::Error {
                    error: error.to_string(),
                });
                let (committed, _) = ttsr.committed();
                let mut interrupted_delta = if committed.is_empty() {
                    let mut fallback = seed_history.clone();
                    fallback.push(seed_prompt.clone());
                    fallback
                } else {
                    committed
                };
                interrupted_delta =
                    conversation::delta_after(&interrupted_delta, durable_history_len);
                conversation::retain_provider_interrupted_turn(
                    handles.memory.as_ref(),
                    &handles.conversation_id,
                    interrupted_delta,
                    streamed_assistant_text,
                    &error.to_string(),
                )
                .await
                .context("retain prematurely-ended turn in conversation memory")?;
                return Err(error);
            };
            if item.is_ok() {
                attempt_observed = true;
            }
            match item {
                Ok(MultiTurnStreamItem::FinalResponse(_)) => {
                    // The turn is over: match any trailing text/reasoning that
                    // never reached the coalesce threshold. Short trailing
                    // content without a newline is otherwise never evaluated.
                    if ttsr.finalize_reasoning() || ttsr.finalize_text() {
                        let firing = ttsr.take_pending().expect("finalize stashed the firing");
                        drop(stream);
                        let (committed, _) = ttsr.committed();
                        seed_history = committed;
                        record_firing_events(&run_recorder, &ttsr, &firing);
                        emit!(PromptEvent::RuleFired {
                            rule: firing.rule.0.clone(),
                            matched: firing.matched.clone(),
                        });
                        run_recorder.record(RunFinished::Cancelled);
                        seed_prompt = reminder_message(&firing);
                        retries_used += 1;
                        continue 'retry;
                    }
                    if let Err(error) = persistence.result() {
                        run_recorder.record(RunFinished::Error {
                            error: error.clone(),
                        });
                        return Err(anyhow!(error)).context("persist conversation memory");
                    }
                    run_recorder.record(RunFinished::Completed);
                    return Ok(RunOutcome::Completed);
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(
                    text,
                ))) => {
                    let turn = ttsr.turn();
                    if turn != streamed_turn {
                        streamed_assistant_text.clear();
                        streamed_turn = turn;
                    }
                    streamed_assistant_text.push_str(&text.text);
                    emit!(PromptEvent::TextDelta(text.text));
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(
                    StreamedAssistantContent::ReasoningDelta {
                        // Handle both summary deltas (`id: None`) and raw
                        // reasoning deltas (`id: Some`); the latter used to fall
                        // through and be dropped, so reasoning-target rules
                        // silently failed to match whenever the backend streamed
                        // raw reasoning instead of a summary.
                        id: _,
                        reasoning,
                    },
                )) => {
                    // rig has no hook event for reasoning deltas, so
                    // reasoning rules match here on the driver side.
                    if ttsr.push_reasoning(&reasoning) {
                        let firing = ttsr
                            .take_pending()
                            .expect("push_reasoning stashed the firing");
                        drop(stream);
                        let (committed, _) = ttsr.committed();
                        seed_history = committed;
                        // `committed` includes the current seed prompt; the
                        // reminder becomes the new prompt.
                        record_firing_events(&run_recorder, &ttsr, &firing);
                        emit!(PromptEvent::RuleFired {
                            rule: firing.rule.0.clone(),
                            matched: firing.matched.clone(),
                        });
                        run_recorder.record(RunFinished::Cancelled);
                        seed_prompt = reminder_message(&firing);
                        retries_used += 1;
                        continue 'retry;
                    }
                    emit!(PromptEvent::ReasoningSummaryDelta(reasoning));
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(
                    StreamedAssistantContent::ToolCall {
                        tool_call,
                        internal_call_id,
                    },
                )) => on_event(PromptEvent::ToolCall {
                    id: internal_call_id,
                    name: tool_call.function.name,
                    arguments: tool_call.function.arguments,
                })?,
                Ok(MultiTurnStreamItem::ToolExecutionCommitted {
                    tool_call,
                    internal_call_id,
                }) => {
                    handles.lifecycle.emit(LifecycleEvent::ToolStarted(format!(
                        "main:{internal_call_id}"
                    )));
                    emit!(PromptEvent::ToolExecutionStart {
                        id: internal_call_id,
                        name: tool_call.function.name,
                    });
                }
                Ok(MultiTurnStreamItem::CompletionCall(call)) => {
                    // Recorded per call, not per run: fallback can move a run
                    // onto a different candidate mid-flight, and caches are
                    // model-scoped, so the cache answer is a property of the
                    // attempt rather than of the run.
                    run_recorder.record(artist_session::RunUsage {
                        provider: format!("{:?}", provider.provider).to_lowercase(),
                        model: model.to_owned(),
                        input_tokens: call.usage.input_tokens,
                        output_tokens: call.usage.output_tokens,
                        total_tokens: call.usage.total_tokens,
                        cached_input_tokens: call.usage.cached_input_tokens,
                    });
                    emit!(PromptEvent::CompletionUsage {
                        total_tokens: call.usage.total_tokens,
                        cached_input_tokens: call.usage.cached_input_tokens,
                    });
                }
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
                                images.extend(store_result_image(&image, result_attachments));
                                None
                            }
                            ToolResultContent::Json { value } => Some(value.to_string()),
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let content = visible_steering
                        .take_original_result(&internal_call_id)
                        .unwrap_or(content);
                    let meta = tool_meta.take(&internal_call_id);
                    handles.lifecycle.emit(LifecycleEvent::ToolFinished(format!(
                        "main:{internal_call_id}"
                    )));
                    on_event(PromptEvent::ToolResult {
                        id: internal_call_id,
                        content,
                        outcome: meta.as_ref().map(|(outcome, _)| outcome.clone()),
                        duration_ms: meta.map(|(_, duration)| duration),
                        images,
                    })?;
                    // A handoff is terminal: end the run rather than let the
                    // model keep working in a context about to be discarded.
                    if let Some(handoff) = pending_handoff.take() {
                        drop(stream);
                        run_recorder.record(RunFinished::Completed);
                        return Ok(RunOutcome::HandedOff {
                            from: profile.name.clone(),
                            handoff,
                        });
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    // A TTSR abort surfaces as PromptCancelled with the
                    // committed history (rig excludes the partial turn).
                    if let Some(firing) = ttsr.take_pending()
                        && let rig_agent::agent::StreamingError::Prompt(boxed) = &error
                        && let PromptError::PromptCancelled { chat_history, .. } = boxed.as_ref()
                    {
                        seed_history = chat_history.clone();
                        record_firing_events(&run_recorder, &ttsr, &firing);
                        emit!(PromptEvent::RuleFired {
                            rule: firing.rule.0.clone(),
                            matched: firing.matched.clone(),
                        });
                        run_recorder.record(RunFinished::Cancelled);
                        seed_prompt = reminder_message(&firing);
                        retries_used += 1;
                        continue 'retry;
                    }
                    // The model named a tool that does not exist: a
                    // hallucination, or the `multi_tool_use.parallel` wrapper
                    // OpenAI injects beside our tools. Rig treats it as a
                    // protocol violation and ends the turn, which throws away a
                    // working session over one bad call. Naming the real tools
                    // and regenerating recovers it.
                    //
                    // Budgeted separately from stream-rule retries, and low: a
                    // model that keeps inventing the same name would otherwise
                    // loop, and after two corrections it is not going to learn.
                    if let rig_agent::agent::StreamingError::Prompt(boxed) = &error
                        && let PromptError::UnknownToolCall {
                            tool_name,
                            allowed_tools,
                            chat_history,
                            ..
                        } = boxed.as_ref()
                        && unknown_tool_retries < UNKNOWN_TOOL_RETRIES
                    {
                        seed_history = *chat_history.clone();
                        run_recorder.record(RunFinished::Cancelled);
                        seed_prompt = rig_core::completion::Message::user(format!(
                            "<system-reminder>\nThere is no tool named `{tool_name}`. \
                             The tools available this turn are: {}. Call one of those, \
                             or answer without a tool.\n</system-reminder>",
                            allowed_tools.join(", ")
                        ));
                        unknown_tool_retries += 1;
                        continue 'retry;
                    }
                    let error_text = error.to_string();
                    run_recorder.record(RunFinished::Error {
                        error: error_text.clone(),
                    });
                    if !attempt_observed
                        && provider_retry::is_overload(provider.provider, &error)
                        && let Some(delay) = overload_retry.schedule()
                    {
                        drop(stream);
                        tokio::select! {
                            _ = tokio::time::sleep(delay) => continue 'retry,
                            _ = handles.cancel.cancelled() => {
                                let mut cancelled_delta = seed_history.clone();
                                cancelled_delta.push(seed_prompt.clone());
                                cancelled_delta =
                                    conversation::delta_after(&cancelled_delta, durable_history_len);
                                conversation::retain_cancelled_turn(
                                    handles.memory.as_ref(),
                                    &handles.conversation_id,
                                    cancelled_delta,
                                    String::new(),
                                )
                                .await
                                .context("retain cancelled turn during provider retry")?;
                                return Ok(RunOutcome::Cancelled);
                            }
                        }
                    }

                    // Preserve the prompt and any partial answer just like a
                    // user cancellation, but tell the next turn why it ended.
                    let (committed, _) = ttsr.committed();
                    let mut interrupted_delta = if committed.is_empty() {
                        let mut fallback = seed_history.clone();
                        fallback.push(seed_prompt.clone());
                        fallback
                    } else {
                        committed
                    };
                    interrupted_delta =
                        conversation::delta_after(&interrupted_delta, durable_history_len);
                    conversation::retain_provider_interrupted_turn(
                        handles.memory.as_ref(),
                        &handles.conversation_id,
                        interrupted_delta,
                        streamed_assistant_text,
                        &error_text,
                    )
                    .await
                    .context("retain provider-interrupted turn in conversation memory")?;
                    if crate::fallback::classify(&error) == crate::fallback::Failure::Unavailable {
                        return Err(anyhow!(fallback::Unavailable(error.to_string())));
                    }
                    return Err(error).context("stream Artist agent");
                }
            }
        }
    }
}

/// Provider parameters shared by every request attempt in a turn.
pub(crate) fn request_params(
    provider: llm_provider::ProviderKind,
    api: Option<llm_provider::OpenAiApi>,
    cache_key: &str,
    reasoning_effort: Option<&str>,
    effective_context_window: Option<u64>,
    fast_mode: bool,
) -> Option<serde_json::Value> {
    if provider == llm_provider::ProviderKind::Openai
        && api.unwrap_or_default() == llm_provider::OpenAiApi::ChatCompletions
    {
        return fast_mode.then(|| json!({ "service_tier": "priority" }));
    }
    if provider != llm_provider::ProviderKind::Chatgpt
        && !(provider == llm_provider::ProviderKind::Openai
            && api.unwrap_or_default() == llm_provider::OpenAiApi::Responses)
    {
        return None;
    }
    // Leave output headroom within the catalog's already-normalized effective
    // window. Only unknown metadata uses the conservative fixed fallback.
    let compact_threshold = effective_context_window
        .map(|window| window.saturating_mul(9) / 10)
        .unwrap_or(100_000);
    let mut params = json!({
        "store": false,
        "include": ["reasoning.encrypted_content"],
        "prompt_cache_key": cache_key,
        "context_management": [{"type": "compaction", "compact_threshold": compact_threshold}]
    });
    // Request a provider-generated trace for the live UI even when the model's
    // default effort is in use. Rig's memory policy is independent: streaming
    // this summary does not make the CLI responsible for model context.
    params["reasoning"] = match reasoning_effort.filter(|effort| {
        matches!(
            *effort,
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
        )
    }) {
        Some(effort) => json!({ "effort": effort, "summary": "auto", "context": "all_turns" }),
        None => json!({ "summary": "auto", "context": "all_turns" }),
    };
    if fast_mode
        && matches!(
            provider,
            llm_provider::ProviderKind::Openai | llm_provider::ProviderKind::Chatgpt
        )
    {
        params["service_tier"] = json!("priority");
    }
    Some(params)
}

/// Log rule bookkeeping; Rig conversation memory persists the reminder prompt
/// when the retried run succeeds.
pub(crate) fn record_firing_events(recorder: &Recorder, ttsr: &TtsrShared, firing: &Firing) {
    recorder.record(RuleFired {
        rule: firing.rule.0.clone(),
        target: firing.target.as_str().to_owned(),
        matched: firing.matched.clone(),
        turn: ttsr.turn(),
        per_turn: firing.fire == artist_rules::types::FirePolicy::PerTurn,
    });
    recorder.record(RuleInjection {
        rule: firing.rule.0.clone(),
        reminder: firing.reminder.clone(),
        session_persistent: firing.persistence == artist_rules::types::Persistence::Session,
    });
}

pub(crate) fn user_message(input: &ChatInput) -> Message {
    let mut content = vec![UserContent::text(input.text.clone())];
    content.extend(input.images.iter().map(|attachment| {
        UserContent::Image(Image {
            data: DocumentSourceKind::Base64(
                base64::engine::general_purpose::STANDARD.encode(&attachment.data),
            ),
            media_type: Some(attachment.media_type.clone()),
            detail: None,
            additional_params: None,
        })
    }));
    Message::User {
        content: OneOrMany::many(content).expect("chat input always contains text"),
    }
}

pub fn available_skills(project: &std::path::Path) -> Vec<AvailableSkill> {
    resources::Resources::discover(project).available_skills()
}

/// Executes a prompt without prior context.
pub async fn stream_prompt(
    provider: &SavedProvider,
    input: &str,
    tools: &ToolBundle,
    mcp: &mcp::McpManager,
    handles: SessionHandles,
    on_event: impl FnMut(PromptEvent) -> Result<()>,
) -> Result<RunOutcome> {
    let input = ChatInput::from(input.to_owned());
    stream_chat(
        provider,
        &input,
        ToolContext {
            native: tools,
            mcp,
            extensions: None,
            disabled: &[],
            canvas: None,
        },
        handles,
        on_event,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::request_params;
    use llm_provider::ProviderKind;

    /// Anchor state is keyed by actor, so a session reaching a real turn on the
    /// placeholder id would share one row with every other such session — each
    /// overwriting the other's anchors, and a resumed one loading a stranger's
    /// table. That has to fail loudly: the whole hazard is that it is invisible.
    #[test]
    fn a_run_refuses_the_placeholder_conversation_id() {
        let bundle = artist_tools::ToolBundle::new(
            artist_tools::Workspace::open(
                tempfile::tempdir().expect("project").path(),
                tempfile::tempdir().expect("state").path(),
                "artist-unbound",
            )
            .expect("workspace"),
        );

        for placeholder in ["default", "", "   "] {
            let refused = super::tools_for_conversation(&bundle, placeholder);
            assert!(
                refused.is_err(),
                "`{placeholder}` was accepted as a conversation id"
            );
        }

        let scoped = super::tools_for_conversation(&bundle, "session-01H8XYZ");
        assert!(scoped.is_ok(), "a real conversation id must be accepted");
    }

    #[test]
    fn reasoning_requests_a_live_summary_trace() {
        let params = request_params(
            ProviderKind::Chatgpt,
            None,
            "cache",
            Some("high"),
            None,
            false,
        )
        .unwrap();
        assert_eq!(params["reasoning"]["effort"], "high");
        assert_eq!(params["reasoning"]["summary"], "auto");

        let default_effort =
            request_params(ProviderKind::Chatgpt, None, "cache", None, None, false).unwrap();
        assert_eq!(default_effort["reasoning"]["summary"], "auto");
        assert!(default_effort["reasoning"].get("effort").is_none());
    }

    #[test]
    fn responses_policy_is_sent_only_to_responses_transports() {
        assert_eq!(
            request_params(
                ProviderKind::Openai,
                Some(llm_provider::OpenAiApi::Responses),
                "cache",
                Some("high"),
                None,
                false,
            )
            .unwrap()["prompt_cache_key"],
            "cache"
        );
        assert!(
            request_params(
                ProviderKind::Openai,
                Some(llm_provider::OpenAiApi::ChatCompletions),
                "cache",
                None,
                None,
                false,
            )
            .is_none()
        );
    }

    #[test]
    fn compaction_threshold_uses_effective_model_window() {
        let params = request_params(
            ProviderKind::Openai,
            Some(llm_provider::OpenAiApi::Responses),
            "cache",
            None,
            Some(200_000),
            false,
        )
        .unwrap();
        assert_eq!(
            params["context_management"][0]["compact_threshold"],
            180_000
        );
        let unknown = request_params(
            ProviderKind::Openai,
            Some(llm_provider::OpenAiApi::Responses),
            "cache",
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            unknown["context_management"][0]["compact_threshold"],
            100_000
        );
    }
    #[test]
    fn fast_mode_requests_openai_priority_tier() {
        let params = request_params(
            ProviderKind::Openai,
            Some(llm_provider::OpenAiApi::Responses),
            "cache",
            None,
            None,
            true,
        )
        .unwrap();
        assert_eq!(params["service_tier"], "priority");

        let normal = request_params(
            ProviderKind::Openai,
            Some(llm_provider::OpenAiApi::Responses),
            "cache",
            None,
            None,
            false,
        )
        .unwrap();
        assert!(normal.get("service_tier").is_none());

        let subscription =
            request_params(ProviderKind::Chatgpt, None, "cache", None, None, true).unwrap();
        assert_eq!(subscription["service_tier"], "priority");
    }
}
