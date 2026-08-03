use crate::{
    HostCore, HostEvent, HostLease, HostRegistry, HostRequest, RuntimeAction, RuntimePhase,
    RuntimeState, SeqPacketListener, ServerPacket,
};
use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    os::fd::{AsRawFd, OwnedFd},
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct DaemonOptions {
    pub session: String,
    pub project: PathBuf,
    pub executable: PathBuf,
    pub idle_timeout: Duration,
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

impl SessionHostDaemon {
    pub fn bind(registry: &HostRegistry, options: DaemonOptions) -> Result<Self> {
        let lease = registry.acquire(&options.session)?;
        let listener = SeqPacketListener::bind(lease.record().socket.clone())?;
        listener.set_nonblocking(true)?;
        let core = HostCore::new(lease.record().token.clone(), 4096);
        let (runtime_send, action_receive) = mpsc::channel();
        let (event_send, runtime_receive) = mpsc::channel();
        let worker_options = options.clone();
        std::thread::Builder::new()
            .name(format!("artist-host-{}", options.session))
            .spawn(move || runtime_loop(worker_options, action_receive, event_send))?;
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

fn runtime_loop(
    options: DaemonOptions,
    actions: mpsc::Receiver<RuntimeAction>,
    events: mpsc::Sender<RuntimePacket>,
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
            let worker_lineage = lineage.clone();
            std::thread::Builder::new()
                .name(format!("artist-runtime-{lineage}"))
                .spawn(move || lineage_loop(options, worker_lineage, receive, events))
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
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            emit_runtime_failure(&lineage, error.to_string(), &events);
            return;
        }
    };
    let mut embedded = match runtime.block_on(artist_cli::EmbeddedRuntime::open(&options.project)) {
        Ok(runtime) => runtime,
        Err(error) => {
            emit_runtime_failure(&lineage, format!("{error:#}"), &events);
            return;
        }
    };
    while let Ok(action) = actions.recv() {
        if let RuntimeAction::Start { text, .. } = action {
            run_turn(
                &options,
                &lineage,
                text,
                &actions,
                &events,
                &runtime,
                &mut embedded,
            );
        }
    }
}

fn run_turn(
    options: &DaemonOptions,
    lineage: &str,
    text: String,
    controls: &mpsc::Receiver<RuntimeAction>,
    events: &mpsc::Sender<RuntimePacket>,
    runtime: &tokio::runtime::Runtime,
    embedded: &mut artist_cli::EmbeddedRuntime,
) {
    let state = |phase| {
        HostEvent::RuntimeState(RuntimeState {
            lineage: lineage.to_owned(),
            profile: "default".into(),
            state: phase,
            queued_turns: 0,
        })
    };
    let _ = events.send(state(RuntimePhase::Running).into());
    let result = runtime.block_on(async {
        let (control_send, mut control_receive) = tokio::sync::mpsc::unbounded_channel();
        let (event_send, mut event_receive) = tokio::sync::mpsc::unbounded_channel();
        let turn = embedded.turn(
            &text,
            &options.session,
            &lineage,
            None,
            None,
            None,
            event_send,
            &mut control_receive,
        );
        tokio::pin!(turn);
        loop {
            tokio::select! {
                result = &mut turn => break result,
                Some(event) = event_receive.recv() => match event {
                    artist_cli::EmbeddedEvent::Prompt(event) => {
                        let event = serde_json::from_value(serde_json::to_value(event)?)?;
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
                            RuntimeAction::Steer { lineage: target, text } if target == lineage => {
                                artist_cli::FrontendControl::Steer { message: text }
                            }
                            RuntimeAction::Answer { lineage: target, answer } if target == lineage => {
                                artist_cli::FrontendControl::Answer { answer }
                            }
                            RuntimeAction::Stop { lineage: target } if target == lineage => {
                                artist_cli::FrontendControl::Stop
                            }
                            _ => continue,
                        };
                        let _ = control_send.send(control);
                    }
                }
            }
        }
    });
    match result {
        Ok(()) => {
            let _ = events.send(state(RuntimePhase::Idle).into());
        }
        Err(error) => runtime_failed(lineage, format!("{error:#}"), events, &state),
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
