//! The surface contract, and the driver that runs programs against it.
//!
//! A `Surface` deals in [`Node`]s and opaque bindings. It never sees an anchor
//! token, never renders text for the model, and never decides what changed —
//! all of that lives one layer up, which is what keeps the four rungs behaving
//! identically.

use std::sync::Arc;

use crate::anchors::{AnchorBook, Observation};
use crate::model::{Caps, Node, Rung, Snapshot, SurfaceId};
use crate::program::{Expect, Program, SettleOutcome, Step, StepError, Target, check_label};

pub mod atspi;
pub mod cdp;
pub mod pixels;
pub mod programmatic;
pub mod pty;

/// A watcher armed *before* the action that it observes.
///
/// This split — rather than a single `act_and_settle` — exists because
/// subscribing after dispatch is a race. A fast surface can finish reacting
/// before the watcher attaches, and the wait then burns the full timeout on an
/// already-settled screen while a slow one is missed entirely.
pub struct SettleWatch(
    pub Box<dyn std::future::Future<Output = SettleOutcome> + Send + Unpin>,
);

impl SettleWatch {
    pub fn ready(outcome: SettleOutcome) -> Self {
        Self(Box::new(std::future::ready(outcome)))
    }

    pub async fn wait(self) -> SettleOutcome {
        self.0.await
    }
}

/// One driveable thing: a window, a page, a terminal, an adapted application.
#[async_trait::async_trait]
pub trait Surface: Send + Sync {
    fn id(&self) -> &SurfaceId;
    fn rung(&self) -> Rung;
    fn caps(&self) -> Caps;
    fn title(&self) -> String;

    /// The complete current element set. Never a diff.
    async fn snapshot(&self) -> Result<Snapshot, StepError>;

    /// Arm a change watcher. Called before the final step dispatches.
    async fn watch(&self, settle: &crate::program::Settle) -> Result<SettleWatch, StepError>;

    /// Perform one step against an already-resolved node.
    ///
    /// The node is borrowed from the current epoch: the anchor has resolved and
    /// the label has been checked, so a backend only has to act.
    async fn apply(&self, step: &Step, node: Option<&Node>) -> Result<(), StepError>;

    async fn pixels(&self) -> Result<Option<crate::model::Frame>, StepError> {
        Ok(None)
    }

    /// Nested surfaces — a browser window's page targets. One level only.
    fn children(&self) -> Vec<Arc<dyn Surface>> {
        Vec::new()
    }
}

/// What happened to one step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepReport {
    pub action: &'static str,
    pub anchor: Option<String>,
    pub label: Option<String>,
    /// The element's real name, for the audit trail. A `label` that differs
    /// from this is the single most useful thing to see after a bad run.
    pub resolved_name: Option<String>,
    /// The payload of a `type` or `key` step.
    ///
    /// Recorded because a report without it is not enough to rebuild the step:
    /// `distill` turns these into a replayable macro, and a `key` step that
    /// forgot which key replays as `Step::Key("")`.
    pub payload: Option<String>,
    pub outcome: String,
}

/// What happened to a program.
#[derive(Debug)]
pub struct ProgramReport {
    pub steps: Vec<StepReport>,
    pub settled: Option<SettleOutcome>,
    pub expect_met: Option<bool>,
    pub failed_step: Option<u32>,
    pub error: Option<StepError>,
    /// The observation taken after the program, which is also what the model
    /// gets to see at the point of failure.
    pub observation: Observation,
}

/// Run a program: resolve, check, arm, dispatch, settle, verify.
///
/// Steps abort on the first failure. That is deliberate even though it discards
/// the remaining steps: a program is a hypothesis about a sequence of screens,
/// and once one step lands somewhere unexpected every later step is operating
/// on a surface the model has not seen.
pub async fn run_program(
    surface: &dyn Surface,
    book: &mut AnchorBook,
    program: &Program,
) -> Result<ProgramReport, StepError> {
    let mut reports = Vec::with_capacity(program.steps.len());
    let mut failed_step = None;
    let mut failure = None;
    let mut settled = None;

    for (index, step) in program.steps.iter().enumerate() {
        let resolved = match resolve_step(book, step) {
            Ok(resolved) => resolved,
            Err(error) => {
                reports.push(report_for(step, None, error.to_string()));
                failed_step = Some(index as u32);
                failure = Some(error);
                break;
            }
        };

        // Arm before the *last* step so nothing that step provokes is missed.
        let watch = if index + 1 == program.steps.len() {
            Some(surface.watch(&program.settle).await?)
        } else {
            None
        };

        let outcome = surface.apply(step, resolved.as_ref()).await;
        let resolved_name = resolved.as_ref().map(|node| node.name.clone());
        match outcome {
            Ok(()) => reports.push(report_for(step, resolved_name, "ok".into())),
            Err(error) => {
                reports.push(report_for(step, resolved_name, error.to_string()));
                failed_step = Some(index as u32);
                failure = Some(error);
                break;
            }
        }

        if let Some(watch) = watch {
            settled = Some(watch.wait().await);
        }
    }

    let snapshot = surface.snapshot().await?;

    // `expect` is only meaningful if the program actually finished; reporting a
    // failed expectation on top of a failed step would just be noise.
    let expect_met = failure
        .is_none()
        .then(|| check_expect(&snapshot, book, &program.expect));

    // A delta is right when the model already knows the surface. It is exactly
    // wrong after a failure: nothing was dispatched, so the surface is
    // unchanged and the delta is literally `(no change)` — while the error says
    // "re-observe to get fresh anchors". The model would re-observe, get
    // nothing again, and flail. Render in full whenever the model's picture of
    // the surface is the thing in question.
    let full = failure.is_some() || expect_met == Some(false);
    let observation = book.observe(&snapshot, full);

    Ok(ProgramReport {
        steps: reports,
        settled,
        expect_met,
        failed_step,
        error: failure,
        observation,
    })
}

/// Evaluate a hypothesis against the surface as it now is.
///
/// Matched with the same normalization the label cross-check uses, so a
/// prediction is accepted on exactly the terms a live step would be — a model
/// that can name a button well enough to click it can name it well enough to
/// predict it.
fn check_expect(snapshot: &Snapshot, book: &AnchorBook, expect: &Expect) -> bool {
    let present = |label: &str| {
        snapshot.nodes.iter().any(|node| {
            !node.name.trim().is_empty() && check_label("expect", Some(label), node).is_ok()
        })
    };
    match expect {
        Expect::Appears(label) => present(label),
        Expect::Gone(label) => !present(label),
        // Both halves, unlike before: the element must still be there *and*
        // still mean what the model said. Checking only the anchor let
        // `expect: met` fire for an element now named "Send failed".
        //
        // Resolved through the binding rather than the book's node, because the
        // book still holds the *pre-program* epoch here — asking it directly
        // would compare the model's claim against the name the element had
        // before the program ran, which is never the question.
        Expect::Still(target) => {
            let Ok(previous) = book.resolve(&target.anchor) else {
                return false;
            };
            snapshot
                .nodes
                .iter()
                .find(|node| node.binding == previous.binding)
                .is_some_and(|node| {
                    check_label(&target.anchor, target.label.as_deref(), node).is_ok()
                })
        }
    }
}

/// Resolve a step's anchor and verify the model's label still describes it.
fn resolve_step(book: &AnchorBook, step: &Step) -> Result<Option<Node>, StepError> {
    let Some(Target { anchor, label }) = step.target() else {
        return Ok(None);
    };
    let node = book.resolve(anchor).map_err(StepError::Anchor)?.clone();
    check_label(anchor, label.as_deref(), &node)?;
    Ok(Some(node))
}

fn report_for(step: &Step, resolved_name: Option<String>, outcome: String) -> StepReport {
    let target = step.target();
    StepReport {
        action: step.action(),
        anchor: target.map(|target| target.anchor.clone()),
        label: target.and_then(|target| target.label.clone()),
        resolved_name,
        payload: step.payload().map(str::to_owned),
        outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Role;
    use crate::program::{Settle, SettleKind};
    use std::sync::Mutex;

    /// A surface that records the order in which it was asked to do things, so
    /// the arm-before-dispatch ordering can be asserted rather than assumed.
    struct Recording {
        id: SurfaceId,
        nodes: Mutex<Vec<Node>>,
        calls: Mutex<Vec<String>>,
        fail_on: Option<&'static str>,
    }

    impl Recording {
        fn new(nodes: Vec<Node>) -> Self {
            Self {
                id: SurfaceId::new("test:1"),
                nodes: Mutex::new(nodes),
                calls: Mutex::new(Vec::new()),
                fail_on: None,
            }
        }

        fn failing(nodes: Vec<Node>, fail_on: &'static str) -> Self {
            Self {
                fail_on: Some(fail_on),
                ..Self::new(nodes)
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Surface for Recording {
        fn id(&self) -> &SurfaceId {
            &self.id
        }
        fn rung(&self) -> Rung {
            Rung::Engine
        }
        fn caps(&self) -> Caps {
            Caps {
                click: true,
                type_text: true,
                key: true,
                scroll: true,
                pixels: false,
            }
        }
        fn title(&self) -> String {
            "test".into()
        }

        async fn snapshot(&self) -> Result<Snapshot, StepError> {
            Ok(Snapshot::new(self.nodes.lock().unwrap().clone()))
        }

        async fn watch(&self, _settle: &Settle) -> Result<SettleWatch, StepError> {
            self.calls.lock().unwrap().push("watch".into());
            Ok(SettleWatch::ready(SettleOutcome::Settled { after_ms: 1 }))
        }

        async fn apply(&self, step: &Step, _node: Option<&Node>) -> Result<(), StepError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("apply:{}", step.action()));
            if self.fail_on == Some(step.action()) {
                return Err(StepError::Backend("backend refused".into()));
            }
            Ok(())
        }
    }

    fn program(steps: Vec<Step>, expect_label: &str) -> Program {
        Program {
            steps,
            settle: Settle {
                until: SettleKind::Quiet,
                timeout_ms: 10,
            },
            expect: Expect::Appears(expect_label.into()),
        }
    }

    fn click(anchor: &str, label: &str) -> Step {
        Step::Click(Target {
            anchor: anchor.into(),
            label: Some(label.into()),
        })
    }

    async fn seeded(nodes: Vec<Node>) -> (Recording, AnchorBook, Vec<String>) {
        let surface = Recording::new(nodes);
        let mut book = AnchorBook::new();
        let observed = book.observe(&surface.snapshot().await.unwrap(), false);
        let anchors = observed
            .entries
            .iter()
            .map(|entry| entry.anchor.clone())
            .collect();
        (surface, book, anchors)
    }

    #[tokio::test]
    async fn the_settle_watcher_is_armed_before_the_final_step_dispatches() {
        let (surface, mut book, anchors) =
            seeded(vec![Node::new("a", Role::Button, "Save")]).await;
        let steps = vec![
            click(&anchors[0], "Save"),
            Step::Key("Enter".into()),
        ];
        let report = run_program(&surface, &mut book, &program(steps, "Save"))
            .await
            .unwrap();

        assert_eq!(
            surface.calls(),
            vec!["apply:click", "watch", "apply:key"],
            "the watcher must attach before the action it is meant to observe"
        );
        assert_eq!(report.settled, Some(SettleOutcome::Settled { after_ms: 1 }));
    }

    #[tokio::test]
    async fn a_stale_anchor_stops_the_program_before_anything_runs() {
        let (surface, mut book, _) = seeded(vec![Node::new("a", Role::Button, "Save")]).await;
        let steps = vec![click("notanissuedanchor", "Save")];
        let report = run_program(&surface, &mut book, &program(steps, "Save"))
            .await
            .unwrap();

        assert_eq!(report.failed_step, Some(0));
        assert!(surface.calls().is_empty(), "nothing may dispatch: {:?}", surface.calls());
    }

    #[tokio::test]
    async fn a_label_mismatch_stops_the_program_before_dispatch() {
        let (surface, mut book, anchors) =
            seeded(vec![Node::new("a", Role::Button, "Cancel")]).await;
        let steps = vec![click(&anchors[0], "Delete account")];
        let report = run_program(&surface, &mut book, &program(steps, "Save"))
            .await
            .unwrap();

        assert_eq!(report.failed_step, Some(0));
        assert!(matches!(
            report.error,
            Some(StepError::LabelMismatch { .. })
        ));
        assert!(
            surface.calls().is_empty(),
            "a misnamed target must never be clicked: {:?}",
            surface.calls()
        );
    }

    #[tokio::test]
    async fn steps_after_a_failure_do_not_run() {
        let surface = Recording::failing(vec![Node::new("a", Role::Button, "Save")], "click");
        let mut book = AnchorBook::new();
        let observed = book.observe(&surface.snapshot().await.unwrap(), false);
        let anchor = observed.entries[0].anchor.clone();

        let steps = vec![
            click(&anchor, "Save"),
            Step::Key("Enter".into()),
            Step::Key("Escape".into()),
        ];
        let report = run_program(&surface, &mut book, &program(steps, &anchor))
            .await
            .unwrap();

        assert_eq!(report.failed_step, Some(0));
        assert_eq!(report.steps.len(), 1, "later steps must not be attempted");
        assert_eq!(report.expect_met, None, "expect is not judged after a failure");
    }

    #[tokio::test]
    async fn expect_is_verified_against_the_surface_after_the_program() {
        let (surface, mut book, anchors) =
            seeded(vec![Node::new("a", Role::Button, "Save")]).await;
        let report = run_program(
            &surface,
            &mut book,
            &program(vec![click(&anchors[0], "Save")], "Save"),
        )
        .await
        .unwrap();
        assert_eq!(report.expect_met, Some(true));

        // Predicting something that is not there must be reported, not ignored.
        let report = run_program(
            &surface,
            &mut book,
            &program(vec![Step::Key("Enter".into())], "Message sent"),
        )
        .await
        .unwrap();
        assert_eq!(report.expect_met, Some(false));
    }

    #[tokio::test]
    async fn an_expectation_about_an_element_that_does_not_exist_yet_is_writable() {
        // The whole point of the rework: an element that appears *because of*
        // the program has no anchor the model could know. Naming it must work.
        let surface = Recording::new(vec![Node::new("a", Role::Button, "Compose")]);
        let mut book = AnchorBook::new();
        let observed = book.observe(&surface.snapshot().await.unwrap(), false);
        let anchor = observed.entries[0].anchor.clone();

        // The click "creates" a confirmation the model predicted by name.
        *surface.nodes.lock().unwrap() = vec![
            Node::new("a", Role::Button, "Compose"),
            Node::new("b", Role::Text, "Message sent"),
        ];

        let report = run_program(
            &surface,
            &mut book,
            &program(vec![click(&anchor, "Compose")], "Message sent"),
        )
        .await
        .unwrap();
        assert_eq!(
            report.expect_met,
            Some(true),
            "a hypothesis about a new element must be expressible and checkable"
        );
    }

    #[tokio::test]
    async fn expect_gone_checks_disappearance() {
        let (surface, mut book, anchors) =
            seeded(vec![Node::new("a", Role::Button, "Compose")]).await;
        *surface.nodes.lock().unwrap() = vec![Node::new("z", Role::Button, "Elsewhere")];

        let report = run_program(
            &surface,
            &mut book,
            &Program {
                steps: vec![click(&anchors[0], "Compose")],
                settle: Settle {
                    until: SettleKind::None,
                    timeout_ms: 10,
                },
                expect: Expect::Gone("Compose".into()),
            },
        )
        .await
        .unwrap();
        assert_eq!(report.expect_met, Some(true));
    }

    #[tokio::test]
    async fn expect_still_checks_the_label_not_just_the_anchor() {
        // The under-checked half: `still` used to pass if the anchor resolved
        // to *anything*, including an element now named "Send failed".
        let (surface, mut book, anchors) =
            seeded(vec![Node::new("a", Role::Button, "Save")]).await;
        // Same binding, different name.
        *surface.nodes.lock().unwrap() = vec![Node::new("a", Role::Button, "Send failed")];

        let report = run_program(
            &surface,
            &mut book,
            &Program {
                steps: vec![Step::Key("Enter".into())],
                settle: Settle {
                    until: SettleKind::None,
                    timeout_ms: 10,
                },
                expect: Expect::Still(Target {
                    anchor: anchors[0].clone(),
                    label: Some("Save".into()),
                }),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            report.expect_met,
            Some(false),
            "an element that changed its name has not stayed the same"
        );
    }

    #[tokio::test]
    async fn a_failure_renders_the_whole_surface_rather_than_an_empty_delta() {
        // After a failed resolve nothing was dispatched, so a delta says
        // `(no change)` — while the error tells the model to re-observe. It
        // would re-observe, get nothing, and flail.
        let (surface, mut book, _) = seeded(vec![Node::new("a", Role::Button, "Save")]).await;
        let report = run_program(
            &surface,
            &mut book,
            &program(vec![click("notanissuedanchor", "Save")], "Save"),
        )
        .await
        .unwrap();

        assert!(report.error.is_some());
        assert!(
            report.observation.full,
            "a failure must hand back the whole surface, not an empty delta"
        );
        assert!(!report.observation.entries.is_empty());
    }

    #[tokio::test]
    async fn the_report_records_claimed_and_actual_names() {
        let (surface, mut book, anchors) =
            seeded(vec![Node::new("a", Role::Button, "Save…")]).await;
        let report = run_program(
            &surface,
            &mut book,
            &program(vec![click(&anchors[0], "Save")], "Save"),
        )
        .await
        .unwrap();

        assert_eq!(report.steps[0].label.as_deref(), Some("Save"));
        assert_eq!(report.steps[0].resolved_name.as_deref(), Some("Save…"));
    }
}
