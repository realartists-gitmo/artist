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
#[cfg(all(feature = "ocr", feature = "stage-wayland"))]
pub mod screen;

/// A watcher armed *before* the action that it observes.
///
/// This split — rather than a single `act_and_settle` — exists because
/// subscribing after dispatch is a race. A fast surface can finish reacting
/// before the watcher attaches, and the wait then burns the full timeout on an
/// already-settled screen while a slow one is missed entirely.
pub struct SettleWatch(pub Box<dyn std::future::Future<Output = SettleOutcome> + Send + Unpin>);

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

    /// The surface as it would be described to someone who has never seen it.
    ///
    /// Identical to [`Surface::snapshot`] for anything with a tree — a tree is
    /// idempotent to read. It exists for the surfaces that are *streams*: a
    /// shell's primary screen hands over what has arrived since the last look
    /// and advances, so reading it twice returns nothing the second time. That
    /// made an elided observation permanently unrecoverable, while the stub left
    /// behind told the model to go and read it again.
    async fn snapshot_full(&self) -> Result<Snapshot, StepError> {
        self.snapshot().await
    }

    /// Arm a change watcher. Called before the final step dispatches.
    async fn watch(&self, settle: &crate::program::Settle) -> Result<SettleWatch, StepError>;

    /// Perform one step against an already-resolved node.
    ///
    /// The node is borrowed from the current epoch: the anchor has resolved and
    /// the label has been checked, so a backend only has to act.
    ///
    /// `secondary` is the second element a step names, and only `drag` names
    /// one. It is resolved and label-checked identically to the first — a drag
    /// onto a stale anchor has to fail as loudly as a click on one, or the file
    /// lands wherever that anchor's element used to be.
    /// `Ok(Some(text))` is an outcome worth reporting in the step's own line —
    /// what `getClipboard` read, and nothing else so far. Every other verb
    /// returns `Ok(None)` and is reported as `ok`, because the surface
    /// observation afterwards is what says what happened.
    async fn apply(
        &self,
        step: &Step,
        node: Option<&Node>,
        secondary: Option<&Node>,
    ) -> Result<Option<String>, StepError>;

    async fn pixels(&self) -> Result<Option<crate::model::Frame>, StepError> {
        Ok(None)
    }

    /// Let go of whatever this surface's input device is still holding.
    ///
    /// A no-op for every backend whose actions are self-contained: a CDP click
    /// and an AT-SPI action cannot leave anything pressed. It matters only where
    /// there is a real seat behind the surface.
    async fn relax(&self) -> Result<(), StepError> {
        Ok(())
    }

    /// Nested surfaces — a browser window's page targets. One level only.
    fn children(&self) -> Vec<Arc<dyn Surface>> {
        Vec::new()
    }

    /// The window on the stage this surface is drawn in, when it is drawn at
    /// all.
    ///
    /// `None` for a page, a terminal and an adapter: they have content but no
    /// window of their own, so anything that is a property of the seat — focus,
    /// most obviously — does not apply to them and should say so rather than
    /// silently doing nothing.
    fn window(&self) -> Option<crate::stage::WindowKey> {
        None
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
        // Resolved before the step is armed or dispatched, like the primary.
        // A drag whose destination is stale must not begin: the press would
        // land, the motion would run, and the release would drop the thing
        // somewhere nobody named.
        let secondary = match resolve_target(book, step.secondary_target()) {
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

        let outcome = surface
            .apply(step, resolved.as_ref(), secondary.as_ref())
            .await;
        let resolved_name = resolved.as_ref().map(|node| node.name.clone());
        match outcome {
            Ok(note) => reports.push(report_for(
                step,
                resolved_name,
                note.unwrap_or_else(|| "ok".into()),
            )),
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

        // A beat before the next step. A person does not dispatch step after
        // step with machine cadence: they observe, decide, act. The surface
        // already pauses *inside* a step (typing, a drag, hover), but a run of
        // steps with zero latency between them is the cadence of a script, so
        // each step is given a jittered interlude before the next begins.
        tokio::time::sleep(crate::human::between_steps()).await;
    }

    // Let go of anything the program was holding, before the surface is read.
    //
    // Two cases need it and they are different. A program that *asked* to hold
    // something — `press` with no `release`, `keyDown` with no `keyUp` — has
    // left the seat armed on purpose and simply never disarmed it. A program
    // that *failed* may have died between a press and its release. Either way
    // the seat outlives the program, so the next one would begin with a button
    // or a modifier already down, and that corruption surfaces somewhere else
    // entirely: a click that arrives as a drag, or typing that arrives as
    // shortcuts. The snapshot is taken afterwards so it describes a surface
    // nothing is still pressing on.
    if failure.is_some() || program.steps.iter().any(|step| step.holds().is_some()) {
        // A failure to relax is not worth replacing the real error with: the
        // program's own outcome is what the model needs to see.
        let _ = surface.relax().await;
    }

    let snapshot = surface.snapshot().await?;

    // `expect` is only meaningful if the program actually finished; reporting a
    // failed expectation on top of a failed step would just be noise.
    let mut expect_met = failure
        .is_none()
        .then(|| check_expect(&snapshot, book, &program.expect));

    // A streaming surface hands over what arrived *since the last look*, so a
    // shell that printed the text one observation ago reports it absent now —
    // and the model is told its assertion failed about something it can plainly
    // see. `Appears` means "it is there", not "it is there in this instant's
    // delta", so a miss is rechecked against everything the surface still holds.
    //
    // Free for every surface with a tree, where the two reads are the same.
    // Deliberately only `Appears`: for `Gone`, the retained buffer of a shell
    // contains every line it ever printed, so consulting it would make "gone"
    // unsatisfiable rather than more accurate.
    if expect_met == Some(false) && matches!(program.expect, Expect::Appears(_)) {
        let retained = surface.snapshot_full().await?;
        if check_expect(&retained, book, &program.expect) {
            expect_met = Some(true);
        }
    }

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
    // A key press aims at whatever holds focus, so the thing to check is not an
    // anchor but the focused node — which the surfaces now report. When the
    // model has named what it expects to activate, that claim is checked exactly
    // like a click's would be.
    if let Step::Key(press) = step {
        let Some(claimed) = press.label() else {
            return Ok(None);
        };
        let Some(focused) = book.focused() else {
            // No backend on this surface reports focus — a terminal, an adapter.
            // Refusing the step would make the label unwritable there; the
            // label's other job, giving the stream rules something to match, is
            // done either way.
            return Ok(None);
        };
        let focused = focused.clone();
        check_label("focus", Some(claimed), &focused)?;
        return Ok(Some(focused));
    }

    // `keyDown` and `keyUp` aim at focus exactly as `key` does, and carry the
    // same optional claim about what they will activate.
    if let Step::KeyDown(press) | Step::KeyUp(press) = step {
        let Some(claimed) = press.label() else {
            return Ok(None);
        };
        let Some(focused) = book.focused() else {
            return Ok(None);
        };
        let focused = focused.clone();
        check_label("focus", Some(claimed), &focused)?;
        return Ok(Some(focused));
    }

    resolve_target(book, step.target())
}

/// Resolve one anchor and verify the model's label still describes it.
///
/// Shared by the primary and the secondary target so the two cannot drift into
/// different strictness — a drag destination checked more loosely than its
/// source would be a hole in exactly the guarantee anchors provide.
fn resolve_target(book: &AnchorBook, target: Option<&Target>) -> Result<Option<Node>, StepError> {
    let Some(Target { anchor, label }) = target else {
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
        label: target
            .and_then(|target| target.label.clone())
            .or_else(|| match step {
                Step::Key(press) => press.label().map(str::to_owned),
                _ => None,
            }),
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

        async fn apply(
            &self,
            step: &Step,
            _node: Option<&Node>,
            _secondary: Option<&Node>,
        ) -> Result<Option<String>, StepError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("apply:{}", step.action()));
            if self.fail_on == Some(step.action()) {
                return Err(StepError::Backend("backend refused".into()));
            }
            Ok(None)
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
        Step::click(Target {
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
        let (surface, mut book, anchors) = seeded(vec![Node::new("a", Role::Button, "Save")]).await;
        let steps = vec![click(&anchors[0], "Save"), Step::Key("Enter".into())];
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
        assert!(
            surface.calls().is_empty(),
            "nothing may dispatch: {:?}",
            surface.calls()
        );
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
        assert_eq!(
            report.expect_met, None,
            "expect is not judged after a failure"
        );
    }

    #[tokio::test]
    async fn expect_is_verified_against_the_surface_after_the_program() {
        let (surface, mut book, anchors) = seeded(vec![Node::new("a", Role::Button, "Save")]).await;
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
        let (surface, mut book, anchors) = seeded(vec![Node::new("a", Role::Button, "Save")]).await;
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

    #[tokio::test]
    async fn a_key_that_names_its_target_is_checked_against_what_holds_focus() {
        use crate::model::NodeState;
        use crate::program::KeyPress;

        let focused = Node::new("b1", Role::Button, "Cancel").with_state(NodeState {
            focused: true,
            ..NodeState::default()
        });
        let (surface, mut book, _) = seeded(vec![
            Node::new("b0", Role::Button, "Delete account"),
            focused,
        ])
        .await;

        // The model believes Enter will press "Delete account". It will not —
        // focus is on Cancel. Before `key` carried a label there was nothing to
        // check and nothing for a guardrail to match on either.
        let claimed = Step::Key(KeyPress::Aimed {
            chord: "Enter".into(),
            label: "Delete account".into(),
        });
        let report = run_program(&surface, &mut book, &program(vec![claimed], "gone"))
            .await
            .unwrap();

        assert!(matches!(
            report.error,
            Some(StepError::LabelMismatch { .. })
        ));
        assert!(
            !surface.calls().iter().any(|call| call == "apply:key"),
            "the key must not be delivered: {:?}",
            surface.calls()
        );
    }

    #[tokio::test]
    async fn a_key_naming_the_focused_element_goes_through() {
        use crate::model::NodeState;
        use crate::program::KeyPress;

        let (surface, mut book, _) = seeded(vec![
            Node::new("b0", Role::Button, "Delete account").with_state(NodeState {
                focused: true,
                ..NodeState::default()
            }),
        ])
        .await;

        let step = Step::Key(KeyPress::Aimed {
            chord: "Enter".into(),
            label: "Delete account".into(),
        });
        let report = run_program(&surface, &mut book, &program(vec![step], "Delete account"))
            .await
            .unwrap();

        assert!(report.error.is_none(), "{:?}", report.error);
        assert_eq!(report.steps[0].outcome, "ok");
        // And it reaches the audit trail, which is what `distill` replays from.
        assert_eq!(report.steps[0].label.as_deref(), Some("Delete account"));
        assert_eq!(report.steps[0].payload.as_deref(), Some("Enter"));
    }

    #[tokio::test]
    async fn a_bare_key_still_works_where_nothing_reports_focus() {
        use crate::program::KeyPress;

        // A terminal or an adapter has no focus concept. Requiring a label there
        // would make whole rungs unusable for no safety gain.
        let (surface, mut book, _) = seeded(vec![Node::new("r0", Role::Row, "PID  COMMAND")]).await;
        let report = run_program(
            &surface,
            &mut book,
            &program(vec![Step::Key(KeyPress::Bare("Enter".into()))], "PID"),
        )
        .await
        .unwrap();

        assert!(report.error.is_none(), "{:?}", report.error);
        assert_eq!(report.steps[0].payload.as_deref(), Some("Enter"));
    }
}
