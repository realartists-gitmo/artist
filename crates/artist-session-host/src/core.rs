use crate::{
    EventJournal, HostCommand, HostEvent, HostRequest, HostResponse, PROTOCOL_VERSION,
    RuntimePhase, RuntimeState, Sequenced,
};
use artist_session::Envelope;
use serde_json::json;
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeAction {
    Start {
        lineage: String,
        text: String,
    },
    Steer {
        lineage: String,
        text: String,
    },
    Answer {
        lineage: String,
        answer: artist_session::Answer,
    },
    Stop {
        lineage: String,
    },
    TaskInput {
        lineage: String,
        task: String,
        data: String,
    },
    StageInput {
        lineage: String,
        stage: String,
        input: serde_json::Value,
    },
    FrameRelease {
        lineage: String,
        stage: String,
        buffer_index: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct HostReply {
    pub response: HostResponse,
    pub replay: Vec<Sequenced<HostEvent>>,
    pub action: Option<RuntimeAction>,
}

pub struct HostCore {
    token: String,
    journal: EventJournal<HostEvent>,
    persisted: Vec<Envelope>,
    runtimes: HashMap<String, RuntimeState>,
    queues: HashMap<String, VecDeque<String>>,
    acknowledged: u64,
}

impl HostCore {
    pub fn new(token: impl Into<String>, replay_capacity: usize) -> Self {
        Self {
            token: token.into(),
            journal: EventJournal::new(replay_capacity),
            persisted: Vec::new(),
            runtimes: HashMap::new(),
            queues: HashMap::new(),
            acknowledged: 0,
        }
    }
    pub fn publish(&mut self, event: HostEvent) -> Sequenced<HostEvent> {
        if let HostEvent::PersistedEnvelope { envelope } = &event {
            self.persisted.push(envelope.clone());
        }
        if let HostEvent::RuntimeState(state) = &event {
            self.runtimes.insert(state.lineage.clone(), state.clone());
        }
        self.journal.push(event)
    }
    pub fn handle(&mut self, request: HostRequest) -> HostReply {
        if request.version != PROTOCOL_VERSION {
            return self.error(
                request.request_id,
                format!("unsupported protocol version {}", request.version),
            );
        }
        if request.token != self.token {
            return self.error(request.request_id, "authentication failed".into());
        }
        let mut replay = Vec::new();
        let mut action = None;
        let result = match request.command {
            HostCommand::Attach { after_seq } => {
                replay = self.journal.after(after_seq).unwrap_or_else(|| {
                    vec![Sequenced {
                        seq: self.journal.latest_seq(),
                        payload: HostEvent::Snapshot {
                            envelopes: self.persisted.clone(),
                            runtime: self.runtimes.values().cloned().collect(),
                        },
                    }]
                });
                Ok(json!({"latest_seq": self.journal.latest_seq()}))
            }
            HostCommand::Acknowledge { seq } => {
                self.acknowledged = self.acknowledged.max(seq.min(self.journal.latest_seq()));
                Ok(json!({"acknowledged": self.acknowledged}))
            }
            HostCommand::Message { lineage, text } => {
                if self.is_running(&lineage) {
                    action = Some(RuntimeAction::Steer { lineage, text });
                } else {
                    action = Some(RuntimeAction::Start { lineage, text });
                }
                Ok(json!({}))
            }
            HostCommand::Steer { lineage, text } => {
                if self.is_running(&lineage) {
                    action = Some(RuntimeAction::Steer { lineage, text });
                    Ok(json!({}))
                } else {
                    Err("cannot steer an idle agent".into())
                }
            }
            HostCommand::QueueNextTurn { lineage, text } => {
                let queue = self.queues.entry(lineage).or_default();
                queue.push_back(text);
                Ok(json!({"queued_turns": queue.len()}))
            }
            HostCommand::Answer { lineage, answer } => {
                action = Some(RuntimeAction::Answer { lineage, answer });
                Ok(json!({}))
            }
            HostCommand::Stop { lineage } => {
                action = Some(RuntimeAction::Stop { lineage });
                Ok(json!({}))
            }
            HostCommand::TaskInput {
                lineage,
                task,
                data,
            } => {
                action = Some(RuntimeAction::TaskInput {
                    lineage,
                    task,
                    data,
                });
                Ok(json!({}))
            }
            HostCommand::StageInput {
                lineage,
                stage,
                input,
            } => {
                action = Some(RuntimeAction::StageInput {
                    lineage,
                    stage,
                    input,
                });
                Ok(json!({}))
            }
            HostCommand::FrameRelease {
                lineage,
                stage,
                buffer_index,
            } => {
                action = Some(RuntimeAction::FrameRelease {
                    lineage,
                    stage,
                    buffer_index,
                });
                Ok(json!({}))
            }
        };
        HostReply {
            response: HostResponse {
                version: PROTOCOL_VERSION,
                request_id: request.request_id,
                result,
            },
            replay,
            action,
        }
    }
    pub fn take_next_turn(&mut self, lineage: &str) -> Option<RuntimeAction> {
        self.queues
            .get_mut(lineage)?
            .pop_front()
            .map(|text| RuntimeAction::Start {
                lineage: lineage.into(),
                text,
            })
    }
    pub fn acknowledged(&self) -> u64 {
        self.acknowledged
    }
    fn is_running(&self, lineage: &str) -> bool {
        self.runtimes.get(lineage).is_some_and(|state| {
            matches!(
                state.state,
                RuntimePhase::Running | RuntimePhase::WaitingForAnswer | RuntimePhase::Stopping
            )
        })
    }
    fn error(&self, request_id: u64, error: String) -> HostReply {
        HostReply {
            response: HostResponse {
                version: PROTOCOL_VERSION,
                request_id,
                result: Err(error),
            },
            replay: Vec::new(),
            action: None,
        }
    }
}
