//! The CLI's side of the canvas bridge.
//!
//! `artist-canvas` describes what a canvas may do; this decides whether it may.
//! The policy lives here because this is where the profile, the settings, and
//! the live session actually are — the canvas crate never sees them.
//!
//! Mirrors `ExtensionControl`, which does the same job for WASM extensions.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use artist_canvas::bridge::{CanvasHost, Denied, HostFuture, SendMode};
use artist_session::ask::{Answer, AskRegistry, Question};

/// What the canvas bridge is allowed to reach.
#[derive(Clone, Default)]
pub struct CanvasControl {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Where a canvas-originated prompt goes. Shared with the extension path so
    /// both arrive through the same drain in the TUI loop.
    control: Mutex<Option<crate::extension_control::ExtensionControl>>,
    ask: Mutex<Option<AskRegistry>>,
    /// Tools the model itself may call this turn. A canvas can only ever be a
    /// subset of this — never a way around it.
    permitted: Mutex<Vec<String>>,
    /// A turn is running, so `auto` should steer rather than queue.
    busy: AtomicBool,
    session: Mutex<SessionInfo>,
    /// Tool calls a canvas made, for the TUI to show. A page acting on the
    /// user's behalf must be visible, not silent.
    audit: Mutex<Vec<AuditEntry>>,
}

#[derive(Clone, Debug, Default)]
struct SessionInfo {
    model: Option<String>,
    profile: Option<String>,
    project: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AuditEntry {
    pub tool: String,
    pub allowed: bool,
}

impl CanvasControl {
    pub fn attach(&self, control: crate::extension_control::ExtensionControl, ask: AskRegistry) {
        *self.inner.control.lock().expect("canvas control poisoned") = Some(control);
        *self.inner.ask.lock().expect("canvas ask poisoned") = Some(ask);
    }

    /// Publish the tool names the active profile permits this turn.
    pub fn set_permitted(&self, names: Vec<String>) {
        *self.inner.permitted.lock().expect("canvas permits poisoned") = names;
    }

    pub fn set_busy(&self, busy: bool) {
        self.inner.busy.store(busy, Ordering::Release);
    }

    pub fn set_session(&self, model: Option<String>, profile: Option<String>, project: Option<String>) {
        *self.inner.session.lock().expect("canvas session poisoned") = SessionInfo {
            model,
            profile,
            project,
        };
    }

    /// Drain what canvases have done since the last check, for the transcript.
    pub fn take_audit(&self) -> Vec<AuditEntry> {
        std::mem::take(&mut *self.inner.audit.lock().expect("canvas audit poisoned"))
    }

    pub fn ask_registry(&self) -> Option<AskRegistry> {
        self.inner.ask.lock().expect("canvas ask poisoned").clone()
    }
}

impl CanvasHost for CanvasControl {
    fn send(&self, text: String, mode: SendMode) -> HostFuture<'_, ()> {
        Box::pin(async move {
            let control = self
                .inner
                .control
                .lock()
                .expect("canvas control poisoned")
                .clone();
            let Some(control) = control else { return };

            use artist_extensions::HostControl as _;
            let steer = match mode {
                SendMode::Steer => true,
                SendMode::Next => false,
                // A button's author cannot know whether a turn happens to be
                // running; pick the delivery that reaches the model soonest.
                SendMode::Auto => self.inner.busy.load(Ordering::Acquire),
            };
            if steer {
                control.steer(text).await;
            } else {
                control.prompt_after(text).await;
            }
        })
    }

    fn call_tool(
        &self,
        tool: String,
        _arguments: serde_json::Value,
        allowed: Vec<String>,
    ) -> HostFuture<'_, Result<String, Denied>> {
        Box::pin(async move {
            // Two gates, in this order, because the messages differ: telling
            // the model to add a permission it could never use would be wrong.
            let permitted = self
                .inner
                .permitted
                .lock()
                .expect("canvas permits poisoned")
                .clone();
            if !permitted.iter().any(|name| name == &tool) {
                self.record_audit(&tool, false);
                return Err(Denied::NotPermitted { tool });
            }
            if !allowed.iter().any(|name| name == &tool) {
                self.record_audit(&tool, false);
                return Err(Denied::NotDeclared { tool });
            }
            self.record_audit(&tool, true);
            // Dispatch is not wired yet: the registry is rebuilt per attempt
            // inside the agent loop and is not reachable from here. Refusing
            // loudly beats appearing to work.
            Err(Denied::Unknown { tool })
        })
    }

    fn pending_questions(&self) -> Vec<Question> {
        self.ask_registry()
            .map(|registry| registry.pending())
            .unwrap_or_default()
    }

    fn answer_question(&self, answer: Answer, _surface: &str) -> bool {
        self.ask_registry()
            .is_some_and(|registry| registry.answer(answer))
    }

    fn context(&self) -> serde_json::Value {
        let session = self.inner.session.lock().expect("canvas session poisoned").clone();
        serde_json::json!({
            "attached": true,
            "busy": self.inner.busy.load(Ordering::Acquire),
            "model": session.model,
            "profile": session.profile,
            "project": session.project,
        })
    }
}

impl CanvasControl {
    fn record_audit(&self, tool: &str, allowed: bool) {
        let mut audit = self.inner.audit.lock().expect("canvas audit poisoned");
        // Bounded: a page in a loop must not grow this without limit.
        if audit.len() >= 256 {
            audit.remove(0);
        }
        audit.push(AuditEntry {
            tool: tool.to_owned(),
            allowed,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control() -> CanvasControl {
        let canvas = CanvasControl::default();
        canvas.attach(
            crate::extension_control::ExtensionControl::default(),
            AskRegistry::new(),
        );
        canvas
    }

    /// A canvas must never be a way around the profile. Declaring a tool it was
    /// not granted is refused as *not permitted*, not as *not declared* — the
    /// distinction is what tells the model whether editing canvas.toml helps.
    #[tokio::test]
    async fn a_canvas_cannot_widen_its_own_reach() {
        let canvas = control();
        canvas.set_permitted(vec!["read".into()]);

        let denied = canvas
            .call_tool("bash".into(), serde_json::json!({}), vec!["bash".into()])
            .await
            .expect_err("refused");
        assert!(matches!(denied, Denied::NotPermitted { .. }), "{denied:?}");
    }

    /// The converse: permitted for the model, but this canvas never asked.
    #[tokio::test]
    async fn a_tool_the_canvas_did_not_declare_is_refused() {
        let canvas = control();
        canvas.set_permitted(vec!["read".into(), "bash".into()]);

        let denied = canvas
            .call_tool("bash".into(), serde_json::json!({}), vec!["read".into()])
            .await
            .expect_err("refused");
        assert!(matches!(denied, Denied::NotDeclared { .. }), "{denied:?}");
        assert!(denied.to_string().contains("canvas.toml"));
    }

    /// Every attempt is auditable, refusals included — a page probing for tools
    /// it cannot reach is exactly what the user needs to see.
    #[tokio::test]
    async fn refused_calls_are_still_audited() {
        let canvas = control();
        canvas.set_permitted(vec!["read".into()]);
        let _ = canvas
            .call_tool("bash".into(), serde_json::json!({}), vec![])
            .await;

        let audit = canvas.take_audit();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].tool, "bash");
        assert!(!audit[0].allowed);
        assert!(canvas.take_audit().is_empty(), "draining is destructive");
    }

    /// Auto must reach the model soonest: steering lands on the next tool
    /// result mid-turn, but is never delivered if no turn is running.
    #[tokio::test]
    async fn auto_steers_mid_turn_and_queues_when_idle() {
        let extension = crate::extension_control::ExtensionControl::default();
        let canvas = CanvasControl::default();
        canvas.attach(extension.clone(), AskRegistry::new());

        canvas.set_busy(false);
        canvas.send("while idle".into(), SendMode::Auto).await;
        assert_eq!(extension.take_prompts(), ["while idle"]);

        canvas.set_busy(true);
        canvas.send("mid turn".into(), SendMode::Auto).await;
        // Steering goes to the steering handle, not the prompt queue.
        assert!(extension.take_prompts().is_empty());
    }

    #[tokio::test]
    async fn questions_are_answerable_through_the_bridge() {
        let registry = AskRegistry::new();
        let canvas = CanvasControl::default();
        canvas.attach(
            crate::extension_control::ExtensionControl::default(),
            registry.clone(),
        );

        let waiting = registry.post(Question {
            id: "q1".into(),
            header: String::new(),
            question: "Which?".into(),
            multi_select: false,
            options: Vec::new(),
        });
        assert_eq!(canvas.pending_questions().len(), 1);

        assert!(canvas.answer_question(
            Answer {
                question_id: "q1".into(),
                selected: vec!["A".into()],
                notes: None,
            },
            "canvas:demo",
        ));
        assert_eq!(waiting.await.expect("answered").selected, ["A"]);
    }

    #[test]
    fn context_reports_the_live_session() {
        let canvas = control();
        canvas.set_session(Some("gpt-5-codex".into()), Some("default".into()), None);
        canvas.set_busy(true);

        let context = canvas.context();
        assert_eq!(context["model"], "gpt-5-codex");
        assert_eq!(context["busy"], true);
        assert_eq!(context["attached"], true);
    }
}
