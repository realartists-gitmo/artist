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

/// One pointer click, fully specified.
///
/// A struct rather than five positional arguments because the last three are
/// all "usually the default" — a bare left click is `Pointing::at(rect)`, and
/// the interesting cases name what makes them interesting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pointing {
    pub at: Rect,
    /// An evdev button code. [`crate::program::Button::evdev`] produces it.
    pub button: u32,
    /// 2 is a double click, delivered inside the toolkit's double-click window
    /// rather than as two independent clicks — which is the whole difference
    /// between opening a file and renaming it.
    pub count: u8,
    /// Held down for the duration, then released.
    pub modifiers: crate::keys::Modifiers,
}

impl Pointing {
    /// A single unmodified left click, which is what most callers want.
    pub fn at(at: Rect) -> Self {
        Self {
            at,
            button: 0x110,
            count: 1,
            modifiers: crate::keys::Modifiers::default(),
        }
    }

    pub fn with_button(mut self, button: u32) -> Self {
        self.button = button;
        self
    }

    pub fn with_count(mut self, count: u8) -> Self {
        self.count = count;
        self
    }

    pub fn with_modifiers(mut self, modifiers: crate::keys::Modifiers) -> Self {
        self.modifiers = modifiers;
        self
    }
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
    /// Buttons past the left one, and a pointer that can move without pressing.
    /// Separate from `pointer` because a seat can deliver a click and still have
    /// no way to express hover or a right button.
    pub buttons: bool,
    /// A selection this seat can read and write.
    pub clipboard: bool,
    /// A stylus with pressure and tilt.
    pub tablet: bool,
    /// A gamepad the seat can drive.
    pub gamepad: bool,
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
    async fn pointer(&self, window: WindowKey, pointing: Pointing) -> Result<(), StepError>;
    async fn capture(&self, window: Option<WindowKey>) -> Result<Frame, StepError>;

    /// What this seat has. The default is the seat as it shipped: keys and a
    /// pointer, nothing else.
    fn seat(&self) -> SeatCaps {
        SeatCaps {
            keyboard: true,
            pointer: true,
            ..SeatCaps::default()
        }
    }

    /// Move the pointer onto something and leave it there.
    ///
    /// The pointer stays put afterwards, which is the point: a menu opened by
    /// hovering closes the instant the pointer leaves, so a hover that tidied up
    /// after itself would be indistinguishable from doing nothing.
    async fn hover(&self, _window: WindowKey, _at: Rect) -> Result<(), StepError> {
        Err(StepError::Backend(
            "this stage's seat cannot move the pointer without clicking".into(),
        ))
    }

    /// Press a button and hold it. Paired with [`Stage::release`].
    async fn press(&self, _window: WindowKey, _at: Rect, _button: u32) -> Result<(), StepError> {
        Err(StepError::Backend(
            "this stage's seat cannot hold a button down".into(),
        ))
    }

    /// Let go of a held button, wherever the pointer now is.
    async fn release(&self, _window: WindowKey, _button: u32) -> Result<(), StepError> {
        Err(StepError::Backend(
            "this stage's seat cannot hold a button down".into(),
        ))
    }

    /// Drag along a path, with real intermediate motion.
    ///
    /// Not press-then-release at two points: a drag with no motion between its
    /// endpoints is ignored by every toolkit that starts a drag from a movement
    /// threshold, which is all of them.
    async fn drag(
        &self,
        _window: WindowKey,
        _from: Rect,
        _to: Rect,
        _button: u32,
        _modifiers: crate::keys::Modifiers,
    ) -> Result<(), StepError> {
        Err(StepError::Backend("this stage's seat cannot drag".into()))
    }

    /// Hold a key down, or let it go.
    async fn key_hold(
        &self,
        _window: WindowKey,
        _stroke: &str,
        _pressed: bool,
    ) -> Result<(), StepError> {
        Err(StepError::Backend(
            "this stage's seat cannot hold a key down".into(),
        ))
    }

    /// Read the seat's selection.
    ///
    /// `None` means the clipboard is empty, which is different from a seat that
    /// has no clipboard at all — that is the error.
    async fn clipboard_get(&self) -> Result<Option<String>, StepError> {
        Err(StepError::Backend("this stage has no clipboard".into()))
    }

    async fn clipboard_set(&self, _text: &str) -> Result<(), StepError> {
        Err(StepError::Backend("this stage has no clipboard".into()))
    }

    /// Ask a window to close, the way its titlebar button would.
    ///
    /// A request, not a kill: the client may raise "save your work?" instead,
    /// and that dialog is a surface like any other.
    async fn close_window(&self, _window: WindowKey) -> Result<(), StepError> {
        Err(StepError::Backend(
            "this stage cannot close a window".into(),
        ))
    }

    /// Resize the output every window is given.
    ///
    /// The stage has one output and every toplevel fills it, so this is what
    /// "resize the window" means here — and it is worth having because viewport
    /// size changes what a responsive application shows.
    async fn resize(&self, _width: u32, _height: u32) -> Result<(), StepError> {
        Err(StepError::Backend("this stage cannot be resized".into()))
    }

    /// Let go of everything: held buttons, held keys, live touch contacts.
    ///
    /// Called after every program, always. A seat outlives the program that
    /// used it, so a button left down turns the next program's click into a
    /// drag and a held ctrl turns its typing into shortcuts — a corruption that
    /// shows up somewhere else entirely and looks like a backend fault.
    async fn relax(&self, _window: WindowKey) -> Result<(), StepError> {
        Ok(())
    }

    /// Scroll at a point. Positive `amount` scrolls down, in pixels, matching
    /// [`crate::program::Step::Scroll`].
    ///
    /// Defaults to a refusal rather than a silent success: a stage that cannot
    /// scroll and says `ok` is indistinguishable, to the model, from a list that
    /// was already at the bottom.
    async fn scroll(
        &self,
        _window: WindowKey,
        _at: Rect,
        _amount: i32,
        _axis: crate::program::Axis,
    ) -> Result<(), StepError> {
        Err(StepError::Backend(
            "this stage's seat has no scroll axis".into(),
        ))
    }

    /// The Android accessibility bridge, when this stage has one.
    ///
    /// On the trait rather than reached for by downcasting, because the attach
    /// path should not have to know which concrete stage it is holding — it
    /// asks every stage the same question and Android is the only one that
    /// answers. `None` also covers the ordinary Android case where the bridge
    /// APK is simply not installed, which is a rung to decline rather than a
    /// failure.
    #[cfg(all(target_os = "linux", feature = "stage-wayland"))]
    fn bridge(&self) -> Option<std::sync::Arc<crate::android::bridge::Bridge>> {
        None
    }

    /// Deliver a touch gesture.
    async fn gesture(&self, _window: WindowKey, _gesture: &Gesture) -> Result<(), StepError> {
        Err(StepError::Backend(
            "this stage's seat has no touch device".into(),
        ))
    }

    /// Draw a stroke with a stylus.
    ///
    /// Separate from [`Stage::drag`] because a stylus is not a mouse that
    /// reports extra numbers: a drawing application reads pressure to decide
    /// stroke width and tilt to decide nib shape, and it reads them from a
    /// different protocol object entirely. A drag with a pressure asked for and
    /// no tablet under it must refuse rather than fall back to the pointer —
    /// the resulting line would be the right shape and the wrong weight, which
    /// looks like the application misbehaving.
    ///
    /// `pressure` is 0.0 to 1.0. `tilt` is degrees from vertical on each axis.
    async fn stylus(
        &self,
        _window: WindowKey,
        _from: Rect,
        _to: Rect,
        _pressure: f32,
        _tilt: (f32, f32),
    ) -> Result<(), StepError> {
        Err(StepError::Backend("this stage's seat has no tablet".into()))
    }

    /// Subscribe to damage. Used to build settle predicates.
    fn damage(&self) -> tokio::sync::broadcast::Receiver<Damage>;

    async fn shutdown(&self) -> Result<(), StepError>;
}
