//! The tool registry, published so surfaces outside the loop can reach it.
//!
//! The registry is rebuilt on every attempt — profile policy, the disabled
//! list, and the live MCP and extension sets all feed it — so it exists only
//! inside `stream_chat_with`. A canvas needs to invoke exactly the tools the
//! model can invoke, which means it needs *that* list, not a second one
//! assembled alongside it.
//!
//! Reassembling it elsewhere would be the mistake: two registries drift, and
//! the one the canvas sees would eventually permit something the model's
//! cannot. So the loop publishes a snapshot here, and everything else reads it.

use std::sync::{Arc, Mutex};

use rig_core::tool::{PortableDynamicTool, ToolExecutionError};
use tokio_util::sync::CancellationToken;

/// A shared, swappable view of the tools registered for the current attempt.
#[derive(Clone, Default)]
pub struct ToolRegistryHandle {
    tools: Arc<Mutex<Arc<Vec<PortableDynamicTool>>>>,
    /// Cancelled and replaced on every publish.
    ///
    /// A canvas call that began under one attempt would otherwise keep running
    /// against a tool the next attempt removed from policy — and if a tool was
    /// disabled *because* the turn went wrong, that is exactly the call you do
    /// not want completing.
    generation: Arc<Mutex<CancellationToken>>,
}

impl ToolRegistryHandle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the published set. Called once per attempt, after policy has
    /// already filtered it — so what lands here is exactly what the model got.
    pub fn publish(&self, tools: Vec<PortableDynamicTool>) {
        *self.tools.lock().expect("tool registry poisoned") = Arc::new(tools);
        let mut generation = self.generation.lock().expect("tool registry poisoned");
        generation.cancel();
        *generation = CancellationToken::new();
    }

    /// Names currently registered, for permission checks and diagnostics.
    pub fn names(&self) -> Vec<String> {
        self.snapshot()
            .iter()
            .map(|tool| tool.name().to_owned())
            .collect()
    }

    /// The JSON Schema a tool publishes, so a canvas can render a form for it.
    pub fn schema(&self, name: &str) -> Option<serde_json::Value> {
        self.snapshot()
            .iter()
            .find(|tool| tool.name() == name)
            .map(|tool| tool.definition().parameters)
    }

    /// Invoke a tool by name.
    ///
    /// The lookup clones out of the snapshot before awaiting: holding the lock
    /// across the call would serialize every canvas invocation behind the
    /// slowest one, and a tool that blocks would freeze the next attempt's
    /// publish.
    pub async fn execute(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Option<Result<String, ToolExecutionError>> {
        let tool = self
            .snapshot()
            .iter()
            .find(|tool| tool.name() == name)
            .cloned()?;
        let cancel = self
            .generation
            .lock()
            .expect("tool registry poisoned")
            .clone();
        Some(tokio::select! {
            result = tool.execute(arguments) => result.map(flatten),
            () = cancel.cancelled() => Err(ToolExecutionError::other(
                "the turn moved on before this canvas call finished",
            )),
        })
    }

    fn snapshot(&self) -> Arc<Vec<PortableDynamicTool>> {
        Arc::clone(&self.tools.lock().expect("tool registry poisoned"))
    }
}

/// Collapse a tool result the same way the streaming loop does, so a canvas
/// sees what the model would have seen.
fn flatten(output: rig_core::tool::ToolOutput) -> String {
    use rig_core::completion::message::ToolResultContent;

    output
        .into_content()
        .into_iter()
        .filter_map(|item| match item {
            ToolResultContent::Text(text) => Some(text.text),
            ToolResultContent::Json { value } => Some(value.to_string()),
            // Images ride the event log, not a JSON response body.
            ToolResultContent::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl std::fmt::Debug for ToolRegistryHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolRegistryHandle")
            .field("tools", &self.names())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo(name: &str) -> PortableDynamicTool {
        PortableDynamicTool::new(
            name,
            "echo",
            serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}}),
            |arguments: serde_json::Value| {
                Box::pin(async move {
                    Ok(rig_core::tool::ToolOutput::text(
                        arguments
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_owned(),
                    ))
                })
            },
        )
    }

    #[tokio::test]
    async fn a_published_tool_can_be_invoked_by_name() {
        let registry = ToolRegistryHandle::new();
        registry.publish(vec![echo("read")]);

        let result = registry
            .execute("read", serde_json::json!({"text": "hello"}))
            .await
            .expect("registered")
            .expect("ran");
        assert_eq!(result, "hello");
    }

    /// An unknown name is `None`, not an error: the caller distinguishes "no
    /// such tool" from "the tool failed", and they read very differently.
    #[tokio::test]
    async fn an_unknown_tool_is_absent_rather_than_failing() {
        let registry = ToolRegistryHandle::new();
        registry.publish(vec![echo("read")]);
        assert!(
            registry
                .execute("bash", serde_json::json!({}))
                .await
                .is_none()
        );
    }

    /// Each attempt republishes. A canvas must see the current set, because
    /// profile policy and the disabled list can change between turns.
    #[tokio::test]
    async fn republishing_replaces_the_previous_set() {
        let registry = ToolRegistryHandle::new();
        registry.publish(vec![echo("read"), echo("write")]);
        assert_eq!(registry.names(), ["read", "write"]);

        registry.publish(vec![echo("read")]);
        assert_eq!(registry.names(), ["read"]);
        assert!(
            registry
                .execute("write", serde_json::json!({}))
                .await
                .is_none()
        );
    }

    /// A call in flight when policy changes must not outlive the policy that
    /// permitted it.
    #[tokio::test]
    async fn republishing_cancels_a_call_already_running() {
        let registry = ToolRegistryHandle::new();
        registry.publish(vec![PortableDynamicTool::new(
            "slow",
            "never finishes",
            serde_json::json!({"type": "object"}),
            |_| {
                Box::pin(async {
                    std::future::pending::<()>().await;
                    unreachable!()
                })
            },
        )]);

        let running = {
            let registry = registry.clone();
            tokio::spawn(async move { registry.execute("slow", serde_json::json!({})).await })
        };
        tokio::task::yield_now().await;

        registry.publish(Vec::new());
        let outcome = running.await.expect("joined").expect("was registered");
        assert!(outcome.is_err(), "the call should have been cancelled");
    }

    #[test]
    fn an_unpublished_registry_is_empty_rather_than_panicking() {
        let registry = ToolRegistryHandle::new();
        assert!(registry.names().is_empty());
        assert!(registry.schema("read").is_none());
    }

    /// A canvas renders SchemaForm from this, so it has to be the same schema
    /// the model was given.
    #[test]
    fn schemas_are_readable_for_form_rendering() {
        let registry = ToolRegistryHandle::new();
        registry.publish(vec![echo("read")]);

        let schema = registry.schema("read").expect("schema");
        assert_eq!(schema["properties"]["text"]["type"], "string");
    }
}
