//! What a canvas is allowed to do to the running agent.
//!
//! The canvas crate cannot depend on the agent crate — the agent registers the
//! canvas tool, so the arrow points the other way. Instead this defines the
//! shape of a host and lets `artist-cli` implement it against the live session,
//! exactly as `ExtensionControl` implements `HostControl` for extensions.
//!
//! Everything here is deliberately narrow. A canvas may prompt, invoke a tool
//! it was granted, and answer a question. It cannot reach into the session,
//! rewrite history, or widen its own permissions.

use std::{future::Future, pin::Pin};

use artist_session::ask::{Answer, Question};
use serde::{Deserialize, Serialize};

pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// When a prompt from a canvas should reach the model.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SendMode {
    /// Correct the model mid-turn. Delivered on the next tool result.
    Steer,
    /// Start a turn once the current one finishes.
    Next,
    /// Steer while a turn is in flight, prompt otherwise.
    ///
    /// This is the default because the page cannot know which it wants: the
    /// user clicked a button, and whether a turn happens to be running is not
    /// something the button's author should have to reason about.
    #[default]
    Auto,
}

/// Why a tool call from a canvas was refused.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Denied {
    /// The canvas did not declare it in `[permissions] allow`.
    NotDeclared { tool: String },
    /// The profile or settings deny it, so the model could not call it either.
    NotPermitted { tool: String },
    /// No such tool is registered this turn.
    Unknown { tool: String },
}

impl std::fmt::Display for Denied {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Denied::NotDeclared { tool } => write!(
                formatter,
                "this canvas has not declared `{tool}` — add it to [permissions] allow in canvas.toml"
            ),
            Denied::NotPermitted { tool } => write!(
                formatter,
                "`{tool}` is denied by the active profile or settings"
            ),
            Denied::Unknown { tool } => write!(formatter, "no tool named `{tool}` is registered"),
        }
    }
}

/// The agent, as far as a canvas can see it.
pub trait CanvasHost: Send + Sync {
    /// Put text into the conversation.
    fn send(&self, text: String, mode: SendMode) -> HostFuture<'_, ()>;

    /// Invoke a tool on the canvas's behalf.
    ///
    /// Implementations must apply the same gate a model tool call goes through,
    /// then narrow further by the canvas's own declaration. `allowed` is the
    /// set the canvas declared, passed in so the policy decision stays with the
    /// host that owns the profile and settings.
    fn call_tool(
        &self,
        tool: String,
        arguments: serde_json::Value,
        allowed: Vec<String>,
    ) -> HostFuture<'_, Result<String, Denied>>;

    /// Questions currently awaiting an answer.
    fn pending_questions(&self) -> Vec<Question>;

    /// Answer one. False if it was already answered elsewhere.
    fn answer_question(&self, answer: Answer, surface: &str) -> bool;

    /// A description of the session for the page to render — model, profile,
    /// project, and whether a turn is running.
    fn context(&self) -> serde_json::Value;
}

/// A host that refuses everything.
///
/// Used when a canvas is served outside a session — `examples/serve`, tests,
/// and any future read-only viewer. Refusing is the right default: a page that
/// silently no-ops would look like a bug in the canvas.
#[derive(Clone, Copy, Debug, Default)]
pub struct DetachedHost;

impl CanvasHost for DetachedHost {
    fn send(&self, _text: String, _mode: SendMode) -> HostFuture<'_, ()> {
        Box::pin(async {})
    }

    fn call_tool(
        &self,
        tool: String,
        _arguments: serde_json::Value,
        _allowed: Vec<String>,
    ) -> HostFuture<'_, Result<String, Denied>> {
        Box::pin(async move { Err(Denied::Unknown { tool }) })
    }

    fn pending_questions(&self) -> Vec<Question> {
        Vec::new()
    }

    fn answer_question(&self, _answer: Answer, _surface: &str) -> bool {
        false
    }

    fn context(&self) -> serde_json::Value {
        serde_json::json!({"attached": false})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_is_the_default_send_mode() {
        assert_eq!(SendMode::default(), SendMode::Auto);
        assert_eq!(
            serde_json::from_str::<SendMode>("\"steer\"").expect("parses"),
            SendMode::Steer
        );
    }

    /// The message is what the model reads when a call is refused, so it has to
    /// say what to change rather than just that something was denied.
    #[test]
    fn a_refusal_says_how_to_fix_it() {
        let denied = Denied::NotDeclared {
            tool: "bash".into(),
        };
        let message = denied.to_string();
        assert!(message.contains("canvas.toml"), "{message}");
        assert!(message.contains("bash"), "{message}");
    }

    #[tokio::test]
    async fn a_detached_host_refuses_rather_than_silently_doing_nothing() {
        let host = DetachedHost;
        let result = host
            .call_tool("read".into(), serde_json::json!({}), vec!["read".into()])
            .await;
        assert!(matches!(result, Err(Denied::Unknown { .. })));
    }
}
