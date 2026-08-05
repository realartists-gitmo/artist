use crate::{
    HostCore, HostEvent, HostLease, HostRegistry, HostRequest, RuntimeAction, RuntimePhase,
    RuntimeState, SeqPacketListener, ServerPacket,
};
use anyhow::{Context, Result};
use std::{
    collections::{HashMap, HashSet},
    os::fd::{AsRawFd, OwnedFd},
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct DaemonOptions {
    pub session: String,
    pub project: PathBuf,
    pub idle_timeout: Duration,
}

pub struct RuntimeEmission {
    pub event: HostEvent,
    pub descriptors: Vec<OwnedFd>,
}

impl From<HostEvent> for RuntimeEmission {
    fn from(event: HostEvent) -> Self {
        Self {
            event,
            descriptors: Vec::new(),
        }
    }
}

pub struct RuntimeTurn<'a> {
    pub session: &'a str,
    pub lineage: &'a str,
    pub text: String,
    pub controls: &'a mpsc::Receiver<RuntimeAction>,
    pub events: &'a mpsc::Sender<RuntimeEmission>,
}

pub trait RuntimeDriver: Send {
    fn run_turn(&mut self, turn: RuntimeTurn<'_>) -> Result<()>;
    fn handle(&mut self, action: RuntimeAction) -> Result<()> {
        anyhow::bail!("runtime control is unavailable: {action:?}")
    }
}

pub trait RuntimeFactory: Send + Sync {
    fn open(&self, options: &DaemonOptions, lineage: &str) -> Result<Box<dyn RuntimeDriver>>;
}

#[derive(Default)]
pub struct EmbeddedRuntimeFactory;

struct EmbeddedRuntimeDriver {
    runtime: tokio::runtime::Runtime,
    embedded: artist_cli::EmbeddedRuntime,
    stage: Option<artist_computer::stage::wayland::StageLease>,
    events: Option<mpsc::Sender<RuntimeEmission>>,
}

impl RuntimeFactory for EmbeddedRuntimeFactory {
    fn open(&self, options: &DaemonOptions, _lineage: &str) -> Result<Box<dyn RuntimeDriver>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let embedded = runtime.block_on(artist_cli::EmbeddedRuntime::open(&options.project))?;
        Ok(Box::new(EmbeddedRuntimeDriver {
            runtime,
            embedded,
            stage: None,
            events: None,
        }))
    }
}

pub struct SessionHostDaemon {
    options: DaemonOptions,
    _lease: HostLease,
    listener: SeqPacketListener,
    core: HostCore,
    runtime_send: mpsc::Sender<RuntimeAction>,
    runtime_receive: mpsc::Receiver<RuntimePacket>,
    descriptor_events: HashMap<u64, Vec<Arc<OwnedFd>>>,
}

struct RuntimePacket {
    event: HostEvent,
    descriptors: Vec<OwnedFd>,
}

impl From<HostEvent> for RuntimePacket {
    fn from(event: HostEvent) -> Self {
        Self {
            event,
            descriptors: Vec::new(),
        }
    }
}

impl From<RuntimeEmission> for RuntimePacket {
    fn from(emission: RuntimeEmission) -> Self {
        Self {
            event: emission.event,
            descriptors: emission.descriptors,
        }
    }
}

impl SessionHostDaemon {
    pub fn bind(registry: &HostRegistry, options: DaemonOptions) -> Result<Self> {
        Self::bind_with_factory(registry, options, Arc::new(EmbeddedRuntimeFactory))
    }

    pub fn bind_with_factory(
        registry: &HostRegistry,
        options: DaemonOptions,
        factory: Arc<dyn RuntimeFactory>,
    ) -> Result<Self> {
        let lease = registry.acquire(&options.session)?;
        let listener = SeqPacketListener::bind(lease.record().socket.clone())?;
        listener.set_nonblocking(true)?;
        let core = HostCore::new(lease.record().token.clone(), 4096);
        let (runtime_send, action_receive) = mpsc::channel();
        let (event_send, runtime_receive) = mpsc::channel();
        let worker_options = options.clone();
        std::thread::Builder::new()
            .name(format!("artist-host-{}", options.session))
            .spawn(move || runtime_loop(worker_options, action_receive, event_send, factory))?;
        Ok(Self {
            options,
            _lease: lease,
            listener,
            core,
            runtime_send,
            runtime_receive,
            descriptor_events: HashMap::new(),
        })
    }

    pub fn run(mut self) -> Result<()> {
        let mut idle_since = Instant::now();
        loop {
            if idle_since.elapsed() >= self.options.idle_timeout {
                return Ok(());
            }
            while let Ok(event) = self.runtime_receive.try_recv() {
                self.record_runtime_event(event);
                idle_since = Instant::now();
            }
            let connection = match self.listener.accept() {
                Ok(connection) => connection,
                Err(error) if is_would_block(&error) => {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                Err(error) => return Err(error),
            };
            let peer = connection.peer_credentials()?;
            if peer.uid != unsafe { libc::geteuid() } {
                continue;
            }
            connection.set_nonblocking(true)?;
            idle_since = Instant::now();
            loop {
                let mut progressed = false;
                match connection.receive::<HostRequest>() {
                    Ok(request) => {
                        progressed = true;
                        let reply = self.core.handle(request);
                        connection.send(&ServerPacket::Response(reply.response))?;
                        for event in reply.replay {
                            let descriptors = self
                                .descriptor_events
                                .get(&event.seq)
                                .map(Vec::as_slice)
                                .unwrap_or_default();
                            let raw = descriptors
                                .iter()
                                .map(|descriptor| descriptor.as_raw_fd())
                                .collect::<Vec<_>>();
                            connection.send_with_fds(&ServerPacket::Event(event), &raw)?;
                        }
                        if let Some(action) = reply.action {
                            self.runtime_send.send(action).context("runtime stopped")?;
                        }
                    }
                    Err(error) if is_would_block(&error) => {}
                    Err(error) if is_disconnect(&error) => break,
                    Err(error) => return Err(error),
                }
                while let Ok(event) = self.runtime_receive.try_recv() {
                    progressed = true;
                    let event = self.record_runtime_event(event);
                    let descriptors = self
                        .descriptor_events
                        .get(&event.seq)
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    let raw = descriptors
                        .iter()
                        .map(|descriptor| descriptor.as_raw_fd())
                        .collect::<Vec<_>>();
                    if let Err(error) = connection.send_with_fds(&ServerPacket::Event(event), &raw)
                    {
                        if is_would_block(&error) {
                            break;
                        }
                        break;
                    }
                }
                if !progressed {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    fn record_runtime_event(&mut self, packet: RuntimePacket) -> crate::Sequenced<HostEvent> {
        let idle_lineage = match &packet.event {
            HostEvent::RuntimeState(state) if state.state == RuntimePhase::Idle => {
                Some(state.lineage.clone())
            }
            _ => None,
        };
        let sequenced = self.core.publish(packet.event);
        if !packet.descriptors.is_empty() {
            self.descriptor_events.insert(
                sequenced.seq,
                packet.descriptors.into_iter().map(Arc::new).collect(),
            );
        }
        self.descriptor_events
            .retain(|seq, _| seq.saturating_add(4096) >= sequenced.seq);
        if let Some(lineage) = idle_lineage
            && let Some(action) = self.core.take_next_turn(&lineage)
        {
            let _ = self.runtime_send.send(action);
        }
        sequenced
    }
}

impl RuntimeDriver for EmbeddedRuntimeDriver {
    fn run_turn(&mut self, turn: RuntimeTurn<'_>) -> Result<()> {
        let RuntimeTurn {
            session,
            lineage,
            text,
            controls,
            events,
        } = turn;
        self.events = Some(events.clone());
        let controller = self.embedded.controller();
        let runtime = &self.runtime;
        let embedded = &mut self.embedded;
        let stage = &mut self.stage;

        runtime.block_on(async {
            let (control_send, mut control_receive) = tokio::sync::mpsc::unbounded_channel();
            let (event_send, mut event_receive) = tokio::sync::mpsc::unbounded_channel();
            let run = embedded.turn(
                &text,
                session,
                lineage,
                None,
                None,
                None,
                event_send,
                &mut control_receive,
            );
            tokio::pin!(run);
            let mut canvas_calls = HashMap::<String, String>::new();
            let mut computer_calls = HashSet::<String>::new();
            let mut computer_launches = HashSet::<String>::new();

            loop {
                tokio::select! {
                    result = &mut run => break result,
                    Some(event) = event_receive.recv() => match event {
                        artist_cli::EmbeddedEvent::Prompt(event) => {
                            let event: artist_ui_core::PromptEvent =
                                serde_json::from_value(serde_json::to_value(event)?)?;
                            match &event {
                                artist_ui_core::PromptEvent::ToolCall { id, name, arguments }
                                    if name == "canvas"
                                        && arguments.get("mode").and_then(serde_json::Value::as_str)
                                            == Some("open") =>
                                {
                                    if let Some(slug) = arguments
                                        .get("name")
                                        .and_then(serde_json::Value::as_str)
                                    {
                                        canvas_calls.insert(id.clone(), slug.to_owned());
                                    }
                                }
                                artist_ui_core::PromptEvent::ToolCall { id, name, arguments }
                                    if name == "computer" =>
                                {
                                    computer_calls.insert(id.clone());
                                    if arguments.get("mode").and_then(serde_json::Value::as_str)
                                        == Some("launch")
                                        && arguments.get("gui").and_then(serde_json::Value::as_bool)
                                            == Some(true)
                                    {
                                        computer_launches.insert(id.clone());
                                    }
                                }
                                artist_ui_core::PromptEvent::ToolResult { id, content, .. } => {
                                    if let Some(slug) = canvas_calls.remove(id)
                                        && let Some(origin) = first_http_url(content)
                                    {
                                        let _ = events.send(HostEvent::CanvasEndpoint {
                                            lineage: lineage.to_owned(),
                                            slug,
                                            origin,
                                        }.into());
                                    }
                                    if computer_launches.remove(id) && stage.is_none() {
                                        match controller.stage_export().await {
                                            Ok(export) => {
                                                *stage = Some(install_stage_export(
                                                    export,
                                                    events,
                                                    lineage,
                                                ));
                                            }
                                            Err(error) => {
                                                emit_embedded_control_failure(events, lineage, error);
                                            }
                                        }
                                    }
                                    if computer_calls.remove(id) {
                                        emit_accessibility_snapshot(
                                            &controller,
                                            events,
                                            lineage,
                                            stage.as_ref().map(|lease| lease.stage.as_str()),
                                        )
                                        .await;
                                    }
                                }
                                _ => {}
                            }
                            let _ = events.send(HostEvent::StreamingDelta {
                                lineage: lineage.to_owned(),
                                event,
                            }.into());
                        }
                        artist_cli::EmbeddedEvent::Question(question) => {
                            let _ = events.send(HostEvent::Attention {
                                lineage: lineage.to_owned(),
                                kind: crate::AttentionKind::Question,
                                message: serde_json::to_string(&question).unwrap_or_default(),
                            }.into());
                        }
                        artist_cli::EmbeddedEvent::Session(_) => {}
                    },
                    _ = tokio::time::sleep(Duration::from_millis(15)) => {
                        while let Ok(action) = controls.try_recv() {
                            let control = match action {
                                RuntimeAction::Steer { lineage: target, text }
                                    if target == lineage =>
                                {
                                    artist_cli::FrontendControl::Steer { message: text }
                                }
                                RuntimeAction::Answer { lineage: target, answer }
                                    if target == lineage =>
                                {
                                    artist_cli::FrontendControl::Answer { answer }
                                }
                                RuntimeAction::Stop { lineage: target } if target == lineage => {
                                    artist_cli::FrontendControl::Stop
                                }
                                RuntimeAction::TaskInput { lineage: target, task, data }
                                    if target == lineage =>
                                {
                                    if let Err(error) = controller.task_input(&task, &data).await {
                                        emit_embedded_control_failure(events, lineage, error);
                                    }
                                    continue;
                                }
                                RuntimeAction::StageInput {
                                    lineage: target,
                                    stage: target_stage,
                                    input,
                                } if target == lineage =>
                                {
                                    if let Err(error) = controller.stage_input(input, &target_stage).await {
                                        emit_embedded_control_failure(events, lineage, error);
                                    } else {
                                        emit_accessibility_snapshot(
                                            &controller,
                                            events,
                                            lineage,
                                            stage.as_ref().map(|lease| lease.stage.as_str()),
                                        )
                                        .await;
                                    }
                                    continue;
                                }
                                RuntimeAction::FrameRelease {
                                    lineage: target,
                                    stage: target_stage,
                                    buffer_index,
                                } if target == lineage =>
                                {
                                    if let Some(lease) = stage.as_ref()
                                        && let Err(error) = release_stage_frame(
                                            lease,
                                            events,
                                            lineage,
                                            &target_stage,
                                            buffer_index,
                                        )
                                    {
                                        emit_embedded_control_failure(events, lineage, error);
                                    }
                                    continue;
                                }
                                _ => continue,
                            };
                            let _ = control_send.send(control);
                        }
                    }
                }
            }
        })
    }

    fn handle(&mut self, action: RuntimeAction) -> Result<()> {
        let controller = self.embedded.controller();
        let stage_object = self.stage.as_ref().map(|lease| lease.stage.clone());
        let host_events = self.events.clone();
        match action {
            RuntimeAction::FrameRelease {
                lineage,
                stage,
                buffer_index,
            } => release_stage_frame(
                self.stage
                    .as_ref()
                    .context("the runtime has not exported a stage")?,
                self.events
                    .as_ref()
                    .context("the runtime has no host event channel")?,
                &lineage,
                &stage,
                buffer_index,
            ),
            action => self.runtime.block_on(async move {
                match action {
                    RuntimeAction::TaskInput { task, data, .. } => {
                        controller.task_input(&task, &data).await
                    }
                    RuntimeAction::StageInput {
                        lineage,
                        stage,
                        input,
                    } => {
                        controller.stage_input(input, &stage).await?;
                        if let Some(events) = host_events.as_ref() {
                            emit_accessibility_snapshot(
                                &controller,
                                events,
                                &lineage,
                                stage_object.as_deref(),
                            )
                            .await;
                        }
                        Ok(())
                    }
                    action => anyhow::bail!("runtime control is invalid while idle: {action:?}"),
                }
            }),
        }
    }
}

async fn emit_accessibility_snapshot(
    controller: &artist_cli::EmbeddedController,
    events: &mpsc::Sender<RuntimeEmission>,
    lineage: &str,
    stage: Option<&str>,
) {
    let mut attempt = 0;
    let result = loop {
        match controller.accessibility_snapshot().await {
            Ok(None) if attempt < 10 => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            result => break result,
        }
    };
    match result {
        Ok(Some((surface, snapshot))) => {
            let nodes = snapshot
                .nodes
                .into_iter()
                .map(|node| {
                    serde_json::json!({
                        "binding": node.binding.0,
                        "role": node.role,
                        "name": node.name,
                        "value": node.value,
                        "state": node.state,
                        "actions": node.actions,
                        "depth": node.depth,
                    })
                })
                .collect::<Vec<_>>();
            let _ = events.send(
                HostEvent::AccessibilityPatch {
                    lineage: lineage.to_owned(),
                    object: stage.unwrap_or(&surface).to_owned(),
                    patch: serde_json::json!({
                        "surface": surface,
                        "nodes": nodes,
                    }),
                }
                .into(),
            );
        }
        Ok(None) => {}
        Err(error) => emit_embedded_control_failure(events, lineage, error),
    }
}

fn install_stage_export(
    mut export: artist_computer::stage::wayland::StageExport,
    events: &mpsc::Sender<RuntimeEmission>,
    lineage: &str,
) -> artist_computer::stage::wayland::StageLease {
    let lease = export.lease.clone();
    for buffer in export.buffers {
        let fd_count = buffer.planes.len() as u8;
        let offsets = buffer.planes.iter().map(|plane| plane.offset).collect();
        let strides = buffer.planes.iter().map(|plane| plane.stride).collect();
        let descriptors = buffer.planes.into_iter().map(|plane| plane.fd).collect();
        let _ = events.send(RuntimeEmission {
            event: HostEvent::StageExport {
                lineage: lineage.to_owned(),
                stage: lease.stage.clone(),
                descriptor: crate::StageDescriptor {
                    buffer_index: buffer.index,
                    width: buffer.width,
                    height: buffer.height,
                    format: buffer.format,
                    modifier: buffer.modifier,
                    offsets,
                    strides,
                    fd_count,
                },
            },
            descriptors,
        });
    }
    present_stage_front(&lease, events, lineage, Vec::new());

    let monitor = lease.clone();
    let monitor_events = events.clone();
    let monitor_lineage = lineage.to_owned();
    tokio::spawn(async move {
        loop {
            match export.damage.recv().await {
                Ok(damage) => {
                    let region = damage.region;
                    present_stage_front(
                        &monitor,
                        &monitor_events,
                        &monitor_lineage,
                        vec![[
                            region.x.max(0) as u32,
                            region.y.max(0) as u32,
                            region.width,
                            region.height,
                        ]],
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    lease
}

fn present_stage_front(
    lease: &artist_computer::stage::wayland::StageLease,
    events: &mpsc::Sender<RuntimeEmission>,
    lineage: &str,
    damage: Vec<[u32; 4]>,
) {
    let buffer_index = lease.current_buffer();
    if lease.hold(buffer_index) {
        let _ = events.send(
            HostEvent::StagePresent {
                lineage: lineage.to_owned(),
                stage: lease.stage.clone(),
                buffer_index,
                damage,
            }
            .into(),
        );
    }
}

fn release_stage_frame(
    lease: &artist_computer::stage::wayland::StageLease,
    events: &mpsc::Sender<RuntimeEmission>,
    lineage: &str,
    stage: &str,
    buffer_index: u32,
) -> Result<()> {
    if stage != lease.stage {
        anyhow::bail!(
            "stage release targeted {stage}, but this runtime owns {}",
            lease.stage
        );
    }
    if !lease.release(buffer_index) {
        anyhow::bail!("stage buffer {buffer_index} was not in flight");
    }
    let current = lease.current_buffer();
    if current != buffer_index {
        present_stage_front(lease, events, lineage, Vec::new());
    }
    Ok(())
}

fn emit_embedded_control_failure(
    events: &mpsc::Sender<RuntimeEmission>,
    lineage: &str,
    error: anyhow::Error,
) {
    let _ = events.send(
        HostEvent::Attention {
            lineage: lineage.to_owned(),
            kind: crate::AttentionKind::Failure,
            message: format!("{error:#}"),
        }
        .into(),
    );
}

fn first_http_url(text: &str) -> Option<String> {
    let start = text.find("http://").or_else(|| text.find("https://"))?;
    let tail = &text[start..];
    let end = tail
        .find(|character: char| {
            character.is_whitespace() || matches!(character, '`' | ')' | ']' | '>' | '"')
        })
        .unwrap_or(tail.len());
    Some(tail[..end].trim_end_matches(['.', ',']).to_owned())
}

fn runtime_loop(
    options: DaemonOptions,
    actions: mpsc::Receiver<RuntimeAction>,
    events: mpsc::Sender<RuntimePacket>,
    factory: Arc<dyn RuntimeFactory>,
) {
    let mut workers = HashMap::<String, mpsc::Sender<RuntimeAction>>::new();
    while let Ok(action) = actions.recv() {
        let lineage = match &action {
            RuntimeAction::Start { lineage, .. }
            | RuntimeAction::Steer { lineage, .. }
            | RuntimeAction::Answer { lineage, .. }
            | RuntimeAction::Stop { lineage }
            | RuntimeAction::TaskInput { lineage, .. }
            | RuntimeAction::StageInput { lineage, .. }
            | RuntimeAction::FrameRelease { lineage, .. } => lineage.clone(),
        };
        if !workers.contains_key(&lineage) {
            let (send, receive) = mpsc::channel();
            workers.insert(lineage.clone(), send);
            let options = options.clone();
            let events = events.clone();
            let factory = factory.clone();
            let worker_lineage = lineage.clone();
            std::thread::Builder::new()
                .name(format!("artist-runtime-{lineage}"))
                .spawn(move || lineage_loop(options, worker_lineage, receive, events, factory))
                .ok();
        }
        if workers
            .get(&lineage)
            .is_some_and(|sender| sender.send(action).is_err())
        {
            workers.remove(&lineage);
        }
    }
}

fn lineage_loop(
    options: DaemonOptions,
    lineage: String,
    actions: mpsc::Receiver<RuntimeAction>,
    events: mpsc::Sender<RuntimePacket>,
    factory: Arc<dyn RuntimeFactory>,
) {
    let mut runtime = match factory.open(&options, &lineage) {
        Ok(runtime) => runtime,
        Err(error) => {
            emit_runtime_failure(&lineage, format!("{error:#}"), &events);
            return;
        }
    };
    while let Ok(action) = actions.recv() {
        if let RuntimeAction::Start { text, .. } = action {
            let state = |phase| {
                HostEvent::RuntimeState(RuntimeState {
                    lineage: lineage.clone(),
                    profile: "default".into(),
                    state: phase,
                    queued_turns: 0,
                })
            };
            let _ = events.send(state(RuntimePhase::Running).into());
            let (runtime_events, runtime_receive) = mpsc::channel::<RuntimeEmission>();
            let forward_events = events.clone();
            std::thread::spawn(move || {
                while let Ok(event) = runtime_receive.recv() {
                    if forward_events.send(event.into()).is_err() {
                        break;
                    }
                }
            });
            let result = runtime.run_turn(RuntimeTurn {
                session: &options.session,
                lineage: &lineage,
                text,
                controls: &actions,
                events: &runtime_events,
            });
            match result {
                Ok(()) => {
                    let _ = events.send(state(RuntimePhase::Idle).into());
                }
                Err(error) => runtime_failed(&lineage, format!("{error:#}"), &events, &state),
            }
        } else if let Err(error) = runtime.handle(action) {
            let _ = events.send(
                HostEvent::Attention {
                    lineage: lineage.clone(),
                    kind: crate::AttentionKind::Failure,
                    message: format!("{error:#}"),
                }
                .into(),
            );
        }
    }
}

fn emit_runtime_failure(lineage: &str, message: String, events: &mpsc::Sender<RuntimePacket>) {
    let state = |phase| {
        HostEvent::RuntimeState(RuntimeState {
            lineage: lineage.to_owned(),
            profile: "default".into(),
            state: phase,
            queued_turns: 0,
        })
    };
    runtime_failed(lineage, message, events, &state);
}

fn runtime_failed(
    lineage: &str,
    message: String,
    events: &mpsc::Sender<RuntimePacket>,
    state: &impl Fn(RuntimePhase) -> HostEvent,
) {
    let _ = events.send(
        HostEvent::Attention {
            lineage: lineage.to_owned(),
            kind: crate::AttentionKind::Failure,
            message,
        }
        .into(),
    );
    let _ = events.send(state(RuntimePhase::Failed).into());
}

fn is_would_block(error: &anyhow::Error) -> bool {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
        .is_some_and(|error| error.kind() == std::io::ErrorKind::WouldBlock)
}

fn is_disconnect(error: &anyhow::Error) -> bool {
    error.to_string().contains("disconnected")
        || error
            .chain()
            .find_map(|cause| cause.downcast_ref::<std::io::Error>())
            .is_some_and(|error| {
                matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::UnexpectedEof
                )
            })
}

#[cfg(test)]
mod embedded_runtime_tests {
    use super::first_http_url;

    #[test]
    fn extracts_the_authenticated_canvas_origin_from_tool_text() {
        assert_eq!(
            first_http_url("Opened inside Artist.\n\nhttp://127.0.0.1:8123/key/demo"),
            Some("http://127.0.0.1:8123/key/demo".into())
        );
    }
}
