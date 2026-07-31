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
///
/// There is deliberately no default and no `auto`. `auto` resolved by whether a
/// turn happened to be running, so the same button either interrupted the
/// agent's current work or scheduled new work depending on timing the button's
/// author could not observe — a race the user loses, dressed as a convenience.
/// Making the caller choose costs one argument and removes the ambiguity.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SendMode {
    /// Correct the model mid-turn. Delivered on the next tool result, and
    /// refused outright when no turn is running rather than silently dropped.
    Steer,
    /// Start a turn once the current one finishes.
    Queue,
}

/// What became of a prompt the page sent.
///
/// Returned so the page can show the user something. A button that fires into
/// silence is the loop's worst moment, and `steer` with no turn running used to
/// be exactly that.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SendOutcome {
    /// Delivered into the running turn.
    Steered,
    /// Will start a turn when the current one ends, or immediately if idle.
    Queued,
    /// `steer` was asked for with nothing to steer.
    NoTurnRunning,
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
    /// Put text into the conversation, reporting what became of it.
    fn send(&self, text: String, mode: SendMode) -> HostFuture<'_, SendOutcome>;

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

    /// A canvas wrote state it wants acted on.
    ///
    /// State is otherwise passive: the model sees it only when it thinks to
    /// ask, so a click while no turn is running was invisible. This is the
    /// nudge — a badge in the terminal, not a fabricated user message.
    fn state_changed(&self, slug: &str, keys: Vec<String>);

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
    fn send(&self, _text: String, _mode: SendMode) -> HostFuture<'_, SendOutcome> {
        Box::pin(async { SendOutcome::NoTurnRunning })
    }

    fn call_tool(
        &self,
        tool: String,
        _arguments: serde_json::Value,
        _allowed: Vec<String>,
    ) -> HostFuture<'_, Result<String, Denied>> {
        Box::pin(async move { Err(Denied::Unknown { tool }) })
    }

    fn state_changed(&self, _slug: &str, _keys: Vec<String>) {}

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

    /// No default: a page has to say which it means. `auto` silently picked
    /// based on whether a turn was running, which the page cannot see.
    #[test]
    fn a_send_mode_must_be_named() {
        assert_eq!(
            serde_json::from_str::<SendMode>("\"steer\"").expect("parses"),
            SendMode::Steer
        );
        assert_eq!(
            serde_json::from_str::<SendMode>("\"queue\"").expect("parses"),
            SendMode::Queue
        );
        assert!(serde_json::from_str::<SendMode>("\"auto\"").is_err());
        assert!(serde_json::from_str::<SendMode>("null").is_err());
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
