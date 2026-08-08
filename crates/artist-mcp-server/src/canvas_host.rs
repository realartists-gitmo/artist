//! MCP canvas bridge. Canvas-internal tool calls use the owning artist's ordinary
//! profile-filtered surface but deliberately bypass transport mail consumption.

use std::sync::OnceLock;

use artist_canvas::bridge::{CanvasHost, Denied, HostFuture, SendMode, SendOutcome};
use artist_session::ask::{Answer, Question};

use crate::McpServer;

#[derive(Clone)]
pub struct McpCanvasHost {
    server: OnceLock<McpServer>,
    ask: artist_session::AskRegistry,
    artist: String,
    project: String,
    profile: String,
}

impl McpCanvasHost {
    pub fn new(
        ask: artist_session::AskRegistry,
        artist: &str,
        project: &std::path::Path,
        profile: &str,
    ) -> Self {
        Self {
            server: OnceLock::new(),
            ask,
            artist: artist.to_owned(),
            project: project.display().to_string(),
            profile: profile.to_owned(),
        }
    }

    pub fn attach(&self, server: McpServer) {
        let _ = self.server.set(server);
    }
}

impl CanvasHost for McpCanvasHost {
    fn send(&self, slug: &str, text: String, _mode: SendMode) -> HostFuture<'_, SendOutcome> {
        let artist = self.artist.clone();
        let sender = format!("canvas:{slug}");
        Box::pin(async move {
            let message = artist_registry::Message {
                id: artist_tools::short_id("mail"),
                from: sender,
                to: artist,
                audience: artist_registry::Audience::Direct,
                body: text,
                expects_reply: false,
                sent_at: artist_registry::now(),
            };
            match artist_registry::messages().send(&message) {
                Ok(()) => SendOutcome::Queued,
                Err(error) => {
                    tracing::warn!(%error, "canvas mail delivery failed");
                    SendOutcome::NoTurnRunning
                }
            }
        })
    }

    fn call_tool(
        &self,
        tool: String,
        arguments: serde_json::Value,
        allowed: Vec<String>,
    ) -> HostFuture<'_, Result<String, Denied>> {
        Box::pin(async move {
            let Some(server) = self.server.get() else {
                return Err(Denied::Unknown { tool });
            };
            if !server.names().iter().any(|name| name == &tool) {
                return Err(Denied::NotPermitted { tool });
            }
            if !allowed.iter().any(|name| name == &tool) {
                return Err(Denied::NotDeclared { tool });
            }
            // `invoke` is intentionally transport-neutral. In particular it must not
            // drain the artist mailbox merely because a canvas called a tool.
            let result = server.invoke(&tool, arguments, Default::default()).await;
            Ok(render(result))
        })
    }

    fn state_changed(&self, _slug: &str, _keys: Vec<String>) {}

    fn pending_questions(&self) -> Vec<Question> {
        self.ask.pending()
    }

    fn answer_question(&self, answer: Answer, surface: &str) -> bool {
        self.ask.answer_from(answer, surface)
    }

    fn context(&self) -> serde_json::Value {
        serde_json::json!({
            "attached": true,
            "busy": false,
            "model": null,
            "profile": self.profile,
            "project": self.project,
            "artist": self.artist,
        })
    }
}

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
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&rendered);
        }
    }
    text
}
