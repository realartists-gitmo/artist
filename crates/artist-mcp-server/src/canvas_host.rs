//! The MCP server's side of the canvas bridge.
//!
//! `artist-canvas` describes what a canvas may do; this decides whether it may.
//! Over MCP the "live session" is the server's own tool surface, so a canvas
//! dispatches through [`McpServer::invoke`] — the same path a model tool call
//! takes — rather than a second assembly that could drift.
//!
//! What is genuinely different from the CLI host: there is no running agent
//! loop in the MCP process, so `send` cannot steer a live turn (it logs and
//! reports [`SendOutcome::NoTurnRunning`]), and questions live in the durable
//! outbox rather than an in-process registry, so a canvas answering one records
//! the same answer the model's `ask_result` poll sees.

use std::sync::OnceLock;

use artist_canvas::bridge::{CanvasHost, Denied, HostFuture, SendMode, SendOutcome};
use artist_session::ask::{Answer, Question};

use crate::McpServer;

/// The canvas host for one MCP server.
#[derive(Clone)]
pub struct McpCanvasHost {
    server: OnceLock<McpServer>,
    outbox: artist_session::AskOutbox,
    actor: String,
    project: String,
    profile: String,
}

impl McpCanvasHost {
    pub fn new(
        outbox: artist_session::AskOutbox,
        actor: &str,
        project: &std::path::Path,
        profile: &str,
    ) -> Self {
        Self {
            server: OnceLock::new(),
            outbox,
            actor: actor.to_owned(),
            project: project.display().to_string(),
            profile: profile.to_owned(),
        }
    }

    /// Point the host at the server its surface is part of. Called once, after
    /// the `McpServer` is built — the surface cannot exist without the host,
    /// and the host cannot dispatch without the surface, so the link is made in
    /// between.
    pub fn attach(&self, server: McpServer) {
        let _ = self.server.set(server);
    }
}

impl CanvasHost for McpCanvasHost {
    fn send(&self, text: String, mode: SendMode) -> HostFuture<'_, SendOutcome> {
        tracing::info!(%text, ?mode, "canvas tried to send text over MCP; no turn to steer");
        Box::pin(async { SendOutcome::NoTurnRunning })
    }

    fn call_tool(
        &self,
        tool: String,
        arguments: serde_json::Value,
        allowed: Vec<String>,
    ) -> HostFuture<'_, Result<String, Denied>> {
        Box::pin(async move {
            // Two gates, in this order, because the messages differ: telling
            // the canvas to add a permission the model could never use would
            // be wrong. The "permitted" set is the published surface — exactly
            // the tools the model can call — so a canvas can never widen its
            // own reach past the model's.
            let Some(server) = self.server.get() else {
                return Err(Denied::Unknown { tool });
            };
            if !server.names().iter().any(|name| name == &tool) {
                return Err(Denied::NotPermitted { tool });
            }
            if !allowed.iter().any(|name| name == &tool) {
                return Err(Denied::NotDeclared { tool });
            }
            let result = server.invoke(&tool, arguments, Default::default()).await;
            Ok(render(result))
        })
    }

    fn state_changed(&self, _slug: &str, _keys: Vec<String>) {}

    fn pending_questions(&self) -> Vec<Question> {
        self.outbox.pending()
    }

    fn answer_question(&self, answer: Answer, _surface: &str) -> bool {
        self.outbox.answer(answer).unwrap_or(false)
    }

    fn context(&self) -> serde_json::Value {
        serde_json::json!({
            "attached": true,
            "busy": false,
            "model": null,
            "profile": self.profile,
            "project": self.project,
            "actor": self.actor,
        })
    }
}

/// Collapse a tool result the way the model sees it, for the page to show.
fn render(result: rmcp::model::CallToolResult) -> String {
    let mut text = result
        .content
        .iter()
        .filter_map(|block| match block {
            rmcp::model::ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if let Some(json) = result.structured_content {
        let rendered = json.to_string();
        if !text.contains(&rendered) {
            text.push('\n');
            text.push_str(&rendered);
        }
    }
    text
}
