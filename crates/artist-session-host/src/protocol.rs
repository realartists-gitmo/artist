use artist_session::{Answer, Envelope};
use artist_ui_core::PromptEvent;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostRequest {
    pub version: u16,
    pub request_id: u64,
    pub token: String,
    pub command: HostCommand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostResponse {
    pub version: u16,
    pub request_id: u64,
    pub result: Result<serde_json::Value, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "packet", content = "data", rename_all = "snake_case")]
pub enum ServerPacket {
    Response(HostResponse),
    Event(SequencedHostEvent),
}

pub type SequencedHostEvent = crate::Sequenced<HostEvent>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostCommand {
    Attach {
        after_seq: u64,
    },
    Message {
        lineage: String,
        text: String,
    },
    Steer {
        lineage: String,
        text: String,
    },
    QueueNextTurn {
        lineage: String,
        text: String,
    },
    Answer {
        lineage: String,
        answer: Answer,
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
    Acknowledge {
        seq: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostEvent {
    Snapshot {
        envelopes: Vec<Envelope>,
        runtime: Vec<RuntimeState>,
    },
    PersistedEnvelope {
        envelope: Envelope,
    },
    StreamingDelta {
        lineage: String,
        event: PromptEvent,
    },
    RuntimeState(RuntimeState),
    Attention {
        lineage: String,
        kind: AttentionKind,
        message: String,
    },
    CanvasEndpoint {
        lineage: String,
        slug: String,
        origin: String,
    },
    StageExport {
        lineage: String,
        stage: String,
        descriptor: StageDescriptor,
    },
    StagePresent {
        lineage: String,
        stage: String,
        buffer_index: u32,
        damage: Vec<[u32; 4]>,
    },
    AccessibilityPatch {
        lineage: String,
        object: String,
        patch: serde_json::Value,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeState {
    pub lineage: String,
    pub profile: String,
    pub state: RuntimePhase,
    pub queued_turns: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePhase {
    Idle,
    Running,
    WaitingForAnswer,
    Stopping,
    Failed,
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    Question,
    Failure,
}

/// Metadata travels in JSON; the matching DMA-BUF plane descriptors travel via SCM_RIGHTS.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageDescriptor {
    pub buffer_index: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub modifier: u64,
    pub strides: Vec<u32>,
    pub offsets: Vec<u32>,
    pub fd_count: u8,
}
