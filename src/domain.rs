use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub type NodeId = Uuid;
pub type RunId = Uuid;
pub type EventId = i64;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModelPolicy {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

impl Default for ModelPolicy {
    fn default() -> Self {
        Self {
            provider: "openai".into(),
            model: "gpt-5".into(),
            temperature: None,
            max_tokens: None,
        }
    }
}

/// Immutable recipe for an agent. Updating one means creating a new version.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    pub version: u32,
    pub instructions: String,
    pub input_schema: Value,
    pub output_schema: Value,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub plugins: Vec<String>,
    #[serde(default)]
    pub model: ModelPolicy,
    pub created_at: DateTime<Utc>,
}

impl Node {
    pub fn new(
        name: impl Into<String>,
        instructions: impl Into<String>,
        input_schema: Value,
        output_schema: Value,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            version: 1,
            instructions: instructions.into(),
            input_schema,
            output_schema,
            capabilities: Vec::new(),
            plugins: Vec::new(),
            model: ModelPolicy::default(),
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChildKind {
    Spawn,
    Fork,
    Replacement,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ForkPoint {
    pub run_id: RunId,
    /// Include parent events through this per-run sequence number.
    pub through_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Ready,
    Running,
    WaitingForAnswer,
    WaitingForChildren,
    Completed,
    Replaced,
    Interrupted,
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Replaced | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct InitialContext {
    pub system: String,
    pub agents: String,
    pub node: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventData {
    RunCreated {
        node_id: NodeId,
        input: Value,
        initial_context: InitialContext,
        parent_id: Option<RunId>,
        child_kind: Option<ChildKind>,
        fork_point: Option<ForkPoint>,
    },
    RunStarted,
    ContextAppended {
        text: String,
    },
    ModelRequested {
        request_id: Uuid,
    },
    ModelResponse {
        request_id: Uuid,
        content: Value,
    },
    PythonRequested {
        request_id: Uuid,
        code: String,
    },
    PythonCompleted {
        request_id: Uuid,
        output: Value,
    },
    Asked {
        question: String,
    },
    Answered {
        answer: String,
    },
    ChildrenWaiting {
        child_ids: Vec<RunId>,
    },
    ChildrenJoined {
        child_ids: Vec<RunId>,
        results: Vec<Value>,
    },
    Yielded {
        value: Value,
    },
    Replaced {
        successor_ids: Vec<RunId>,
    },
    Interrupted {
        action: String,
        reason: String,
    },
    Cancelled {
        reason: String,
    },
    ReplRestarted {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Event {
    pub id: EventId,
    pub run_id: RunId,
    pub sequence: u64,
    pub occurred_at: DateTime<Utc>,
    pub data: EventData,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Run {
    pub id: RunId,
    pub node_id: NodeId,
    pub input: Value,
    pub initial_context: InitialContext,
    pub parent_id: Option<RunId>,
    pub child_kind: Option<ChildKind>,
    pub fork_point: Option<ForkPoint>,
    pub status: RunStatus,
    pub result: Option<Value>,
    pub waiting_for: Vec<RunId>,
    pub last_sequence: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Run {
    pub fn rebuild(events: &[Event]) -> Result<Self, String> {
        let first = events
            .first()
            .ok_or_else(|| "run has no events".to_string())?;
        let EventData::RunCreated {
            node_id,
            input,
            initial_context,
            parent_id,
            child_kind,
            fork_point,
        } = &first.data
        else {
            return Err("first event is not run_created".into());
        };
        let mut run = Self {
            id: first.run_id,
            node_id: *node_id,
            input: input.clone(),
            initial_context: initial_context.clone(),
            parent_id: *parent_id,
            child_kind: *child_kind,
            fork_point: fork_point.clone(),
            status: RunStatus::Ready,
            result: None,
            waiting_for: Vec::new(),
            last_sequence: first.sequence,
            created_at: first.occurred_at,
            updated_at: first.occurred_at,
        };
        for event in &events[1..] {
            run.apply(event)?;
        }
        Ok(run)
    }

    fn apply(&mut self, event: &Event) -> Result<(), String> {
        if event.run_id != self.id {
            return Err("event belongs to another run".into());
        }
        if event.sequence != self.last_sequence + 1 {
            return Err(format!(
                "event sequence gap: expected {}, got {}",
                self.last_sequence + 1,
                event.sequence
            ));
        }
        match &event.data {
            EventData::RunCreated { .. } => return Err("duplicate run_created".into()),
            EventData::RunStarted => self.status = RunStatus::Running,
            EventData::Asked { .. } => self.status = RunStatus::WaitingForAnswer,
            EventData::Answered { .. } => self.status = RunStatus::Running,
            EventData::ChildrenWaiting { child_ids } => {
                self.status = RunStatus::WaitingForChildren;
                self.waiting_for = child_ids.clone();
            }
            EventData::ChildrenJoined { .. } => {
                self.status = RunStatus::Running;
                self.waiting_for.clear();
            }
            EventData::Yielded { value } => {
                self.status = RunStatus::Completed;
                self.result = Some(value.clone());
                self.waiting_for.clear();
            }
            EventData::Replaced { .. } => self.status = RunStatus::Replaced,
            EventData::Interrupted { .. } => self.status = RunStatus::Interrupted,
            EventData::Cancelled { .. } => self.status = RunStatus::Cancelled,
            EventData::ContextAppended { .. }
            | EventData::ModelRequested { .. }
            | EventData::ModelResponse { .. }
            | EventData::PythonRequested { .. }
            | EventData::PythonCompleted { .. }
            | EventData::ReplRestarted { .. } => {}
        }
        self.last_sequence = event.sequence;
        self.updated_at = event.occurred_at;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RunHandle {
    pub run_id: RunId,
}
