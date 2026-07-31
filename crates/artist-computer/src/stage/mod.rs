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

    /// Subscribe to damage. Used to build settle predicates.
    fn damage(&self) -> tokio::sync::broadcast::Receiver<Damage>;

    async fn shutdown(&self) -> Result<(), StepError>;
}
