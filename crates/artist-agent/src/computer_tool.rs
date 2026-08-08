//! Shallow model-facing computer tool over the durable universal session substrate.

use std::{collections::BTreeSet, sync::Arc};

use artist_registry::SessionStatus;
use futures::future::BoxFuture;
use rig_core::{
    OneOrMany,
    completion::message::ToolResultContent,
    tool::{PortableTool, ToolExecutionError, ToolOutput},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::session_tools::{OwnedSession, OwnedState, SessionHub};

const ACTIONS: &[&str] = &[
    "launch",
    "attach",
    "observe",
    "find",
    "extract",
    "zoom",
    "screenshot",
    "focus",
    "resize",
    "watch",
    "do",
];

#[derive(Clone)]
pub(crate) struct ComputerTool {
    inner: Arc<artist_computer::ComputerTool>,
    registry: artist_computer::SurfaceRegistry,
    sessions: SessionHub,
    states: artist_registry::ArtistStates,
}

impl ComputerTool {
    pub fn new(
        registry: artist_computer::SurfaceRegistry,
        recorder: artist_session::Recorder,
        attachments: Option<artist_session::AttachmentStore>,
        sessions: SessionHub,
        project: &std::path::Path,
    ) -> Self {
        let inner = Arc::new(artist_computer::ComputerTool::with_recorder(
            registry.clone(),
            recorder,
            attachments,
        ));
        Self {
            inner,
            registry,
            sessions,
            states: artist_registry::Registry::for_project(project).artist_states(),
        }
    }

    fn take_reference_on_first_invocation(&self) -> Result<Option<String>, ComputerError> {
        let first = self
            .states
            .update(self.sessions.artist(), |state| {
                if state.computer_skill_delivered {
                    false
                } else {
                    state.computer_skill_delivered = true;
                    true
                }
            })
            .map_err(|error| ComputerError::plain(error.to_string()))?;
        Ok(first.then(|| reference(&self.inner)))
    }

    fn session_surface(&self, session: &str) -> Result<String, ComputerError> {
        let record = self
            .sessions
            .registry()
            .get(session)
            .map_err(|error| ComputerError::plain(error.to_string()))?
            .ok_or_else(|| ComputerError::plain(format!("unknown session `{session}`")))?;
        if record.kind != "computer" {
            return Err(ComputerError::plain(format!(
                "session `{session}` is kind `{}`, not computer",
                record.kind
            )));
        }
        if record.artist != self.sessions.artist() {
            return Err(ComputerError::plain(format!(
                "computer session `{session}` belongs to another artist"
            )));
        }
        if !record.lifecycle.is_live() {
            return Err(ComputerError::plain(format!(
                "computer session `{session}` is stopped"
            )));
        }
        record
            .snapshot
            .get("surface")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                ComputerError::plain(format!("computer session `{session}` has no live surface"))
            })
    }

    fn validate(
        &self,
        action: &str,
        session: Option<&str>,
        args: &Value,
    ) -> Result<(), ComputerError> {
        if !ACTIONS.contains(&action) {
            return Err(ComputerError::plain(format!(
                "unknown computer action `{action}`"
            )));
        }
        match action {
            "launch" | "attach" if session.is_some() => {
                return Err(ComputerError::plain(format!(
                    "computer action `{action}` must omit `session`"
                )));
            }
            "launch" | "attach" => {}
            _ if session.is_none() => {
                return Err(ComputerError::plain(format!(
                    "computer action `{action}` requires a live `computer:<slug>` session"
                )));
            }
            _ => {}
        }
        if !args.is_object() {
            return Err(ComputerError::plain(
                "computer action payload must be an object",
            ));
        }
        let schema = action_schema(&self.inner, action);
        let compiled = jsonschema::JSONSchema::compile(&schema).map_err(|error| {
            ComputerError::plain(format!("invalid built-in computer schema: {error}"))
        })?;
        if let Err(errors) = compiled.validate(args) {
            let detail = errors
                .take(6)
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ComputerError::plain(format!(
                "invalid args for computer `{action}`: {detail}"
            )));
        }
        Ok(())
    }

    async fn spawn(&self, action: &str, args: Value) -> Result<ToolOutput, ComputerError> {
        let before = self
            .registry
            .list()
            .into_iter()
            .map(|(id, _, _)| id)
            .collect::<BTreeSet<_>>();
        let mut legacy = args.as_object().cloned().unwrap_or_default();
        legacy.insert("mode".into(), Value::String(action.to_owned()));
        let launched = self
            .inner
            .invoke_value(Value::Object(legacy))
            .await
            .map_err(|error| ComputerError::plain(error.to_string()))?;
        let rendered = launched.render();
        let after = self
            .registry
            .list()
            .into_iter()
            .map(|(id, _, _)| id)
            .collect::<BTreeSet<_>>();
        let surface = primary_surface(action, &rendered, &before, &after).ok_or_else(|| {
            ComputerError::plain("computer launch succeeded without a primary surface")
        })?;
        let attached = self.registry.get(&surface).ok_or_else(|| {
            ComputerError::plain("computer launch surface disappeared before registration")
        })?;
        let content = args
            .get(if action == "launch" {
                "command"
            } else {
                "endpoint"
            })
            .and_then(Value::as_str)
            .unwrap_or(action);
        let snapshot = json!({
            "surface": surface,
            "control": "live",
            "rung": attached.surface.rung().label(),
        });
        let record = self
            .sessions
            .registry()
            .create_content("computer", content, self.sessions.artist(), None, snapshot)
            .map_err(|error| ComputerError::plain(error.to_string()))?;
        let session_id = record.id.clone();
        self.sessions.own(
            record.id.clone(),
            Arc::new(ComputerSession {
                registry: self.registry.clone(),
                surface: surface.clone(),
            }),
        );

        let output = if action == "attach" {
            self.inner
                .invoke_value(json!({"mode":"observe","surface":surface,"full":true}))
                .await
                .map_err(|error| ComputerError::plain(error.to_string()))?
        } else {
            launched
        };
        let output = rewrite_output(output, &surface, &session_id);
        Ok(prepend_session(output, &session_id))
    }

    async fn act(
        &self,
        action: &str,
        session: &str,
        args: Value,
    ) -> Result<ToolOutput, ComputerError> {
        let surface = self.session_surface(session)?;
        if self.registry.get(&surface).is_none() {
            return Err(ComputerError::plain(format!(
                "computer session `{session}` is not live in its owning process"
            )));
        }
        let wants_observation = args
            .get("observe")
            .is_some_and(|value| !value.is_null() && value != false && value != "none");
        let mut legacy = args.as_object().cloned().unwrap_or_default();
        legacy.insert("mode".into(), Value::String(action.to_owned()));
        legacy.insert("surface".into(), Value::String(surface.clone()));
        let output = self
            .inner
            .invoke_value(Value::Object(legacy))
            .await
            .map_err(|error| ComputerError::plain(error.to_string()))?;
        if matches!(action, "focus" | "resize") || (action == "do" && !wants_observation) {
            Ok(ToolOutput::text("ok"))
        } else {
            Ok(rewrite_output(output, &surface, session))
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ComputerArgs {
    action: String,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    args: Option<Value>,
}

impl ComputerArgs {
    fn into_action(self) -> Result<(String, Option<String>, Value), ComputerError> {
        if self.action.trim().is_empty() {
            return Err(ComputerError::plain(
                "computer action must be a non-empty string",
            ));
        }
        if self.session.as_deref().is_some_and(str::is_empty) {
            return Err(ComputerError::plain(
                "computer session must be a non-empty string",
            ));
        }
        let args = self.args.unwrap_or_else(|| json!({}));
        if !args.is_object() {
            return Err(ComputerError::plain("computer args must be an object"));
        }
        Ok((self.action, self.session, args))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{message}{reference}")]
pub(crate) struct ComputerError {
    message: String,
    reference: String,
}

impl ComputerError {
    fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            reference: String::new(),
        }
    }

    fn with_reference(mut self, reference: Option<String>) -> Self {
        if let Some(reference) = reference {
            self.reference = format!("\n\n{reference}");
        }
        self
    }
}

impl From<ComputerError> for ToolExecutionError {
    fn from(value: ComputerError) -> Self {
        ToolExecutionError::other(value.to_string())
            .with_code("computer_error")
            .with_retryable(false)
    }
}

impl PortableTool for ComputerTool {
    const NAME: &'static str = "computer";
    type Error = ComputerError;
    type Args = Value;
    type Output = ToolOutput;

    fn description(&self) -> String {
        "Drive one computer session action with `{ action, session?, args? }`. `launch` and `attach` create `computer:<slug>` sessions and must omit `session`; every other action requires a live session. Launch uses `args: { command: string, cwd?: string, gui?: boolean, platform?: \"linux\"|\"android\" }`. Attach uses `args: { endpoint: string }`. Use universal `list` to discover live computer sessions and `abort` to close one. Full advanced action schemas and safety guidance are injected once on this artist's first computer invocation, including an invalid first call.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "action":{
                    "enum": ACTIONS,
                    "description":"launch/attach create sessions; observe/find/extract/zoom/screenshot are read projections; focus/resize/watch are controls; do is atomic composite interaction."
                },
                "session":{"type":"string","description":"Required live computer:<slug> for every action except launch and attach."},
                "args":{"type":"object","description":"Action-specific arguments. launch requires command and optionally cwd/gui/platform. attach requires endpoint. Full advanced schemas are injected on first invocation."}
            },
            "required":["action"],
            "additionalProperties":false
        })
    }

    async fn call(&self, input: Value) -> Result<ToolOutput, ComputerError> {
        let result: Result<ToolOutput, ComputerError> = async {
            let input = serde_json::from_value::<ComputerArgs>(input).map_err(|error| {
                ComputerError::plain(format!("invalid computer input: {error}"))
            })?;
            let (action, session, args) = input.into_action()?;
            self.validate(&action, session.as_deref(), &args)?;
            let output = match action.as_str() {
                "launch" | "attach" => self.spawn(&action, args).await?,
                _ => {
                    self.act(
                        &action,
                        session.as_deref().expect("validated required session"),
                        args,
                    )
                    .await?
                }
            };
            Ok(output)
        }
        .await;
        let reference = self.take_reference_on_first_invocation()?;
        match result {
            Ok(output) => Ok(append_reference(output, reference)),
            Err(error) => Err(error.with_reference(reference)),
        }
    }
}

struct ComputerSession {
    registry: artist_computer::SurfaceRegistry,
    surface: String,
}

impl OwnedSession for ComputerSession {
    fn state(&self) -> BoxFuture<'_, Result<OwnedState, String>> {
        Box::pin(async move {
            match self.registry.get(&self.surface) {
                Some(attached) => Ok(OwnedState::live(json!({
                    "surface": self.surface,
                    "control": "live",
                    "rung": attached.surface.rung().label(),
                }))),
                None => Ok(OwnedState::stopped(
                    SessionStatus::Cancelled,
                    json!({"control":"closed"}),
                )),
            }
        })
    }

    fn observe(&self) -> BoxFuture<'_, Result<OwnedState, String>> {
        self.state()
    }

    fn send(&self, _input: Value) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async {
            Err("generic send is unsupported for computer sessions; use the computer tool".into())
        })
    }

    fn abort(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.registry.close(&self.surface);
            Ok(())
        })
    }
}

fn action_schema(inner: &artist_computer::ComputerTool, action: &str) -> Value {
    let full = inner.parameters();
    let source = full
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let (fields, required): (&[&str], &[&str]) = match action {
        "launch" => (&["command", "cwd", "gui", "platform"], &["command"]),
        "attach" => (&["endpoint"], &["endpoint"]),
        "observe" => (&["full"], &[]),
        "find" => (&["query"], &["query"]),
        "extract" => (&["fields"], &["fields"]),
        "zoom" => (&["anchor"], &["anchor"]),
        "screenshot" => (&["full"], &[]),
        "focus" => (&[], &[]),
        "resize" => (&["width", "height"], &["width", "height"]),
        "watch" => (&[], &[]),
        "do" => (
            &["steps", "settle", "expect", "observe"],
            &["steps", "expect"],
        ),
        _ => (&[], &[]),
    };
    let properties = fields
        .iter()
        .filter_map(|field| {
            source
                .get(*field)
                .cloned()
                .map(|schema| ((*field).to_owned(), schema))
        })
        .collect::<Map<_, _>>();
    json!({
        "type":"object",
        "properties":properties,
        "required":required,
        "additionalProperties":false
    })
}

fn reference(inner: &artist_computer::ComputerTool) -> String {
    let mut out = String::from(
        "<computer_skill>\nThe `computer` tool uses `{ action, session?, args? }`. `launch` and `attach` omit `session`; every other action carries the returned `computer:<slug>` in `session`, with action-specific fields inside `args`. Universal `list` discovers live computer sessions and `abort` closes them. Generic `poll` reports lifecycle/control only; use the read actions for page/surface content.\n\nNaming and safety rules: use anchors, never coordinates. Any step naming an anchor must also carry its current label when the element has a name. `do` is atomic with guardrails before step 1, stops at the first failed step, and requires `expect`. Arm browser dialogs before triggering them. A stale anchor must be refreshed with observe/find rather than guessed. `screenshot`/`zoom` are for visual information that structured observation cannot answer.\n",
    );
    for action in ACTIONS {
        let schema = action_schema(inner, action);
        out.push_str(&format!(
            "\n### {action}\nargs schema:\n{}\n",
            serde_json::to_string_pretty(&schema).unwrap_or_else(|_| "{}".into())
        ));
    }
    out.push_str("\nSemantics: `observe` reads the surface (full=true forces a complete snapshot); `find` returns matching named elements and anchors; `extract` reads values by labels without taking a picture; `zoom` returns a close view of one anchored element; `screenshot` returns pixels plus structured context; `focus` gives the selected surface keyboard focus; `resize` changes the isolated display dimensions; `watch` shows the isolated display to the user; `do` executes its steps in order and verifies `expect`. `launch` uses a PTY unless gui=true; platform=android launches the package in the Android container. `attach` drives the user's already-running debug-enabled browser and therefore acts on their real tabs/session.\n</computer_skill>");
    out
}

fn primary_surface(
    action: &str,
    rendered: &str,
    before: &BTreeSet<String>,
    after: &BTreeSet<String>,
) -> Option<String> {
    let parsed = match action {
        "launch" => rendered
            .lines()
            .next()
            .and_then(|line| line.rsplit_once(" as "))
            .map(|(_, id)| id.trim().to_owned()),
        "attach" => rendered
            .lines()
            .nth(1)
            .and_then(|line| line.split_whitespace().next())
            .map(str::to_owned),
        _ => None,
    };
    if parsed.as_ref().is_some_and(|id| after.contains(id)) {
        return parsed;
    }
    let mut added = after.difference(before).cloned();
    let first = added.next()?;
    if added.next().is_none() {
        Some(first)
    } else {
        None
    }
}

fn rewrite_output(output: ToolOutput, old: &str, new: &str) -> ToolOutput {
    let blocks = output
        .into_content()
        .into_iter()
        .map(|block| match block {
            ToolResultContent::Text(mut text) => {
                text.text = text.text.replace(old, new);
                ToolResultContent::Text(text)
            }
            ToolResultContent::Json { mut value } => {
                replace_json_strings(&mut value, old, new);
                ToolResultContent::Json { value }
            }
            image @ ToolResultContent::Image(_) => image,
        })
        .collect::<Vec<_>>();
    ToolOutput::content(OneOrMany::many(blocks).expect("computer output is non-empty"))
}

fn replace_json_strings(value: &mut Value, old: &str, new: &str) {
    match value {
        Value::String(text) => *text = text.replace(old, new),
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| replace_json_strings(value, old, new)),
        Value::Object(values) => values
            .values_mut()
            .for_each(|value| replace_json_strings(value, old, new)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn prepend_session(output: ToolOutput, session: &str) -> ToolOutput {
    let mut blocks = vec![ToolResultContent::text(session.to_owned())];
    blocks.extend(output.into_content());
    ToolOutput::content(OneOrMany::many(blocks).expect("session plus output"))
}

fn append_reference(output: ToolOutput, reference: Option<String>) -> ToolOutput {
    let Some(reference) = reference else {
        return output;
    };
    let mut blocks = output.into_content().into_iter().collect::<Vec<_>>();
    blocks.push(ToolResultContent::text(reference));
    ToolOutput::content(OneOrMany::many(blocks).expect("computer output plus reference"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(root: &std::path::Path) -> ComputerTool {
        let registry = artist_computer::SurfaceRegistry::for_project(root, (1024, 768));
        ComputerTool::new(
            registry,
            artist_session::Recorder::noop(),
            None,
            SessionHub::standard(root, "Goethe", None),
            root,
        )
    }

    #[test]
    fn shallow_schema_is_action_session_args_only() {
        let root = tempfile::tempdir().unwrap();
        let schema = tool(root.path()).parameters();
        let keys = schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(keys, ["action", "args", "session"].into_iter().collect());
        assert_eq!(schema["required"], json!(["action"]));
        assert_eq!(schema["properties"]["action"]["enum"], json!(ACTIONS));
        for deleted in [
            "mode", "surface", "surfaces", "close", "steps", "command", "endpoint",
        ] {
            assert!(schema["properties"].get(deleted).is_none());
        }
        assert!(ACTIONS.contains(&"watch"));
        assert!(!ACTIONS.contains(&"surfaces"));
        assert!(!ACTIONS.contains(&"close"));
    }

    #[test]
    fn base_description_is_enough_for_launch_and_attach() {
        let root = tempfile::tempdir().unwrap();
        let description = tool(root.path()).description();
        assert!(description.contains("command: string"));
        assert!(description.contains("endpoint: string"));
        assert!(description.contains("universal `list`"));
        assert!(description.contains("`abort`"));
    }

    #[tokio::test]
    async fn invalid_first_invocation_still_delivers_reference_exactly_once() {
        let root = tempfile::tempdir().unwrap();
        let first_tool = tool(root.path());
        let invalid = || json!({"action":"observe","args":{}});
        let first = first_tool.call(invalid()).await.unwrap_err().to_string();
        assert!(first.contains("requires a live `computer:<slug>` session"));
        assert!(first.contains("<computer_skill>"));
        assert!(first.contains("### watch"));
        let second = first_tool.call(invalid()).await.unwrap_err().to_string();
        assert!(!second.contains("<computer_skill>"));

        // A malformed top-level object still reaches the wrapper and therefore
        // consumes/delivers the same first-use reference contract.
        let root = tempfile::tempdir().unwrap();
        let malformed = tool(root.path());
        let first = malformed
            .call(json!({"args":{}}))
            .await
            .unwrap_err()
            .to_string();
        assert!(first.contains("invalid computer input"));
        assert!(first.contains("<computer_skill>"));
        let second = malformed
            .call(json!({"args":{}}))
            .await
            .unwrap_err()
            .to_string();
        assert!(!second.contains("<computer_skill>"));
    }

    #[test]
    fn reference_matches_the_final_action_set() {
        let registry = artist_computer::SurfaceRegistry::new();
        let inner = artist_computer::ComputerTool::new(registry);
        let text = reference(&inner);
        for action in ACTIONS {
            assert!(text.contains(&format!("### {action}")), "missing {action}");
        }
        assert!(!text.contains("### surfaces"));
        assert!(!text.contains("### close"));
        assert!(text.contains("\"click\""));
    }
}
