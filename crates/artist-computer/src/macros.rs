//! Replaying a trajectory the agent already worked out.
//!
//! This is what makes computer use economically viable rather than merely
//! possible. Working out which element to click, in what order, is the expensive
//! part and the model pays it every single time — unless the successful path is
//! kept. Every program the agent ran is already in the event log as
//! `(anchor, action, expect)` triples, so a run that worked once is a script.
//!
//! **Steps replay by label, not by anchor.** An anchor is a per-session token
//! from a specific observation; it means nothing tomorrow. A label is how the
//! element identifies itself, so a replayer re-observes and resolves the label
//! against the current screen. That also makes replay self-checking: if the
//! interface changed, the label no longer resolves and the macro stops at the
//! step that broke rather than clicking whatever now occupies that position.

use serde::{Deserialize, Serialize};

use crate::anchors::AnchorBook;
use crate::program::{Program, Settle, Step, StepError, Target, check_label};
use crate::surface::{Surface, run_program};

/// One recorded program, in a form that survives the session it came from.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MacroProgram {
    pub surface: String,
    pub steps: Vec<MacroStep>,
    /// The label the original run expected to land on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MacroStep {
    pub action: String,
    /// How the element identified itself. The durable half.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// For `key` steps, which have no element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// A whole distilled trajectory.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Macro {
    #[serde(default)]
    pub session: String,
    pub programs: Vec<MacroProgram>,
}

/// Why a macro stopped.
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error(
        "step {step} of program {program} wanted {label:?}, which is not on the surface any more. \
         The interface has changed; re-do this task with the model rather than replaying it."
    )]
    Drifted {
        program: usize,
        step: usize,
        label: String,
    },
    #[error("program {program}: {source}")]
    Failed {
        program: usize,
        #[source]
        source: StepError,
    },
}

/// Resolve a macro against the surface as it is now, and run it.
///
/// Each program is re-anchored from a fresh observation immediately before it
/// runs. That is the whole safety property: a macro never carries stale
/// coordinates or stale tokens, only names, and a name that no longer resolves
/// stops the replay.
pub async fn replay(
    surface: &dyn Surface,
    book: &mut AnchorBook,
    recorded: &Macro,
) -> Result<usize, ReplayError> {
    let mut completed = 0;

    for (index, program) in recorded.programs.iter().enumerate() {
        let snapshot = surface
            .snapshot()
            .await
            .map_err(|source| ReplayError::Failed {
                program: index,
                source,
            })?;
        let observed = book.observe(&snapshot, true);

        // Resolve every label first, so a macro that cannot run at all fails
        // before it has half-run.
        let mut steps = Vec::with_capacity(program.steps.len());
        for (step_index, step) in program.steps.iter().enumerate() {
            let resolved = match step.action.as_str() {
                "key" => Step::Key(crate::program::KeyPress::Bare(
                    step.key.clone().unwrap_or_default(),
                )),
                _ => {
                    let label = step.label.clone().ok_or_else(|| ReplayError::Drifted {
                        program: index,
                        step: step_index,
                        label: String::new(),
                    })?;
                    let anchor = find_anchor(&observed, &label).ok_or_else(|| {
                        ReplayError::Drifted {
                            program: index,
                            step: step_index,
                            label: label.clone(),
                        }
                    })?;
                    let target = Target {
                        anchor,
                        label: Some(label),
                    };
                    match step.action.as_str() {
                        "type" => Step::Type {
                            target,
                            text: step.text.clone().unwrap_or_default(),
                            clear: true,
                        },
                        _ => Step::Click(target),
                    }
                }
            };
            steps.push(resolved);
        }

        let resolved = Program {
            steps,
            settle: Settle::default(),
            // The recorded expectation is a label, which is exactly what
            // `Expect::Appears` wants — no anchor invention needed.
            expect: match &program.expect {
                Some(label) => crate::program::Expect::Appears(label.clone()),
                // A macro recorded before expectations were durable: assert the
                // surface still has something rather than skipping the check.
                None => crate::program::Expect::Gone("\u{0}unmatchable".into()),
            },
        };

        let report = run_program(surface, book, &resolved)
            .await
            .map_err(|source| ReplayError::Failed {
                program: index,
                source,
            })?;
        if let Some(error) = report.error {
            return Err(ReplayError::Failed {
                program: index,
                source: error,
            });
        }
        completed += 1;
    }
    Ok(completed)
}

/// Find the anchor for an element that calls itself `label`.
///
/// Exact match first, then the same [`check_label`] tolerance a live step gets —
/// literally the same function, rather than a second lookalike that drifts. The
/// earlier hand-rolled version lowercased without normalizing and had no
/// minimum-length guard, so it could select a one-glyph node that `check_label`
/// would then rubber-stamp: both defence layers failing in the same step.
///
/// A containment match must also be *unique*. Replay picking the first of three
/// plausible candidates in tree order is the silent-wrong-target failure this
/// whole design exists to prevent; ambiguity is a miss, and the macro reports a
/// missing anchor rather than guessing.
fn find_anchor(observed: &crate::anchors::Observation, label: &str) -> Option<String> {
    let wanted = crate::program::normalize(label);
    if let Some(entry) = observed
        .entries
        .iter()
        .find(|entry| crate::program::normalize(&entry.node.name) == wanted && !wanted.is_empty())
    {
        return Some(entry.anchor.clone());
    }

    let mut loose = observed.entries.iter().filter(|entry| {
        !entry.node.name.trim().is_empty()
            && check_label("replay", Some(label), &entry.node).is_ok()
    });
    let first = loose.next()?;
    loose.next().is_none().then(|| first.anchor.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Caps, Node, Role, Rung, Snapshot, SurfaceId};
    use crate::program::SettleOutcome;
    use crate::surface::SettleWatch;
    use std::sync::Mutex;

    struct Fake {
        id: SurfaceId,
        nodes: Mutex<Vec<Node>>,
        clicked: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(names: &[&str]) -> Self {
            Self {
                id: SurfaceId::new("fake:1"),
                nodes: Mutex::new(
                    names
                        .iter()
                        .enumerate()
                        .map(|(index, name)| Node::new(format!("n{index}"), Role::Button, *name))
                        .collect(),
                ),
                clicked: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl Surface for Fake {
        fn id(&self) -> &SurfaceId {
            &self.id
        }
        fn rung(&self) -> Rung {
            Rung::Engine
        }
        fn caps(&self) -> Caps {
            Caps {
                click: true,
                ..Caps::default()
            }
        }
        fn title(&self) -> String {
            "fake".into()
        }
        async fn snapshot(&self) -> Result<Snapshot, StepError> {
            Ok(Snapshot::new(self.nodes.lock().unwrap().clone()))
        }
        async fn watch(&self, _settle: &Settle) -> Result<SettleWatch, StepError> {
            Ok(SettleWatch::ready(SettleOutcome::Settled { after_ms: 0 }))
        }
        async fn apply(&self, _step: &Step, node: Option<&Node>) -> Result<(), StepError> {
            if let Some(node) = node {
                self.clicked.lock().unwrap().push(node.name.clone());
            }
            Ok(())
        }
    }

    fn recorded(labels: &[&str]) -> Macro {
        Macro {
            session: "s".into(),
            programs: vec![MacroProgram {
                surface: "fake:1".into(),
                steps: labels
                    .iter()
                    .map(|label| MacroStep {
                        action: "click".into(),
                        label: Some((*label).to_owned()),
                        text: None,
                        key: None,
                    })
                    .collect(),
                expect: Some("Save".into()),
            }],
        }
    }

    #[tokio::test]
    async fn a_macro_replays_against_a_fresh_observation() {
        let surface = Fake::new(&["Compose", "Save", "Cancel"]);
        let mut book = AnchorBook::new();
        let done = replay(&surface, &mut book, &recorded(&["Compose", "Save"]))
            .await
            .unwrap();

        assert_eq!(done, 1);
        assert_eq!(
            *surface.clicked.lock().unwrap(),
            vec!["Compose".to_owned(), "Save".to_owned()],
            "replay must hit the recorded elements, in order"
        );
    }

    #[tokio::test]
    async fn replay_uses_current_anchors_not_recorded_ones() {
        // The surface is the same but the anchors are freshly minted; a macro
        // that carried session-scoped tokens would fail here.
        let surface = Fake::new(&["Save"]);
        let mut book = AnchorBook::new();
        // Churn the book so any anchor from a prior session is long gone.
        for round in 0..5 {
            book.observe(
                &Snapshot::new(vec![Node::new(format!("other{round}"), Role::Button, "x")]),
                true,
            );
        }
        assert_eq!(
            replay(&surface, &mut book, &recorded(&["Save"])).await.unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn a_macro_stops_at_the_step_that_no_longer_exists() {
        // The interface changed: "Send" is gone. Replay must stop and say so
        // rather than click whatever occupies that spot now.
        let surface = Fake::new(&["Compose", "Cancel"]);
        let mut book = AnchorBook::new();
        let error = replay(&surface, &mut book, &recorded(&["Compose", "Send"]))
            .await
            .unwrap_err();

        match error {
            ReplayError::Drifted { step, label, .. } => {
                assert_eq!(step, 1);
                assert_eq!(label, "Send");
            }
            other => panic!("expected drift, got {other:?}"),
        }
        assert!(
            surface.clicked.lock().unwrap().is_empty(),
            "nothing may run when the macro cannot run in full"
        );
    }

    #[test]
    fn a_macro_round_trips_through_json() {
        let original = recorded(&["Compose", "Save"]);
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(serde_json::from_str::<Macro>(&json).unwrap(), original);
        // Anchors must not appear: they are exactly what does not survive.
        assert!(!json.contains("anchor"));
    }

    #[test]
    fn label_lookup_tolerates_the_same_drift_a_live_step_would() {
        let mut book = AnchorBook::new();
        let observed = book.observe(
            &Snapshot::new(vec![Node::new("a", Role::Button, "Save…")]),
            false,
        );
        assert!(find_anchor(&observed, "Save").is_some());
        assert!(find_anchor(&observed, "Delete").is_none());
    }
}
