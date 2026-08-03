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

/// Which pointer button an action uses.
///
/// Named rather than numbered: the model says `"right"`, and each backend maps
/// that to its own spelling — evdev `BTN_RIGHT`, CDP `MouseButton::Right`. A
/// numeric button in the tool schema would be a coordinate by another name,
/// meaningful only to whoever knows the platform's numbering.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

impl Button {
    /// The Linux input event code. `BTN_LEFT` and its two neighbours.
    pub fn evdev(self) -> u32 {
        match self {
            Self::Left => 0x110,
            Self::Right => 0x111,
            Self::Middle => 0x112,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
        }
    }
}

/// Which way a scroll or a drag runs.
///
/// Horizontal exists because a wide table, a timeline and a carousel are all
/// unreachable without it, and the vertical-only seat reported `ok` while moving
/// nothing — the silent-success failure this subsystem is built to eliminate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    #[default]
    Vertical,
    Horizontal,
}

/// One action.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    /// Activate an element.
    ///
    /// `button` and `count` default to a single left click, so the wire form
    /// `{"click":{"anchor":…,"label":…}}` is unchanged. They are on `click`
    /// rather than being three more verbs because "right-click" is one action
    /// with a parameter, and a model that knows `click` then needs to learn
    /// nothing new to open a context menu.
    ///
    /// `modifiers` is what makes a file list or a spreadsheet usable at all:
    /// ctrl+click extends a selection and shift+click ranges it, and neither is
    /// expressible as a click followed by a key.
    Click {
        #[serde(flatten)]
        target: Target,
        #[serde(default, skip_serializing_if = "is_default")]
        button: Button,
        /// 2 is a double click. Capped at 3 — nothing means anything past a
        /// triple, and an unbounded count is a way to hang a client.
        #[serde(default = "one", skip_serializing_if = "is_one")]
        count: u8,
        /// Held for the duration of the click, e.g. `"ctrl"` or `"ctrl+shift"`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        modifiers: Option<String>,
    },
    /// Put the pointer over an element and leave it there.
    ///
    /// Not a click with the press removed: a hover menu, a tooltip and a
    /// reveal-on-hover control are all things a person sees without committing
    /// to anything, and there was no way to ask for one. The pointer stays where
    /// it lands, because that is what makes the menu it opened stay open for the
    /// next step.
    Hover(Target),
    /// Press a button on an element and hold it.
    ///
    /// Paired with [`Step::Release`]. Separate from `click` because the interval
    /// between them is where the meaning lives — a marquee selection, a slider
    /// grab, a canvas stroke. A button still held when the program ends is
    /// released by the harness rather than left to poison the next program.
    Press {
        #[serde(flatten)]
        target: Target,
        #[serde(default, skip_serializing_if = "is_default")]
        button: Button,
    },
    /// Let go, wherever the pointer currently is.
    ///
    /// Targets nothing by design: the whole point of a held press is that the
    /// pointer has since moved somewhere the model may not be able to name.
    Release {
        #[serde(default, skip_serializing_if = "is_default")]
        button: Button,
    },
    /// Drag from one element to another, or from one element in a direction.
    ///
    /// Two named endpoints is the anchor-native way to say "drag this onto
    /// that", and it is what a file manager, a kanban board and a reorderable
    /// list all need. The direction form covers the case with no second element
    /// to name — a resize handle, a slider thumb, a canvas stroke — and keeps
    /// the model out of coordinates there too.
    ///
    /// Exactly one of `to` or `direction` is required; both or neither is a hard
    /// error rather than a guess about which was meant.
    Drag {
        from: Target,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<Target>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction: Option<Direction>,
        /// How far the direction form travels, in pixels.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        distance: Option<u32>,
        #[serde(default, skip_serializing_if = "is_default")]
        button: Button,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        modifiers: Option<String>,
        /// How hard, 0 to 1 — which makes this a stylus stroke rather than a
        /// pointer drag.
        ///
        /// Folded into `drag` rather than given a verb of its own because it is
        /// the same gesture with a different instrument: a line from here to
        /// there. What changes is which device delivers it, and a drawing
        /// application reads pressure to decide stroke width — so a drag with a
        /// pressure asked for is refused where there is no tablet rather than
        /// falling back to the pointer and drawing the right shape at the wrong
        /// weight.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pressure: Option<f32>,
        /// Degrees from vertical, `[x, y]`. Changes the nib shape, not the path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tilt: Option<[f32; 2]>,
    },
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
    /// A press held long enough to mean something else.
    ///
    /// Not a slow click. On a touch platform a held contact is a *different
    /// gesture* with a different meaning — the context menu, the drag handle,
    /// the multi-select — and there is no way to reach any of it by clicking for
    /// longer. Refused by surfaces that have no notion of it, rather than
    /// quietly downgraded to a click, because a long press that silently becomes
    /// a click opens the wrong thing.
    LongPress(Target),
    /// Drag across the surface, in a named direction.
    ///
    /// A direction rather than two points, deliberately: the model naming
    /// coordinates is the thing this subsystem exists to prevent, and "swipe
    /// left on this row" is both what a person means and what survives a
    /// relayout. The harness turns it into a contact path with real velocity,
    /// which is what separates a scroll from a fling.
    Swipe {
        /// What to swipe on. Absent means the surface itself.
        #[serde(default, flatten, skip_serializing_if = "Option::is_none")]
        target: Option<Target>,
        direction: Direction,
        /// How far, in pixels. Defaults to something proportional to the target.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        distance: Option<u32>,
    },
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
        /// Positive scrolls down, or right on the horizontal axis.
        amount: i32,
        #[serde(default, skip_serializing_if = "is_default")]
        axis: Axis,
    },
    /// Two contacts moving apart or together, about an element.
    ///
    /// Stated as a scale rather than as two gaps in pixels — "zoom in twice" is
    /// what a person means, and the contact geometry that produces it is the
    /// harness's problem. Implemented on the stage's touch device since before
    /// this verb existed; it simply had no way to be asked for.
    Pinch {
        #[serde(default, flatten, skip_serializing_if = "Option::is_none")]
        target: Option<Target>,
        /// Above 1 zooms in, below 1 zooms out.
        scale: f32,
    },
    /// Hold a key down, and release it.
    ///
    /// Paired, like [`Step::Press`]. A game's movement key, a push-to-talk
    /// control and a modifier held across several other actions are all things
    /// a tap cannot express. A key still held at the end of a program is
    /// released by the harness.
    KeyDown(KeyPress),
    KeyUp(KeyPress),
    /// Put text on the clipboard.
    ///
    /// The clipboard is how text crosses an application boundary, and it is the
    /// only way to enter a character the stage's keymap cannot produce. Both
    /// were unreachable: the global was advertised and no selection was ever
    /// set.
    SetClipboard {
        text: String,
    },
    /// Read the clipboard into the step report.
    ///
    /// The other half of a copy: an application's own copy button or `ctrl+c`
    /// puts something on the clipboard, and without this the agent cannot see
    /// what it got.
    GetClipboard {},
    /// Hand files to a file input or a drop target.
    ///
    /// Paths on the machine the stage shares `$HOME` with, so a file the agent
    /// just wrote can be uploaded without a round trip through a picker.
    Upload {
        #[serde(flatten)]
        target: Target,
        paths: Vec<String>,
    },
    /// Decide in advance how the next dialog is answered.
    ///
    /// Armed *before* the step that triggers it, because a modal dialog blocks
    /// the surface that raised it — on a page it blocks the renderer, so the
    /// observation that would have shown the dialog cannot be taken. Arming is
    /// the only ordering that works, and it makes the model state its intent
    /// before the irreversible thing rather than after.
    Dialog {
        /// Accept it, or dismiss it. Dismiss is the default everywhere: a
        /// `confirm()` guarding a delete is the case that matters, and the safe
        /// answer to a question nobody armed for is no.
        #[serde(default)]
        accept: bool,
        /// The answer to a `prompt()`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// Go to a URL. Browser surfaces only.
    ///
    /// Without this, a second URL cost an entire browser launch: a new process,
    /// a new profile, and up to 35 s of connect-and-map polling.
    Navigate {
        url: String,
    },
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

fn one() -> u8 {
    1
}

fn is_one(count: &u8) -> bool {
    *count == 1
}

/// So a defaulted enum field stays out of the serialized form, and the wire
/// shape of an ordinary left click is exactly what it was before buttons
/// existed.
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

impl Step {
    /// An ordinary single left click with no modifiers.
    ///
    /// What almost every caller means, and what the wire form
    /// `{"click":{"anchor":…}}` deserializes to. Spelling the three defaults out
    /// at each construction site would bury the one case that is interesting.
    pub fn click(target: Target) -> Self {
        Self::Click {
            target,
            button: Button::Left,
            count: 1,
            modifiers: None,
        }
    }

    pub fn action(&self) -> &'static str {
        match self {
            Self::Click { .. } => "click",
            Self::Type { .. } => "type",
            Self::Key(_) => "key",
            Self::KeyDown(_) => "keyDown",
            Self::KeyUp(_) => "keyUp",
            Self::LongPress(_) => "longPress",
            Self::Swipe { .. } => "swipe",
            Self::Pinch { .. } => "pinch",
            Self::Hover(_) => "hover",
            Self::Press { .. } => "press",
            Self::Release { .. } => "release",
            Self::Drag { .. } => "drag",
            Self::Scroll { .. } => "scroll",
            Self::Navigate { .. } => "navigate",
            Self::Back { .. } => "back",
            Self::Forward { .. } => "forward",
            Self::Invoke { .. } => "invoke",
            Self::SetClipboard { .. } => "setClipboard",
            Self::GetClipboard { .. } => "getClipboard",
            Self::Upload { .. } => "upload",
            Self::Dialog { .. } => "dialog",
        }
    }

    /// The element this step names, when it names one. `key` targets focus and
    /// so has nothing to check.
    pub fn target(&self) -> Option<&Target> {
        match self {
            Self::LongPress(target) | Self::Hover(target) => Some(target),
            Self::Click { target, .. }
            | Self::Press { target, .. }
            | Self::Type { target, .. }
            | Self::Upload { target, .. }
            | Self::Invoke { target, .. } => Some(target),
            // The element the drag starts on. The destination is checked
            // separately by `resolve_drag_to`, because one step naming two
            // elements needs both resolved and only one can be reported here.
            Self::Drag { from, .. } => Some(from),
            Self::Scroll { target, .. }
            | Self::Swipe { target, .. }
            | Self::Pinch { target, .. } => target.as_ref(),
            Self::Key(_) | Self::KeyDown(_) | Self::KeyUp(_) | Self::Release { .. } => None,
            Self::Navigate { .. } | Self::Back { .. } | Self::Forward { .. } => None,
            Self::SetClipboard { .. } | Self::GetClipboard { .. } | Self::Dialog { .. } => None,
        }
    }

    /// The second element a step names, when it names two.
    ///
    /// Only `drag` does. It is resolved and label-checked exactly like the
    /// first: a drag onto a stale anchor is the same error as a click on one,
    /// and dropping a file on whatever happens to be at a remembered position
    /// is precisely the failure anchors exist to prevent.
    pub fn secondary_target(&self) -> Option<&Target> {
        match self {
            Self::Drag { to, .. } => to.as_ref(),
            _ => None,
        }
    }

    /// What this step carries beyond its target: the text to type, or the chord
    /// to press. Without it a step report cannot be replayed.
    pub fn payload(&self) -> Option<&str> {
        match self {
            Self::Type { text, .. } | Self::SetClipboard { text } => Some(text),
            Self::Key(press) | Self::KeyDown(press) | Self::KeyUp(press) => Some(press.chord()),
            Self::Navigate { url } => Some(url),
            Self::Invoke { action, .. } => Some(action),
            Self::Dialog { text, .. } => text.as_deref(),
            // The first path only. A report is a record of what was asked for,
            // not a second copy of the argument list.
            Self::Upload { paths, .. } => paths.first().map(String::as_str),
            Self::Click { .. }
            | Self::LongPress(_)
            | Self::Hover(_)
            | Self::Press { .. }
            | Self::Release { .. }
            | Self::Drag { .. }
            | Self::Scroll { .. }
            | Self::Swipe { .. }
            | Self::Pinch { .. }
            | Self::GetClipboard { .. }
            | Self::Back { .. }
            | Self::Forward { .. } => None,
        }
    }

    /// Whether this step leaves the seat holding something down.
    ///
    /// A program that ends mid-gesture would poison the next one — the button
    /// or key stays pressed on a seat that outlives the program, and the next
    /// click arrives as a drag. `run_program` uses this to know what to let go
    /// of, which is the same guarantee `gesture` already gives a touch contact.
    pub fn holds(&self) -> Option<Held> {
        match self {
            Self::Press { button, .. } => Some(Held::Button(*button)),
            Self::Release { button } => Some(Held::Released(*button)),
            Self::KeyDown(press) => Some(Held::Key(press.chord().to_owned())),
            Self::KeyUp(press) => Some(Held::KeyReleased(press.chord().to_owned())),
            _ => None,
        }
    }
}

/// What a step left held, for the release-on-exit guard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Held {
    Button(Button),
    Released(Button),
    Key(String),
    KeyReleased(String),
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

/// Which way a swipe goes.
///
/// Named for the direction the *finger* travels, which is how a person
/// describes it — "swipe up" moves content up and reveals what is below. The
/// opposite convention reads correctly to nobody outside the code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    /// The offset a swipe of this length travels.
    pub fn offset(self, distance: i32) -> (i32, i32) {
        match self {
            Self::Up => (0, -distance),
            Self::Down => (0, distance),
            Self::Left => (-distance, 0),
            Self::Right => (distance, 0),
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
///
/// Not `Eq`: `pinch` carries a scale factor, and a float has no total equality.
/// Nothing compares programs for identity — the derives exist for tests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    #[error("{anchor} is {role:?} {name:?}, which does not accept `{action}` on this surface")]
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
    let stripped: String = folded
        .chars()
        .filter(|character| *character != '&')
        .collect();
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
