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

use std::sync::{Arc, Mutex};

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
    /// What the last action printed.
    ///
    /// Rung 0 actions are not all commands — most of the useful ones are
    /// *questions*: `status`, `diff`, `log`. Running one and discarding its
    /// output made every read action report "ok" and then "(no change)", so
    /// the model could ask and never hear the answer. Found by asking a real
    /// adapter for a git log and getting nothing back.
    last: Mutex<Option<(String, String)>>,
    /// Environment for spawned calls — the stage's, when there is one, so a
    /// D-Bus call lands on the stage bus rather than the user's.
    env: Vec<(String, String)>,
}

impl ProgrammaticSurface {
    pub fn new(id: impl Into<String>, adapter: Arc<Adapter>) -> Self {
        Self {
            id: SurfaceId::new(id),
            adapter,
            last: Mutex::new(None),
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
                // Loopback only, and checked *after* substitution — that is the
                // whole point. An adapter's URL template is trusted, but the
                // value spliced into it comes from the model, so a template like
                // `http://127.0.0.1:8080/open?target={value}` is one careless
                // adapter away from being an outbound request primitive with
                // the user's network position behind it.
                //
                // Rung 0 adapters talk to things already running on this
                // machine — a media player, a local daemon, a dev server. That
                // is not a restriction on what they can express; it is what
                // they are for. Anything genuinely remote belongs behind a CLI
                // the user chose to install.
                if let Err(reason) = check_loopback(&url) {
                    return Err(StepError::Backend(format!("{}: {reason}", action.name)));
                }
                self.run("curl", &["-fsS".into(), "-X".into(), method.clone(), url])
                    .await
            }
        };
        match result {
            Ok(output) => {
                // Kept so the next observation can show it. Bounded by the
                // shared truncation contract rather than a number invented
                // here, so a `diff` of a large file is cut the same way every
                // other oversized output in the harness is.
                let output = artist_tools::output::head(output.trim().to_owned(), OUTPUT_LIMIT);
                *self.last.lock().unwrap() = Some((action.name.clone(), output.clone()));
                Ok(output)
            }
            // Name the action, not just the transport: "pause failed" is what a
            // reader needs, and the underlying command is an implementation
            // detail of the adapter.
            Err(error) => Err(StepError::Backend(format!("{}: {error}", action.name))),
        }
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
        let mut nodes: Vec<Node> = Vec::new();
        // The answer first, when there is one. An action's output is the whole
        // point of asking, and burying it under the verb list would make the
        // model scroll past its own result.
        if let Some((action, output)) = self.last.lock().unwrap().clone() {
            nodes.push(
                Node::new(
                    format!("adapter:{}:output", self.adapter.name),
                    Role::Text,
                    format!("{action} output"),
                )
                .with_value(output),
            );
        }
        nodes.extend(
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
                .collect::<Vec<_>>(),
        );
        Ok(Snapshot::new(nodes))
    }

    async fn watch(&self, _settle: &Settle) -> Result<SettleWatch, StepError> {
        // A programmatic call has already happened by the time it returns.
        // There is no screen to wait for, and pretending otherwise would add
        // latency to the fastest rung.
        Ok(SettleWatch::ready(SettleOutcome::Settled { after_ms: 0 }))
    }

    async fn apply(
        &self,
        step: &Step,
        node: Option<&Node>,
        _secondary: Option<&Node>,
    ) -> Result<Option<String>, StepError> {
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
        self.invoke(action, value).await.map(|_| None)
    }
}

/// How much of an action's output to keep for the next observation.
///
/// A `diff` can be enormous, and an observation is charged to the model's
/// context whether it reads it or not. Cut the same way every other oversized
/// output in the harness is, rather than by a number invented here.
const OUTPUT_LIMIT: usize = 8 * 1024;

/// Hosts an adapter may call. Loopback, and nothing else.
const LOOPBACK: &[&str] = &["127.0.0.1", "[::1]", "localhost"];

/// Reject anything that is not a plain HTTP call to this machine.
///
/// Written as an allow-list rather than a block-list on purpose. The block-list
/// version of this ("reject 169.254.169.254", "reject 10.0.0.0/8") is the one
/// that keeps losing: DNS names that resolve inward, redirects, IPv6 mapped
/// forms, decimal-encoded addresses. An allow-list of three literal hosts has
/// none of those failure modes, and it costs nothing here because rung 0 exists
/// to talk to local things.
fn check_loopback(url: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| format!("{url:?} is not an http(s) url"))?;
    // Everything before the first `/`, `?` or `#`, minus any credentials — a
    // URL like `http://127.0.0.1@evil.example/` has an authority of
    // `evil.example`, and reading the host as "the bit before the colon" is
    // exactly how that trick works.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = match host_port.rsplit_once(':') {
        // An IPv6 literal keeps its brackets and its inner colons.
        Some((head, _)) if head.ends_with(']') || !head.contains(':') => head,
        _ => host_port,
    };
    if LOOPBACK.contains(&host) {
        Ok(())
    } else {
        Err(format!(
            "adapters may only call this machine, and {host:?} is not loopback. \
             Use a CLI transport if this really needs to reach the network."
        ))
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
                &Step::click(Target {
                    anchor: "x".into(),
                    label: None,
                }),
                Some(&node),
                None,
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

    #[test]
    fn adapters_may_only_call_this_machine() {
        for allowed in [
            "http://127.0.0.1:8080/health",
            "http://localhost:9222/json/version",
            "http://[::1]:8080/x",
            "https://127.0.0.1/x",
        ] {
            assert!(
                check_loopback(allowed).is_ok(),
                "{allowed} should be allowed"
            );
        }
        for refused in [
            "http://example.com/x",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.5:8080/x",
            "file:///etc/passwd",
            "http://internal.corp:8080/x",
        ] {
            assert!(
                check_loopback(refused).is_err(),
                "{refused} should be refused"
            );
        }
    }

    #[test]
    fn credentials_in_the_authority_do_not_smuggle_a_host_past_the_check() {
        // `http://127.0.0.1@evil.example/` has an authority of `evil.example`.
        // Reading the host as "the text before the colon" accepts it, which is
        // exactly how this trick is played.
        assert!(check_loopback("http://127.0.0.1@evil.example/x").is_err());
        assert!(check_loopback("http://localhost:8080@evil.example/x").is_err());
    }

    #[test]
    fn the_check_runs_after_substitution_not_before() {
        // The template is ours; the value spliced into it comes from the model.
        // Checking the template would pass a URL that the substitution then
        // turns into an outbound request.
        let template = "http://127.0.0.1:8080/open?to={value}";
        assert!(check_loopback(template).is_ok());
        let hostile = substitute(template, Some("x"));
        assert!(check_loopback(&hostile).is_ok());

        // And a template whose *host* is the placeholder is caught once filled.
        let worse = substitute("http://{value}/x", Some("evil.example"));
        assert!(check_loopback(&worse).is_err(), "{worse}");
    }
}

#[cfg(test)]
mod output_tests {
    use super::*;
    use crate::program::Target;

    fn echoing() -> ProgrammaticSurface {
        let adapter = Adapter {
            name: "demo".into(),
            match_app_id: vec!["demo".into()],
            actions: vec![Action {
                name: "status".into(),
                description: "Say something back.".into(),
                call: Call::Cli {
                    argv: vec!["echo".into(), "branch main, clean".into()],
                },
            }],
        };
        ProgrammaticSurface::new("app:1", Arc::new(adapter))
    }

    #[tokio::test]
    async fn an_action_that_answers_a_question_shows_its_answer() {
        // Found by driving a real adapter: `invoke log` reported "ok" and then
        // "(no change)". Most useful rung-0 actions are *questions* — status,
        // diff, log — and discarding their output made the model able to ask
        // and never hear.
        let surface = echoing();
        let before = surface.snapshot().await.unwrap();
        assert!(
            !before.nodes.iter().any(|node| node.name.contains("output")),
            "nothing has run yet, so there is no answer to show"
        );

        let node = before
            .nodes
            .iter()
            .find(|node| node.name == "status")
            .cloned()
            .unwrap();
        surface
            .apply(
                &Step::Invoke {
                    target: Target {
                        anchor: "x".into(),
                        label: Some("status".into()),
                    },
                    action: "status".into(),
                },
                Some(&node),
                None,
            )
            .await
            .unwrap();

        let after = surface.snapshot().await.unwrap();
        let answer = after
            .nodes
            .iter()
            .find(|node| node.name.contains("output"))
            .expect("the answer must be in the next observation");
        assert_eq!(answer.value.as_deref(), Some("branch main, clean"));
        // And first, because the answer is what was asked for — burying it
        // under the verb list makes the model scroll past its own result.
        assert!(after.nodes[0].name.contains("output"));
    }

    #[tokio::test]
    async fn the_verbs_are_still_there_after_an_answer() {
        // The answer is added, never substituted: an adapter whose actions
        // vanished after the first call could be used exactly once.
        let surface = echoing();
        let node = surface.snapshot().await.unwrap().nodes[0].clone();
        surface
            .apply(
                &Step::Invoke {
                    target: Target {
                        anchor: "x".into(),
                        label: Some("status".into()),
                    },
                    action: "status".into(),
                },
                Some(&node),
                None,
            )
            .await
            .unwrap();
        let after = surface.snapshot().await.unwrap();
        assert!(after.nodes.iter().any(|node| node.name == "status"));
    }
}
