//! Action programs: what the model asks for, and the checks it must pass.
//!
//! The unit of action is a short program rather than a single verb, because
//! round-trips are the dominant cost of driving a UI. A three-step login is one
//! tool call and one observation, not three of each.
//!
//! Two properties make a program safe to run blind:
//!
//! * every anchor reference carries a `label` — the model's echo of what it
//!   believes it is touching — which is checked against the element's real name
//!   before dispatch; and
//! * `expect` states where the model believes the program lands, so a wrong
//!   click fails loudly instead of succeeding at the wrong thing.

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::anchors::AnchorError;
use crate::model::Node;

/// A reference to one element: the anchor, plus what the model called it.
///
/// The label exists for two independent reasons, and it would be worth keeping
/// for either alone. It gives the stream-rules engine something meaningful to
/// match — a guardrail regex over `{"click":"kv7"}` can see nothing, but over
/// `"label":"Delete account"` it can — and it is a second referential check that
/// catches an anchor which is still live but no longer means what the model
/// thinks it does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub anchor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// A key press, optionally naming what the model believes holds focus.
///
/// The label exists because `key` was the guardrail's blind spot. Every other
/// step carries a [`Target`], so a stream rule watching for `"label":"Delete
/// account"` can see what is about to happen; `key` carried a bare chord, and
/// `[click "More options", key "Enter"]` on a focused destructive default button
/// went through a rule that stopped the plain click. A pattern cannot match text
/// that is not in the arguments.
///
/// The plain string form is kept — `{"key":"Enter"}` — because most key presses
/// aim at nothing in particular: typing into a field, dismissing a menu,
/// scrolling. Demanding a label for those would be noise that teaches the model
/// to write one that is not true.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeyPress {
    Bare(String),
    Aimed {
        chord: String,
        /// What the model believes this key will activate.
        label: String,
    },
}

impl KeyPress {
    pub fn chord(&self) -> &str {
        match self {
            Self::Bare(chord) | Self::Aimed { chord, .. } => chord,
        }
    }

    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Bare(_) => None,
            Self::Aimed { label, .. } => Some(label),
        }
    }
}

impl From<&str> for KeyPress {
    fn from(chord: &str) -> Self {
        Self::Bare(chord.to_owned())
    }
}

/// One action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    Click(Target),
    /// Type into a named element.
    Type {
        #[serde(flatten)]
        target: Target,
        text: String,
        /// Replace what is there, rather than appending to it.
        ///
        /// Defaults to true because that is what "type this into the field"
        /// means every time it is said. Appending silently turned a re-filled
        /// form field into `oldnew`, and the conventional workaround —
        /// `ctrl+a` first — costs a round trip and does not work everywhere.
        #[serde(default = "yes")]
        clear: bool,
    },
    /// A key or chord delivered to whatever holds focus.
    Key(KeyPress),
    /// Scroll a container, or the surface itself.
    ///
    /// The target is a full [`Target`] rather than a bare anchor because it is
    /// resolved and label-checked like any other reference. A bare anchor was
    /// accepted by the schema, never resolved, and silently scrolled the whole
    /// document — so a virtualized inner list could not be scrolled at all and
    /// the attempt reported `ok`.
    Scroll {
        #[serde(default, flatten, skip_serializing_if = "Option::is_none")]
        target: Option<Target>,
        /// Positive scrolls down.
        amount: i32,
    },
    /// Go to a URL. Browser surfaces only.
    ///
    /// Without this, a second URL cost an entire browser launch: a new process,
    /// a new profile, and up to 35 s of connect-and-map polling.
    Navigate { url: String },
    /// Browser history, one entry at a time.
    ///
    /// Braced rather than unit variants so the wire form is `{"back":{}}` — the
    /// same shape as every other step. A unit variant would serialize to the
    /// bare string `"back"`, making one entry in a `steps` array look nothing
    /// like its neighbours.
    Back {},
    Forward {},
    /// Run one of an element's declared actions by name.
    ///
    /// The escape hatch that makes `Node::actions` mean something. Elements
    /// advertise verbs — an AT-SPI menu item's non-default actions, a tab's
    /// `close`, an adapter's `pause` — and without this the model could read
    /// them and never invoke them.
    Invoke {
        #[serde(flatten)]
        target: Target,
        action: String,
    },
}

fn yes() -> bool {
    true
}

impl Step {
    pub fn action(&self) -> &'static str {
        match self {
            Self::Click(_) => "click",
            Self::Type { .. } => "type",
            Self::Key(_) => "key",
            Self::Scroll { .. } => "scroll",
            Self::Navigate { .. } => "navigate",
            Self::Back { .. } => "back",
            Self::Forward { .. } => "forward",
            Self::Invoke { .. } => "invoke",
        }
    }

    /// The element this step names, when it names one. `key` targets focus and
    /// so has nothing to check.
    pub fn target(&self) -> Option<&Target> {
        match self {
            Self::Click(target) | Self::Type { target, .. } | Self::Invoke { target, .. } => {
                Some(target)
            }
            Self::Scroll { target, .. } => target.as_ref(),
            Self::Key(_) => None,
            Self::Navigate { .. } | Self::Back { .. } | Self::Forward { .. } => None,
        }
    }

    /// What this step carries beyond its target: the text to type, or the chord
    /// to press. Without it a step report cannot be replayed.
    pub fn payload(&self) -> Option<&str> {
        match self {
            Self::Type { text, .. } => Some(text),
            Self::Key(press) => Some(press.chord()),
            Self::Navigate { url } => Some(url),
            Self::Invoke { action, .. } => Some(action),
            Self::Click(_) | Self::Scroll { .. } | Self::Back { .. } | Self::Forward { .. } => None,
        }
    }
}

/// How to decide the surface has finished reacting.
///
/// Never a sleep the model guesses at: each backend knows what settling means
/// for it, and a wrong guess is either a flake or wasted seconds on every step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settle {
    #[serde(default)]
    pub until: SettleKind,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    3_000
}

impl Default for Settle {
    fn default() -> Self {
        Self {
            until: SettleKind::default(),
            timeout_ms: default_timeout_ms(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettleKind {
    /// Nothing is changing any more, by whatever measure the backend has.
    #[default]
    Quiet,
    /// Network requests have drained (engine rung only).
    NetworkIdle,
    /// A specific anchor exists.
    Anchor(String),
    /// Don't wait.
    None,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettleOutcome {
    Settled { after_ms: u64 },
    TimedOut { after_ms: u64 },
    Unsupported,
}

/// What the model believes will be true once the program finishes.
///
/// Stated in **durable terms — labels, not anchors.** An anchor is minted by
/// observation, so an element that appears *as a result of* the program has one
/// the model cannot possibly know; requiring it made the honest hypothesis
/// unwritable and produced a false alarm on runs that worked. A label is how an
/// element identifies itself and can be predicted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Expect {
    /// Something named this should now be on the surface.
    Appears(String),
    /// Something named this should now be gone.
    Gone(String),
    /// This specific element should still be there, and still be called this.
    Still(Target),
}

impl Expect {
    /// The label at the heart of the hypothesis, for the audit trail.
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Appears(label) | Self::Gone(label) => Some(label),
            Self::Still(target) => target.label.as_deref(),
        }
    }
}

/// A program: steps, how to wait, and where the model believes it lands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Program {
    pub steps: Vec<Step>,
    #[serde(default)]
    pub settle: Settle,
    /// Required. An optional hypothesis is one nobody states, and the whole
    /// value of `expect` is that it converts a silent wrong turn into an error.
    pub expect: Expect,
}

/// Why a step could not run.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StepError {
    #[error("{}", .0.message("surface"))]
    Anchor(AnchorError),
    #[error(
        "label mismatch: {anchor} is {actual:?}, not {claimed:?}. Re-observe the surface and retry with the current label."
    )]
    LabelMismatch {
        anchor: String,
        claimed: String,
        actual: String,
    },
    #[error(
        "{anchor} is {role:?} {name:?}, which does not accept `{action}` on this surface"
    )]
    Unsupported {
        anchor: String,
        role: String,
        name: String,
        action: &'static str,
    },
    #[error("{0}")]
    Backend(String),
}

/// Check that a label the model supplied still describes the element.
///
/// Normalizes both sides, then accepts equality **or containment either way**:
/// `"Save"` against `"Save…"`, or `"Delete"` against `"Delete account
/// permanently"`, are the same intent expressed at different lengths.
///
/// Deliberately no edit-distance fuzzing. A threshold loose enough to absorb
/// real wording drift also accepts `Cancel` for `Confirm` — four edits apart on
/// seven characters — which is precisely the confusion this check exists to
/// catch.
pub fn check_label(anchor: &str, claimed: Option<&str>, node: &Node) -> Result<(), StepError> {
    let actual = normalize(&node.name);
    // Elements with no accessible name legitimately exist — a terminal cell
    // range, a bare icon — and requiring a label for them would make whole rungs
    // unusable for no safety gain.
    //
    // Keyed on the *raw* name, not the normalized one. Normalization strips
    // punctuation, so a `…` overflow button normalizes to the empty string; if
    // that counted as unnamed, the one control most likely to hide a
    // destructive menu would accept any label at all.
    if node.name.trim().is_empty() {
        return Ok(());
    }
    let Some(claimed) = claimed else {
        return Err(StepError::LabelMismatch {
            anchor: anchor.to_owned(),
            claimed: String::new(),
            actual: node.name.clone(),
        });
    };
    let claimed_norm = normalize(claimed);
    if claimed_norm == actual && !actual.is_empty() || contains_meaningfully(&actual, &claimed_norm)
    {
        return Ok(());
    }
    Err(StepError::LabelMismatch {
        anchor: anchor.to_owned(),
        claimed: claimed.to_owned(),
        actual: node.name.clone(),
    })
}

/// The shortest normalized name that may stand in for a longer one.
///
/// Containment is what lets `"Delete"` match `"Delete account permanently"`, but
/// it is symmetric, and the short side is not always the model's. An element
/// whose accessible name is `"a"` — a terminal cell, a list bullet, a bare icon
/// that leaked one glyph — is contained in *every* label, so unguarded
/// containment rubber-stamps any claim about it. Three characters is the point
/// where a name is plausibly a word rather than a fragment.
const MIN_CONTAINMENT: usize = 3;

/// Containment in either direction, with the short side required to carry
/// enough signal to mean anything.
fn contains_meaningfully(actual: &str, claimed: &str) -> bool {
    let long_enough = |value: &str| value.chars().count() >= MIN_CONTAINMENT;
    (actual.contains(claimed) && long_enough(claimed))
        || (claimed.contains(actual) && long_enough(actual))
}

/// NFKC, casefold, strip access-key markers and trailing ellipses, collapse
/// whitespace. Toolkits differ on all of these for the same visible button.
pub(crate) fn normalize(value: &str) -> String {
    let folded: String = value
        .nfkc()
        .flat_map(|character| character.to_lowercase())
        .collect();
    // Access-key markers only. `&` never occurs in a real name, but `_` does —
    // stripping it everywhere made `delete_all` and `deleteall` the same string,
    // and identifiers are exactly where a one-character difference matters.
    let stripped: String = folded.chars().filter(|character| *character != '&').collect();
    let trimmed = stripped
        .trim()
        .trim_end_matches('…')
        .trim_end_matches("...")
        .trim_matches(|c: char| c.is_ascii_punctuation() && c != ')' && c != '(');
    trimmed.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Role;

    fn node(name: &str) -> Node {
        Node::new("b", Role::Button, name)
    }

    #[test]
    fn an_exact_label_passes() {
        assert!(check_label("kv7", Some("Delete account"), &node("Delete account")).is_ok());
    }

    #[test]
    fn normalization_absorbs_toolkit_noise() {
        for (claimed, actual) in [
            ("save", "Save"),
            ("Save", "Save…"),
            ("Save", "&Save"),
            ("Open file", "Open  file"),
            ("OK", "OK:"),
        ] {
            assert!(
                check_label("kv7", Some(claimed), &node(actual)).is_ok(),
                "{claimed:?} should match {actual:?}"
            );
        }
    }

    #[test]
    fn containment_matches_in_both_directions() {
        assert!(check_label("kv7", Some("Delete"), &node("Delete account permanently")).is_ok());
        assert!(check_label("kv7", Some("Delete account permanently"), &node("Delete")).is_ok());
    }

    #[test]
    fn a_genuinely_different_element_is_rejected() {
        let error = check_label("kv7", Some("Delete account"), &node("Cancel")).unwrap_err();
        assert!(matches!(error, StepError::LabelMismatch { .. }));
        let rendered = error.to_string();
        assert!(rendered.contains("Cancel") && rendered.contains("Delete account"));
    }

    #[test]
    fn cancel_is_never_accepted_for_confirm() {
        // Four edits apart on seven characters: any fuzzy matcher loose enough
        // to be useful would accept this pair. Ours must not.
        assert!(check_label("kv7", Some("Confirm"), &node("Cancel")).is_err());
        assert!(check_label("kv7", Some("Cancel"), &node("Confirm")).is_err());
    }

    #[test]
    fn a_missing_label_is_required_only_when_the_element_has_a_name() {
        assert!(check_label("kv7", None, &node("Delete")).is_err());
        assert!(
            check_label("kv7", None, &node("")).is_ok(),
            "unnamed elements are legitimate and must stay usable"
        );
    }

    #[test]
    fn a_one_character_name_does_not_swallow_every_label() {
        // Containment is symmetric, so an element named "a" is inside every
        // label there is. Accepting that made the cross-check a no-op for
        // terminal cells, bullets and stray-glyph icons.
        for actual in ["a", "▸", "x"] {
            assert!(
                check_label("kv7", Some("Delete account"), &node(actual)).is_err(),
                "{actual:?} must not accept an arbitrary label"
            );
        }
        // The real short names still work, by equality rather than containment.
        assert!(check_label("kv7", Some("x"), &node("x")).is_ok());
        assert!(check_label("kv7", Some("OK"), &node("OK")).is_ok());
    }

    #[test]
    fn a_name_that_normalization_empties_is_still_a_name() {
        // "…" strips to nothing. Treating that as unnamed let the one control
        // most likely to hide a destructive menu accept any label at all.
        for actual in ["…", "...", "!"] {
            assert!(
                check_label("kv7", Some("Delete account"), &node(actual)).is_err(),
                "{actual:?} must not accept an arbitrary label"
            );
        }
    }

    #[test]
    fn key_steps_name_no_element_to_check() {
        assert!(Step::Key("Enter".into()).target().is_none());
        assert_eq!(Step::Key("Enter".into()).action(), "key");
    }

    #[test]
    fn a_program_deserializes_from_the_documented_shape() {
        let program: Program = serde_json::from_value(serde_json::json!({
            "steps": [
                {"click": {"anchor": "kv7", "label": "Compose"}},
                {"type": {"anchor": "m2q", "label": "To", "text": "adam@example.com"}},
                {"key": "Enter"}
            ],
            "settle": {"until": "quiet", "timeoutMs": 3000},
            "expect": {"appears": "Message sent"}
        }))
        .unwrap();

        assert_eq!(program.steps.len(), 3);
        assert_eq!(program.steps[0].action(), "click");
        assert_eq!(program.steps[0].target().unwrap().anchor, "kv7");
        assert_eq!(program.expect, Expect::Appears("Message sent".into()));
        assert_eq!(program.settle.timeout_ms, 3000);
    }

    #[test]
    fn every_expect_form_deserializes() {
        // The old shape demanded an anchor for an element that does not exist
        // yet — a hypothesis the model could not write. All three of these are
        // writable from what the model already knows.
        for (json, expected) in [
            (
                serde_json::json!({"appears": "Message sent"}),
                Expect::Appears("Message sent".into()),
            ),
            (
                serde_json::json!({"gone": "Compose"}),
                Expect::Gone("Compose".into()),
            ),
            (
                serde_json::json!({"still": {"anchor": "kv7", "label": "Save"}}),
                Expect::Still(Target {
                    anchor: "kv7".into(),
                    label: Some("Save".into()),
                }),
            ),
        ] {
            assert_eq!(serde_json::from_value::<Expect>(json).unwrap(), expected);
        }
    }

    #[test]
    fn a_key_step_takes_a_bare_chord_or_a_named_target() {
        // Both forms, because most key presses aim at nothing in particular and
        // demanding a label for those would teach the model to invent one.
        let bare: Step = serde_json::from_value(serde_json::json!({"key": "Enter"})).unwrap();
        assert_eq!(bare, Step::Key(KeyPress::Bare("Enter".into())));
        assert_eq!(bare.payload(), Some("Enter"));

        let aimed: Step = serde_json::from_value(
            serde_json::json!({"key": {"chord": "Enter", "label": "Delete account"}}),
        )
        .unwrap();
        assert_eq!(
            aimed,
            Step::Key(KeyPress::Aimed {
                chord: "Enter".into(),
                label: "Delete account".into(),
            })
        );
        // The label has to survive into the serialized arguments, or the stream
        // rule that watches for it has nothing to match.
        let json = serde_json::to_string(&aimed).unwrap();
        assert!(json.contains("\"label\":\"Delete account\""), "{json}");
    }

    #[test]
    fn typing_replaces_the_field_unless_told_otherwise() {
        let step: Step = serde_json::from_value(
            serde_json::json!({"type": {"anchor": "m2q", "label": "To", "text": "x"}}),
        )
        .unwrap();
        assert!(
            matches!(step, Step::Type { clear: true, .. }),
            "a re-filled field became `oldnew` when this defaulted the other way"
        );

        let appending: Step = serde_json::from_value(serde_json::json!({
            "type": {"anchor": "m2q", "label": "To", "text": "x", "clear": false}
        }))
        .unwrap();
        assert!(matches!(appending, Step::Type { clear: false, .. }));
    }

    #[test]
    fn the_navigation_and_invoke_steps_deserialize() {
        for (json, expected) in [
            (
                serde_json::json!({"navigate": {"url": "https://example.com"}}),
                Step::Navigate {
                    url: "https://example.com".into(),
                },
            ),
            (serde_json::json!({"back": {}}), Step::Back {}),
            (serde_json::json!({"forward": {}}), Step::Forward {}),
            (
                serde_json::json!({"invoke": {"anchor": "kv7", "label": "Docs", "action": "close"}}),
                Step::Invoke {
                    target: Target {
                        anchor: "kv7".into(),
                        label: Some("Docs".into()),
                    },
                    action: "close".into(),
                },
            ),
        ] {
            assert_eq!(serde_json::from_value::<Step>(json).unwrap(), expected);
        }
    }

    #[test]
    fn settle_defaults_to_quiet_with_a_bounded_wait() {
        let settle = Settle::default();
        assert_eq!(settle.until, SettleKind::Quiet);
        assert_eq!(settle.timeout_ms, 3_000);
    }
}
