use artist_config::ProviderChoice;
use artist_session::{Answer, Question, ReplayItem, Session, SessionEvent, SessionStore};
use artist_ui_core::{
    Block, FocusStack, FocusableObject, InlineKind, InlineLine, PromptEvent,
    Role as TranscriptRole, TokenKind, ToolStatus, Transcript, WorkspaceProjection,
};
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, Entity, Focusable, KeyBinding,
    PathPromptOptions, PromptLevel, Render, Role, ScrollHandle, Subscription, TitlebarOptions,
    Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, size,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Root, Sizable, StyledExt,
    alert::Alert,
    breadcrumb::{Breadcrumb, BreadcrumbItem},
    button::{Button, ButtonGroup, ButtonVariants},
    collapsible::Collapsible,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    resizable::{h_resizable, resizable_panel},
    scroll::ScrollableElement,
    separator::Separator,
    spinner::Spinner,
    tab::{Tab, TabBar},
    text::TextView,
    tree::{TreeItem, TreeState, tree},
};
use gpui_component_assets::Assets;
use gpui_platform::application;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    time::Duration,
};

pub fn run() {
    #[cfg(feature = "embedded-canvas")]
    {
        let cef_runtime = std::env::current_exe()
            .expect("resolve executable")
            .parent()
            .expect("executable has no parent")
            .to_owned();
        let cache_root = dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("artist/cef");
        std::fs::create_dir_all(&cache_root).expect("create CEF cache root");
        gpui_webview::wef::launch(
            gpui_webview::wef::Settings::new()
                .resources_dir_path(cef_runtime.as_os_str().as_encoded_bytes())
                .locales_dir_path(cef_runtime.join("locales").as_os_str().as_encoded_bytes())
                .root_cache_path(cache_root.as_os_str().as_encoded_bytes())
                .external_message_pump(true),
            run_application,
        );
    }
    #[cfg(not(feature = "embedded-canvas"))]
    run_application();
}

fn run_application() {
    application().with_assets(Assets).run(|cx: &mut App| {
        #[cfg(all(feature = "embedded-canvas", target_os = "linux"))]
        start_cef_message_pump(cx);
        gpui_component::init(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-n", NewConversation, None),
            KeyBinding::new("ctrl-o", OpenProject, None),
            KeyBinding::new("ctrl-b", ToggleSidebar, None),
            KeyBinding::new("ctrl-p", QuickOpen, None),
            KeyBinding::new("ctrl-l", FocusComposer, None),
            KeyBinding::new("ctrl-period", StopRun, None),
            KeyBinding::new("escape", DismissOverlay, None),
        ]);
        let bounds = Bounds::centered(None, size(px(1080.), px(980.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(680.), px(480.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Artist".into()),
                    ..Default::default()
                }),
                app_id: Some("dev.artist.gui".to_owned()),
                ..Default::default()
            },
            |window, cx| {
                let app = cx.new(|cx| ArtistApp::new(window, cx));
                cx.new(|cx| Root::new(app, window, cx))
            },
        )
        .expect("open Artist window");
        cx.activate(true);
    });
}

#[cfg(all(feature = "embedded-canvas", target_os = "linux"))]
fn start_cef_message_pump(cx: &mut App) {
    cx.spawn(async move |_cx| {
        let (send, receive) = flume::unbounded();
        std::thread::Builder::new()
            .name("artist-cef-message-pump".into())
            .spawn(move || {
                while send.send(()).is_ok() {
                    std::thread::sleep(Duration::from_millis(1000 / 60));
                }
            })
            .expect("spawn CEF message pump ticker");
        while receive.recv_async().await.is_ok() {
            gpui_webview::wef::do_message_work();
        }
    })
    .detach();
}

actions!(
    artist,
    [
        NewConversation,
        OpenProject,
        ToggleSidebar,
        QuickOpen,
        FocusComposer,
        StopRun,
        DismissOverlay
    ]
);

#[derive(Clone, Debug, PartialEq, Eq)]
struct ChangedFile {
    status: String,
    path: String,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct StageAccessibilityTree {
    surface: String,
    #[serde(default)]
    nodes: Vec<StageAccessibilityNode>,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct StageAccessibilityNode {
    binding: String,
    role: serde_json::Value,
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    state: StageAccessibilityState,
    #[serde(default)]
    actions: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, serde::Deserialize)]
struct StageAccessibilityState {
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    checked: bool,
    #[serde(default)]
    expanded: bool,
    #[serde(default)]
    selected: bool,
    #[serde(default)]
    offscreen: bool,
}

#[derive(Clone, Debug, Default)]
struct SessionSummary {
    updated_at_ms: u64,
    preview: String,
    searchable_text: String,
    provider: Option<String>,
    model: Option<String>,
    failed: bool,
}

struct ArtistApp {
    transcript: Transcript,
    composer: Entity<InputState>,
    session_search: Entity<InputState>,
    workspace_tree: Entity<TreeState>,
    session_name: Entity<InputState>,
    model_override: Entity<InputState>,
    question_notes: Entity<InputState>,
    status: String,
    session_id: Option<String>,
    session_events: Vec<artist_session::Envelope>,
    running: bool,
    #[cfg(target_os = "linux")]
    host_controllers: HashMap<String, crate::host_controller::RootController>,
    #[cfg(target_os = "linux")]
    host_active_session: Option<String>,
    #[cfg(target_os = "linux")]
    host_active_lineage: Option<String>,
    #[cfg(target_os = "linux")]
    stage_surfaces: crate::stage_surface::StageSurfaces,
    stage_accessibility: HashMap<(String, String, String), StageAccessibilityTree>,
    pending_questions: Vec<Question>,
    question_choices: HashMap<String, HashSet<String>>,
    transcript_scroll: ScrollHandle,
    follow_output: bool,
    session_store: SessionStore,
    projects: Vec<PathBuf>,
    sessions: Vec<Session>,
    session_summaries: HashMap<String, SessionSummary>,
    session_trees: HashMap<String, Vec<artist_ui_core::SessionNode>>,
    focus_stack: FocusStack,
    focused_object: Option<FocusableObject>,
    #[cfg(feature = "embedded-canvas")]
    canvas_endpoints: HashMap<(String, String, String), String>,
    #[cfg(feature = "embedded-canvas")]
    canvas_views: HashMap<artist_ui_core::FocusableObjectId, Entity<gpui_webview::WebView>>,
    #[cfg(feature = "embedded-canvas")]
    canvas_accessibility: HashMap<
        artist_ui_core::FocusableObjectId,
        crate::canvas_accessibility::CanvasAccessibilityTree,
    >,
    project: PathBuf,
    run_baseline: HashMap<String, String>,
    session_changed_files: HashSet<String>,
    sidebar_visible: bool,
    compact_sidebar_open: bool,
    collapsed_blocks: HashSet<usize>,
    active_profile: String,
    run_provider: Option<String>,
    run_model: Option<String>,
    providers: Vec<ProviderChoice>,
    profiles: Vec<String>,
    selected_provider: Option<String>,
    selected_profile: String,
    tree_signature: String,
    _subscriptions: Vec<Subscription>,
}

impl ArtistApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            InputState::new(window, cx)
                .auto_grow(1, 6)
                .submit_on_enter(true)
                .placeholder("Enter an instruction…")
        });
        let session_search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter project tree…")
                .clean_on_escape()
        });
        let workspace_tree = cx.new(|cx| TreeState::new(cx));
        let session_name = cx.new(|cx| InputState::new(window, cx).placeholder("Session name"));
        let model_override =
            cx.new(|cx| InputState::new(window, cx).placeholder("Provider default"));
        let question_notes = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Add context (optional)")
                .auto_grow(1, 4)
        });
        window.focus(&composer.read(cx).focus_handle(cx), cx);
        let subscriptions = vec![
            cx.subscribe_in(&composer, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                    this.submit(window, cx);
                }
            }),
            cx.subscribe(&session_search, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&session_name, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&model_override, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&question_notes, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        let project = std::env::current_dir()
            .ok()
            .and_then(|path| path.canonicalize().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let session_store = SessionStore::new(config_root());
        let mut app = Self {
            transcript: Transcript::new(),
            composer,
            session_search,
            workspace_tree,
            session_name,
            model_override,
            question_notes,
            status: "Ready".into(),
            session_id: None,
            session_events: Vec::new(),
            running: false,
            #[cfg(target_os = "linux")]
            host_controllers: HashMap::new(),
            #[cfg(target_os = "linux")]
            host_active_session: None,
            #[cfg(target_os = "linux")]
            host_active_lineage: None,
            #[cfg(target_os = "linux")]
            stage_surfaces: crate::stage_surface::StageSurfaces::default(),
            stage_accessibility: HashMap::new(),
            pending_questions: Vec::new(),
            question_choices: HashMap::new(),
            transcript_scroll: ScrollHandle::new(),
            follow_output: true,
            session_store,
            projects: Vec::new(),
            sessions: Vec::new(),
            session_summaries: HashMap::new(),
            session_trees: HashMap::new(),
            focus_stack: FocusStack::default(),
            focused_object: None,
            #[cfg(feature = "embedded-canvas")]
            canvas_endpoints: HashMap::new(),
            #[cfg(feature = "embedded-canvas")]
            canvas_views: HashMap::new(),
            #[cfg(feature = "embedded-canvas")]
            canvas_accessibility: HashMap::new(),
            project,
            run_baseline: HashMap::new(),
            session_changed_files: HashSet::new(),
            sidebar_visible: true,
            compact_sidebar_open: false,
            collapsed_blocks: HashSet::new(),
            active_profile: "default".into(),
            run_provider: None,
            run_model: None,
            providers: Vec::new(),
            profiles: Vec::new(),
            selected_provider: None,
            selected_profile: "default".into(),
            tree_signature: String::new(),
            _subscriptions: subscriptions,
        };
        app.refresh_workspace();
        app.refresh_configuration(window, cx);
        app
    }

    fn refresh_configuration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.providers = artist_config::providers(&config_root()).unwrap_or_default();
        self.profiles = artist_config::profiles(&config_root(), &self.project);
        if self.selected_provider.as_deref().is_none_or(|selected| {
            !self
                .providers
                .iter()
                .any(|provider| provider.id == selected)
        }) {
            self.selected_provider = self
                .providers
                .iter()
                .find(|provider| provider.is_default)
                .or_else(|| self.providers.first())
                .map(|provider| provider.id.clone());
        }
        if !self.profiles.contains(&self.selected_profile) {
            self.selected_profile = "default".into();
        }
        let model = self
            .selected_provider
            .as_deref()
            .and_then(|id| self.providers.iter().find(|provider| provider.id == id))
            .and_then(|provider| provider.model.clone())
            .unwrap_or_default();
        self.model_override.update(cx, |state, cx| {
            state.set_value(model, window, cx);
        });
    }

    fn refresh_workspace(&mut self) {
        let all = self.session_store.list().unwrap_or_default();
        self.projects = load_recent_projects();
        for project in std::iter::once(self.project.clone())
            .chain(all.iter().map(|session| session.project.clone()))
        {
            if !self.projects.contains(&project) {
                self.projects.push(project);
            }
        }
        self.sessions = all
            .into_iter()
            .filter(|session| session.project == self.project)
            .collect();
        self.sessions
            .sort_by_key(|session| std::cmp::Reverse(session.created_at_ms));
        self.session_summaries = self
            .sessions
            .iter()
            .filter_map(|session| {
                self.session_store
                    .peek(&session.id)
                    .ok()
                    .map(|(_, events)| (session.id.clone(), summarize_session(session, &events)))
            })
            .collect();
        self.session_trees = self
            .sessions
            .iter()
            .filter_map(|session| {
                self.session_store
                    .peek(&session.id)
                    .ok()
                    .map(|(_, events)| {
                        (
                            session.id.clone(),
                            WorkspaceProjection::from_envelopes(&events).visible_tree(),
                        )
                    })
            })
            .collect();
        sort_sessions_by_activity(&mut self.sessions, &self.session_summaries);
        self.refresh_changes();
    }

    fn refresh_changes(&mut self) {
        if self.running || !self.run_baseline.is_empty() {
            for change in git_changes(&self.project) {
                let current = git_diff(&self.project, &change.path);
                if self.run_baseline.get(&change.path) != Some(&current) {
                    self.session_changed_files.insert(change.path);
                }
            }
        }
    }

    fn refresh_session_metadata(&mut self) {
        let Some(id) = self.session_id.as_deref() else {
            return;
        };
        let Ok((_, events)) = self.session_store.peek(id) else {
            return;
        };
        self.active_profile =
            artist_session::active_profile(&events).unwrap_or_else(|| "default".into());
        let run = events
            .iter()
            .filter_map(|envelope| match envelope.event() {
                SessionEvent::RunStarted(run) => Some((run.provider, run.model)),
                _ => None,
            })
            .next_back();
        (self.run_provider, self.run_model) = run
            .map(|(provider, model)| (Some(provider), Some(model)))
            .unwrap_or_default();
        self.session_events = events;
    }

    fn select_project(&mut self, project: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.running || !project.is_dir() {
            return;
        }
        self.project = project.canonicalize().unwrap_or(project);
        remember_project(&self.project);
        self.transcript = Transcript::new();
        self.session_id = None;
        self.session_events.clear();
        self.active_profile = "default".into();
        self.run_provider = None;
        self.run_model = None;
        self.run_baseline.clear();
        self.session_changed_files.clear();
        self.status = "Ready".into();
        self.refresh_workspace();
        self.refresh_configuration(window, cx);
    }

    fn select_session(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        match self.session_store.peek(id) {
            Ok((session, events)) => {
                self.focus_stack = FocusStack::default();
                self.focused_object = None;
                self.run_baseline.clear();
                self.session_changed_files.clear();
                self.active_profile =
                    artist_session::active_profile(&events).unwrap_or_else(|| "default".into());
                let run = events
                    .iter()
                    .filter_map(|envelope| match envelope.event() {
                        SessionEvent::RunStarted(run) => Some((run.provider, run.model)),
                        _ => None,
                    })
                    .next_back();
                (self.run_provider, self.run_model) = run
                    .map(|(provider, model)| (Some(provider), Some(model)))
                    .unwrap_or_default();
                self.selected_profile = self.active_profile.clone();
                let label = session.label.clone().unwrap_or_default();
                self.session_name
                    .update(cx, |state, cx| state.set_value(label, window, cx));
                if let Some(model) = self.run_model.clone() {
                    self.model_override
                        .update(cx, |state, cx| state.set_value(model, window, cx));
                }
                self.project = session.project;
                self.session_id = Some(session.id);
                self.session_events = events.clone();
                self.transcript = transcript_from_replay(artist_session::replay_for_ui(&events));
                self.follow_output = true;
                self.status = "Resumed".into();
                self.refresh_workspace();
                if self.follow_output {
                    self.transcript_scroll.scroll_to_bottom();
                }
            }
            Err(error) => self.status = format!("Error opening session: {error}"),
        }
    }

    fn choose_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open an Artist project".into()),
        });
        cx.spawn_in(window, async move |this, window| {
            let path = receiver.await.ok()?.ok()??.into_iter().next()?;
            window
                .update(|window, cx| {
                    this.update(cx, |app, cx| {
                        app.select_project(path, window, cx);
                        cx.notify();
                    })
                    .ok();
                })
                .ok()
        })
        .detach();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.composer.read(cx).value().trim().to_owned();
        if input.is_empty() {
            return;
        }
        if self.running {
            #[cfg(target_os = "linux")]
            let active_lineage = self
                .host_active_lineage
                .clone()
                .unwrap_or_else(|| "main".into());
            #[cfg(target_os = "linux")]
            if let Some(session) = self.host_active_session.as_ref()
                && self
                    .host_controllers
                    .get(session)
                    .is_some_and(|controller| {
                        controller
                            .send(artist_session_host::HostCommand::Steer {
                                lineage: active_lineage.clone(),
                                text: input.clone(),
                            })
                            .is_ok()
                    })
            {
                self.transcript.push_user(&input);
                self.status = "Steering will be delivered at the next safe boundary".into();
                self.composer
                    .update(cx, |state, cx| state.set_value("", window, cx));
                return;
            }
            self.status = "Session host control channel unavailable".into();
            return;
        }
        self.composer
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.start_prompt(input, cx);
    }

    fn queue_next_turn(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.composer.read(cx).value().trim().to_owned();
        if input.is_empty() {
            return;
        }
        #[cfg(target_os = "linux")]
        let active_lineage = self
            .host_active_lineage
            .clone()
            .unwrap_or_else(|| "main".into());
        #[cfg(target_os = "linux")]
        if let Some(session) = self.host_active_session.as_ref()
            && self
                .host_controllers
                .get(session)
                .is_some_and(|controller| {
                    controller
                        .send(artist_session_host::HostCommand::QueueNextTurn {
                            lineage: active_lineage.clone(),
                            text: input.clone(),
                        })
                        .is_ok()
                })
        {
            self.composer
                .update(cx, |state, cx| state.set_value("", window, cx));
            self.status = "Queued for next turn in session host".into();
            return;
        }
        self.status = "Session host queue unavailable".into();
    }

    fn start_prompt(&mut self, input: String, cx: &mut Context<Self>) {
        self.run_baseline = git_changes(&self.project)
            .into_iter()
            .map(|change| {
                let diff = git_diff(&self.project, &change.path);
                (change.path, diff)
            })
            .collect();
        self.transcript.push_user(&input);
        self.follow_output = true;
        self.status = "Thinking…".into();
        self.running = true;
        self.transcript_scroll.scroll_to_bottom();

        #[cfg(target_os = "linux")]
        {
            let session = match self.session_id.clone() {
                Some(session) => session,
                None => match self.session_store.create_snapshot(&self.project, None) {
                    Ok(session) => {
                        let id = session.id.clone();
                        self.session_id = Some(id.clone());
                        self.refresh_workspace();
                        id
                    }
                    Err(error) => {
                        self.receive(
                            HarnessMessage::Done(Err(format!("create session: {error:#}"))),
                            cx,
                        );
                        return;
                    }
                },
            };
            if self.start_host_prompt(session, input, cx) {
                return;
            }
            self.receive(
                HarnessMessage::Done(Err("session host unavailable".into())),
                cx,
            );
        }

        #[cfg(not(target_os = "linux"))]
        self.receive(
            HarnessMessage::Done(Err(
                "the resident Artist session host currently requires Linux".into(),
            )),
            cx,
        );
    }

    #[cfg(target_os = "linux")]
    fn start_host_prompt(
        &mut self,
        session: String,
        input: String,
        cx: &mut Context<Self>,
    ) -> bool {
        use artist_session_host::HostCommand;
        let lineage = self.composer_lineage();
        if let Some(controller) = self.host_controllers.get(&session) {
            if controller
                .send(HostCommand::Message {
                    lineage: lineage.clone(),
                    text: input.clone(),
                })
                .is_ok()
            {
                self.host_active_session = Some(session);
                self.host_active_lineage = Some(lineage);
                self.status = "Thinking… · session host".into();
                return true;
            }
            self.host_controllers.remove(&session);
        }
        let (controller, receive) = match crate::host_controller::RootController::connect(
            session.clone(),
            self.project.clone(),
        ) {
            Ok(value) => value,
            Err(error) => {
                self.status = format!("Session host unavailable: {error}");
                return false;
            }
        };
        if controller
            .send(HostCommand::Message {
                lineage: lineage.clone(),
                text: input,
            })
            .is_err()
        {
            return false;
        }
        let source_session = session.clone();
        self.host_controllers.insert(session.clone(), controller);
        self.host_active_session = Some(session);
        self.host_active_lineage = Some(lineage);
        cx.spawn(async move |this, cx| {
            loop {
                loop {
                    match receive.try_recv() {
                        Ok(message) => {
                            this.update(cx, |app, cx| {
                                app.receive_host(&source_session, message, cx);
                                cx.notify();
                            })
                            .ok();
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
            }
        })
        .detach();
        self.status = "Thinking… · session host".into();
        true
    }

    #[cfg(target_os = "linux")]
    fn composer_lineage(&self) -> String {
        self.focused_object
            .as_ref()
            .filter(|object| object.kind == artist_ui_core::ObjectKind::Agent)
            .map(|object| object.lineage.clone())
            .unwrap_or_else(|| "main".into())
    }

    #[cfg(target_os = "linux")]
    fn receive_host(
        &mut self,
        source_session: &str,
        message: crate::host_controller::ControllerMessage,
        cx: &mut Context<Self>,
    ) {
        use artist_session_host::{AttentionKind, HostCommand, HostEvent, RuntimePhase};
        let active = self.session_id.as_deref() == Some(source_session);
        match message {
            crate::host_controller::ControllerMessage::Event(
                HostEvent::StreamingDelta { event, .. },
                _,
            ) => {
                if active {
                    self.receive(HarnessMessage::Event(event), cx);
                } else {
                    self.refresh_workspace();
                }
            }
            crate::host_controller::ControllerMessage::Event(HostEvent::RuntimeState(state), _) => {
                if active {
                    match state.state {
                        RuntimePhase::Idle => {
                            self.receive(HarnessMessage::Done(Ok("Ready".into())), cx)
                        }
                        RuntimePhase::Failed | RuntimePhase::Interrupted => self.receive(
                            HarnessMessage::Done(Err(format!("session host {:?}", state.state))),
                            cx,
                        ),
                        RuntimePhase::WaitingForAnswer => {
                            self.status = "Artist needs your input".into()
                        }
                        RuntimePhase::Running => {
                            self.running = true;
                            self.status = "Thinking… · session host".into();
                        }
                        RuntimePhase::Stopping => self.status = "Stopping…".into(),
                    }
                } else {
                    self.refresh_workspace();
                }
            }
            crate::host_controller::ControllerMessage::Event(
                HostEvent::Attention {
                    kind: AttentionKind::Question,
                    message,
                    ..
                },
                _,
            ) => {
                if active {
                    if let Ok(question) = serde_json::from_str::<Question>(&message) {
                        self.receive(HarnessMessage::Question(question), cx);
                    }
                } else {
                    self.status = format!("Background session {source_session} needs input");
                }
            }
            crate::host_controller::ControllerMessage::Event(
                HostEvent::Attention { message, .. },
                _,
            )
            | crate::host_controller::ControllerMessage::Error(message) => {
                if active {
                    self.receive(HarnessMessage::Done(Err(message)), cx);
                } else {
                    self.status = format!("Background session {source_session}: {message}");
                }
            }
            crate::host_controller::ControllerMessage::Event(
                HostEvent::CanvasEndpoint {
                    lineage,
                    slug,
                    origin,
                },
                _,
            ) => {
                #[cfg(feature = "embedded-canvas")]
                self.canvas_endpoints
                    .insert((source_session.to_owned(), lineage, slug), origin);
                #[cfg(not(feature = "embedded-canvas"))]
                let _ = (lineage, slug, origin);
            }
            crate::host_controller::ControllerMessage::Event(
                HostEvent::StageExport {
                    lineage,
                    stage,
                    descriptor,
                },
                descriptors,
            ) => {
                if let Err(error) = self.stage_surfaces.export(
                    source_session.to_owned(),
                    lineage,
                    stage,
                    descriptor,
                    descriptors,
                ) {
                    self.status = format!("Stage unavailable: {error}");
                }
            }
            crate::host_controller::ControllerMessage::Event(
                HostEvent::StagePresent {
                    lineage,
                    stage,
                    buffer_index,
                    damage,
                },
                _,
            ) => {
                let release = HostCommand::FrameRelease {
                    lineage: lineage.clone(),
                    stage: stage.clone(),
                    buffer_index,
                };
                let Some(controller) = self.host_controllers.get(source_session) else {
                    self.status = "Stage host disconnected before presentation".into();
                    return;
                };
                let completion = controller.completion_callback(release.clone());
                if let Err(error) = self.stage_surfaces.present(
                    source_session,
                    &lineage,
                    &stage,
                    buffer_index,
                    damage,
                    completion,
                ) {
                    let _ = controller.send(release);
                    self.status = format!("Stage unavailable: {error}");
                } else if active {
                    self.status = "Stage connected · zero-copy Vulkan DMA-BUF".into();
                }
                cx.notify();
            }
            crate::host_controller::ControllerMessage::Event(
                HostEvent::AccessibilityPatch {
                    lineage,
                    object,
                    patch,
                },
                _,
            ) => match serde_json::from_value::<StageAccessibilityTree>(patch) {
                Ok(tree) => {
                    self.stage_accessibility
                        .insert((source_session.to_owned(), lineage, object), tree);
                    cx.notify();
                }
                Err(error) => {
                    self.status = format!("Stage accessibility unavailable: {error}");
                }
            },
            crate::host_controller::ControllerMessage::Event(_, _) => {}
        }
    }

    fn receive(&mut self, message: HarnessMessage, _cx: &mut Context<Self>) {
        match message {
            HarnessMessage::Event(event) => {
                let changed_files = matches!(&event, PromptEvent::ToolResult { .. });
                self.transcript.apply(&event);
                if changed_files {
                    self.refresh_changes();
                    self.refresh_session_metadata();
                }
                if self.follow_output {
                    self.transcript_scroll.scroll_to_bottom();
                }
            }
            HarnessMessage::Question(question) => {
                if !self
                    .pending_questions
                    .iter()
                    .any(|pending| pending.id == question.id)
                {
                    self.status = "Artist needs your input".into();
                    self.pending_questions.push(question);
                }
            }
            HarnessMessage::Done(result) => {
                self.transcript.close_turn();
                self.running = false;
                #[cfg(target_os = "linux")]
                {
                    self.host_active_session = None;
                    self.host_active_lineage = None;
                }
                self.pending_questions.clear();
                self.question_choices.clear();
                self.status = result.unwrap_or_else(|error| format!("Error: {error}"));
                if self.follow_output {
                    self.transcript_scroll.scroll_to_bottom();
                }
                self.refresh_workspace();
                self.refresh_session_metadata();
            }
        }
    }

    fn stop(&mut self) {
        #[cfg(target_os = "linux")]
        {
            let active_lineage = self
                .host_active_lineage
                .clone()
                .unwrap_or_else(|| "main".into());
            if let Some(session) = self.host_active_session.as_ref()
                && self
                    .host_controllers
                    .get(session)
                    .is_some_and(|controller| {
                        controller
                            .send(artist_session_host::HostCommand::Stop {
                                lineage: active_lineage.clone(),
                            })
                            .is_ok()
                    })
            {
                self.status = "Stopping…".into();
                return;
            }
        }
        self.status = "Session host control channel unavailable".into();
    }

    fn open_stage_viewer(&mut self, root_session: String, lineage: String, stage: String) {
        #[cfg(target_os = "linux")]
        if let Some(controller) = self.host_controllers.get(&root_session)
            && controller
                .send(artist_session_host::HostCommand::StageInput {
                    lineage,
                    stage,
                    input: serde_json::json!({ "mode": "watch" }),
                })
                .is_ok()
        {
            self.status = "Opening standalone stage viewer…".into();
            return;
        }
        self.status = "Stage viewer is unavailable because its session host is disconnected".into();
    }

    fn activate_stage_accessibility(
        &mut self,
        root_session: String,
        lineage: String,
        stage: String,
        surface: String,
        binding: String,
        label: String,
        action: Option<String>,
    ) {
        #[cfg(target_os = "linux")]
        if let Some(controller) = self.host_controllers.get(&root_session) {
            let step = match action {
                Some(action) => serde_json::json!({
                    "invoke": {
                        "anchor": binding.clone(),
                        "label": label.clone(),
                        "action": action
                    }
                }),
                None => serde_json::json!({
                    "click": { "anchor": binding.clone(), "label": label.clone() }
                }),
            };
            let input = serde_json::json!({
                "mode": "do",
                "surface": surface,
                "steps": [step],
                "settle": { "until": "quiet", "timeoutMs": 3000 },
                "expect": { "still": { "anchor": binding, "label": label } }
            });
            if controller
                .send(artist_session_host::HostCommand::StageInput {
                    lineage,
                    stage,
                    input,
                })
                .is_ok()
            {
                self.status = "Stage accessibility action sent".into();
                return;
            }
        }
        self.status =
            "Stage accessibility action is unavailable because its host is disconnected".into();
    }

    fn toggle_question_choice(&mut self, question_id: &str, label: &str, multi_select: bool) {
        let selected = self
            .question_choices
            .entry(question_id.to_owned())
            .or_default();
        if !multi_select {
            selected.clear();
        }
        if !selected.remove(label) {
            selected.insert(label.to_owned());
        }
    }

    fn answer_question(
        &mut self,
        question_id: String,
        dismissed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut selected = if dismissed {
            Vec::new()
        } else {
            self.question_choices
                .remove(&question_id)
                .unwrap_or_default()
                .into_iter()
                .collect()
        };
        selected.sort();
        let notes = self.question_notes.read(cx).value().trim().to_owned();
        let answer = Answer {
            question_id: question_id.clone(),
            selected,
            notes: (!notes.is_empty()).then_some(notes),
        };
        #[cfg(target_os = "linux")]
        let active_lineage = self
            .host_active_lineage
            .clone()
            .unwrap_or_else(|| "main".into());
        #[cfg(target_os = "linux")]
        let host_sent = self.host_active_session.as_ref().is_some_and(|session| {
            self.host_controllers
                .get(session)
                .is_some_and(|controller| {
                    controller
                        .send(artist_session_host::HostCommand::Answer {
                            lineage: active_lineage.clone(),
                            answer: answer.clone(),
                        })
                        .is_ok()
                })
        });
        #[cfg(not(target_os = "linux"))]
        let host_sent = false;
        if host_sent {
            self.pending_questions
                .retain(|question| question.id != question_id);
            self.question_notes
                .update(cx, |state, cx| state.set_value("", window, cx));
            self.status = "Answer delivered".into();
        } else {
            self.status = "Could not deliver answer".into();
        }
    }

    fn jump_to_latest(&mut self) {
        self.follow_output = true;
        self.transcript_scroll.scroll_to_bottom();
    }

    fn promote_object(
        &mut self,
        object: FocusableObject,
        #[cfg_attr(not(feature = "embedded-canvas"), allow(unused_variables))] window: &mut Window,
        #[cfg_attr(not(feature = "embedded-canvas"), allow(unused_variables))] cx: &mut Context<
            Self,
        >,
    ) {
        if self.focus_stack.current() == Some(&object.id) {
            return;
        }
        self.focus_stack.push_context(
            object.id.clone(),
            self.transcript_scroll.top_item(),
            Some("composer".into()),
        );
        #[cfg(feature = "embedded-canvas")]
        if object.kind == artist_ui_core::ObjectKind::Canvas {
            self.ensure_canvas_view(&object, window, cx);
            if let Some(webview) = self.canvas_views.get(&object.id) {
                webview.update(cx, |webview, _| webview.set_interactive(true));
            }
        }
        self.focused_object = Some(object);
        self.transcript_scroll.scroll_to_item(0);
    }

    #[cfg(feature = "embedded-canvas")]
    fn ensure_canvas_view(
        &mut self,
        object: &FocusableObject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_webview::events::{
            AccessibilityUpdateEvent, AddressChangedEvent, BeforePopupEvent,
        };

        if self.canvas_views.contains_key(&object.id) {
            return;
        }
        let Some(origin) = self
            .canvas_endpoints
            .get(&(
                object.root_session.clone(),
                object.lineage.clone(),
                object.durable_id.clone(),
            ))
            .cloned()
        else {
            return;
        };
        if !crate::canvas_security::is_authenticated_local_canvas_origin(&origin) {
            self.status =
                "Canvas endpoint rejected: expected an authenticated loopback origin".into();
            return;
        }
        let webview = gpui_webview::WebView::new(&origin, window, cx);
        window
            .subscribe(&webview, cx, {
                let webview = webview.clone();
                let origin = origin.clone();
                move |_, event: &AddressChangedEvent, _, cx| {
                    if event.url != "about:blank"
                        && !crate::canvas_security::same_origin(&origin, &event.url)
                    {
                        cx.open_url(&event.url);
                        webview.read(cx).browser().load_url(&origin);
                    }
                }
            })
            .detach();
        window
            .subscribe(&webview, cx, move |_, event: &BeforePopupEvent, _, cx| {
                cx.open_url(&event.url);
            })
            .detach();
        cx.subscribe(&webview, {
            let object_id = object.id.clone();
            move |this, _, event: &AccessibilityUpdateEvent, cx| {
                this.canvas_accessibility
                    .entry(object_id.clone())
                    .or_default()
                    .apply(&event.update);
                cx.notify();
            }
        })
        .detach();
        self.canvas_views.insert(object.id.clone(), webview);
    }

    fn pop_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        #[cfg(feature = "embedded-canvas")]
        if let Some(object) = self.focused_object.as_ref()
            && object.kind == artist_ui_core::ObjectKind::Canvas
            && let Some(webview) = self.canvas_views.get(&object.id)
        {
            webview.update(cx, |webview, _| webview.set_interactive(false));
        }
        let Some(context) = self.focus_stack.pop() else {
            return false;
        };
        self.focused_object = self.focus_stack.current().and_then(|id| {
            self.session_trees
                .values()
                .flatten()
                .find_map(|root| find_workspace_object(root, id))
        });
        self.transcript_scroll.scroll_to_item(context.scroll_anchor);
        if context.keyboard_focus.as_deref() == Some("composer") {
            window.focus(&self.composer.read(cx).focus_handle(cx), cx);
        }
        true
    }

    fn new_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        self.transcript = Transcript::new();
        self.focus_stack = FocusStack::default();
        self.focused_object = None;
        self.session_id = None;
        self.session_events.clear();
        self.active_profile = "default".into();
        self.run_provider = None;
        self.run_model = None;
        self.status = "Ready".into();
        self.run_baseline.clear();
        self.session_changed_files.clear();
        self.composer
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.session_name
            .update(cx, |state, cx| state.set_value("", window, cx));
        window.focus(&self.composer.read(cx).focus_handle(cx), cx);
        self.refresh_workspace();
    }

    #[allow(dead_code)]
    fn delete_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Some(id) = self.session_id.clone() else {
            return;
        };
        let receiver = window.prompt(
            PromptLevel::Warning,
            "Delete this session?",
            Some("Its transcript and event log will be permanently removed."),
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, window| {
            if receiver.await.ok()? != 0 {
                return None;
            }
            window
                .update(|window, cx| {
                    this.update(cx, |app, cx| {
                        match app.session_store.remove(&id) {
                            Ok(()) => {
                                app.transcript = Transcript::new();
                                app.session_id = None;
                                app.session_events.clear();
                                app.active_profile = "default".into();
                                app.run_provider = None;
                                app.run_model = None;
                                app.session_name
                                    .update(cx, |state, cx| state.set_value("", window, cx));
                                app.status = "Session deleted".into();
                                app.refresh_workspace();
                            }
                            Err(error) => app.status = format!("Could not delete session: {error}"),
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .ok()
        })
        .detach();
    }

    #[allow(dead_code)]
    fn rename_session(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.session_id.as_deref() else {
            return;
        };
        let name = self.session_name.read(cx).value().to_string();
        match self.session_store.rename(id, Some(&name)) {
            Ok(_) => {
                self.status = "Session renamed".into();
                self.refresh_workspace();
            }
            Err(error) => self.status = format!("Could not rename session: {error}"),
        }
    }

    #[allow(dead_code)]
    fn toggle_session_pinned(&mut self) {
        let Some(id) = self.session_id.clone() else {
            return;
        };
        let pinned = self
            .sessions
            .iter()
            .find(|session| session.id == id)
            .is_some_and(|session| session.pinned);
        match self.session_store.set_pinned(&id, !pinned) {
            Ok(_) => {
                self.status = if pinned {
                    "Session unpinned".into()
                } else {
                    "Session pinned".into()
                };
                self.refresh_workspace();
            }
            Err(error) => self.status = format!("Could not update session: {error}"),
        }
    }

    #[allow(dead_code)]
    fn fork_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Some(parent_id) = self.session_id.clone() else {
            return;
        };
        let result = self
            .session_store
            .peek(&parent_id)
            .and_then(|(parent, events)| {
                let up_to_seq = events.last().map_or(0, |event| event.seq);
                let fork = self.session_store.fork_snapshot(&parent_id, up_to_seq)?;
                let label = format!("{} (fork)", parent.label.as_deref().unwrap_or("Untitled"));
                let fork = self.session_store.rename(&fork.id, Some(&label))?;
                Ok(fork.id)
            });
        match result {
            Ok(id) => {
                self.refresh_workspace();
                self.select_session(&id, window, cx);
                self.status = "Forked into a new session".into();
            }
            Err(error) => self.status = format!("Could not fork session: {error}"),
        }
    }

    fn toggle_sidebar(&mut self, window: &Window) {
        if window.viewport_size().width < px(980.) {
            self.compact_sidebar_open = !self.compact_sidebar_open;
        } else {
            self.sidebar_visible = !self.sidebar_visible;
        }
    }

    fn action_new_conversation(
        &mut self,
        _: &NewConversation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_conversation(window, cx);
        cx.notify();
    }

    fn action_open_project(
        &mut self,
        _: &OpenProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.choose_project(window, cx);
    }

    fn action_toggle_sidebar(
        &mut self,
        _: &ToggleSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_sidebar(window);
        cx.notify();
    }

    fn action_quick_open(&mut self, _: &QuickOpen, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_visible = true;
        self.compact_sidebar_open = window.viewport_size().width < px(980.);
        window.focus(&self.session_search.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    fn action_focus_composer(
        &mut self,
        _: &FocusComposer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.composer.read(cx).focus_handle(cx), cx);
    }

    fn action_stop_run(&mut self, _: &StopRun, _: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            self.stop();
            cx.notify();
        }
    }

    fn action_dismiss_overlay(
        &mut self,
        _: &DismissOverlay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.compact_sidebar_open {
            self.compact_sidebar_open = false;
            window.focus(&self.composer.read(cx).focus_handle(cx), cx);
            cx.notify();
        } else if self.pop_focus(window, cx) {
            cx.notify();
        }
    }

    fn sync_workspace_tree(&mut self, cx: &mut Context<Self>) {
        let query = self.session_search.read(cx).value().trim().to_lowercase();
        let signature = workspace_tree_signature(
            &self.project,
            &self.sessions,
            &self.session_summaries,
            &self.session_trees,
            self.session_id.as_deref(),
            self.running,
            !self.pending_questions.is_empty(),
            &query,
        );
        if signature == self.tree_signature {
            return;
        }
        self.tree_signature = signature;
        let items = workspace_tree_items(
            &self.project,
            &self.sessions,
            &self.session_summaries,
            &self.session_trees,
            &query,
        );
        let selected = self
            .focused_object
            .as_ref()
            .map(|object| format!("object|{}|{}", object.root_session, object.id.as_str()))
            .or_else(|| self.session_id.as_ref().map(|id| format!("session|{id}")));
        self.workspace_tree.update(cx, |state, cx| {
            state.set_items(items, cx);
            if let Some(selected) = selected {
                state.set_selected_item(Some(&TreeItem::new(selected, "")), cx);
            }
        });
    }

    fn sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let app = cx.entity().downgrade();
        let tree_view = tree(
            &self.workspace_tree,
            move |ix, entry, _selected, _window, cx| {
                let item = entry.item();
                let id = item.id.to_string();
                let depth = entry.depth();
                let expanded = entry.is_expanded();
                let folder = entry.is_folder();
                let project_row = id.starts_with("project|");
                let session_row = id.starts_with("session|");
                let mut title = item.label.to_string();
                let mut preview = None;
                let mut state = None;
                let mut active = false;
                if let Some(app) = app.upgrade() {
                    let this = app.read(cx);
                    if let Some(session_id) = id.strip_prefix("session|") {
                        if let Some(session) = this
                            .sessions
                            .iter()
                            .find(|session| session.id == session_id)
                        {
                            let roots = this
                                .session_trees
                                .get(session_id)
                                .map(Vec::as_slice)
                                .unwrap_or_default();
                            title = session_identity(session, roots);
                            preview = this
                                .session_summaries
                                .get(session_id)
                                .map(|summary| summary.preview.clone())
                                .filter(|preview| !preview.trim().is_empty());
                            active = this.session_id.as_deref() == Some(session_id)
                                && this.focused_object.is_none();
                            state = Some(if active && !this.pending_questions.is_empty() {
                                "needs_input"
                            } else if active && this.running {
                                "running"
                            } else if this
                                .session_summaries
                                .get(session_id)
                                .is_some_and(|summary| summary.failed)
                            {
                                "failed"
                            } else {
                                "completed"
                            });
                        }
                    } else if let Some(rest) = id.strip_prefix("object|") {
                        if let Some((session_id, object_id)) = rest.split_once('|') {
                            if let Some(object) = this
                                .session_trees
                                .get(session_id)
                                .and_then(|roots| find_workspace_object_by_str(roots, object_id))
                            {
                                title = object.title.clone();
                                active = this
                                    .focused_object
                                    .as_ref()
                                    .is_some_and(|focused| focused.id == object.id);
                                state = Some(match object.state {
                                    artist_ui_core::ObjectState::Active => "running",
                                    artist_ui_core::ObjectState::Failed => "failed",
                                    artist_ui_core::ObjectState::Completed => "completed",
                                    artist_ui_core::ObjectState::Interrupted => "failed",
                                    artist_ui_core::ObjectState::Dormant => "ready",
                                });
                            }
                        }
                    }
                }

                let disclosure = if folder {
                    Icon::new(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size_3()
                    .text_color(cx.theme().sidebar_foreground.opacity(0.68))
                    .into_any_element()
                } else {
                    div().w(px(12.)).flex_none().into_any_element()
                };
                let status = state.map(|state| match state {
                    "running" => Spinner::new()
                        .xsmall()
                        .color(cx.theme().success)
                        .into_any_element(),
                    "needs_input" => div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .font_medium()
                        .text_color(cx.theme().warning)
                        .child(Icon::new(IconName::TriangleAlert).size_3())
                        .child("Needs input")
                        .into_any_element(),
                    "failed" => Icon::new(IconName::Close)
                        .text_color(cx.theme().danger)
                        .size_3p5()
                        .into_any_element(),
                    "completed" => Icon::new(IconName::Check)
                        .text_color(cx.theme().success)
                        .size_3p5()
                        .into_any_element(),
                    _ => Icon::new(IconName::CircleCheck)
                        .text_color(cx.theme().sidebar_foreground.opacity(0.45))
                        .size_3p5()
                        .into_any_element(),
                });
                let has_preview = preview.is_some();
                let mut row_label = preview
                    .as_deref()
                    .map(|preview| format!("{title}. {preview}"))
                    .unwrap_or_else(|| title.clone());
                if state == Some("needs_input") {
                    row_label.push_str(". Needs input");
                }
                let click_id = id.clone();
                let app_click = app.clone();
                // gpui-component's Tree is backed by a uniform list. Keep one
                // fixed row extent while reserving a second line for dialogue
                // previews on root-agent rows.
                ListItem::new(ix)
                    .h(px(48.))
                    .min_h(px(48.))
                    .px_1()
                    .when(project_row, |item| {
                        item.bg(cx.theme().list_head)
                            .border_b_1()
                            .border_color(cx.theme().sidebar_border)
                    })
                    .when(active, |item| {
                        item.bg(cx.theme().sidebar_accent)
                            .text_color(cx.theme().sidebar_accent_foreground)
                            .border_l_2()
                            .border_color(cx.theme().list_active_border)
                    })
                    .child(
                        div()
                            .id(("workspace-tree-row", ix))
                            .role(Role::ListItem)
                            .aria_label(row_label)
                            .w_full()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .items_stretch()
                            .child(tree_parenthood_guides(depth, cx))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .h_full()
                                    .px_1()
                                    .py_1()
                                    .flex()
                                    .flex_col()
                                    .justify_center()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex()
                                            .items_center()
                                            .gap_1p5()
                                            .child(disclosure)
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .flex_1()
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                                    .when(project_row, |label| label.font_bold())
                                                    .when(session_row, |label| {
                                                        label.font_semibold()
                                                    })
                                                    .child(title),
                                            )
                                            .when_some(status, |row, status| row.child(status)),
                                    )
                                    .when_some(preview, |column, preview| {
                                        column.child(
                                            div()
                                                .min_w_0()
                                                .pl(px(18.))
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .text_xs()
                                                .text_color(if active {
                                                    cx.theme()
                                                        .sidebar_accent_foreground
                                                        .opacity(0.68)
                                                } else {
                                                    cx.theme().sidebar_foreground.opacity(0.56)
                                                })
                                                .child(preview),
                                        )
                                    })
                                    .when(!has_preview && !project_row, |column| column.text_sm()),
                            ),
                    )
                    .on_click(move |_, window, cx| {
                        let Some(app) = app_click.upgrade() else {
                            return;
                        };
                        app.update(cx, |this, cx| {
                            if let Some(session_id) = click_id.strip_prefix("session|") {
                                this.select_session(session_id, window, cx);
                            } else if let Some(rest) = click_id.strip_prefix("object|") {
                                if let Some((session_id, object_id)) = rest.split_once('|') {
                                    if let Some(object) =
                                        this.session_trees.get(session_id).and_then(|roots| {
                                            find_workspace_object_by_str(roots, object_id)
                                        })
                                    {
                                        this.promote_object(object, window, cx);
                                    }
                                }
                            }
                            cx.notify();
                        });
                    })
            },
        );

        div()
            .id("workspace-sidebar")
            .role(Role::Navigation)
            .aria_label("Project and object tree")
            .size_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(
                div()
                    .h(px(40.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(cx.theme().sidebar)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.session_search).w_full()),
                    )
                    .child(
                        Button::new("open-project")
                            .icon(IconName::FolderOpen)
                            .compact()
                            .ghost()
                            .tooltip("Open project")
                            .disabled(self.running)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.choose_project(window, cx);
                            })),
                    ),
            )
            .child(Separator::horizontal())
            .child(
                div()
                    .id("workspace-tree")
                    .flex_1()
                    .min_h_0()
                    .child(tree_view),
            )
    }

    fn question_view(&self, cx: &Context<Self>) -> AnyElement {
        let Some(question) = self.pending_questions.first() else {
            return div().into_any_element();
        };
        let question_id = question.id.clone();
        let selected = self
            .question_choices
            .get(&question.id)
            .cloned()
            .unwrap_or_default();
        div()
            .id("pending-question")
            .role(Role::Dialog)
            .aria_label(format!("{}: {}", question.header, question.question))
            .w_full()
            .max_w(px(820.))
            .mx_auto()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().warning)
            .bg(cx.theme().warning.opacity(0.08))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                Alert::warning("pending-question-alert", question.question.clone()).title(
                    if question.header.is_empty() {
                        "Artist needs your input".to_owned()
                    } else {
                        question.header.clone()
                    },
                ),
            )
            .children(question.options.iter().enumerate().map(|(index, option)| {
                let id = question.id.clone();
                let label = option.label.clone();
                let multi_select = question.multi_select;
                let is_selected = selected.contains(&option.label);
                Button::new(("question-option", index))
                    .w_full()
                    .h_auto()
                    .min_h(px(48.))
                    .px_2()
                    .py_2()
                    .when(is_selected, |button| button.primary())
                    .when(!is_selected, |button| button.ghost())
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().font_bold().child(option.label.clone()))
                            .when(!option.description.is_empty(), |view| {
                                view.child(
                                    div()
                                        .text_sm()
                                        .whitespace_normal()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(option.description.clone()),
                                )
                            })
                            .when_some(option.preview.clone(), |view, preview| {
                                view.child(
                                    div()
                                        .p_2()
                                        .rounded_md()
                                        .bg(cx.theme().group_box)
                                        .font_family("monospace")
                                        .text_xs()
                                        .whitespace_normal()
                                        .child(preview),
                                )
                            }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_question_choice(&id, &label, multi_select);
                        cx.notify();
                    }))
            }))
            .child(Input::new(&self.question_notes).w_full())
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("dismiss-question")
                            .label("Dismiss")
                            .ghost()
                            .on_click({
                                let id = question_id.clone();
                                cx.listener(move |this, _, window, cx| {
                                    this.answer_question(id.clone(), true, window, cx);
                                    cx.notify();
                                })
                            }),
                    )
                    .child(
                        Button::new("answer-question")
                            .label("Answer")
                            .primary()
                            .disabled(selected.is_empty())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.answer_question(question_id.clone(), false, window, cx);
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn block_view(&self, block: &Block, index: usize, cx: &Context<Self>) -> AnyElement {
        match block {
            Block::Message {
                role,
                source,
                lines: _,
            } => {
                let user = *role == TranscriptRole::User;
                let user_copy = source.clone();
                let assistant_copy = source.clone();
                div()
                    .id(("message", index))
                    .role(Role::Article)
                    .aria_label(if user {
                        "Your message"
                    } else {
                        "Artist response"
                    })
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when(user, |view| {
                        view.p_3()
                            .rounded_md()
                            .bg(cx.theme().accent.opacity(0.12))
                            .border_l_2()
                            .border_color(cx.theme().accent)
                    })
                    .when(!user, |view| view.px_1().py_2())
                    .when(user, |view| {
                        view.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_xs()
                                .font_medium()
                                .text_color(cx.theme().accent)
                                .child("YOU")
                                .child(
                                    Button::new(("copy-user-message", index))
                                        .icon(IconName::Copy)
                                        .compact()
                                        .ghost()
                                        .tooltip("Copy message")
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                user_copy.clone(),
                                            ));
                                        }),
                                ),
                        )
                    })
                    .child(
                        TextView::markdown(("message-markdown", index), source.clone())
                            .selectable(true)
                            .w_full(),
                    )
                    .when(!user, |view| {
                        view.child(
                            div().flex().justify_end().child(
                                Button::new(("copy-assistant-message", index))
                                    .icon(IconName::Copy)
                                    .compact()
                                    .ghost()
                                    .tooltip("Copy response")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            assistant_copy.clone(),
                                        ));
                                    }),
                            ),
                        )
                    })
                    .into_any_element()
            }
            Block::Reasoning { lines } => {
                let collapsed = self.collapsed_blocks.contains(&index);
                Collapsible::new()
                    .open(!collapsed)
                    .w_full()
                    .pl_3()
                    .border_l_2()
                    .border_color(cx.theme().info)
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        Button::new(("reasoning-toggle", index))
                            .icon(if collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            })
                            .label("Reasoning")
                            .compact()
                            .ghost()
                            .tooltip(if collapsed {
                                "Show reasoning"
                            } else {
                                "Hide reasoning"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.collapsed_blocks.remove(&index) {
                                    this.collapsed_blocks.insert(index);
                                }
                                cx.notify();
                            })),
                    )
                    .content(
                        div().pt_1().flex().flex_col().gap_1().children(
                            lines.iter().enumerate().map(|(line_index, line)| {
                                Self::line_view(line, index, line_index, cx)
                            }),
                        ),
                    )
                    .into_any_element()
            }
            Block::Tool(tool) => {
                let destination = self
                    .session_id
                    .as_deref()
                    .and_then(|session_id| self.session_trees.get(session_id))
                    .and_then(|roots| {
                        workspace_rows(roots)
                            .into_iter()
                            .map(|(_, object)| object)
                            .find(|object| object.durable_id == tool.id)
                            .or_else(|| {
                                let expected_kind = tool_destination_kind(&tool.name);
                                workspace_rows(roots)
                                    .into_iter()
                                    .map(|(_, object)| object)
                                    .filter(|object| {
                                        expected_kind.is_none_or(|kind| object.kind == kind)
                                    })
                                    .find(|object| {
                                        tool.arguments.contains(&object.title)
                                            || tool.arguments.contains(&object.durable_id)
                                            || object.title.contains(&tool.name)
                                    })
                            })
                    });
                let state = match tool.status {
                    ToolStatus::Requested => "requested",
                    ToolStatus::Running => "running",
                    ToolStatus::Done => "completed",
                    ToolStatus::Failed => "failed",
                };
                let line_count = tool
                    .result
                    .as_deref()
                    .map(|result| result.lines().count())
                    .unwrap_or_default();
                let command = tool_portal_hint(&tool.name, &tool.arguments);
                let destination_title = destination
                    .as_ref()
                    .map(|object| object.title.clone())
                    .unwrap_or_else(|| tool.name.clone());
                let promoted = destination.clone();
                let portal_accent = match destination.as_ref().map(|object| object.kind) {
                    Some(artist_ui_core::ObjectKind::Task) => cx.theme().secondary_active,
                    Some(artist_ui_core::ObjectKind::Canvas) => cx.theme().info,
                    Some(artist_ui_core::ObjectKind::Stage) => cx.theme().primary,
                    _ => cx.theme().border,
                };
                let state_label = match state {
                    "requested" => "Requested",
                    "running" => "Running",
                    "failed" => "Failed",
                    _ => "Completed",
                };
                let result_hint = match line_count {
                    0 => state_label.to_owned(),
                    1 => format!("{state_label} · 1 line"),
                    lines => format!("{state_label} · {lines} lines"),
                };
                Button::new(("tool-portal", index))
                    .w_full()
                    .h_auto()
                    .min_h(px(58.))
                    .px_3()
                    .py_2()
                    .ghost()
                    .disabled(destination.is_none())
                    .border_l_2()
                    .border_color(portal_accent)
                    .rounded_sm()
                    .bg(cx.theme().group_box.opacity(0.58))
                    .text_color(cx.theme().group_box_foreground)
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .font_semibold()
                                            .child(destination_title),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .font_family("monospace")
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(command),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(result_hint)
                                    .child(
                                        Icon::new(IconName::ChevronRight)
                                            .size_3p5()
                                            .text_color(cx.theme().link),
                                    ),
                            ),
                    )
                    .when_some(promoted, |button, object| {
                        button.on_click(cx.listener(move |this, _, window, cx| {
                            this.promote_object(object.clone(), window, cx);
                            cx.notify();
                        }))
                    })
                    .into_any_element()
            }
            Block::Notice(notice) => Alert::warning(("notice", index), notice.detail.clone())
                .title(notice.title.clone())
                .w_full()
                .into_any_element(),
            Block::Subagent(agent) => {
                let destination = self
                    .session_id
                    .as_deref()
                    .and_then(|session_id| self.session_trees.get(session_id))
                    .and_then(|roots| {
                        workspace_rows(roots)
                            .into_iter()
                            .map(|(_, object)| object)
                            .filter(|object| object.kind == artist_ui_core::ObjectKind::Agent)
                            .find(|object| {
                                object.durable_id.contains(&agent.id)
                                    || object.lineage.contains(&agent.id)
                                    || object.detail.as_deref() == Some(agent.prompt.as_str())
                                    || object.title.eq_ignore_ascii_case(&agent.role)
                                    || object.lineage.contains(&agent.role)
                            })
                    });
                let promoted = destination.clone();
                let title = destination
                    .as_ref()
                    .map(|object| object.title.clone())
                    .unwrap_or_else(|| agent.role.clone());
                Button::new(("subagent-portal", index))
                    .w_full()
                    .h_auto()
                    .min_h(px(58.))
                    .px_3()
                    .py_2()
                    .ghost()
                    .disabled(destination.is_none())
                    .border_l_2()
                    .border_color(cx.theme().info)
                    .rounded_sm()
                    .bg(cx.theme().group_box.opacity(0.58))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(div().font_semibold().child(title))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(agent.prompt.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(cx.theme().info)
                                    .child("Conversation")
                                    .child(Icon::new(IconName::ChevronRight).size_3p5()),
                            ),
                    )
                    .when_some(promoted, |button, object| {
                        button.on_click(cx.listener(move |this, _, window, cx| {
                            this.promote_object(object.clone(), window, cx);
                            cx.notify();
                        }))
                    })
                    .into_any_element()
            }
        }
    }

    fn turn_view(
        &self,
        turn_index: usize,
        start: usize,
        end: usize,
        cx: &Context<Self>,
    ) -> AnyElement {
        let active = self.running && end == self.transcript.blocks().len();
        div()
            .id(("turn", turn_index))
            .role(Role::Article)
            .aria_label(format!(
                "Conversation turn {}{}",
                turn_index + 1,
                if active { ", in progress" } else { "" }
            ))
            .w_full()
            .flex()
            .flex_col()
            .gap_3()
            .pb_4()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.55))
            .children(
                self.transcript.blocks()[start..end]
                    .iter()
                    .enumerate()
                    .map(|(offset, block)| self.block_view(block, start + offset, cx)),
            )
            .when(active, |view| {
                view.child(
                    div()
                        .id("active-turn-status")
                        .role(Role::Status)
                        .aria_label("Artist is working")
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Artist is working…"),
                )
            })
            .into_any_element()
    }

    fn line_view(
        line: &InlineLine,
        block_index: usize,
        line_index: usize,
        cx: &App,
    ) -> impl IntoElement {
        div()
            .id(format!("block-{block_index}-line-{line_index}"))
            .w_full()
            .min_w_0()
            .flex()
            .flex_wrap()
            .whitespace_normal()
            .font_family(if line.code {
                "monospace"
            } else {
                ".SystemUIFont"
            })
            .children(line.inlines.iter().enumerate().map(|(run_index, run)| {
                let color = match run.kind {
                    InlineKind::Prose => cx.theme().foreground,
                    InlineKind::Code | InlineKind::FenceDelimiter => cx.theme().primary,
                    InlineKind::StructuralMarker => cx.theme().success,
                    InlineKind::Syntax(TokenKind::Comment) => cx.theme().muted_foreground,
                    InlineKind::Syntax(TokenKind::StringLit | TokenKind::Number) => {
                        cx.theme().warning
                    }
                    InlineKind::Syntax(TokenKind::Keyword) => cx.theme().danger,
                    InlineKind::Syntax(TokenKind::Function) => cx.theme().success,
                    InlineKind::Syntax(TokenKind::Type) => cx.theme().primary,
                    InlineKind::Syntax(TokenKind::Plain) => cx.theme().foreground,
                };
                div()
                    .id(format!(
                        "block-{block_index}-line-{line_index}-run-{run_index}"
                    ))
                    .min_w_0()
                    .max_w_full()
                    .whitespace_normal()
                    .text_color(color)
                    .child(run.text.clone())
            }))
    }

    fn stage_accessibility_nodes(
        &self,
        object: &FocusableObject,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        self.stage_accessibility
            .get(&(
                object.root_session.clone(),
                object.lineage.clone(),
                object.durable_id.clone(),
            ))
            .into_iter()
            .flat_map(|tree| {
                tree.nodes.iter().enumerate().map(|(index, node)| {
                    let root_session = object.root_session.clone();
                    let lineage = object.lineage.clone();
                    let stage = object.durable_id.clone();
                    let surface = tree.surface.clone();
                    let binding = node.binding.clone();
                    let action = node.actions.first().cloned();
                    let mut label = node.value.as_ref().map_or_else(
                        || node.name.clone(),
                        |value| format!("{}: {value}", node.name),
                    );
                    let mut states = Vec::new();
                    for (set, name) in [
                        (node.state.focused, "focused"),
                        (node.state.disabled, "disabled"),
                        (node.state.checked, "checked"),
                        (node.state.expanded, "expanded"),
                        (node.state.selected, "selected"),
                        (node.state.offscreen, "offscreen"),
                    ] {
                        if set {
                            states.push(name);
                        }
                    }
                    if !states.is_empty() {
                        label.push_str(&format!(" ({})", states.join(", ")));
                    }
                    let action_label = label.clone();
                    div()
                        .id(format!("stage-ax-{}-{index}", node.binding))
                        .role(stage_accessibility_role(&node.role))
                        .aria_label(label)
                        .absolute()
                        .w(px(1.))
                        .h(px(1.))
                        .overflow_hidden()
                        .when(!node.state.disabled, |view| {
                            view.on_click(cx.listener(move |this, _, _, cx| {
                                this.activate_stage_accessibility(
                                    root_session.clone(),
                                    lineage.clone(),
                                    stage.clone(),
                                    surface.clone(),
                                    binding.clone(),
                                    action_label.clone(),
                                    action.clone(),
                                );
                                cx.notify();
                            }))
                        })
                        .into_any_element()
                })
            })
            .collect()
    }

    fn inline_object_previews(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(session_id) = self.session_id.as_deref() else {
            return Vec::new();
        };
        self.session_trees
            .get(session_id)
            .map(|roots| workspace_rows(roots))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(_, object)| {
                if !matches!(
                    object.kind,
                    artist_ui_core::ObjectKind::Canvas | artist_ui_core::ObjectKind::Stage
                ) {
                    return None;
                }
                let promoted = object.clone();
                let kind = object_kind_label(object.kind);
                let state = format!("{:?}", object.state).to_lowercase();
                let (preview_bg, preview_border) = match object.kind {
                    artist_ui_core::ObjectKind::Canvas => {
                        (cx.theme().info.opacity(0.14), cx.theme().info)
                    }
                    artist_ui_core::ObjectKind::Stage => {
                        (cx.theme().primary.opacity(0.12), cx.theme().primary)
                    }
                    _ => (cx.theme().group_box, cx.theme().border),
                };
                Some(
                    Button::new(format!("inline-object-preview-{}", object.durable_id))
                        .w_full()
                        .h_auto()
                        .min_h(px(68.))
                        .px_3()
                        .py_2()
                        .ghost()
                        .border_1()
                        .border_color(preview_border)
                        .rounded_md()
                        .bg(preview_bg)
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .font_bold()
                                                .child(object.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(format!("{kind} · {state}")),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_lg()
                                        .text_color(cx.theme().link)
                                        .child(Icon::new(IconName::ArrowRight).size_4()),
                                ),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.promote_object(promoted.clone(), window, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn focused_object_view(&self, object: &FocusableObject, cx: &Context<Self>) -> AnyElement {
        #[cfg(feature = "embedded-canvas")]
        if object.kind == artist_ui_core::ObjectKind::Canvas
            && let Some(webview) = self.canvas_views.get(&object.id)
        {
            return div()
                .id("focused-canvas")
                .role(Role::Region)
                .aria_label(format!("Canvas {}", object.title))
                .size_full()
                .min_h(px(480.))
                .overflow_hidden()
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .child(webview.clone())
                .children(
                    self.canvas_accessibility
                        .get(&object.id)
                        .into_iter()
                        .flat_map(|tree| {
                            tree.nodes.values().map(|node| {
                                let label = node.value.as_ref().map_or_else(
                                    || node.name.clone(),
                                    |value| format!("{}: {value}", node.name),
                                );
                                div()
                                    .id(format!("canvas-ax-{}", node.id))
                                    .role(canvas_accessibility_role(&node.role))
                                    .aria_label(label)
                                    .absolute()
                                    .w(px(1.))
                                    .h(px(1.))
                                    .overflow_hidden()
                            })
                        }),
                )
                .into_any_element();
        }
        #[cfg(target_os = "linux")]
        if object.kind == artist_ui_core::ObjectKind::Stage
            && let Some(stage_surface) = self.stage_surfaces.current(
                &object.root_session,
                &object.lineage,
                &object.durable_id,
            )
        {
            let root_session = object.root_session.clone();
            let lineage = object.lineage.clone();
            let stage = object.durable_id.clone();
            return div()
                .id("focused-stage")
                .role(Role::Region)
                .aria_label(format!("Stage {}", object.title))
                .relative()
                .size_full()
                .min_h(px(480.))
                .overflow_hidden()
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().group_box)
                .child(
                    gpui::surface(stage_surface)
                        .object_fit(gpui::ObjectFit::Contain)
                        .size_full(),
                )
                .children(self.stage_accessibility_nodes(object, cx))
                .child(
                    div().absolute().top_3().right_3().child(
                        Button::new("open-stage-viewer")
                            .label("Open standalone viewer")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_stage_viewer(
                                    root_session.clone(),
                                    lineage.clone(),
                                    stage.clone(),
                                );
                                cx.notify();
                            })),
                    ),
                )
                .into_any_element();
        }
        let kind = object_kind_label(object.kind);
        let detail = object.detail.as_deref().unwrap_or(match object.kind {
            artist_ui_core::ObjectKind::Agent => {
                "This agent's causal transcript is recorded in the parent session."
            }
            artist_ui_core::ObjectKind::Task => "No task output has been recorded yet.",
            artist_ui_core::ObjectKind::Stage => "No live stage frame has been presented yet.",
            artist_ui_core::ObjectKind::Canvas => "The canvas endpoint is not currently connected.",
            artist_ui_core::ObjectKind::Change => "No inline diff was recorded.",
            _ => "No structured detail was recorded.",
        });
        div()
            .id("focused-object")
            .role(Role::Region)
            .aria_label(format!("Focused {kind} {}", object.title))
            .flex()
            .flex_col()
            .gap_4()
            .p_5()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().group_box)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_bold()
                            .text_color(cx.theme().muted_foreground)
                            .child(kind),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{:?}", object.state)),
                    ),
            )
            .child(
                div()
                    .id("focused-object-title")
                    .role(Role::Heading)
                    .aria_level(2)
                    .text_xl()
                    .font_bold()
                    .child(object.title.clone()),
            )
            .child(
                div()
                    .font_family(
                        if matches!(
                            object.kind,
                            artist_ui_core::ObjectKind::Task | artist_ui_core::ObjectKind::Change
                        ) {
                            "monospace"
                        } else {
                            ".SystemUIFont"
                        },
                    )
                    .whitespace_normal()
                    .child(detail.to_owned()),
            )
            .children(self.stage_accessibility_nodes(object, cx))
            .when(object.kind == artist_ui_core::ObjectKind::Stage, |view| {
                let root_session = object.root_session.clone();
                let lineage = object.lineage.clone();
                let stage = object.durable_id.clone();
                view.child(
                    Button::new("open-stage-viewer")
                        .label("Open standalone viewer")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_stage_viewer(
                                root_session.clone(),
                                lineage.clone(),
                                stage.clone(),
                            );
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }
}

impl Render for ArtistApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.transcript.is_empty();
        let turns = turn_ranges(self.transcript.blocks());
        let viewport = window.viewport_size();
        let narrow_workspace = viewport.width < px(1100.);
        self.sync_workspace_tree(cx);
        let show_sidebar = self.sidebar_visible && viewport.width >= px(980.);
        let compact_sidebar = viewport.width < px(980.) && self.compact_sidebar_open;
        let focused_object = self.focused_object.clone();
        let showing_focus = focused_object.is_some();
        let conversation_title = self
            .session_id
            .as_ref()
            .and_then(|session_id| {
                self.sessions
                    .iter()
                    .find(|session| &session.id == session_id)
                    .map(|session| {
                        let roots = self
                            .session_trees
                            .get(session_id)
                            .map(Vec::as_slice)
                            .unwrap_or_default();
                        session_identity(session, roots)
                    })
            })
            .unwrap_or_else(|| "New conversation".to_owned());
        let contextual_title = focused_object
            .as_ref()
            .map(|object| format!("{} / {}", project_name(&self.project), object.title))
            .unwrap_or_else(|| project_name(&self.project));
        let breadcrumb_app = cx.entity().downgrade();
        let mut breadcrumb = Breadcrumb::new().child(
            BreadcrumbItem::new(project_name(&self.project)).on_click(move |_, window, cx| {
                let Some(app) = breadcrumb_app.upgrade() else {
                    return;
                };
                app.update(cx, |this, cx| {
                    if this.focused_object.is_some() {
                        this.pop_focus(window, cx);
                        cx.notify();
                    }
                });
            }),
        );
        if let Some(object) = &focused_object {
            breadcrumb = breadcrumb.child(BreadcrumbItem::new(object.title.clone()));
        }
        let tabs_app = cx.entity().downgrade();
        let mut workspace_tabs = vec![Tab::new().label("Conversation").aria_label("Conversation")];
        if let Some(object) = &focused_object {
            workspace_tabs.push(
                Tab::new()
                    .label(object.title.clone())
                    .aria_label(format!("Focused {}", object.title)),
            );
        }
        let workspace_tabs = TabBar::new("workspace-tabs")
            .underline()
            .small()
            .selected_index(if showing_focus { 1 } else { 0 })
            .children(workspace_tabs)
            .on_click(move |index, window, cx| {
                if *index != 0 {
                    return;
                }
                let Some(app) = tabs_app.upgrade() else {
                    return;
                };
                app.update(cx, |this, cx| {
                    if this.focused_object.is_some() {
                        this.pop_focus(window, cx);
                    }
                    window.focus(&this.composer.read(cx).focus_handle(cx), cx);
                    cx.notify();
                });
            });

        let workspace_main = div()
            .id("workspace-main")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("application-header")
                    .role(Role::Group)
                    .aria_label("Conversation breadcrumb")
                    .h(px(36.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .bg(cx.theme().background)
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("toggle-sidebar")
                                    .icon(IconName::GalleryVerticalEnd)
                                    .compact()
                                    .ghost()
                                    .tooltip("Toggle object tree · Ctrl+B")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_sidebar(window);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("application-title")
                                    .role(Role::Heading)
                                    .aria_label(format!("Current context: {contextual_title}"))
                                    .aria_level(1)
                                    .min_w_0()
                                    .child(breadcrumb),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .when(showing_focus, |view| {
                                view.child(
                                    Button::new("close-focus")
                                        .icon(IconName::ArrowLeft)
                                        .compact()
                                        .ghost()
                                        .tooltip("Back one level")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.pop_focus(window, cx);
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(
                                Button::new("new-conversation")
                                    .icon(IconName::Plus)
                                    .compact()
                                    .ghost()
                                    .tooltip("New conversation · Ctrl+N")
                                    .disabled(self.running)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.new_conversation(window, cx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(Separator::horizontal())
            .child(
                div()
                    .id("workspace-switcher")
                    .role(Role::Navigation)
                    .aria_label("Open views")
                    .h(px(32.))
                    .flex_none()
                    .px_4()
                    .bg(cx.theme().muted.opacity(0.34))
                    .child(workspace_tabs),
            )
            .child(Separator::horizontal())
            .child(
                div()
                    .id("transcript")
                    .role(Role::Log)
                    .aria_label("Conversation transcript")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.transcript_scroll)
                    .vertical_scrollbar(&self.transcript_scroll)
                    .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                        this.follow_output = false;
                        cx.notify();
                    }))
                    .when(!showing_focus && narrow_workspace, |view| {
                        view.px_3().py_4()
                    })
                    .when(!showing_focus && !narrow_workspace, |view| {
                        view.px_6().py_6()
                    })
                    .when(showing_focus, |view| view.p_2())
                    .child(
                        div()
                            .w_full()
                            .when(!showing_focus, |view| view.max_w(px(820.)).mx_auto())
                            .when(showing_focus, |view| view.h_full())
                            .flex()
                            .flex_col()
                            .gap_4()
                            .when_some(focused_object.clone(), |view, object| {
                                view.child(self.focused_object_view(&object, cx))
                            })
                            .when(empty && !showing_focus, |view| {
                                view.child(
                                    div()
                                        .id("empty-transcript")
                                        .role(Role::Status)
                                        .aria_label("No dialogue yet")
                                        .mt_12()
                                        .max_w(px(620.))
                                        .pl_4()
                                        .border_l_2()
                                        .border_color(cx.theme().primary)
                                        .flex()
                                        .flex_col()
                                        .items_start()
                                        .gap_1p5()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_medium()
                                                .text_color(cx.theme().primary)
                                                .child("NEW CONVERSATION"),
                                        )
                                        .child(
                                            div()
                                                .id("empty-heading")
                                                .role(Role::Heading)
                                                .aria_level(2)
                                                .text_2xl()
                                                .font_semibold()
                                                .child(conversation_title.clone()),
                                        )
                                        .child(
                                            div().text_color(cx.theme().muted_foreground).child(
                                                "No dialogue yet. Send an instruction below.",
                                            ),
                                        ),
                                )
                            })
                            .when(!showing_focus, |view| {
                                view.children(turns.into_iter().enumerate().map(
                                    |(turn_index, (start, end))| {
                                        self.turn_view(turn_index, start, end, cx)
                                    },
                                ))
                                .children(self.inline_object_previews(cx))
                            }),
                    ),
            )
            .when(
                !showing_focus && !self.pending_questions.is_empty(),
                |view| {
                    view.child(
                        div().flex_none().px_5().pb_2().child(
                            div()
                                .w_full()
                                .max_w(px(820.))
                                .mx_auto()
                                .child(self.question_view(cx)),
                        ),
                    )
                },
            )
            .when(!showing_focus, |view| {
                view.child(
                    div()
                        .id("composer-region")
                        .role(Role::Group)
                        .aria_label("Message composer")
                        .flex_none()
                        .flex()
                        .flex_col()
                        .when(narrow_workspace, |view| view.px_3())
                        .when(!narrow_workspace, |view| view.px_6())
                        .py_2()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().muted.opacity(0.22))
                        .when(!self.follow_output && !empty, |view| {
                            view.child(
                                div()
                                    .w_full()
                                    .max_w(px(900.))
                                    .mx_auto()
                                    .mb_1()
                                    .flex()
                                    .justify_end()
                                    .child(
                                        Button::new("jump-latest")
                                            .label("Latest")
                                            .secondary()
                                            .compact()
                                            .tooltip("Return to the newest message")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.jump_to_latest();
                                                cx.notify();
                                            })),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .w_full()
                                .max_w(px(900.))
                                .mx_auto()
                                .p_1()
                                .rounded_lg()
                                .border_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().group_box)
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().flex_1().child(Input::new(&self.composer).w_full()))
                                .child(
                                    ButtonGroup::new("composer-actions")
                                        .compact()
                                        .child(
                                            Button::new("send")
                                                .label(if self.running { "Steer" } else { "Send" })
                                                .when(!self.running, |button| button.primary())
                                                .when(self.running, |button| button.ghost())
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.submit(window, cx);
                                                    cx.notify();
                                                })),
                                        )
                                        .when(self.running, |group| {
                                            group
                                                .child(
                                                    Button::new("queue-next")
                                                        .label("Queue")
                                                        .ghost()
                                                        .on_click(cx.listener(
                                                            |this, _, window, cx| {
                                                                this.queue_next_turn(window, cx);
                                                                cx.notify();
                                                            },
                                                        )),
                                                )
                                                .child(
                                                    Button::new("stop")
                                                        .label("Stop")
                                                        .danger()
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.stop();
                                                            cx.notify();
                                                        })),
                                                )
                                        }),
                                ),
                        ),
                )
            });

        div()
            .id("artist-document")
            .role(Role::Application)
            .aria_label("Artist conversation")
            .size_full()
            .relative()
            .flex()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .font_family(".SystemUIFont")
            .on_action(cx.listener(Self::action_new_conversation))
            .on_action(cx.listener(Self::action_open_project))
            .on_action(cx.listener(Self::action_toggle_sidebar))
            .on_action(cx.listener(Self::action_quick_open))
            .on_action(cx.listener(Self::action_focus_composer))
            .on_action(cx.listener(Self::action_stop_run))
            .on_action(cx.listener(Self::action_dismiss_overlay))
            .child(if show_sidebar {
                h_resizable("artist-workspace-layout")
                    .child(
                        resizable_panel()
                            .size(px(300.))
                            .size_range(px(240.)..px(440.))
                            .flex_none()
                            .child(self.sidebar(cx)),
                    )
                    .child(resizable_panel().child(workspace_main))
                    .into_any_element()
            } else {
                workspace_main.into_any_element()
            })
            .when(compact_sidebar, |view| {
                view.child(
                    div()
                        .id("compact-sidebar-drawer")
                        .w(px(300.))
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .shadow_lg()
                        .child(self.sidebar(cx)),
                )
            })
    }
}

fn tree_parenthood_guides(depth: usize, cx: &App) -> AnyElement {
    if depth == 0 {
        return div().w(px(4.)).flex_none().into_any_element();
    }
    let guide = cx.theme().sidebar_foreground.opacity(0.26);
    div()
        .w(px(depth as f32 * 18.))
        .flex_none()
        .relative()
        .children((0..depth).map(|level| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(level as f32 * 18. + 8.))
                .border_l_1()
                .border_color(guide)
        }))
        .child(
            div()
                .absolute()
                .top_1_2()
                .right_0()
                .w(px(10.))
                .border_t_1()
                .border_color(guide),
        )
        .into_any_element()
}

enum HarnessMessage {
    Event(PromptEvent),
    Question(Question),
    Done(Result<String, String>),
}

fn workspace_tree_signature(
    project: &Path,
    sessions: &[Session],
    summaries: &HashMap<String, SessionSummary>,
    trees: &HashMap<String, Vec<artist_ui_core::SessionNode>>,
    active_session: Option<&str>,
    running: bool,
    needs_input: bool,
    query: &str,
) -> String {
    let mut signature = format!(
        "{}|{:?}|{running}|{needs_input}|{query}",
        project.display(),
        active_session
    );
    for session in sessions {
        let summary = summaries.get(&session.id);
        signature.push_str(&format!(
            "|{}:{}:{}:{}",
            session.id,
            summary.map_or(0, |summary| summary.updated_at_ms),
            summary.is_some_and(|summary| summary.failed),
            trees.get(&session.id).map_or(0, Vec::len),
        ));
        if let Some(roots) = trees.get(&session.id) {
            for (_, object) in workspace_rows(roots) {
                signature.push_str(&format!(
                    ":{}:{:?}:{}",
                    object.id.as_str(),
                    object.state,
                    object.updated_seq
                ));
            }
        }
    }
    signature
}

fn workspace_tree_items(
    project: &Path,
    sessions: &[Session],
    summaries: &HashMap<String, SessionSummary>,
    trees: &HashMap<String, Vec<artist_ui_core::SessionNode>>,
    query: &str,
) -> Vec<TreeItem> {
    fn object_item(session_id: &str, node: &artist_ui_core::SessionNode) -> TreeItem {
        TreeItem::new(
            format!("object|{session_id}|{}", node.object.id.as_str()),
            node.object.title.clone(),
        )
        .expanded(true)
        .children(
            node.children
                .iter()
                .map(|child| object_item(session_id, child)),
        )
    }

    let children = sessions.iter().filter_map(|session| {
        let summary = summaries.get(&session.id);
        if !session_matches(session, summary, query, true) {
            return None;
        }
        let roots = trees
            .get(&session.id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let name = session_identity(session, roots);
        let object_children = roots
            .iter()
            .flat_map(|root| root.children.iter())
            .map(|child| object_item(&session.id, child));
        Some(
            TreeItem::new(format!("session|{}", session.id), name)
                .expanded(true)
                .children(object_children),
        )
    });

    vec![
        TreeItem::new(
            format!("project|{}", project.display()),
            project_name(project),
        )
        .expanded(true)
        .children(children),
    ]
}

fn find_workspace_object_by_str(
    roots: &[artist_ui_core::SessionNode],
    id: &str,
) -> Option<FocusableObject> {
    fn visit(node: &artist_ui_core::SessionNode, id: &str) -> Option<FocusableObject> {
        if node.object.id.as_str() == id {
            return Some(node.object.clone());
        }
        node.children.iter().find_map(|child| visit(child, id))
    }
    roots.iter().find_map(|root| visit(root, id))
}

fn workspace_rows(
    roots: &[artist_ui_core::SessionNode],
) -> Vec<(usize, artist_ui_core::FocusableObject)> {
    fn visit(
        node: &artist_ui_core::SessionNode,
        depth: usize,
        rows: &mut Vec<(usize, artist_ui_core::FocusableObject)>,
    ) {
        // The session row already represents the root agent. Child agents and
        // durable surfaces are the hierarchy beneath it.
        if depth > 0 {
            rows.push((depth - 1, node.object.clone()));
        }
        for child in &node.children {
            visit(child, depth + 1, rows);
        }
    }
    let mut rows = Vec::new();
    for root in roots {
        visit(root, 0, &mut rows);
    }
    rows
}

fn find_workspace_object(
    node: &artist_ui_core::SessionNode,
    id: &artist_ui_core::FocusableObjectId,
) -> Option<FocusableObject> {
    if &node.object.id == id {
        return Some(node.object.clone());
    }
    node.children
        .iter()
        .find_map(|child| find_workspace_object(child, id))
}

fn session_identity(session: &Session, roots: &[artist_ui_core::SessionNode]) -> String {
    roots
        .iter()
        .find(|root| root.object.kind == artist_ui_core::ObjectKind::Agent)
        .map(|root| root.object.title.trim())
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Conversation {}", excerpt(&session.id, 12)))
}

fn tool_portal_hint(name: &str, arguments: &str) -> String {
    fn first_useful_string(value: &serde_json::Value) -> Option<&str> {
        match value {
            serde_json::Value::String(value) if !value.trim().is_empty() => Some(value),
            serde_json::Value::Object(values) => [
                "command", "cmd", "script", "query", "path", "url", "prompt", "input", "text",
            ]
            .into_iter()
            .find_map(|key| values.get(key).and_then(first_useful_string)),
            serde_json::Value::Array(values) => values.iter().find_map(first_useful_string),
            _ => None,
        }
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(arguments) {
        if let Some(value) = first_useful_string(&value) {
            return value.lines().next().unwrap_or(value).trim().to_owned();
        }
    }

    let raw = arguments.trim();
    if !raw.is_empty() && !matches!(raw, "null" | "{}" | "[]") {
        return raw.lines().next().unwrap_or(raw).trim().to_owned();
    }

    let lower = name.to_ascii_lowercase();
    if lower.contains("bash") || lower.contains("shell") || lower.contains("terminal") {
        "Shell session".to_owned()
    } else if lower.contains("canvas") {
        "Canvas workspace".to_owned()
    } else if lower.contains("stage") || lower.contains("computer") {
        "Computer stage".to_owned()
    } else if lower.contains("agent") || lower.contains("delegate") {
        "Delegated conversation".to_owned()
    } else {
        "Tool invocation".to_owned()
    }
}

fn tool_destination_kind(name: &str) -> Option<artist_ui_core::ObjectKind> {
    let name = name.to_ascii_lowercase();
    if name.contains("bash") || name.contains("shell") || name.contains("terminal") {
        Some(artist_ui_core::ObjectKind::Task)
    } else if name.contains("canvas") {
        Some(artist_ui_core::ObjectKind::Canvas)
    } else if name.contains("computer") || name.contains("stage") || name.contains("browser") {
        Some(artist_ui_core::ObjectKind::Stage)
    } else {
        None
    }
}

fn object_kind_label(kind: artist_ui_core::ObjectKind) -> &'static str {
    match kind {
        artist_ui_core::ObjectKind::Agent => "Agent",
        artist_ui_core::ObjectKind::Task => "Task",
        artist_ui_core::ObjectKind::Stage => "Stage",
        artist_ui_core::ObjectKind::Canvas => "Canvas",
        artist_ui_core::ObjectKind::Tool => "Tool",
        artist_ui_core::ObjectKind::Change => "Change",
        artist_ui_core::ObjectKind::Ask => "Ask",
    }
}

fn stage_accessibility_role(role: &serde_json::Value) -> Role {
    let name = role
        .as_str()
        .or_else(|| {
            role.as_object()
                .and_then(|object| object.keys().next().map(String::as_str))
        })
        .unwrap_or("region");
    match name {
        "button" | "radiobutton" | "menuitem" | "tab" => Role::Button,
        "link" => Role::Link,
        "checkbox" => Role::CheckBox,
        "textbox" | "combobox" => Role::TextInput,
        "image" => Role::Image,
        "heading" => Role::Heading,
        _ => Role::Region,
    }
}

#[cfg(feature = "embedded-canvas")]
fn canvas_accessibility_role(role: &str) -> Role {
    match role.to_ascii_lowercase().as_str() {
        "button" | "togglebutton" => Role::Button,
        "link" => Role::Link,
        "checkbox" => Role::CheckBox,
        "textbox" | "searchbox" => Role::TextInput,
        "img" | "image" => Role::Image,
        "heading" => Role::Heading,
        "alert" => Role::Alert,
        "article" => Role::Article,
        "navigation" => Role::Navigation,
        _ => Role::Group,
    }
}

fn config_root() -> PathBuf {
    std::env::var_os("ARTIST_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|path| path.join("artist")))
        .unwrap_or_else(|| PathBuf::from(".artist"))
}

fn project_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_str().unwrap_or("Project"))
        .to_owned()
}

fn recent_projects_path() -> PathBuf {
    config_root().join("recent-projects.json")
}

fn load_recent_projects() -> Vec<PathBuf> {
    std::fs::read(recent_projects_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<PathBuf>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|path| path.is_dir())
        .collect()
}

fn remember_project(project: &Path) {
    let mut projects = load_recent_projects();
    projects.retain(|path| path != project);
    projects.insert(0, project.to_path_buf());
    projects.truncate(12);
    let path = recent_projects_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&projects) {
        let _ = std::fs::write(path, bytes);
    }
}

fn excerpt(text: &str, max_chars: usize) -> String {
    let mut chars = text.trim().chars();
    let mut result: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

fn summarize_session(session: &Session, events: &[artist_session::Envelope]) -> SessionSummary {
    let mut summary = SessionSummary {
        updated_at_ms: events.last().map_or(session.created_at_ms, |event| {
            event.ts.max(session.created_at_ms)
        }),
        ..SessionSummary::default()
    };
    for event in events {
        match event.event() {
            SessionEvent::RunStarted(run) => {
                summary.provider = Some(run.provider);
                summary.model = Some(run.model);
                summary.failed = false;
            }
            SessionEvent::RunFinished(artist_session::RunFinished::Error { .. }) => {
                summary.failed = true;
            }
            SessionEvent::RunFinished(_) => summary.failed = false,
            _ => {}
        }
    }
    let replay = artist_session::replay_for_ui(events);
    summary.searchable_text = replay
        .iter()
        .filter_map(|item| match item {
            ReplayItem::User(text)
            | ReplayItem::Assistant(text)
            | ReplayItem::Steering(text)
            | ReplayItem::Reasoning(text) => Some(text.as_str()),
            ReplayItem::Tool { name, preview } => Some(if preview.is_empty() {
                name.as_str()
            } else {
                preview.as_str()
            }),
            ReplayItem::RuleFired { rule, matched } => Some(if matched.is_empty() {
                rule.as_str()
            } else {
                matched.as_str()
            }),
        })
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    summary.preview = replay
        .into_iter()
        .rev()
        .find_map(|item| match item {
            ReplayItem::User(text) | ReplayItem::Assistant(text) | ReplayItem::Steering(text) => {
                Some(excerpt(text.lines().next().unwrap_or(""), 72))
            }
            _ => None,
        })
        .unwrap_or_else(|| "No messages yet".into());
    summary
}

fn sort_sessions_by_activity(
    sessions: &mut [Session],
    summaries: &HashMap<String, SessionSummary>,
) {
    sessions.sort_by_key(|session| {
        std::cmp::Reverse(
            summaries
                .get(&session.id)
                .map_or(session.created_at_ms, |summary| summary.updated_at_ms),
        )
    });
}

fn session_matches(
    session: &Session,
    summary: Option<&SessionSummary>,
    query: &str,
    _show_archived: bool,
) -> bool {
    query.is_empty()
        || session
            .label
            .as_deref()
            .unwrap_or("Untitled")
            .to_lowercase()
            .contains(query)
        || session.id.to_lowercase().contains(query)
        || summary.is_some_and(|summary| {
            summary.preview.to_lowercase().contains(query)
                || summary.searchable_text.contains(query)
        })
}

#[allow(dead_code)]
fn relative_time(timestamp_ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(timestamp_ms);
    let seconds = now.saturating_sub(timestamp_ms) / 1_000;
    match seconds {
        0..=59 => "now".into(),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        86_400..=604_799 => format!("{}d", seconds / 86_400),
        _ => format!("{}w", seconds / 604_800),
    }
}

fn git_changes(project: &Path) -> Vec<ChangedFile> {
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            if line.len() < 4 {
                return None;
            }
            let status = line[..2].trim().to_owned();
            let path = line[3..]
                .rsplit_once(" -> ")
                .map_or(&line[3..], |(_, target)| target)
                .trim_matches('"')
                .to_owned();
            Some(ChangedFile { status, path })
        })
        .collect()
}

fn git_diff(project: &Path, path: &str) -> String {
    let mut diff = String::new();
    for args in [
        vec!["diff", "--", path],
        vec!["diff", "--cached", "--", path],
    ] {
        if let Ok(output) = Command::new("git")
            .arg("-C")
            .arg(project)
            .args(args)
            .output()
        {
            if output.status.success() {
                diff.push_str(&String::from_utf8_lossy(&output.stdout));
            }
        }
    }
    if diff.is_empty() {
        let file = project.join(path);
        if let Ok(content) = std::fs::read_to_string(file) {
            diff = content;
        }
    }
    const LIMIT: usize = 48_000;
    if diff.len() > LIMIT {
        diff.truncate(LIMIT);
        diff.push_str("\n… diff truncated …");
    }
    if diff.is_empty() {
        "No textual diff available".to_owned()
    } else {
        diff
    }
}

fn turn_ranges(blocks: &[Block]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        if matches!(
            block,
            Block::Message {
                role: TranscriptRole::User,
                ..
            }
        ) {
            starts.push(index);
        }
    }
    if starts.first().copied().unwrap_or(usize::MAX) != 0 && !blocks.is_empty() {
        starts.insert(0, 0);
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(blocks.len());
            (*start, end)
        })
        .collect()
}

fn transcript_from_replay(items: Vec<ReplayItem>) -> Transcript {
    let mut transcript = Transcript::new();
    for (index, item) in items.into_iter().enumerate() {
        match item {
            ReplayItem::User(text) | ReplayItem::Steering(text) => transcript.push_user(&text),
            ReplayItem::Assistant(text) => {
                transcript.apply(&PromptEvent::TextDelta(text));
                transcript.close_turn();
            }
            ReplayItem::Reasoning(text) => {
                transcript.apply(&PromptEvent::ReasoningSummaryDelta(text));
            }
            ReplayItem::Tool { name, preview } => {
                let id = format!("replay-tool-{index}");
                transcript.apply(&PromptEvent::ToolCall {
                    id: id.clone(),
                    name,
                    arguments: serde_json::Value::Null,
                });
                transcript.apply(&PromptEvent::ToolResult {
                    id,
                    content: preview,
                    outcome: None,
                    duration_ms: None,
                    images: Vec::new(),
                });
            }
            ReplayItem::RuleFired { rule, matched } => {
                transcript.apply(&PromptEvent::RuleFired { rule, matched });
            }
        }
    }
    transcript.close_turn();
    transcript
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_blocks_are_grouped_at_user_turns() {
        let mut transcript = Transcript::new();
        transcript.push_user("one");
        transcript.apply(&PromptEvent::TextDelta("answer".into()));
        transcript.close_turn();
        transcript.push_user("two");
        assert_eq!(turn_ranges(transcript.blocks()), [(0, 2), (2, 3)]);
    }

    #[test]
    fn relative_times_are_compact() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert_eq!(relative_time(now.saturating_sub(90_000)), "1m");
        assert_eq!(relative_time(now.saturating_sub(7_200_000)), "2h");
    }

    #[test]
    fn session_search_includes_dialogue_preview_without_archive_filtering() {
        let session = Session {
            id: "s1".into(),
            created_at_ms: 1,
            label: Some("Build UI".into()),
            project: PathBuf::from("/tmp/project"),
            transcript: PathBuf::from("/tmp/transcript"),
            parent: None,
            archived: true,
            pinned: false,
        };
        let summary = SessionSummary {
            preview: "fix the scrolling bug".into(),
            ..SessionSummary::default()
        };
        assert!(session_matches(
            &session,
            Some(&summary),
            "scrolling",
            false
        ));
        assert!(session_matches(&session, Some(&summary), "scrolling", true));
    }

    #[test]
    fn session_order_is_based_on_activity_not_selection() {
        let make_session = |id: &str, created_at_ms| Session {
            id: id.into(),
            created_at_ms,
            label: Some(id.into()),
            project: PathBuf::from("/tmp/project"),
            transcript: PathBuf::from(format!("/tmp/{id}.jsonl")),
            parent: None,
            archived: false,
            pinned: false,
        };
        let mut sessions = vec![make_session("older", 10), make_session("newer", 20)];
        let summaries = HashMap::from([
            (
                "older".into(),
                SessionSummary {
                    updated_at_ms: 30,
                    ..SessionSummary::default()
                },
            ),
            (
                "newer".into(),
                SessionSummary {
                    updated_at_ms: 40,
                    ..SessionSummary::default()
                },
            ),
        ]);

        sort_sessions_by_activity(&mut sessions, &summaries);

        assert_eq!(
            sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            ["newer", "older"]
        );
    }
}
