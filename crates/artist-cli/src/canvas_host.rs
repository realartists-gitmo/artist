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

use artist_agent::ToolRegistryHandle;
use artist_canvas::bridge::{CanvasHost, Denied, HostFuture, SendMode, SendOutcome};
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
    /// The live registry the agent loop publishes each attempt. Dispatching
    /// through it rather than a second assembly is what guarantees a canvas
    /// and the model cannot disagree about what exists.
    registry: Mutex<Option<ToolRegistryHandle>>,
    /// A turn is running, so `auto` should steer rather than queue.
    busy: AtomicBool,
    session: Mutex<SessionInfo>,
    /// Tool calls a canvas made, for the TUI to show. A page acting on the
    /// user's behalf must be visible, not silent.
    audit: Mutex<Vec<AuditEntry>>,
    /// Canvases that wrote state asking to be noticed, and which keys.
    nudges: Mutex<Vec<(String, Vec<String>)>>,
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
    pub fn attach(
        &self,
        control: crate::extension_control::ExtensionControl,
        ask: AskRegistry,
        registry: ToolRegistryHandle,
    ) {
        *self.inner.control.lock().expect("canvas control poisoned") = Some(control);
        *self.inner.ask.lock().expect("canvas ask poisoned") = Some(ask);
        *self
            .inner
            .registry
            .lock()
            .expect("canvas registry poisoned") = Some(registry);
    }

    /// The tools the model itself can call right now.
    fn permitted(&self) -> Vec<String> {
        self.inner
            .registry
            .lock()
            .expect("canvas registry poisoned")
            .as_ref()
            .map(ToolRegistryHandle::names)
            .unwrap_or_default()
    }

    pub fn set_busy(&self, busy: bool) {
        self.inner.busy.store(busy, Ordering::Release);
    }

    pub fn set_session(
        &self,
        model: Option<String>,
        profile: Option<String>,
        project: Option<String>,
    ) {
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

    /// Drain the canvases asking to be noticed, for the TUI to badge.
    pub fn take_nudges(&self) -> Vec<(String, Vec<String>)> {
        std::mem::take(&mut *self.inner.nudges.lock().expect("canvas nudges poisoned"))
    }

    pub fn ask_registry(&self) -> Option<AskRegistry> {
        self.inner.ask.lock().expect("canvas ask poisoned").clone()
    }
}

impl CanvasHost for CanvasControl {
    fn send(&self, text: String, mode: SendMode) -> HostFuture<'_, SendOutcome> {
        Box::pin(async move {
            let control = self
                .inner
                .control
                .lock()
                .expect("canvas control poisoned")
                .clone();
            let Some(control) = control else {
                return SendOutcome::NoTurnRunning;
            };

            use artist_extensions::HostControl as _;
            // Steering is only ever delivered on a tool result, so asking for it
            // with no turn running dropped the click on the floor with nothing
            // shown on either side. Refuse it and say so instead.
            if mode == SendMode::Steer && !self.inner.busy.load(Ordering::Acquire) {
                self.record_audit("send (steer, no turn)", false);
                return SendOutcome::NoTurnRunning;
            }

            // Audited like a tool call: a page injecting text into the
            // conversation is if anything more notable than one reading a file.
            match mode {
                SendMode::Steer => {
                    self.record_audit("send (steer)", true);
                    control.steer(text).await;
                    SendOutcome::Steered
                }
                SendMode::Queue => {
                    self.record_audit("send (queue)", true);
                    control.prompt_after(text).await;
                    SendOutcome::Queued
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
            // Two gates, in this order, because the messages differ: telling
            // the model to add a permission it could never use would be wrong.
            let permitted = self.permitted();
            if !permitted.iter().any(|name| name == &tool) {
                self.record_audit(&tool, false);
                return Err(Denied::NotPermitted { tool });
            }
            if !allowed.iter().any(|name| name == &tool) {
                self.record_audit(&tool, false);
                return Err(Denied::NotDeclared { tool });
            }
            self.record_audit(&tool, true);
            let registry = self
                .inner
                .registry
                .lock()
                .expect("canvas registry poisoned")
                .clone();
            let Some(registry) = registry else {
                return Err(Denied::Unknown { tool });
            };
            match registry.execute(&tool, arguments).await {
                // A tool that failed is not a tool that is missing: the page
                // gets the error text so it can show the user what went wrong.
                Some(Ok(output)) => Ok(output),
                Some(Err(error)) => Ok(format!("tool error: {error}")),
                None => Err(Denied::Unknown { tool }),
            }
        })
    }

    fn state_changed(&self, slug: &str, keys: Vec<String>) {
        let mut nudges = self.inner.nudges.lock().expect("canvas nudges poisoned");
        if nudges.len() >= 32 {
            nudges.remove(0);
        }
        nudges.push((slug.to_owned(), keys));
    }

    fn pending_questions(&self) -> Vec<Question> {
        self.ask_registry()
            .map(|registry| registry.pending())
            .unwrap_or_default()
    }

    fn answer_question(&self, answer: Answer, surface: &str) -> bool {
        self.ask_registry()
            .is_some_and(|registry| registry.answer_from(answer, surface))
    }

    fn context(&self) -> serde_json::Value {
        let session = self
            .inner
            .session
            .lock()
            .expect("canvas session poisoned")
            .clone();
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

    fn echo(name: &str) -> artist_tool_api::ArtistDynamicTool {
        let portable = rig_core::tool::PortableDynamicTool::new(
            name,
            "echo",
            serde_json::json!({"type": "object"}),
            |arguments: serde_json::Value| {
                Box::pin(async move { Ok(rig_core::tool::ToolOutput::text(arguments.to_string())) })
            },
        );
        artist_tool_api::ArtistDynamicTool::from_portable(
            portable,
            artist_tool_api::text_output_schema(name, "Echoed text."),
            artist_tool_api::ToolCategory::Administration,
            artist_tool_api::ArtistToolAnnotations::read_only(),
        )
    }

    /// Build a control whose registry publishes `tools`, mimicking what the
    /// agent loop does at the start of every attempt.
    fn control_with(tools: &[&str]) -> CanvasControl {
        let registry = ToolRegistryHandle::new();
        registry.publish(tools.iter().map(|name| echo(name)).collect());
        let canvas = CanvasControl::default();
        canvas.attach(
            crate::extension_control::ExtensionControl::default(),
            AskRegistry::new(),
            registry,
        );
        canvas
    }

    fn control() -> CanvasControl {
        control_with(&["read"])
    }

    /// A canvas must never be a way around the profile. Declaring a tool it was
    /// not granted is refused as *not permitted*, not as *not declared* — the
    /// distinction is what tells the model whether editing canvas.toml helps.
    #[tokio::test]
    async fn a_canvas_cannot_widen_its_own_reach() {
        let canvas = control_with(&["read"]);

        let denied = canvas
            .call_tool("bash".into(), serde_json::json!({}), vec!["bash".into()])
            .await
            .expect_err("refused");
        assert!(matches!(denied, Denied::NotPermitted { .. }), "{denied:?}");
    }

    /// The converse: permitted for the model, but this canvas never asked.
    #[tokio::test]
    async fn a_tool_the_canvas_did_not_declare_is_refused() {
        let canvas = control_with(&["read", "bash"]);

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
        let canvas = control_with(&["read"]);
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
    async fn steering_is_refused_when_there_is_no_turn_to_steer() {
        let extension = crate::extension_control::ExtensionControl::default();
        let canvas = CanvasControl::default();
        canvas.attach(
            extension.clone(),
            AskRegistry::new(),
            ToolRegistryHandle::new(),
        );

        canvas.set_busy(false);
        assert_eq!(
            canvas.send("while idle".into(), SendMode::Queue).await,
            SendOutcome::Queued
        );
        assert_eq!(extension.take_prompts(), ["while idle"]);

        // Steering with nothing to steer is refused rather than swallowed.
        assert_eq!(
            canvas
                .send("nothing to correct".into(), SendMode::Steer)
                .await,
            SendOutcome::NoTurnRunning
        );
        assert!(
            extension.take_prompts().is_empty(),
            "it must not become a queued turn"
        );

        canvas.set_busy(true);
        assert_eq!(
            canvas.send("mid turn".into(), SendMode::Steer).await,
            SendOutcome::Steered
        );
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
            ToolRegistryHandle::new(),
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

    /// The whole point of publishing the registry: a permitted, declared tool
    /// actually runs, rather than being refused as unknown.
    #[tokio::test]
    async fn a_permitted_declared_tool_actually_runs() {
        let canvas = control_with(&["read"]);

        let output = canvas
            .call_tool(
                "read".into(),
                serde_json::json!({"path": "x.rs"}),
                vec!["read".into()],
            )
            .await
            .expect("dispatched");
        assert!(output.contains("x.rs"), "{output}");

        let audit = canvas.take_audit();
        assert_eq!(audit.len(), 1);
        assert!(audit[0].allowed);
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
