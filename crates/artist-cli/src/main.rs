mod activity_indicator;
mod args;
mod ask_ui;
mod canvas_host;
mod canvas_window;
mod chat_ui;
mod clipboard;
mod command_ui;
mod compaction;
mod custom_commands;
mod extension_control;
mod input_atoms;
mod input_border;
mod input_images;
mod interaction;
mod login;
mod message_box;
mod models;
mod prompt;
mod provider_commands;
mod response_output;
mod settings;
mod slash_commands;
mod startup_splash;
mod status_bar;
mod store;
mod subagent_ui;
mod test_provider;
mod text_wrap;
mod theme;
mod tool_ui;

use anyhow::{Context, Result, bail};
use args::{Cli, Command, ProfilesCommand, RulesCommand, SessionsCommand};
use artist_session::{ActiveSession, SessionStore};
use artist_tools::{ToolBundle, Workspace};
use clap::Parser;
use llm_provider::ChatGptOAuth;
use rig_core::memory::ConversationMemory;
use serde::Deserialize;
use std::{
    collections::HashSet,
    io::{BufRead, IsTerminal},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FrontendControl {
    Steer { message: String },
    Answer { answer: artist_session::Answer },
    Stop,
}

#[derive(Debug)]
pub enum EmbeddedEvent {
    Prompt(artist_agent::PromptEvent),
    Question(artist_session::Question),
    Session(String),
}

/// Reusable non-terminal runtime owned by a session host. Provider state, MCP
/// connections, extensions, and their caches live for the host lifetime rather
/// than being reconstructed by a child process for every prompt.
pub struct EmbeddedRuntime {
    store: ProviderStore,
    provider_path: std::path::PathBuf,
    mcp: artist_agent::mcp::McpManager,
    extensions: Arc<artist_extensions::Manager>,
    extension_control: extension_control::ExtensionControl,
}

impl EmbeddedRuntime {
    pub async fn open(project: &std::path::Path) -> Result<Self> {
        std::env::set_current_dir(project)
            .with_context(|| format!("enter project {}", project.display()))?;
        let provider_path = config_path()?;
        let store = ProviderStore::load(&provider_path)?;
        let config_root = provider_path
            .parent()
            .context("providers path has no parent")?;
        let mcp = artist_agent::mcp::McpManager::load(config_root).await?;
        let extension_control = extension_control::ExtensionControl::default();
        let extensions = extension_manager(config_root, &store, extension_control.clone()).await?;
        Ok(Self {
            store,
            provider_path,
            mcp,
            extensions,
            extension_control,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn turn(
        &mut self,
        input: &str,
        session: &str,
        lineage: &str,
        provider: Option<&str>,
        model: Option<&str>,
        profile: Option<&str>,
        events: tokio::sync::mpsc::UnboundedSender<EmbeddedEvent>,
        controls: &mut tokio::sync::mpsc::UnboundedReceiver<FrontendControl>,
    ) -> Result<()> {
        execute_prompt(
            &mut self.store,
            &self.provider_path,
            input,
            Some(session),
            provider,
            model,
            profile,
            Some(lineage),
            &self.mcp,
            &self.extensions,
            &self.extension_control,
            Some(&events),
            Some(controls),
        )
        .await
    }
}
use store::{ProviderStore, config_path};

#[tokio::main]
pub async fn main() {
    if let Err(error) = run().await {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    // Dispatched before clap, and before anything touches the terminal: this
    // process exists only to own a webview's event loop, and it is spawned by
    // artist itself rather than typed by a user.
    let raw: Vec<String> = std::env::args().collect();
    if raw
        .get(1)
        .is_some_and(|arg| arg == artist_canvas::window::WINDOW_SUBCOMMAND)
    {
        // Out of band, because the URL carries the session key and arguments
        // are world-readable.
        let url = std::env::var(artist_canvas::window::URL_VAR)
            .context("canvas window needs ARTIST_CANVAS_URL")?;
        let title = raw.get(2).map(String::as_str).unwrap_or("Canvas");
        return canvas_window::run(&url, title);
    }

    let mut cli = Cli::parse();
    enter_positional_project(&mut cli)?;
    let path = config_path()?;
    let mut store = ProviderStore::load(&path)?;
    let config_root = path.parent().context("providers path has no parent")?;

    if let Some(prompt) = cli.print_prompt {
        if cli.command.is_some() {
            bail!("-p cannot be combined with a subcommand");
        }
        if let Some(extra) = cli.prompt {
            bail!("with -p, the positional argument must be a project directory: {extra}");
        }
        let mcp = artist_agent::mcp::McpManager::load(config_root).await?;
        let extension_control = extension_control::ExtensionControl::default();
        let extensions = extension_manager(config_root, &store, extension_control.clone()).await?;
        let result = execute_prompt(
            &mut store,
            &path,
            &prompt,
            cli.resume.as_deref(),
            cli.provider.as_deref(),
            cli.model.as_deref(),
            cli.profile.as_deref(),
            cli.lineage.as_deref(),
            &mcp,
            &extensions,
            &extension_control,
            None,
            None,
        )
        .await;
        if std::env::var("ARTIST_EVENT_STREAM").as_deref() == Ok("jsonl") {
            eprintln!("ARTIST_EVENT_STREAM_DONE=1");
        }
        return result;
    }
    match cli.command {
        Some(Command::Model) if cli.prompt.is_none() && cli.resume.is_none() => {
            let selected = default_index(&store)?;
            if refresh_if_needed(&mut store.providers[selected]).await? {
                store.save(&path)?;
            }
            models::select(&mut store.providers[selected]).await?;
            store.save(&path)?;
        }
        Some(Command::Rules(args)) if cli.prompt.is_none() && cli.resume.is_none() => {
            match args.action {
                RulesCommand::New { name } => scaffold_rule(&name)?,
            }
        }
        Some(Command::Sessions(args)) if cli.prompt.is_none() && cli.resume.is_none() => {
            let sessions = SessionStore::new(config_root);
            match args.action {
                SessionsCommand::List => sessions_list(&sessions)?,
                SessionsCommand::Render { id } => sessions_render(&sessions, &id)?,
                SessionsCommand::Gc {
                    keep,
                    older_than_days,
                    dry_run,
                } => sessions_gc(&sessions, config_root, keep, older_than_days, dry_run).await?,
            }
        }
        Some(Command::Profiles(args)) if cli.prompt.is_none() && cli.resume.is_none() => {
            let project = std::env::current_dir().context("find current project directory")?;
            match args.action {
                ProfilesCommand::List => profiles_list(&project),
                ProfilesCommand::Show { name } => profiles_show(&name)?,
            }
        }
        Some(Command::Memory(args)) if cli.prompt.is_none() && cli.resume.is_none() => {
            memory_command(config_root, args.action).await?;
        }
        Some(Command::Computer(args)) if cli.prompt.is_none() && cli.resume.is_none() => {
            let project = std::env::current_dir().context("find current project directory")?;
            let sessions = SessionStore::new(config_root);
            match args.action {
                args::ComputerCommand::Doctor { fix } => {
                    let report = artist_computer::doctor::run();
                    print!("{}", artist_computer::doctor::render(&report));
                    if fix {
                        println!();
                        print!("{}", artist_computer::doctor::repair(&report));
                        // Deliberately not re-running the checks and reporting
                        // success here: a repair that claims to have worked
                        // should be confirmed by a fresh run, not by the code
                        // that just performed it.
                    }
                    // A blocker is worth a non-zero exit so a setup script can
                    // branch on it without parsing the text.
                    if !report.can_start_a_stage() {
                        std::process::exit(1);
                    }
                }
                args::ComputerCommand::Log { id } => {
                    computer_log(&sessions, &project, id.as_deref())?
                }
                args::ComputerCommand::Serve { socket } => computer_serve(socket).await?,
                args::ComputerCommand::Call { json, socket } => computer_call(&json, socket)?,
                args::ComputerCommand::Export { id, out } => {
                    computer_export(&sessions, &project, id.as_deref(), out.as_deref())?
                }
                args::ComputerCommand::Replay { file, launch, heal } => {
                    computer_replay(&file, launch.as_deref(), heal).await?
                }
                args::ComputerCommand::Distill { id, include_failed } => {
                    computer_distill(&sessions, &project, id.as_deref(), include_failed)?
                }
                args::ComputerCommand::Frame { digest, out } => {
                    computer_frame(&sessions, &project, &digest, out)?
                }
            }
        }
        Some(_) => bail!("prompts and --resume cannot be combined with a subcommand"),
        None => {
            let selected = (!store.providers.is_empty())
                .then(|| default_index(&store))
                .transpose()?;
            let project = std::env::current_dir().context("find current project directory")?;
            // Layered settings (global ~/.config/artist + project .artist) resolve the
            // model/reasoning overrides and the effective tool denylist.
            let effective = settings::load_effective(
                config_root,
                &project,
                &settings::Overrides::default(),
                &store.disabled_tools,
            )?;
            // Catch a missing effective model before the TUI takes over. Legacy
            // settings scalars supply ChatGPT only; other providers must have a
            // provider-local selection.
            if let Some(selected) = selected
                && effective
                    .apply_to(store.providers[selected].clone())
                    .model
                    .is_none()
            {
                // Legacy/incomplete provider records should recover through the
                // interactive model setup instead of referring to the removed
                // standalone model command.
                models::select(&mut store.providers[selected]).await?;
                store.save(&path)?;
            }
            // Resolve an interactive resume before entering inline TUI mode so the
            // selector cannot be painted underneath the splash and input viewport.
            // Draw the startup UI before loading models, extensions, indexes, or
            // servers, then initialize integrations concurrently below.
            let sessions = SessionStore::new(config_root);
            let resumed = load_resumed(&sessions, &project, cli.resume.as_deref())?;
            let show_splash = resumed.is_none() && cli.prompt.is_none();
            let extension_control = extension_control::ExtensionControl::default();
            let mut refreshed_provider = selected.map(|index| store.providers[index].clone());
            let (mcp, extensions, refreshed) = tokio::join!(
                artist_agent::mcp::McpManager::load(config_root),
                extension_manager(config_root, &store, extension_control.clone()),
                async {
                    match refreshed_provider.as_mut() {
                        Some(provider) => refresh_if_needed(provider).await,
                        None => Ok(false),
                    }
                }
            );
            let mcp = mcp?;
            let extensions = extensions?;
            let terminal = chat_ui::start_terminal(
                show_splash,
                cli.prompt.is_some(),
                &extensions.extension_ids(),
                selected.is_none(),
            )?;
            if refreshed? {
                let selected = selected.expect("a refreshed provider is selected");
                store.providers[selected] = refreshed_provider.expect("refreshed provider exists");
                store.save(&path)?;
            }
            let tools = tool_bundle(config_root, &project)?;
            let rules_engine = artist_rules::RulesEngine::discover(&project);
            let rules_handle = artist_rules::state::RulesHandle::default();
            // Canvases share one loopback server for the session. A failed bind
            // is not fatal — the session works fine without them, and the tool
            // is simply not offered rather than failing when the model calls it.
            let canvas_control = canvas_host::CanvasControl::default();
            let ask = artist_session::ask::AskRegistry::new();
            // One registry for the session: the agent loop republishes into it
            // each attempt, and the canvas bridge dispatches through it.
            let tool_registry = artist_agent::ToolRegistryHandle::new();
            canvas_control.attach(
                extension_control.clone(),
                ask.clone(),
                tool_registry.clone(),
            );
            // Not started here. Most sessions never open a canvas, and binding
            // a port plus standing up an RPC surface for those is a cost and an
            // exposure with nothing on the other side of it.
            let canvas = artist_canvas::server::Lazy::new(
                project.clone(),
                std::sync::Arc::new(canvas_control.clone()),
            );
            chat_ui::run(
                terminal,
                &mut store,
                selected,
                &path,
                chat_ui::ChatResources {
                    sessions: &sessions,
                    project: &project,
                    tools: &tools,
                    mcp: &mcp,
                    extensions: &extensions,
                    extension_control: &extension_control,
                    rules_engine: &rules_engine,
                    rules_handle: &rules_handle,
                    settings: &effective,
                    canvas: Some(&canvas),
                    canvas_control: &canvas_control,
                    tool_registry: &tool_registry,
                },
                resumed,
                cli.prompt,
                show_splash,
            )
            .await?;
        }
    }
    Ok(())
}

fn enter_positional_project(cli: &mut Cli) -> Result<()> {
    let Some(candidate) = cli.prompt.as_deref() else {
        return Ok(());
    };
    let path = std::path::Path::new(candidate);
    if path.is_dir() {
        std::env::set_current_dir(path)
            .with_context(|| format!("enter project directory {}", path.display()))?;
        cli.prompt = None;
    }
    Ok(())
}

fn load_resumed(
    sessions: &SessionStore,
    project: &std::path::Path,
    resume: Option<&str>,
) -> Result<Option<(ActiveSession, Vec<artist_session::Envelope>)>> {
    let Some(requested) = resume else {
        return Ok(None);
    };
    let mut available = sessions.list_project(project)?;
    available.sort_by_key(|session| std::cmp::Reverse(session.created_at_ms));
    // An unknown id shouldn't abort the launch — a typo or stale id falls back
    // to the interactive picker instead of killing the process.
    let requested_missing =
        !requested.is_empty() && !available.iter().any(|session| session.id == requested);
    if requested_missing {
        eprintln!(
            "session '{requested}' was not found in this project — pick one to resume instead"
        );
    }
    let id = if requested.is_empty() || requested_missing {
        if available.is_empty() {
            bail!("no sessions found for {}", project.display());
        }
        let items = available
            .iter()
            .map(|session| {
                format!(
                    "{}  {}{}",
                    session.id,
                    session
                        .label
                        .as_deref()
                        .and_then(|label| label.lines().next())
                        .unwrap_or("Untitled"),
                    session
                        .parent
                        .as_deref()
                        .map(|parent| format!("  (fork of {parent})"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>();
        available[prompt::select_paged("Session to resume", &items, 0, 10)?]
            .id
            .clone()
    } else {
        requested.to_owned()
    };
    Ok(Some(sessions.open(&id)?))
}

async fn execute_prompt(
    store: &mut ProviderStore,
    path: &std::path::Path,
    input: &str,
    resume: Option<&str>,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    profile_override: Option<&str>,
    lineage_override: Option<&str>,
    mcp: &artist_agent::mcp::McpManager,
    extensions: &Arc<artist_extensions::Manager>,
    extension_control: &extension_control::ExtensionControl,
    embedded_events: Option<&tokio::sync::mpsc::UnboundedSender<EmbeddedEvent>>,
    mut embedded_controls: Option<&mut tokio::sync::mpsc::UnboundedReceiver<FrontendControl>>,
) -> Result<()> {
    let selected = match provider_override {
        Some(reference) => store
            .providers
            .iter()
            .position(|provider| {
                provider.id.as_str() == reference || provider.name.eq_ignore_ascii_case(reference)
            })
            .with_context(|| format!("configured provider not found: {reference}"))?,
        None => default_index(store)?,
    };
    if refresh_if_needed(&mut store.providers[selected]).await? {
        store.save(path)?;
    }
    let config_root = path.parent().context("providers path has no parent")?;
    let sessions = SessionStore::new(config_root);
    let project = std::env::current_dir().context("find current project directory")?;
    let effective = settings::load_effective(
        config_root,
        &project,
        &settings::Overrides::default(),
        &store.disabled_tools,
    )?;
    // Session-scoped provider carrying the settings model/reasoning override
    // (a throwaway clone, never persisted).
    let mut session_provider = effective.apply_to(store.providers[selected].clone());
    if let Some(model) = model_override {
        if model.trim().is_empty() {
            bail!("model override cannot be empty");
        }
        session_provider.model = Some(model.to_owned());
    }
    let tools = tool_bundle(config_root, &project)?;
    let (active, resumed_events) = match load_resumed(&sessions, &project, resume)? {
        Some(resumed) => resumed,
        None => (sessions.create(&project, Some(input))?, Vec::new()),
    };
    let target_lineage = lineage_override.unwrap_or(artist_session::MAIN_LINEAGE);
    anyhow::ensure!(
        target_lineage == artist_session::MAIN_LINEAGE
            || target_lineage.starts_with("main/delegate-"),
        "invalid agent lineage {target_lineage:?}"
    );
    anyhow::ensure!(
        target_lineage == artist_session::MAIN_LINEAGE
            || resumed_events
                .iter()
                .any(|event| event.lineage == target_lineage),
        "agent lineage {target_lineage:?} does not exist in session {}",
        active.session.id
    );
    if std::env::var("ARTIST_EVENT_STREAM").as_deref() == Ok("jsonl") {
        eprintln!("ARTIST_EVENT_STREAM_READY=1");
    }
    if std::env::var_os("ARTIST_EMIT_SESSION_ID").is_some() {
        eprintln!("ARTIST_SESSION_ID={}", active.session.id);
    }
    if let Some(events) = embedded_events {
        let _ = events.send(EmbeddedEvent::Session(active.session.id.clone()));
    }
    if target_lineage == artist_session::MAIN_LINEAGE {
        compact_noninteractive_if_needed(&active, &session_provider, effective.compaction, input)
            .await?;
    }
    let rules_engine = artist_rules::RulesEngine::discover(&project);
    // Restore prior rule state (once-per-session fires, persistent injections)
    // when resuming — the TUI path does this too; `-p` must match or a resumed
    // session re-fires once-only rules.
    let rules = artist_rules::state::RulesHandle::default();
    rules.restore_from_log(&resumed_events);
    let steering = artist_agent::SteeringHandle::default();
    let cancel = tokio_util::sync::CancellationToken::new();
    let frontend_control = std::env::var("ARTIST_CONTROL_STREAM").as_deref() == Ok("jsonl")
        || embedded_controls.is_some();
    let ask = frontend_control
        .then(|| artist_session::AskRegistry::with_recorder(active.recorder.clone()));
    let (control_send, mut control_receive) = tokio::sync::mpsc::unbounded_channel();
    if frontend_control {
        std::thread::spawn(move || {
            for line in std::io::stdin()
                .lock()
                .lines()
                .map_while(std::result::Result::ok)
            {
                if let Ok(command) = serde_json::from_str::<FrontendControl>(&line) {
                    if control_send.send(command).is_err() {
                        break;
                    }
                }
            }
        });
    }
    let effective_context_window =
        models::catalog(&session_provider)
            .await
            .ok()
            .and_then(|catalog| {
                catalog
                    .iter()
                    .find(|model| Some(&model.slug) == session_provider.model.as_ref())
                    .and_then(|model| model.effective_context_window())
            });
    let durable_memory = open_memory(config_root, &effective.memory).await;
    let target_recorder = target_lineage
        .strip_prefix("main/")
        .map(|suffix| {
            suffix
                .split('/')
                .fold(active.recorder.clone(), |recorder, part| {
                    recorder.child_lineage(part)
                })
        })
        .unwrap_or_else(|| active.recorder.clone());
    let conversation_id = if target_lineage == artist_session::MAIN_LINEAGE {
        active.session.id.clone()
    } else {
        format!("{}:{target_lineage}", active.session.id)
    };
    let recorded_identity = resumed_events.iter().rev().find_map(|envelope| {
        if envelope.lineage != target_lineage {
            return None;
        }
        match envelope.event() {
            artist_session::SessionEvent::RunStarted(run) => Some(artist_agent::RecordedIdentity {
                name: run.agent?,
                actor: run.actor?,
            }),
            _ => None,
        }
    });
    let target_memory = if target_lineage == artist_session::MAIN_LINEAGE {
        active.memory.clone()
    } else {
        artist_session::SessionMemory::for_lineage(
            conversation_id.clone(),
            target_lineage,
            active.session.dir(),
            target_recorder.clone(),
            active.attachments.clone(),
        )
    };
    let handles = artist_agent::SessionHandles {
        steering: steering.clone(),
        rules,
        rule_set: rules_engine.snapshot(),
        recorder: target_recorder,
        memory: Arc::new(target_memory),
        durable_memory,
        conversation_id,
        recorded_identity,
        provider_context: active.provider_context.clone(),
        effective_context_window,
        fast_mode: false,
        // A structured frontend control stream makes the one-shot invocation
        // interactive without borrowing the terminal. Plain `artist -p`
        // remains non-interactive and therefore does not advertise `ask`.
        ask: ask.clone(),
        cancel: cancel.clone(),
        attachments: Some(active.attachments.clone()),
        providers: llm_provider::ProviderSet::new(store.providers.clone()),
        todos: {
            // Harness-owned, so a resumed session picks the list back up
            // exactly where it was rather than re-deriving it from prose.
            let todos = artist_agent::todo::TodoStore::default();
            todos.restore(&resumed_events);
            todos
        },
        handoff_depth: artist_session::handoff_depth(&resumed_events),
        // No stage in a one-shot run: bringing a display up costs more than the
        // run is worth, so the tool is simply not offered.
        computer: None,
        // No canvas in a one-shot run, so nothing reads this registry.
        tools: artist_agent::ToolRegistryHandle::new(),
        // A one-shot run is one session, and these handles are built once for
        // it — so constructing the freezer here is the session scope.
        prefix: artist_agent::prefix::PrefixFreezer::for_session(),
        chain: artist_session::ChainState::new(),
        capabilities: artist_session::ProviderCapabilities::for_session(),
        statefulness: effective.statefulness,
    };
    extension_control.set_steering(Some(steering.clone()));
    extensions
        .update_context(|context| context.agent_state = serde_json::json!({"state":"thinking"}));
    let _ = extensions.publish(artist_extensions::Event {
        kind: "state_transition".into(),
        payload: serde_json::json!({"state":"thinking"}),
    });
    let styled = std::io::stdout().is_terminal();
    let json_event_stream = std::env::var("ARTIST_EVENT_STREAM").as_deref() == Ok("jsonl");
    let mut reasoning = false;
    let mut response = String::new();
    let agent_input = artist_agent::ChatInput::from(input.to_owned());
    let outcome = {
        // A resumed session runs as whatever profile it last handed off to.
        let recorded_lineage_profile = resumed_events.iter().rev().find_map(|envelope| {
            if envelope.lineage != target_lineage {
                return None;
            }
            match envelope.event() {
                artist_session::SessionEvent::RunStarted(run) => run.profile,
                _ => None,
            }
        });
        let session_profile = profile_override
            .map(str::to_owned)
            .or(recorded_lineage_profile)
            .unwrap_or_else(|| {
                artist_session::active_profile(&resumed_events)
                    .unwrap_or_else(|| "default".to_owned())
            });
        let chat = artist_agent::stream_chat_as(
            &session_provider,
            &session_profile,
            &agent_input,
            artist_agent::ToolContext {
                // One-shot runs exit before anyone could open a page.
                canvas: None,
                native: &tools,
                mcp,
                extensions: Some(extensions),
                disabled: &effective.denied_tools,
            },
            handles,
            |event| {
                publish_prompt_event(extensions, &event);
                if let Some(events) = embedded_events {
                    let _ = events.send(EmbeddedEvent::Prompt(event.clone()));
                }
                use artist_agent::PromptEvent;
                use std::io::Write;
                if json_event_stream {
                    let mut events = std::io::stderr().lock();
                    serde_json::to_writer(&mut events, &event)?;
                    writeln!(events)?;
                    events.flush()?;
                    if let PromptEvent::TextDelta(delta) = &event {
                        response.push_str(delta);
                    }
                    return Ok(());
                }
                let mut output = std::io::stdout().lock();
                match event {
                    PromptEvent::ReasoningSummaryDelta(delta) => {
                        reasoning = true;
                        if styled {
                            write!(output, "\x1b[90;3m{delta}\x1b[0m")?;
                        } else {
                            write!(output, "{delta}")?;
                        }
                    }
                    PromptEvent::TextDelta(delta) => {
                        response.push_str(&delta);
                        if std::mem::take(&mut reasoning) {
                            writeln!(output)?;
                        }
                        write!(output, "{delta}")?;
                    }
                    PromptEvent::ToolCall { name, .. } => eprintln!("Calling {name}..."),
                    PromptEvent::ToolExecutionStart { .. } => {}
                    PromptEvent::ToolResult { .. } => eprintln!("Tool completed."),
                    PromptEvent::SubagentStarted { role, .. } => {
                        eprintln!("Starting {role} subagent...")
                    }
                    PromptEvent::SubagentEvent { .. } => {}
                    PromptEvent::SubagentFinished { .. } => {
                        eprintln!("Subagent completed.")
                    }
                    PromptEvent::CompletionUsage { .. } => {}
                    PromptEvent::RuleFired { rule, matched } => {
                        let excerpt: String = matched.chars().take(60).collect();
                        eprintln!("rule {rule} fired on \"{excerpt}\" — rewound, retrying");
                    }
                    PromptEvent::HandedOff { from, to } => {
                        eprintln!("── {from} handed off to {to} ──");
                    }
                    PromptEvent::ProviderFallback { from, reason } => {
                        let excerpt: String = reason.chars().take(80).collect();
                        eprintln!("{from} unavailable ({excerpt}) — falling back");
                    }
                }
                output.flush()?;
                Ok(())
            },
        );
        // An extension may request a stop mid-run; that maps onto the
        // cooperative cancellation token (the run records its cancelled
        // state and preserves accumulated output as a partial turn).
        tokio::pin!(chat);
        let mut announced_questions = HashSet::new();
        loop {
            tokio::select! {
                result = &mut chat => break result,
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                    while let Ok(command) = control_receive.try_recv() {
                        match command {
                            FrontendControl::Steer { message } => steering.enqueue(message),
                            FrontendControl::Answer { answer } => {
                                if let Some(ask) = &ask {
                                    ask.answer_from(answer, "gui");
                                }
                            }
                            FrontendControl::Stop => cancel.cancel(),
                        }
                    }
                    if let Some(controls) = embedded_controls.as_deref_mut() {
                        while let Ok(command) = controls.try_recv() {
                            match command {
                                FrontendControl::Steer { message } => steering.enqueue(message),
                                FrontendControl::Answer { answer } => {
                                    if let Some(ask) = &ask {
                                        ask.answer_from(answer, "gui");
                                    }
                                }
                                FrontendControl::Stop => cancel.cancel(),
                            }
                        }
                    }
                    if let Some(ask) = &ask {
                        for question in ask.pending() {
                            if announced_questions.insert(question.id.clone()) {
                                if let Some(events) = embedded_events {
                                    let _ = events.send(EmbeddedEvent::Question(question.clone()));
                                }
                                eprintln!(
                                    "ARTIST_QUESTION={}",
                                    serde_json::to_string(&question).unwrap_or_default()
                                );
                            }
                        }
                    }
                    if extension_control.take_stop() {
                        cancel.cancel();
                    }
                }
            }
        }
    };
    extension_control.set_steering(None);
    extensions.update_context(|context| context.agent_state = serde_json::json!({"state":"idle"}));
    let _ = extensions.publish(artist_extensions::Event {
        kind: "state_transition".into(),
        payload: serde_json::json!({"state":"idle"}),
    });
    let outcome = outcome?;
    let _ = outcome;
    println!();
    let session_id = active.session.id.clone();
    active.close().await?;
    for followup in extension_control.take_prompts() {
        Box::pin(execute_prompt(
            store,
            path,
            &followup,
            Some(&session_id),
            provider_override,
            model_override,
            profile_override,
            lineage_override,
            mcp,
            extensions,
            extension_control,
            embedded_events,
            embedded_controls.as_deref_mut(),
        ))
        .await?;
    }
    Ok(())
}

async fn compact_noninteractive_if_needed(
    active: &ActiveSession,
    provider: &llm_provider::SavedProvider,
    settings: settings::CompactionConfig,
    prompt: &str,
) -> Result<()> {
    if !settings.enabled {
        return Ok(());
    }
    let history = active
        .memory
        .load(&active.session.id)
        .await
        .context("load conversation for compaction check")?;
    if history.is_empty() {
        return Ok(());
    }
    let capacity = models::catalog(provider).await.ok().and_then(|catalog| {
        catalog
            .iter()
            .find(|model| Some(&model.slug) == provider.model.as_ref())
            .and_then(|model| model.effective_context_window())
    });
    let projected = compaction::projected_context_tokens(&history, None, prompt, 0);
    if capacity.is_some_and(|window| compaction::should_compact(projected, window, settings)) {
        eprintln!("Compacting context…");
        match compaction::compact(active, provider, settings, None, "threshold", None).await {
            Ok(Some(result)) => eprintln!(
                "Compacted {} messages; ~{} context tokens retained.",
                result.summarized_messages,
                artist_session::compaction::estimate_messages_tokens(&result.history)
            ),
            Ok(None) => {}
            Err(error) => eprintln!("Warning: automatic compaction failed: {error:#}"),
        }
    }
    Ok(())
}

async fn extension_manager(
    config_root: &std::path::Path,
    store: &ProviderStore,
    control: extension_control::ExtensionControl,
) -> Result<Arc<artist_extensions::Manager>> {
    let project = std::env::current_dir().context("find current project directory")?;
    // Model/reasoning come from the resolved settings now, not the provider.
    let effective = settings::load_effective(
        config_root,
        &project,
        &settings::Overrides::default(),
        &store.disabled_tools,
    )
    .unwrap_or_default();
    let context = artist_extensions::ExtensionContext {
        project,
        model: effective.model.clone(),
        reasoning: effective.reasoning_effort.clone(),
        agent_state: serde_json::json!({"state": "idle"}),
        recent_events: Vec::new(),
    };
    Ok(Arc::new(
        artist_extensions::Manager::load(
            config_root.join("extensions"),
            context,
            Arc::new(control),
        )
        .await,
    ))
}

fn publish_prompt_event(manager: &artist_extensions::Manager, event: &artist_agent::PromptEvent) {
    use artist_agent::PromptEvent;
    let state = match event {
        PromptEvent::ReasoningSummaryDelta(_) => Some("thinking"),
        PromptEvent::TextDelta(_) => Some("responding"),
        PromptEvent::ToolCall { .. } | PromptEvent::ToolExecutionStart { .. } => Some("working"),
        PromptEvent::ToolResult { .. } => Some("thinking"),
        PromptEvent::SubagentStarted { .. } | PromptEvent::SubagentEvent { .. } => Some("working"),
        PromptEvent::SubagentFinished { .. } => Some("thinking"),
        PromptEvent::RuleFired { .. } => Some("rewinding"),
        PromptEvent::ProviderFallback { .. } => Some("working"),
        PromptEvent::HandedOff { .. } => Some("thinking"),
        PromptEvent::CompletionUsage { .. } => None,
    };
    if let Some(state) = state {
        manager.update_context(|context| context.agent_state = serde_json::json!({"state": state}));
    }
    let payload = serde_json::to_value(event).unwrap_or(serde_json::Value::Null);
    let _ = manager.publish(artist_extensions::Event {
        kind: "prompt_event".into(),
        payload,
    });
}

fn sessions_list(sessions: &SessionStore) -> Result<()> {
    let project = std::env::current_dir()?;
    let mut entries = sessions.list_project(&project)?;
    if entries.is_empty() {
        println!("No sessions for {}", project.display());
        return Ok(());
    }
    entries.sort_by_key(|session| std::cmp::Reverse(session.created_at_ms));
    for session in entries {
        let size = dir_size(session.dir());
        println!(
            "{}  {:>8}  {}{}",
            session.id,
            format_size(size),
            session.label.as_deref().unwrap_or("Untitled"),
            session
                .parent
                .as_deref()
                .map(|parent| format!("  (fork of {parent})"))
                .unwrap_or_default()
        );
    }
    Ok(())
}

fn sessions_render(sessions: &SessionStore, id: &str) -> Result<()> {
    let (session, events) = sessions.peek(id)?;
    let markdown = artist_session::render_markdown(&events);
    std::fs::write(&session.transcript, &markdown)?;
    println!("regenerated {}", session.transcript.display());
    Ok(())
}

/// Print the computer-use trace for a session.
///
/// Reads the event log directly rather than a projection: the log is the record,
/// and `computer.acted` carries exactly the `(anchor, action, expect)` triples a
/// replayable macro would be distilled from.
fn computer_log(
    sessions: &SessionStore,
    project: &std::path::Path,
    id: Option<&str>,
) -> Result<()> {
    let id = match id {
        Some(id) => id.to_owned(),
        None => sessions
            .list()?
            .into_iter()
            .filter(|session| session.project == project)
            .max_by_key(|session| session.created_at_ms)
            .map(|session| session.id)
            .context("no sessions for this project")?,
    };
    let (_, events) = sessions.peek(&id)?;

    let mut seen = 0usize;
    for envelope in &events {
        match envelope.event() {
            artist_session::SessionEvent::ComputerStageOpened(opened) => {
                seen += 1;
                println!(
                    "stage {} opened ({}{})",
                    opened.stage,
                    opened.backend,
                    opened
                        .display
                        .map(|display| format!(", {display}"))
                        .unwrap_or_default()
                );
            }
            artist_session::SessionEvent::ComputerStageClosed(closed) => {
                seen += 1;
                println!("stage {} closed: {}", closed.stage, closed.reason);
            }
            artist_session::SessionEvent::ComputerObserved(observed) => {
                seen += 1;
                println!(
                    "observe {} epoch {} rung {} · {} node(s), {} bytes{}",
                    observed.surface,
                    observed.epoch,
                    observed.rung,
                    observed.nodes,
                    observed.bytes,
                    observed
                        .image
                        .map(|digest| format!(" · img:{}", short(&digest)))
                        .unwrap_or_default()
                );
            }
            artist_session::SessionEvent::ComputerActed(acted) => {
                seen += 1;
                println!("act {} epoch {}", acted.surface, acted.epoch);
                for (index, step) in acted.steps.iter().enumerate() {
                    // Claimed vs actual side by side: a mismatch here is the
                    // single most useful thing to see after a bad run.
                    let target = match (&step.label, &step.resolved_name) {
                        (Some(label), Some(actual)) if label != actual => {
                            format!(" {label:?} (actually {actual:?})")
                        }
                        (Some(label), _) => format!(" {label:?}"),
                        (None, Some(actual)) => format!(" {actual:?}"),
                        _ => String::new(),
                    };
                    let anchor = step
                        .anchor
                        .as_ref()
                        .map(|anchor| format!(" [{anchor}]"))
                        .unwrap_or_default();
                    println!(
                        "  {}. {}{target}{anchor} — {}",
                        index + 1,
                        step.action,
                        step.outcome
                    );
                }
                if let Some(failed) = acted.failed_step {
                    println!("  stopped at step {}", failed + 1);
                }
                match acted.expect_met {
                    Some(true) => println!("  expect: met"),
                    Some(false) => println!("  expect: NOT met"),
                    None => {}
                }
                if let Some(ms) = acted.settled_ms {
                    println!("  settled in {ms}ms");
                }
            }
            artist_session::SessionEvent::ComputerElided(elided) => {
                seen += 1;
                println!(
                    "elided {} observation(s), {} reclaimed",
                    elided.count,
                    format_size(elided.bytes_saved)
                );
            }
            _ => {}
        }
    }
    if seen == 0 {
        println!("no computer-use activity in session {id}");
    }
    Ok(())
}

/// Emit a replayable macro from a session's successful programs.
///
/// This is what the event log was shaped for. A trajectory the model worked out
/// once — which anchors, in what order, and what it expected to land on — is
/// exactly a script, and replaying it deterministically is orders of magnitude
/// cheaper than rediscovering it. The `expect` on each step is what makes replay
/// safe: if the interface has changed, the macro stops at the step that no
/// longer lands where it did, rather than carrying on blindly.
///
/// Failed programs are excluded by default: a macro built from steps that did
/// not work is worse than no macro.
/// Replay a distilled macro against a freshly launched application.
///
/// Blocking on purpose: this is a one-shot command, not part of the agent loop,
/// so it builds its own runtime rather than borrowing one.
/// Write the computer-use trajectory of a session as one JSON document.
///
/// Deliberately lossless and deliberately *not* the macro format. A macro drops
/// everything that was not needed to repeat the task — observations, timings,
/// failures, the anchors themselves — because carrying them would make replay
/// brittle. A trajectory keeps them, because the things a harness scores and a
/// fine-tune learns from are exactly what replay throws away: what the screen
/// looked like, what was tried, and what happened next.
/// Where a served session listens, unless told otherwise.
fn computer_socket(given: Option<std::path::PathBuf>) -> Result<std::path::PathBuf> {
    if let Some(path) = given {
        return Ok(path);
    }
    let runtime =
        std::env::var("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is not set; pass --socket")?;
    Ok(std::path::Path::new(&runtime).join("artist-computer.sock"))
}

/// Hold one computer-use session open, taking tool calls over a socket.
///
/// One connection is one call: read a line of JSON, run it, write the reply,
/// close. Deliberately not a persistent protocol — the state that matters lives
/// in the registry, not in the connection, so a caller can be anything that can
/// run a command and nothing is lost if it dies mid-task.
async fn computer_serve(socket: Option<std::path::PathBuf>) -> Result<()> {
    use rig_core::tool::PortableTool;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let path = computer_socket(socket)?;
    // A stale socket from a crashed serve would otherwise make every later
    // start fail with "address in use" and no hint as to why.
    if path.exists() {
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            anyhow::bail!(
                "another `artist computer serve` is already listening on {}",
                path.display()
            );
        }
        std::fs::remove_file(&path)?;
    }
    // Tokio's listener, not std's: `main` is already inside a runtime, so
    // building a second one and calling `block_on` panics with "cannot start a
    // runtime from within a runtime". That is not a detail — the same mistake
    // was sitting latent in `computer_replay`, which had never been run far
    // enough to hit it.
    let listener = tokio::net::UnixListener::bind(&path)
        .with_context(|| format!("bind {}", path.display()))?;

    let project = std::env::current_dir().context("current directory")?;
    let registry = artist_computer::SurfaceRegistry::for_project(
        &project,
        artist_computer::host::DEFAULT_SCREEN,
    );
    let tool = artist_computer::ComputerTool::new(registry);

    println!("listening on {}", path.display());
    println!("drive it with: artist computer call '{{\"mode\":\"surfaces\"}}'");
    println!("watch it with: artist computer call '{{\"mode\":\"watch\"}}'");

    loop {
        let (mut connection, _) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                eprintln!("accept: {error}");
                continue;
            }
        };
        let mut line = String::new();
        {
            let mut reader = BufReader::new(&mut connection);
            if reader.read_line(&mut line).await.is_err() {
                continue;
            }
        }
        let reply = match serde_json::from_str::<artist_computer::ComputerArgs>(line.trim()) {
            Ok(parsed) => match tool.call(parsed).await {
                Ok(output) => output.render(),
                // Errors are returned as text rather than as a transport
                // failure: a model sees tool errors as content, so a rig that
                // hid them behind a non-zero exit would be testing something
                // other than what a model reads.
                Err(error) => format!("ERROR: {error}"),
            },
            Err(error) => format!("ERROR: that is not valid `computer` arguments: {error}"),
        };
        let _ = connection.write_all(reply.as_bytes()).await;
        let _ = connection.flush().await;
        let _ = connection.shutdown().await;
    }
}

/// Send one tool call to a running `serve` and print what comes back.
fn computer_call(json: &str, socket: Option<std::path::PathBuf>) -> Result<()> {
    use std::io::{Read, Write};

    let path = computer_socket(socket)?;
    let mut connection = std::os::unix::net::UnixStream::connect(&path).with_context(|| {
        format!(
            "no session at {}. Start one with `artist computer serve`.",
            path.display()
        )
    })?;
    // Validated here as well as in the server, so a typo is caught without a
    // round trip and without the server having to describe it.
    serde_json::from_str::<serde_json::Value>(json).context("the argument is not valid JSON")?;

    connection.write_all(json.trim().as_bytes())?;
    connection.write_all(b"\n")?;
    connection.flush()?;
    connection.shutdown(std::net::Shutdown::Write).ok();

    let mut reply = String::new();
    connection.read_to_string(&mut reply)?;
    print!("{reply}");
    if !reply.ends_with('\n') {
        println!();
    }
    // A tool error is content, not a failed command — but a script driving this
    // still wants to branch on it.
    if reply.starts_with("ERROR:") {
        std::process::exit(1);
    }
    Ok(())
}

fn computer_export(
    sessions: &SessionStore,
    project: &std::path::Path,
    id: Option<&str>,
    out: Option<&std::path::Path>,
) -> Result<()> {
    let id = match id {
        Some(id) => id.to_owned(),
        None => sessions
            .list()?
            .into_iter()
            .filter(|session| session.project == project)
            .max_by_key(|session| session.created_at_ms)
            .map(|session| session.id)
            .context("no sessions for this project")?,
    };
    let (_, events) = sessions.peek(&id)?;

    let mut steps = Vec::new();
    for envelope in &events {
        let entry = match envelope.event() {
            artist_session::SessionEvent::ComputerObserved(observed) => serde_json::json!({
                "kind": "observed",
                "at_ms": envelope.ts,
                "call": observed.internal_call_id,
                "surface": observed.surface,
                "epoch": observed.epoch,
                "rung": observed.rung,
                "full": observed.full,
                "nodes": observed.nodes,
                "bytes": observed.bytes,
                // The digest, not the pixels: a trajectory stays a document you
                // can read, and `artist computer frame <sha>` writes the image
                // out when one is actually wanted.
                "image": observed.image,
            }),
            artist_session::SessionEvent::ComputerActed(acted) => serde_json::json!({
                "kind": "acted",
                "at_ms": envelope.ts,
                "call": acted.internal_call_id,
                "surface": acted.surface,
                "epoch": acted.epoch,
                "steps": acted.steps.iter().map(|step| serde_json::json!({
                    "action": step.action,
                    "anchor": step.anchor,
                    // Both halves of the cross-check, because a mismatch between
                    // them is the most informative thing in a failed run.
                    "claimed_label": step.label,
                    "resolved_name": step.resolved_name,
                    "payload": step.payload,
                    "outcome": step.outcome,
                })).collect::<Vec<_>>(),
                "settled_ms": acted.settled_ms,
                "expect": acted.expect,
                "expect_met": acted.expect_met,
                "failed_step": acted.failed_step,
            }),
            artist_session::SessionEvent::ComputerStageOpened(opened) => serde_json::json!({
                "kind": "stage_opened",
                "at_ms": envelope.ts,
                "stage": opened.stage,
                "backend": opened.backend,
                "display": opened.display,
            }),
            artist_session::SessionEvent::ComputerStageClosed(closed) => serde_json::json!({
                "kind": "stage_closed",
                "at_ms": envelope.ts,
                "stage": closed.stage,
                "reason": closed.reason,
            }),
            _ => continue,
        };
        steps.push(entry);
    }

    let document = serde_json::json!({
        "format": "artist.computer.trajectory/1",
        "session": id,
        "project": project.display().to_string(),
        "steps": steps,
    });
    let text = serde_json::to_string_pretty(&document)?;

    match out {
        Some(path) => {
            std::fs::write(path, text.as_bytes())
                .with_context(|| format!("write {}", path.display()))?;
            // Said plainly rather than silently: an export that wrote nothing
            // because the session had no computer use looks identical to a
            // successful one from the shell.
            eprintln!(
                "wrote {} step(s) to {}",
                steps_len(&document),
                path.display()
            );
        }
        None => println!("{text}"),
    }
    Ok(())
}

fn steps_len(document: &serde_json::Value) -> usize {
    document
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

async fn computer_replay(file: &std::path::Path, launch: Option<&str>, heal: bool) -> Result<()> {
    use artist_computer::macros::{Healing, Macro};

    let text = std::fs::read_to_string(file).with_context(|| format!("read {}", file.display()))?;
    let recorded: Macro =
        serde_json::from_str(&text).with_context(|| format!("parse {}", file.display()))?;

    // The macro's own launch, unless one was given. A macro that records how to
    // bring up its own subject is a complete artifact; one that does not needs
    // telling, and saying which is which beats failing with "missing argument".
    let (program, args) = match launch {
        Some(given) => {
            let mut words = given.split_whitespace().map(str::to_owned);
            let program = words.next().context("--launch is empty")?;
            (program, words.collect::<Vec<String>>())
        }
        None => {
            let recorded = recorded.launch.clone().with_context(|| {
                format!(
                    "{} does not record how to start the application it was run against \
                     (it predates that being recorded). Pass --launch \"<command>\".",
                    file.display()
                )
            })?;
            (recorded.program, recorded.args)
        }
    };

    // Already inside `main`'s runtime; building a second one here panics.
    {
        let project = std::env::current_dir().context("current directory")?;
        let registry = artist_computer::SurfaceRegistry::for_project(
            &project,
            artist_computer::host::DEFAULT_SCREEN,
        );
        let launched = registry
            .host()
            .launch(&program, &args, None, None)
            .await
            .map_err(|error| anyhow::anyhow!("launch {program}: {error}"))?;
        let id = registry.attach_with_declines(launched.surface, launched.declined);
        let attached = registry
            .get(&id)
            .context("the surface vanished immediately after attaching")?;

        let mut book = attached.book.lock().await;
        let outcome = artist_computer::macros::replay(
            attached.surface.as_ref(),
            &mut book,
            &recorded,
            if heal {
                Healing::Allowed
            } else {
                Healing::Never
            },
        )
        .await;

        match outcome {
            Ok(done) => {
                println!(
                    "replayed {} of {} program(s)",
                    done.completed,
                    recorded.programs.len()
                );
                // Printed even on success, because a healed replay succeeding is
                // exactly when nobody would otherwise look.
                if let Some(note) = done.note() {
                    print!("{note}");
                }
                Ok(())
            }
            Err(error) => {
                // A drift is the macro doing its job, not a crash: the interface
                // moved and it stopped rather than clicking something else.
                eprintln!("{error}");
                if !heal {
                    eprintln!(
                        "If this interface merely renames things, `--heal` will \
                         re-derive the target and tell you what it substituted."
                    );
                }
                std::process::exit(1);
            }
        }
    }
}

fn computer_distill(
    sessions: &SessionStore,
    project: &std::path::Path,
    id: Option<&str>,
    include_failed: bool,
) -> Result<()> {
    let id = match id {
        Some(id) => id.to_owned(),
        None => sessions
            .list()?
            .into_iter()
            .filter(|session| session.project == project)
            .max_by_key(|session| session.created_at_ms)
            .map(|session| session.id)
            .context("no sessions for this project")?,
    };
    let (_, events) = sessions.peek(&id)?;

    let mut programs = Vec::new();
    for envelope in &events {
        let artist_session::SessionEvent::ComputerActed(acted) = envelope.event() else {
            continue;
        };
        let clean = acted.failed_step.is_none() && acted.expect_met != Some(false);
        if !clean && !include_failed {
            continue;
        }
        // Emitted in the schema `artist_computer::macros` replays. Anchors are
        // deliberately *not* carried: they are per-session tokens from one
        // observation, while the label is how the element identifies itself and
        // is what a replayer resolves against a fresh screen.
        let steps: Vec<artist_computer::macros::MacroStep> = acted
            .steps
            .iter()
            .map(|step| artist_computer::macros::MacroStep {
                action: step.action.clone(),
                label: step.resolved_name.clone().or_else(|| step.label.clone()),
                // The payload is the step. Dropping it made every distilled
                // macro a no-op that reported success: `key` replayed as an
                // empty chord, `type` typed nothing.
                text: (step.action == "type")
                    .then(|| step.payload.clone())
                    .flatten(),
                key: (step.action == "key")
                    .then(|| step.payload.clone())
                    .flatten(),
            })
            .collect();

        programs.push(artist_computer::macros::MacroProgram {
            surface: acted.surface.clone(),
            steps,
            expect: acted.expect.clone(),
        });
    }

    if programs.is_empty() {
        println!("no replayable programs in session {id}");
        return Ok(());
    }
    // The last graphical launch in the session. "Last" rather than "first"
    // because a session that opened several things ends up working in the most
    // recent one, and that is what the recorded programs act on.
    let launch = events
        .iter()
        .filter_map(|envelope| match envelope.event() {
            artist_session::SessionEvent::ComputerLaunched(launched) => {
                Some(artist_computer::macros::MacroLaunch {
                    program: launched.program,
                    args: launched.args,
                    cwd: launched.cwd,
                    gui: launched.gui,
                })
            }
            _ => None,
        })
        .next_back();

    let distilled = artist_computer::macros::Macro {
        session: id,
        launch,
        programs,
    };
    println!("{}", serde_json::to_string_pretty(&distilled)?);
    Ok(())
}

/// Write a captured frame out so it can be opened in an image viewer.
fn computer_frame(
    sessions: &SessionStore,
    project: &std::path::Path,
    digest: &str,
    out: Option<std::path::PathBuf>,
) -> Result<()> {
    // Accept the `img:<sha>` form the transcript prints, and short prefixes.
    let wanted = digest.strip_prefix("img:").unwrap_or(digest);

    for session in sessions.list()? {
        if session.project != project {
            continue;
        }
        let dir = session.dir().join("attachments");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(wanted) {
                continue;
            }
            let bytes = std::fs::read(entry.path())?;
            let path = out.unwrap_or_else(|| std::path::PathBuf::from(format!("{name}.png")));
            std::fs::write(&path, &bytes)?;
            println!(
                "wrote {} ({})",
                path.display(),
                format_size(bytes.len() as u64)
            );
            return Ok(());
        }
    }
    bail!("no attachment matching {digest} in this project's sessions")
}

/// The first few characters of a digest, for a log line.
///
/// By character, not by byte. Digests are hex today, but this also renders
/// whatever an attachment id happens to be, and slicing a multi-byte character
/// in half panics rather than printing a short string.
fn short(digest: &str) -> &str {
    let end = digest
        .char_indices()
        .nth(12)
        .map(|(index, _)| index)
        .unwrap_or(digest.len());
    &digest[..end]
}

async fn sessions_gc(
    sessions: &SessionStore,
    config_root: &std::path::Path,
    keep: usize,
    older_than_days: u64,
    dry_run: bool,
) -> Result<()> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let cutoff = now.saturating_sub(older_than_days.saturating_mul(24 * 60 * 60 * 1000));
    let mut by_project: std::collections::BTreeMap<
        std::path::PathBuf,
        Vec<artist_session::Session>,
    > = Default::default();
    for session in sessions.list()? {
        by_project
            .entry(session.project.clone())
            .or_default()
            .push(session);
    }
    let mut removed = 0usize;
    let mut reclaimed = 0u64;
    let mut survivors = Vec::new();
    for (_, mut entries) in by_project {
        entries.sort_by_key(|session| std::cmp::Reverse(session.created_at_ms));
        for (index, session) in entries.into_iter().enumerate() {
            if index < keep || session.created_at_ms >= cutoff {
                survivors.push(session);
                continue;
            }
            let size = dir_size(session.dir());
            removed += 1;
            reclaimed += size;
            if dry_run {
                println!(
                    "would delete {}  {:>8}  {}",
                    session.id,
                    format_size(size),
                    session.label.as_deref().unwrap_or("Untitled")
                );
            } else {
                sessions.remove(&session.id)?;
                // The conversation's anchor state and write attributions die
                // with it. Retention would collect them eventually; doing it
                // here means "delete this session" actually deletes it.
                if let Ok(state_dir) = project_state_dir(config_root, &session.project) {
                    artist_tools::forget_conversation(&state_dir, &session.id).await;
                }
                println!("deleted {}  {:>8}", session.id, format_size(size));
            }
        }
    }
    // Surviving sessions can still hold orphaned image blobs — compaction and
    // rewind both leave attachments no event refers to any more.
    let mut orphans = 0usize;
    for session in survivors {
        let dir = session.dir();
        let store = artist_session::AttachmentStore::new(dir.join("attachments"));
        if !store.dir().exists() {
            continue;
        }
        let Ok(events) = artist_session::EventLogReader::new(dir).read_all() else {
            continue;
        };
        let referenced = artist_session::referenced_attachments(&events);
        if dry_run {
            let present = std::fs::read_dir(store.dir())
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|entry| {
                            entry
                                .file_name()
                                .to_str()
                                .is_some_and(|name| !referenced.contains(name))
                        })
                        .count()
                })
                .unwrap_or(0);
            orphans += present;
        } else if let Ok(pruned) = store.prune(&referenced) {
            orphans += pruned;
        }
    }

    println!(
        "{}{} session(s), {}",
        if dry_run { "would delete " } else { "deleted " },
        removed,
        format_size(reclaimed)
    );
    if orphans > 0 {
        println!(
            "{}{orphans} orphaned attachment(s) in retained sessions",
            if dry_run { "would prune " } else { "pruned " }
        );
    }
    Ok(())
}

fn dir_size(dir: &std::path::Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![dir.to_owned()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                total += metadata.len();
            }
        }
    }
    total
}

pub(crate) fn format_size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1048575 => format!("{:.1} KiB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MiB", bytes as f64 / 1048576.0),
    }
}

/// `artist rules new <name>`: write a commented rule template into the
/// project's .artist/rules/ directory.
fn scaffold_rule(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        bail!("rule names are lowercase-kebab-case (got {name:?})");
    }
    let dir = std::env::current_dir()?.join(".artist/rules");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.md"));
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    let template = format!(
        r#"---
name: {name}
description: One line describing what this rule catches
# What to match against. Any of: assistant-text, tool-args, reasoning-summary.
targets: [assistant-text]
# Linear-time regexes; a match mid-stream aborts the request, injects the
# reminder below, and retries from the same point.
patterns:
  - 'REPLACE ME'
# Only for tool-args targets: restrict to these tools (empty = all).
# tools: [write, edit, bash]
# fire: once        # once per session (default) | per-turn
# persistence: session  # keep reminding every turn (default) | message
# scope: [main, delegate]
---
Write the reminder the model receives here. Say what NOT to do and what to
do instead.
"#
    );
    std::fs::write(&path, template)?;
    println!("created {}", path.display());
    println!(
        "test it against a session with: /rules dry-run {}",
        path.display()
    );
    Ok(())
}

/// Show what this project resolves, and where each profile comes from, so an
/// override that is not taking effect is visible rather than mysterious.
fn profiles_list(project: &std::path::Path) {
    let profiles = artist_agent::profiles::Profiles::discover(project);
    for name in profiles.names() {
        let Ok(profile) = profiles.get(&name) else {
            continue;
        };
        let origin = profile
            .source
            .as_ref()
            .map_or_else(|| "built-in".to_owned(), |path| path.display().to_string());
        println!("{name}\n    {}\n    {origin}", profile.description);
    }
    for diagnostic in profiles.diagnostics() {
        eprintln!("warning: {diagnostic}");
    }
}

/// Print a built-in as a file to save under .artist/profiles/, which is how a
/// built-in becomes editable now that nothing is scaffolded.
fn profiles_show(name: &str) -> Result<()> {
    let source = artist_agent::profiles::builtin_source(name).with_context(|| {
        format!(
            "no built-in profile named {name}; available: {}",
            artist_agent::profiles::builtin_names().join(", ")
        )
    })?;
    print!("{source}");
    Ok(())
}

fn tool_bundle(config_root: &std::path::Path, project: &std::path::Path) -> Result<ToolBundle> {
    Ok(ToolBundle::new(Workspace::open(
        std::fs::canonicalize(project)?,
        project_state_dir(config_root, project)?,
        // The bundle outlives any one conversation and is built before the
        // first exists, so this identity is a placeholder: every real turn
        // re-actors it to its conversation id. Named for what it is, so anchor
        // state written under it is recognisable as nobody's.
        "artist-unbound",
    )?))
}

/// Per-project state, keyed by canonical path. Shared with the file tools, so
/// the memory database lands beside `hashlines.sqlite3` rather than inventing
/// a second location convention.
fn project_state_dir(
    config_root: &std::path::Path,
    project: &std::path::Path,
) -> Result<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    let canonical = std::fs::canonicalize(project)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical.hash(&mut hasher);
    Ok(config_root
        .join("tools")
        .join(format!("{:x}", hasher.finish())))
}

/// Open the memory subsystem, or return `None` when it is off or unusable.
///
/// Every failure here degrades to "no memory" rather than aborting the session:
/// a missing embedding model or an unreadable database should cost recall, not
/// the ability to work. The reason it is disabled by default is the same one —
/// silently degrading to lexical-only recall would read as memory being broken.
///
/// Does the open failure mean someone else holds the store?
///
/// Matched on the message because the lock failure surfaces from RocksDB as a
/// C++ status string through two layers of wrapping, with no typed error to
/// match on. Getting this wrong only costs a less specific warning, never
/// correctness — both branches degrade to the same "no memory".
fn is_locked(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}").to_ascii_lowercase();
    text.contains("lock") || text.contains("no locks available") || text.contains("resource busy")
}

async fn open_memory(
    config_root: &std::path::Path,
    settings: &settings::MemoryConfig,
) -> Option<artist_agent::memory::MemoryHandle> {
    if !settings.enabled {
        return None;
    }
    let memory = match artist_memory::Memory::open(config_root).await {
        Ok(memory) => memory,
        // RocksDB takes an exclusive lock on the store directory. Memory is one
        // undivided store now, so *any* other running artist holds it, not just
        // one in this project — which makes the collision far more likely than
        // it used to be, and worth saying plainly rather than emitting a
        // generic warning that reads like a bug and scrolls away unread.
        Err(error) if is_locked(&error) => {
            eprintln!(
                "warning: another artist already has memory open, so this session is \
                 running WITHOUT memory.\n         \
                 Facts written here will not be saved and past facts will not be recalled.\n         \
                 Store: {}",
                artist_memory::facts_path(config_root).display()
            );
            return None;
        }
        Err(error) => {
            eprintln!("warning: memory disabled, could not open the store: {error}");
            return None;
        }
    };
    let model_dir = settings
        .model_dir
        .as_ref()
        .map(|dir| {
            let path = std::path::Path::new(dir);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                config_root.join(path)
            }
        })
        .unwrap_or_else(|| config_root.join("models").join("code-embed"));
    let embedder = match artist_memory::Embedder::load(&model_dir, settings.dim).await {
        Ok(embedder) => Some(embedder),
        Err(error) => {
            eprintln!(
                "warning: memory recall will be lexical only, no embedding model at {}: {error}",
                model_dir.display()
            );
            None
        }
    };
    Some(artist_agent::memory::MemoryHandle::new(memory, embedder))
}

/// Maintenance for the memory store.
///
/// These deliberately open the store directly rather than going through
/// `open_memory`: they must work when the subsystem is disabled in settings or
/// when no embedding model is installed, since that is exactly when you want to
/// inspect or rescue what is there.
async fn memory_command(config_root: &std::path::Path, action: args::MemoryCommand) -> Result<()> {
    use args::MemoryCommand;
    let memory = artist_memory::Memory::open(config_root)
        .await
        .context("opening the memory store")?;

    match action {
        MemoryCommand::List => {
            let facts = memory.store().live_facts().await?;
            println!("{} fact(s)", facts.len());
            for fact in facts {
                println!("  [{}] ({}) {}", fact.id, fact.origin, fact.text);
            }
        }
        MemoryCommand::Search { query } => {
            // No embedder here, so this exercises the lexical leg only — which
            // is also the honest picture of what recall degrades to without a
            // model installed.
            let hits = memory.search(&query, &[], 20).await?;
            if hits.is_empty() {
                println!("No memories matched.");
            }
            for hit in hits {
                println!("[{}] ({:.4}) {}", hit.id, hit.score, hit.text);
            }
        }
        MemoryCommand::Export => {
            println!("{}", memory.store().export_json().await?);
        }
        MemoryCommand::Import { path } => {
            let payload =
                std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
            memory.store().import_json(payload).await?;
            println!("Imported and reindexed.");
        }
        MemoryCommand::Reindex => {
            memory.store().reindex().await?;
            println!("Rebuilt every index.");
        }
        MemoryCommand::Verify => {
            let store = memory.store();
            println!(
                "schema v{:?}, {} live fact(s), {} chunk(s)",
                store.schema_version().await?,
                store.fact_count().await?,
                store.chunk_count().await?,
            );
        }
    }
    Ok(())
}

fn list(store: &ProviderStore) {
    if store.providers.is_empty() {
        println!("No providers saved.");
        return;
    }
    for provider in &store.providers {
        let marker = if store.default_provider.as_ref() == Some(&provider.id) {
            "*"
        } else {
            " "
        };
        let identity = provider
            .chatgpt_auth()
            .ok()
            .map(|auth| auth.email.as_deref().unwrap_or(&auth.account_id))
            .unwrap_or("API key");
        println!("{marker} {}  {identity}", provider.name);
    }
}

fn choose(store: &ProviderStore, label: &str) -> Result<usize> {
    if store.providers.is_empty() {
        bail!("no accounts saved — run `artist provider --login chatgpt` to sign in");
    }
    let items: Vec<_> = store.providers.iter().map(|p| p.name.clone()).collect();
    let default = store
        .default_provider
        .as_ref()
        .and_then(|id| store.providers.iter().position(|p| &p.id == id))
        .unwrap_or(0);
    prompt::select(label, &items, default)
}
fn set_default(store: &mut ProviderStore) -> Result<()> {
    let selected = choose(store, "Default provider")?;
    let provider = &store.providers[selected];
    store.default_provider = Some(provider.id.clone());
    println!("Default provider set to {}.", provider.name);
    Ok(())
}
async fn test_selected(store: &mut ProviderStore, path: &std::path::Path) -> Result<()> {
    let selected = choose(store, "Provider to test")?;
    if refresh_if_needed(&mut store.providers[selected]).await? {
        store.save(path)?;
    }
    let config_root = path.parent().context("providers path has no parent")?;
    let project = std::env::current_dir().context("find current project directory")?;
    let effective = settings::load_effective(
        config_root,
        &project,
        &settings::Overrides::default(),
        &store.disabled_tools,
    )?;
    let provider = effective.apply_to(store.providers[selected].clone());
    print!("Testing {}... ", provider.name);
    std::io::Write::flush(&mut std::io::stdout())?;
    test_provider::test(&provider).await?;
    println!("OK");
    Ok(())
}
fn default_index(store: &ProviderStore) -> Result<usize> {
    if store.providers.is_empty() {
        bail!("no account configured — run `artist provider --login chatgpt` to sign in");
    }
    let id = store
        .default_provider
        .as_ref()
        .context("no default provider set — run `artist provider set` to choose one")?;
    store
        .providers
        .iter()
        .position(|provider| &provider.id == id)
        .context("default provider is missing")
}

pub(crate) async fn refresh_if_needed(provider: &mut llm_provider::SavedProvider) -> Result<bool> {
    if provider.provider != llm_provider::ProviderKind::Chatgpt {
        return Ok(false);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // A token with no known expiry is refreshed proactively rather than
    // trusted forever — otherwise it silently rots into an unrecoverable 401.
    // The refresh response populates `expires_at`, so this self-corrects.
    let needs_refresh = provider
        .chatgpt_auth()?
        .expires_at
        .is_none_or(|expiry| expiry <= now.saturating_add(60));
    if needs_refresh {
        return force_refresh(provider).await.map(|()| true);
    }
    Ok(false)
}

/// Refresh the access token unconditionally, ignoring `expires_at`. Used after
/// a mid-turn 401 (AUTH-2), where the token is known-bad regardless of its
/// recorded expiry.
pub(crate) async fn force_refresh(provider: &mut llm_provider::SavedProvider) -> Result<()> {
    if provider.provider != llm_provider::ProviderKind::Chatgpt {
        return Ok(());
    }
    let refreshed = ChatGptOAuth::default()
        .refresh(provider.chatgpt_auth()?)
        .await
        .context("refresh ChatGPT login")?
        .auth;
    *provider.chatgpt_auth_mut()? = refreshed;
    Ok(())
}

#[cfg(test)]
mod frontend_control_tests {
    use super::FrontendControl;

    #[test]
    fn structured_frontend_commands_parse_without_cli_text_scraping() {
        let steer: FrontendControl =
            serde_json::from_str(r#"{"type":"steer","message":"look again"}"#).unwrap();
        assert!(matches!(steer, FrontendControl::Steer { message } if message == "look again"));

        let stop: FrontendControl = serde_json::from_str(r#"{"type":"stop"}"#).unwrap();
        assert!(matches!(stop, FrontendControl::Stop));
    }
}
