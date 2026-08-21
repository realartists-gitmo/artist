//! Model-facing harness contracts.
//!
//! The agent engine still owns the lifecycle transition that follows a
//! harness call, but the names, schemas, and argument contract are supplied
//! by this component socket just like any other model-facing tool.

use llm_provider::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HarnessPolicy {
    pub yield_schema: Value,
    pub allow_fork: bool,
    pub allow_handoff: bool,
}

impl Default for HarnessPolicy {
    fn default() -> Self {
        Self {
            yield_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "complete": {"type": "boolean"},
                    "remaining": {"type": "string"}
                },
                "required": ["complete"],
                "additionalProperties": false
            }),
            allow_fork: false,
            allow_handoff: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HarnessOperation {
    Yield {
        complete: bool,
        payload: Value,
    },
    Fork {
        tasks: Vec<String>,
        payload: Value,
    },
    Handoff {
        profile: String,
        brief: String,
        payload: Value,
    },
}

impl HarnessOperation {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Yield { .. } => "yield",
            Self::Fork { .. } => "fork",
            Self::Handoff { .. } => "handoff",
        }
    }

    pub fn payload(&self) -> &Value {
        match self {
            Self::Yield { payload, .. }
            | Self::Fork { payload, .. }
            | Self::Handoff { payload, .. } => payload,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HarnessError {
    #[error("yield.complete must be a boolean")]
    YieldCompletion,
    #[error("fork.tasks must be an array")]
    ForkTasks,
    #[error("fork tasks must be strings")]
    ForkTaskType,
    #[error("handoff.profile is required")]
    HandoffProfile,
    #[error("handoff.brief is required")]
    HandoffBrief,
}

/// Socket for the shipped harness contract. A future harness implementation
/// can provide a different policy or set of contracts without teaching the
/// agent loop how to describe model-facing tools.
#[derive(Clone, Copy, Debug, Default)]
pub struct HarnessSocket;

impl HarnessSocket {
    pub fn definitions(&self, policy: &HarnessPolicy) -> Vec<ToolDefinition> {
        let mut definitions = vec![ToolDefinition {
            name: "yield".into(),
            description: Some("Report completion state to the harness".into()),
            input_schema: policy.yield_schema.clone(),
        }];
        if policy.allow_fork {
            definitions.push(ToolDefinition {
                name: "fork".into(),
                description: Some("Run independent task briefs concurrently".into()),
                input_schema: serde_json::json!({
                    "type":"object",
                    "properties":{"tasks":{"type":"array","items":{"type":"string"}}},
                    "required":["tasks"],
                    "additionalProperties":false
                }),
            });
        }
        if policy.allow_handoff {
            definitions.push(ToolDefinition {
                name: "handoff".into(),
                description: Some("Switch to another profile with a brief".into()),
                input_schema: serde_json::json!({
                    "type":"object",
                    "properties":{"profile":{"type":"string"},"brief":{"type":"string"}},
                    "required":["profile","brief"],
                    "additionalProperties":false
                }),
            });
        }
        definitions
    }

    pub fn parse(
        &self,
        name: &str,
        arguments: &Value,
    ) -> Result<Option<HarnessOperation>, HarnessError> {
        match name {
            "yield" => Ok(Some(HarnessOperation::Yield {
                complete: arguments
                    .get("complete")
                    .and_then(Value::as_bool)
                    .ok_or(HarnessError::YieldCompletion)?,
                payload: arguments.clone(),
            })),
            "fork" => {
                let tasks = arguments
                    .get("tasks")
                    .and_then(Value::as_array)
                    .ok_or(HarnessError::ForkTasks)?
                    .iter()
                    .map(|task| {
                        task.as_str()
                            .map(ToOwned::to_owned)
                            .ok_or(HarnessError::ForkTaskType)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Some(HarnessOperation::Fork {
                    tasks,
                    payload: arguments.clone(),
                }))
            }
            "handoff" => Ok(Some(HarnessOperation::Handoff {
                profile: arguments
                    .get("profile")
                    .and_then(Value::as_str)
                    .ok_or(HarnessError::HandoffProfile)?
                    .to_owned(),
                brief: arguments
                    .get("brief")
                    .and_then(Value::as_str)
                    .ok_or(HarnessError::HandoffBrief)?
                    .to_owned(),
                payload: arguments.clone(),
            })),
            _ => Ok(None),
        }
    }
}
