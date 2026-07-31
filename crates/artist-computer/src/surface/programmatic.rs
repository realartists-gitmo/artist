//! Rung 0: driving an application through its own interface.
//!
//! The cheapest rung, and the one that makes the difference between an
//! operation that works and one that usually works. A media player exposes
//! `Pause` on D-Bus; clicking its pause button instead means a capture, a tree
//! walk, an anchor resolution and a synthetic input event, each of which can
//! fail on its own and none of which is necessary.
//!
//! Surfaces here have no geometry and no tree. Their "nodes" are the declared
//! actions, so the model sees a menu of verbs rather than a picture of a window
//! — and an action either succeeds or reports exactly why.

use std::sync::Arc;

use crate::ladder::adapters::{Action, Adapter, Call};
use crate::model::{Caps, Node, Role, Rung, Snapshot, SurfaceId};
use crate::program::{Settle, SettleOutcome, Step, StepError};
use crate::surface::{SettleWatch, Surface};

/// How long any single rung-0 call may take before it is abandoned.
///
/// A hung D-Bus call would otherwise block the agent indefinitely — the whole
/// appeal of this rung is that it is fast and decisive.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// An application driven through its own API.
pub struct ProgrammaticSurface {
    id: SurfaceId,
    adapter: Arc<Adapter>,
    /// Environment for spawned calls — the stage's, when there is one, so a
    /// D-Bus call lands on the stage bus rather than the user's.
    env: Vec<(String, String)>,
}

impl ProgrammaticSurface {
    pub fn new(id: impl Into<String>, adapter: Arc<Adapter>) -> Self {
        Self {
            id: SurfaceId::new(id),
            adapter,
            env: Vec::new(),
        }
    }

    pub fn with_env<I, K, V>(mut self, env: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.env = env
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }

    pub fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    /// Run one declared action.
    pub async fn invoke(&self, action: &Action, value: Option<&str>) -> Result<String, StepError> {
        let result = match &action.call {
            Call::Dbus(call) => {
                let mut argv = vec![
                    "--print-reply".to_owned(),
                    call.service.clone(),
                    call.path.clone(),
                    format!("{}.{}", call.interface, call.method),
                ];
                argv.extend(call.args.iter().map(|arg| substitute(arg, value)));
                self.run("dbus-send", &argv).await
            }
            Call::Cli { argv } => {
                let mut argv = argv.iter().map(|arg| substitute(arg, value));
                let program = argv
                    .next()
                    .ok_or_else(|| StepError::Backend("cli action has an empty argv".into()))?;
                let rest: Vec<String> = argv.collect();
                self.run(&program, &rest).await
            }
            Call::Http { method, url } => {
                let url = substitute(url, value);
                self.run("curl", &["-fsS".into(), "-X".into(), method.clone(), url])
                    .await
            }
        };
        result.map_err(|error| {
            // Name the action, not just the transport: "pause failed" is what a
            // reader needs, and the underlying command is an implementation
            // detail of the adapter.
            StepError::Backend(format!("{}: {error}", action.name))
        })
    }

    async fn run(&self, program: &str, args: &[String]) -> Result<String, String> {
        let mut command = tokio::process::Command::new(program);
        command.args(args);
        for (key, value) in &self.env {
            command.env(key, value);
        }
        let output = tokio::time::timeout(CALL_TIMEOUT, command.output())
            .await
            .map_err(|_| format!("{program} did not return within {CALL_TIMEOUT:?}"))?
            .map_err(|error| format!("run {program}: {error}"))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            // stderr first: it is where these tools explain themselves.
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            };
            Err(format!("{program} failed: {detail}"))
        }
    }
}

/// Replace `{value}` with the step's text.
fn substitute(template: &str, value: Option<&str>) -> String {
    template.replace("{value}", value.unwrap_or(""))
}

#[async_trait::async_trait]
impl Surface for ProgrammaticSurface {
    fn id(&self) -> &SurfaceId {
        &self.id
    }

    fn rung(&self) -> Rung {
        Rung::Programmatic
    }

    fn caps(&self) -> Caps {
        Caps {
            // There is nothing to point at. Presenting a pointer here would
            // invite the model to try a gesture that cannot exist.
            click: true,
            type_text: false,
            key: false,
            scroll: false,
            pixels: false,
        }
    }

    fn title(&self) -> String {
        self.adapter.name.clone()
    }

    /// The declared actions, as nodes.
    ///
    /// The binding is the action name, which is stable by construction — an
    /// adapter is a file, so its actions do not move between observations the
    /// way a screen's elements do.
    async fn snapshot(&self) -> Result<Snapshot, StepError> {
        Ok(Snapshot::new(
            self.adapter
                .actions
                .iter()
                .map(|action| {
                    let mut node = Node::new(
                        format!("adapter:{}:{}", self.adapter.name, action.name),
                        Role::Button,
                        action.name.clone(),
                    );
                    if !action.description.is_empty() {
                        node = node.with_value(action.description.clone());
                    }
                    node.with_actions(["invoke"])
                })
                .collect(),
        ))
    }

    async fn watch(&self, _settle: &Settle) -> Result<SettleWatch, StepError> {
        // A programmatic call has already happened by the time it returns.
        // There is no screen to wait for, and pretending otherwise would add
        // latency to the fastest rung.
        Ok(SettleWatch::ready(SettleOutcome::Settled { after_ms: 0 }))
    }

    async fn apply(&self, step: &Step, node: Option<&Node>) -> Result<(), StepError> {
        let Some(node) = node else {
            return Err(StepError::Backend(
                "a programmatic surface is driven by naming one of its actions".into(),
            ));
        };
        // `invoke` names the verb outright; everything else takes the node's own
        // name, which for an adapter *is* the verb.
        let wanted = match step {
            Step::Invoke { action, .. } => action.as_str(),
            _ => node.name.as_str(),
        };
        let action = self.adapter.action(wanted).ok_or_else(|| {
            let declared: Vec<&str> = self
                .adapter
                .actions
                .iter()
                .map(|action| action.name.as_str())
                .collect();
            StepError::Backend(format!(
                "{wanted:?} is not an action of the {} adapter — it declares {}",
                self.adapter.name,
                declared.join(", ")
            ))
        })?;

        let value = match step {
            Step::Type { text, .. } => Some(text.as_str()),
            _ => None,
        };
        self.invoke(action, value).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchors::AnchorBook;
    use crate::program::Target;

    fn adapter(toml: &str) -> Arc<Adapter> {
        Arc::new(toml::from_str(toml).unwrap())
    }

    const ECHO: &str = r#"
name = "echo"
match_app_id = ["*"]

[[action]]
name = "greet"
description = "Say hello"
cli = { argv = ["echo", "hello {value}"] }

[[action]]
name = "fail"
cli = { argv = ["false"] }
"#;

    #[tokio::test]
    async fn actions_appear_as_nodes_the_model_can_name() {
        let surface = ProgrammaticSurface::new("app:1", adapter(ECHO));
        let snapshot = surface.snapshot().await.unwrap();
        let mut book = AnchorBook::new();
        let observed = book.observe(&snapshot, false);
        let rendered = crate::render::observation("app:1", &observed, None);

        assert!(rendered.contains("greet"), "{rendered}");
        assert!(rendered.contains("Say hello"), "{rendered}");
    }

    #[tokio::test]
    async fn a_cli_action_runs_and_substitutes_the_step_text() {
        let surface = ProgrammaticSurface::new("app:1", adapter(ECHO));
        let action = surface.adapter().action("greet").unwrap();
        let output = surface.invoke(action, Some("world")).await.unwrap();
        assert_eq!(output, "hello world");
    }

    #[tokio::test]
    async fn a_failing_action_names_itself_in_the_error() {
        let surface = ProgrammaticSurface::new("app:1", adapter(ECHO));
        let action = surface.adapter().action("fail").unwrap();
        let error = surface.invoke(action, None).await.unwrap_err().to_string();
        assert!(
            error.contains("fail"),
            "the error must name the action, not just the command: {error}"
        );
    }

    #[tokio::test]
    async fn a_program_drives_an_adapter_end_to_end() {
        let surface = ProgrammaticSurface::new("app:1", adapter(ECHO));
        let mut book = AnchorBook::new();
        let observed = book.observe(&surface.snapshot().await.unwrap(), false);
        let anchor = observed
            .entries
            .iter()
            .find(|entry| entry.node.name == "greet")
            .unwrap()
            .anchor
            .clone();

        let program = crate::Program {
            steps: vec![Step::Type {
                target: Target {
                    anchor: anchor.clone(),
                    label: Some("greet".into()),
                },
                text: "there".into(),
                clear: true,
            }],
            settle: Settle::default(),
            expect: crate::program::Expect::Appears("greet".into()),
        };

        let report = crate::run_program(&surface, &mut book, &program)
            .await
            .unwrap();
        assert!(report.error.is_none(), "{:?}", report.error);
        assert_eq!(report.steps[0].outcome, "ok");
        assert_eq!(report.expect_met, Some(true));
    }

    #[tokio::test]
    async fn naming_an_action_that_does_not_exist_is_a_clear_error() {
        let surface = ProgrammaticSurface::new("app:1", adapter(ECHO));
        let node = Node::new("adapter:echo:nope", Role::Button, "nope");
        let error = surface
            .apply(
                &Step::Click(Target {
                    anchor: "x".into(),
                    label: None,
                }),
                Some(&node),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("not an action"), "{error}");
    }

    #[test]
    fn substitution_leaves_templates_without_a_placeholder_alone() {
        assert_eq!(substitute("echo {value}", Some("x")), "echo x");
        assert_eq!(substitute("pause", Some("x")), "pause");
        assert_eq!(substitute("echo {value}", None), "echo ");
    }
}
