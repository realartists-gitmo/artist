use artist_session_host::{
    HostCommand, HostEvent, HostRegistry, HostRequest, PROTOCOL_VERSION, SeqPacket, ServerPacket,
};
use std::{
    collections::VecDeque,
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

pub enum ControllerMessage {
    Event(HostEvent, Vec<OwnedFd>),
    Error(String),
}

pub struct RootController {
    commands: mpsc::Sender<HostCommand>,
}

impl RootController {
    pub fn connect(
        session: String,
        project: PathBuf,
    ) -> Result<(Self, mpsc::Receiver<ControllerMessage>), String> {
        let registry = HostRegistry::platform_default().map_err(|error| error.to_string())?;
        let record = match registry
            .resolve(&session)
            .map_err(|error| error.to_string())?
        {
            Some(record) => record,
            None => spawn_host(&registry, &session, &project)?,
        };
        let (command_send, command_receive) = mpsc::channel();
        let (event_send, event_receive) = mpsc::channel();
        std::thread::Builder::new()
            .name(format!("artist-gui-host-{session}"))
            .spawn(move || {
                worker(
                    registry,
                    session,
                    project,
                    record,
                    command_receive,
                    event_send,
                )
            })
            .map_err(|error| error.to_string())?;
        Ok((
            Self {
                commands: command_send,
            },
            event_receive,
        ))
    }

    pub fn send(&self, command: HostCommand) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|_| "session host disconnected".into())
    }
}

fn spawn_host(
    registry: &HostRegistry,
    session: &str,
    project: &Path,
) -> Result<artist_session_host::HostRecord, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let directory = current.parent().unwrap_or_else(|| Path::new("."));
    let host = sibling_or_name(directory, "artist-session-host");
    let artist = sibling_or_name(directory, "artist");
    Command::new(host)
        .arg(session)
        .env("ARTIST_HOST_PROJECT", project)
        .env("ARTIST_EXECUTABLE", artist)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("start session host: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(record) = registry
            .resolve(session)
            .map_err(|error| error.to_string())?
        {
            return Ok(record);
        }
        if Instant::now() >= deadline {
            return Err("session host did not become ready".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn sibling_or_name(directory: &Path, name: &str) -> PathBuf {
    let sibling = directory.join(name);
    if sibling.is_file() {
        sibling
    } else {
        PathBuf::from(name)
    }
}

fn worker(
    registry: HostRegistry,
    session: String,
    project: PathBuf,
    mut record: artist_session_host::HostRecord,
    commands: mpsc::Receiver<HostCommand>,
    events: mpsc::Sender<ControllerMessage>,
) {
    let mut request_id = 1_u64;
    let mut last_seq = 0_u64;
    let mut pending = VecDeque::new();
    loop {
        let connection = match SeqPacket::connect(&record.socket) {
            Ok(connection) => connection,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(100));
                record = match registry.resolve(&session) {
                    Ok(Some(record)) => record,
                    Ok(None) => match spawn_host(&registry, &session, &project) {
                        Ok(record) => record,
                        Err(error) => {
                            let _ = events.send(ControllerMessage::Error(error));
                            return;
                        }
                    },
                    Err(error) => {
                        let _ = events.send(ControllerMessage::Error(error.to_string()));
                        return;
                    }
                };
                continue;
            }
        };
        if connection.set_nonblocking(true).is_err() {
            continue;
        }
        request_id += 1;
        if connection
            .send(&HostRequest {
                version: PROTOCOL_VERSION,
                request_id,
                token: record.token.clone(),
                command: HostCommand::Attach {
                    after_seq: last_seq,
                },
            })
            .is_err()
        {
            continue;
        }
        'connected: loop {
            while let Ok(command) = commands.try_recv() {
                pending.push_back(command);
            }
            while let Some(command) = pending.pop_front() {
                request_id += 1;
                if connection
                    .send(&HostRequest {
                        version: PROTOCOL_VERSION,
                        request_id,
                        token: record.token.clone(),
                        command: command.clone(),
                    })
                    .is_err()
                {
                    pending.push_front(command);
                    break 'connected;
                }
            }
            match connection.receive_with_fds::<ServerPacket>() {
                Ok((ServerPacket::Response(response), _)) => {
                    if let Err(error) = response.result {
                        let _ = events.send(ControllerMessage::Error(error));
                    }
                }
                Ok((ServerPacket::Event(event), descriptors)) => {
                    last_seq = last_seq.max(event.seq);
                    if events
                        .send(ControllerMessage::Event(event.payload, descriptors))
                        .is_err()
                    {
                        return;
                    }
                    request_id += 1;
                    if connection
                        .send(&HostRequest {
                            version: PROTOCOL_VERSION,
                            request_id,
                            token: record.token.clone(),
                            command: HostCommand::Acknowledge { seq: last_seq },
                        })
                        .is_err()
                    {
                        break 'connected;
                    }
                }
                Err(error) if is_would_block(&error) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => break 'connected,
            }
        }
        std::thread::sleep(Duration::from_millis(50));
        if let Ok(Some(current)) = registry.resolve(&session) {
            record = current;
        }
    }
}

fn is_would_block(error: &anyhow::Error) -> bool {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
        .is_some_and(|error| error.kind() == std::io::ErrorKind::WouldBlock)
}
