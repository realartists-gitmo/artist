//! Thin Rig integration. The durable runtime remains provider-independent.

use rig_core::{
    completion::{AssistantContent, CompletionError, CompletionModel, ToolDefinition},
    message::{Message, UserContent},
};
use serde_json::{Value, json};

use crate::{
    context::{ContextBuilder, ContextEntry, ContextRole},
    domain::RunId,
    runtime::{Runtime, RuntimeError},
};

/// These definitions are deliberately constant. Plugin capabilities live behind Python.
pub fn stable_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "python".into(),
            description: "Execute Python in this run's persistent REPL. Use the artist object inside Python to spawn, fork, join, or replace runs. Use fff for fuzzy file/content search when useful.".into(),
            parameters: json!({
                "type":"object",
                "properties":{"code":{"type":"string"}},
                "required":["code"],
                "additionalProperties":false
            }),
        },
        ToolDefinition {
            name: "ask".into(),
            description: "Pause durably and ask the human one direct question.".into(),
            parameters: json!({
                "type":"object",
                "properties":{"question":{"type":"string"}},
                "required":["question"],
                "additionalProperties":false
            }),
        },
        ToolDefinition {
            name: "yield".into(),
            description: "Submit the node's final value. It must match the node output schema.".into(),
            parameters: json!({
                "type":"object",
                "properties":{"value":{}},
                "required":["value"],
                "additionalProperties":false
            }),
        },
    ]
}

#[derive(Debug, thiserror::Error)]
pub enum RigDriverError {
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("context error: {0}")]
    Context(String),
    #[error(transparent)]
    Completion(#[from] CompletionError),
}

/// Calls any Rig completion model for one turn, then durably records its response.
/// Tool execution is kept separate so scheduler policy can pause/restart between actions.
pub async fn complete_turn<M>(
    runtime: &Runtime,
    run_id: RunId,
    model: &M,
) -> Result<Vec<AssistantContent>, RigDriverError>
where
    M: CompletionModel + Clone,
{
    let run = runtime.store().run(run_id).map_err(RuntimeError::from)?;
    let node = runtime
        .store()
        .get_node(run.node_id)
        .map_err(RuntimeError::from)?;
    let context = ContextBuilder::new(runtime.store())
        .build(run_id)
        .map_err(|error| RigDriverError::Context(error.to_string()))?;
    let messages = context.into_iter().map(to_rig_message).collect::<Vec<_>>();
    let request = model
        .clone()
        .completion_request(
            "Continue this run. Use only the stable tools when an action is needed.",
        )
        .messages(messages)
        .tools(stable_tools())
        .temperature_opt(node.model.temperature)
        .max_tokens_opt(node.model.max_tokens)
        .build();
    let request_id = runtime.begin_model_request(run_id)?;
    let response = match model.completion(request).await {
        Ok(response) => response,
        Err(error) => {
            runtime.interrupt(run_id, format!("model:{request_id}"), error.to_string())?;
            return Err(error.into());
        }
    };
    runtime.record_model_response(
        run_id,
        request_id,
        serde_json::to_value(&response.choice).expect("Rig response is serializable"),
    )?;
    Ok(response.choice)
}

fn to_rig_message(entry: ContextEntry) -> Message {
    let text = match &entry.content {
        Value::String(text) => text.clone(),
        value => serde_json::to_string(value).expect("JSON context is serializable"),
    };
    match entry.role {
        ContextRole::System => Message::System { content: text },
        ContextRole::Assistant => {
            if let Ok(content) = serde_json::from_value::<Vec<AssistantContent>>(entry.content) {
                Message::Assistant { id: None, content }
            } else {
                Message::Assistant {
                    id: None,
                    content: vec![AssistantContent::text(text)],
                }
            }
        }
        ContextRole::User | ContextRole::Tool | ContextRole::Harness => Message::User {
            content: vec![UserContent::text(text)],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_tool_surface_is_fixed() {
        let names: Vec<_> = stable_tools().into_iter().map(|tool| tool.name).collect();
        assert_eq!(names, ["python", "ask", "yield"]);
    }
}
