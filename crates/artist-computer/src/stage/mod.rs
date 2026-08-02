//! The Stage: an isolated graphical session on the user's own machine.
//!
//! The requirement that shapes everything here is *do not disturb the user's
//! live session*. That rules out the usual input paths outright: `uinput` is a
//! kernel-level device whose events go to whatever seat currently has focus, and
//! XTEST on the live display is the same problem wearing a different hat. Either
//! one means the agent and the user fight over one keyboard.
//!
//! So the agent gets its own display server. Once that decision is forced, it
//! turns out to be the better architecture anyway:
//!
//! * **Input is a function call**, delivered into our own seat's focus. There is
//!   no race, because there is no shared device.
//! * **Damage regions are the change signal.** A compositor already computes,
//!   per frame, exactly which rectangles changed — a far better answer to "has
//!   anything happened yet?" than polling a tree or differencing screenshots.
//! * **Window identity is a fact we hold**, not a heuristic over window titles.
//! * **Capture is a buffer read**, with no portal prompt and none of the user's
//!   windows in frame.
//! * **The accessibility tree is unpolluted**, because the stage runs its own
//!   session bus and only agent-launched applications appear on it.
//!
//! The stage shares `$HOME`. An overlay would break the credential reuse that is
//! the entire reason to run locally rather than in a VM, and it would contradict
//! this harness's existing "bash is unsandboxed by design" stance. The safety
//! layer is the stream-rule guardrail and the label cross-check, not the
//! filesystem.

use std::collections::BTreeMap;

use crate::model::{Frame, Rect};
use crate::program::StepError;

pub mod bus;
pub mod damage;
pub mod diff;
#[cfg(all(target_os = "linux", feature = "stage-wayland"))]
pub mod viewer;
#[cfg(all(target_os = "linux", feature = "stage-wayland"))]
pub mod wayland;

/// A stage-scoped identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StageId(pub String);

impl StageId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for StageId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A compositor-assigned window handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowKey(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Wayland,
    X11,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowInfo {
    pub key: WindowKey,
    pub title: String,
    pub app_id: String,
    /// The owning process, which is how a surface is correlated to the browser
    /// or toolkit that produced it — never inferred from the title.
    pub pid: Option<i32>,
    pub geometry: Rect,
    pub kind: WindowKind,
    pub mapped: bool,
}

/// The environment every application launched into a stage inherits.
///
/// Getting this wrong is the difference between a working rung 2 and one that
/// is silently, inexplicably empty — see [`bus`] for the accessibility flags in
/// particular.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StageEnv(pub BTreeMap<String, String>);

impl StageEnv {
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.0.insert(key.into(), value.into());
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.0.iter()
    }
}

/// A command to run inside a stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<std::path::PathBuf>,
}

impl AppCommand {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }
}

/// A running application, and the facts needed to attach a surface to it.
#[derive(Clone, Debug)]
pub struct AppHandle {
    pub pid: i32,
    pub command: AppCommand,
}

/// One frame's worth of change.
#[derive(Clone, Debug, PartialEq)]
pub struct Damage {
    pub window: Option<WindowKey>,
    pub region: Rect,
    /// Whether the client said *this* is what changed.
    ///
    /// False means it committed a new buffer and named no damage, so the region
    /// is the whole surface as a conservative bound — the compositor's way of
    /// saying "something changed and I do not know where".
    ///
    /// The distinction is not cosmetic. A precise full-surface rect is a real
    /// full repaint; an imprecise one carries no location information at all,
    /// and treating them alike makes a client that never reports damage look
    /// like a client repainting everything constantly. Anything that reasons
    /// about *where* — the noise filter, incremental OCR — has to know which it
    /// is holding.
    pub precise: bool,
}

/// A touch gesture, in coordinates the harness resolved from an anchor.
///
/// Separate from [`Stage::pointer`] rather than folded into it, because a
/// touchscreen is not a mouse with one button. Android decides what a contact
/// *meant* from how long it stayed down and how far it travelled: the same two
/// endpoints are a scroll, a fling or a drag depending on timing, and a contact
/// held in place is a long press rather than a slow tap. Collapsing those into a
/// click would make three quarters of a phone UI unreachable.
///
/// Durations are honoured by delivering the contact as a real sequence over
/// wall-clock time, not by stamping the endpoints and hoping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    /// One contact placed and lifted at the centre of `at`.
    ///
    /// `hold_ms` past Android's long-press threshold is a *different gesture*,
    /// which is why it is a parameter rather than an implementation detail.
    Tap { at: Rect, hold_ms: u64 },
    /// One contact dragged from the centre of `from` to the centre of `to`.
    ///
    /// `duration_ms` is what separates a scroll from a fling: the velocity of
    /// the last few points is what Android integrates, so a swipe delivered
    /// instantly scrolls by exactly its length and coasts nowhere.
    Swipe {
        from: Rect,
        to: Rect,
        duration_ms: u64,
    },
    /// Two contacts moving symmetrically about the centre of `at`.
    Pinch {
        at: Rect,
        /// Distance between the two contacts at the start, in pixels.
        from_gap: u32,
        /// …and at the end. Larger than `from_gap` zooms in.
        to_gap: u32,
        duration_ms: u64,
    },
}

/// What a stage's seat can actually do.
///
/// Reported rather than assumed, because a surface's [`crate::model::Caps`] is
/// derived from it: a rung-3 surface may only advertise scrolling if the thing
/// underneath it can scroll, and advertising what the seat does not have is how
/// a step comes back `ok` having done nothing.
/// Named `SeatCaps` rather than `Seat` because the compositor backend already
/// has a `Seat` — smithay's, which is the thing this describes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeatCaps {
    pub keyboard: bool,
    pub pointer: bool,
    pub scroll: bool,
    pub touch: bool,
}

/// An isolated graphical session.
#[async_trait::async_trait]
pub trait Stage: Send + Sync {
    fn id(&self) -> &StageId;
    fn env(&self) -> &StageEnv;

    async fn spawn(&self, command: AppCommand) -> Result<AppHandle, StepError>;
    async fn windows(&self) -> Result<Vec<WindowInfo>, StepError>;

    async fn focus(&self, window: WindowKey) -> Result<(), StepError>;
    async fn key(&self, window: WindowKey, stroke: &str) -> Result<(), StepError>;
    async fn text(&self, window: WindowKey, text: &str) -> Result<(), StepError>;
    async fn pointer(&self, window: WindowKey, at: Rect, button: u32) -> Result<(), StepError>;
    async fn capture(&self, window: Option<WindowKey>) -> Result<Frame, StepError>;

    /// What this seat has. The default is the seat as it shipped: keys and a
    /// pointer, nothing else.
    fn seat(&self) -> SeatCaps {
        SeatCaps {
            keyboard: true,
            pointer: true,
            scroll: false,
            touch: false,
        }
    }

    /// Scroll at a point. Positive `amount` scrolls down, in pixels, matching
    /// [`crate::program::Step::Scroll`].
    ///
    /// Defaults to a refusal rather than a silent success: a stage that cannot
    /// scroll and says `ok` is indistinguishable, to the model, from a list that
    /// was already at the bottom.
    async fn scroll(&self, _window: WindowKey, _at: Rect, _amount: i32) -> Result<(), StepError> {
        Err(StepError::Backend(
            "this stage's seat has no scroll axis".into(),
        ))
    }

    /// Deliver a touch gesture.
    async fn gesture(&self, _window: WindowKey, _gesture: &Gesture) -> Result<(), StepError> {
        Err(StepError::Backend(
            "this stage's seat has no touch device".into(),
        ))
    }

    /// Subscribe to damage. Used to build settle predicates.
    fn damage(&self) -> tokio::sync::broadcast::Receiver<Damage>;

    async fn shutdown(&self) -> Result<(), StepError>;
}
