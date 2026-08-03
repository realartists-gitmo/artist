//! Android as a stage.
//!
//! The plan this implements is in `docs/computer-use-platforms.md`: Waydroid
//! inside artist's own compositor, rather than beside it. Android-in-a-container
//! renders through Wayland, so pointing its `hwcomposer` at a display we already
//! own means its windows arrive as ordinary `xdg_toplevel`s — and six of the
//! nine properties the Stage exists to provide transfer unchanged, including the
//! one that matters most, damage regions, because Waydroid forwards
//! SurfaceFlinger's own per-layer dirty rectangles to `wl_surface_damage`.
//!
//! So [`AndroidStage`] is a wrapper, not a second compositor. It delegates every
//! input and capture verb to the Wayland stage underneath and adds the three
//! things Android needs on top:
//!
//! * a **session supervisor** ([`session`]) — because the container is a
//!   host-wide singleton with a lifecycle of its own, and it freezes itself;
//! * a **command channel** ([`adb`]) — because rung 0 on Android is intents and
//!   `content` providers, not D-Bus;
//! * **package identity** ([`identity`]) — because every Android toplevel is
//!   owned by the same process, so the pid that identifies a window everywhere
//!   else identifies nothing here.
//!
//! ## The one property that does not transfer
//!
//! The stage shares `$HOME`, which is what lets an agent use the user's real
//! credentials rather than a blank VM. Android's container does not: files
//! bind-mount, but *accounts do not exist*. Apps are signed out and stay that
//! way until somebody signs in by hand. That is a real cost, it is not fixable
//! from here, and the mitigation is [`crate::stage::viewer`] — a window onto the
//! stage through which a person signs in once, against a data directory that
//! persists.

pub mod adb;
pub mod bridge;
pub mod identity;
pub mod session;
pub mod surface;

use std::sync::Arc;

use crate::model::{Frame, Rect};
use crate::program::StepError;
use crate::stage::{
    AppCommand, AppHandle, Damage, Gesture, SeatCaps, Stage, StageEnv, StageId, WindowInfo,
    WindowKey,
};

pub use session::WindowMode;

/// How long to wait for Android's system services after the container is up.
const SERVICES_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Where Waydroid bind-mounts the container's `/data` from.
///
/// Fixed by Waydroid rather than chosen by us, and the same path its own session
/// dictionary reports as `waydroid_data`.
fn waydroid_data_dir() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("waydroid/data")
}

/// An Android container presented as a stage.
pub struct AndroidStage {
    /// The Wayland stage Android is rendering onto. Every input verb is
    /// delegated here: Waydroid turns `wl_pointer` and `wl_touch` into Android
    /// `input_event`s inside the container, so a tap really is a tap.
    inner: Arc<dyn Stage>,
    session: session::Session,
    adb: adb::Adb,
    identity: identity::Identity,
    /// The accessibility bridge, when its APK is installed and answering.
    ///
    /// Optional on purpose: a container without it is still driveable at rung 3,
    /// and refusing to open the stage because an optional rung is missing would
    /// make the better path a prerequisite for the worse one.
    bridge: Option<Arc<bridge::Bridge>>,
}

impl AndroidStage {
    /// Bring Android up on a Wayland stage and attach to it.
    ///
    /// The stage must already have been prepared with [`prepare_env`]; doing it
    /// here is not possible, because by the time we hold a `dyn Stage` its
    /// environment has already been handed to the compositor thread.
    pub async fn open(inner: Arc<dyn Stage>, mode: WindowMode) -> Result<Self, StepError> {
        let session = session::Session::start(inner.as_ref(), mode).await?;
        let ip = session.ip_address().await?;

        // Before connecting, not after: the first connection to an
        // unauthorized container is refused outright, and the refusal is
        // answered by a dialog on a screen nothing can drive yet.
        adb::authorize(&waydroid_data_dir())?;
        let adb = adb::Adb::connect(&ip).await?;
        // `boot_completed` is not enough — see `wait_for_services`.
        adb.wait_for_services(SERVICES_TIMEOUT).await?;
        let identity = identity::Identity::new(adb.clone());

        // Best effort. A failure here is the ordinary "bridge not installed"
        // case, and it is reported by the ladder as a declined rung rather than
        // as an error that stops Android working at all.
        let bridge = match bridge::Bridge::connect(&adb).await {
            Ok(bridge) => Some(Arc::new(bridge)),
            Err(error) => {
                eprintln!("artist: no Android accessibility bridge ({error}); rung 3 only");
                None
            }
        };

        Ok(Self {
            inner,
            session,
            adb,
            identity,
            bridge,
        })
    }

    /// The command channel into the container.
    pub fn adb(&self) -> &adb::Adb {
        &self.adb
    }

    pub fn session(&self) -> &session::Session {
        &self.session
    }

    /// Launch a package and make it the active app.
    ///
    /// Two steps rather than one, and both are needed. `am start` launches the
    /// activity; `waydroid.active_apps` is what lets its windows reach the
    /// compositor at all in multi-window mode. Doing only the first is the most
    /// common way to have Android boot perfectly and show nothing.
    pub async fn launch_package(&self, package: &str) -> Result<(), StepError> {
        self.session.set_active(package).await?;

        // Ask the package manager which activity is the launcher one rather
        // than guessing `.MainActivity`. The brief form is one line:
        // `com.example/.Main`.
        let resolved = self
            .adb
            .shell(&["cmd", "package", "resolve-activity", "--brief", package])
            .await?;
        let component = resolved
            .lines()
            .map(str::trim)
            .filter(|line| line.contains('/'))
            .next_back()
            .ok_or_else(|| {
                StepError::Backend(format!(
                    "{package} has no launchable activity — is it installed? \
                     `adb shell pm list packages` will say."
                ))
            })?
            .to_owned();

        self.adb
            .shell(&["am", "start", "-W", "-n", &component])
            .await?;
        self.identity.note_launch(package).await;
        Ok(())
    }
}

/// Add what Waydroid needs to a stage environment, before the stage starts.
///
/// One variable, and it is not optional. Waydroid derives the PulseAudio socket
/// from `XDG_RUNTIME_DIR`, which on a stage points at a private directory
/// containing a Wayland socket and nothing else. LXC is then asked to bind-mount
/// a socket that does not exist, the mount fails, and the **container** fails to
/// start with no mention of audio anywhere in the error — which reads as a
/// binder or image fault and sends you looking in the wrong place entirely.
pub fn prepare_env(env: &mut StageEnv, host_runtime_dir: &std::path::Path) {
    if env.get("PULSE_RUNTIME_PATH").is_some() {
        return;
    }
    let pulse = host_runtime_dir.join("pulse");
    if pulse.join("native").exists() {
        env.set("PULSE_RUNTIME_PATH", pulse.to_string_lossy());
    }
}

#[async_trait::async_trait]
impl Stage for AndroidStage {
    fn id(&self) -> &StageId {
        self.inner.id()
    }

    fn env(&self) -> &StageEnv {
        self.inner.env()
    }

    /// Launch a package. `command.program` is a package id, not a path.
    ///
    /// The arguments are ignored rather than forwarded: an Android activity is
    /// started with an intent, and quietly dropping argv into one would invent a
    /// mapping that does not exist. Callers that want an intent should use
    /// [`AndroidStage::adb`] and say so explicitly.
    async fn spawn(&self, command: AppCommand) -> Result<AppHandle, StepError> {
        self.launch_package(&command.program).await?;
        // The pid of an Android app is meaningless on this side of the
        // container boundary — it is a pid in the container's namespace, and
        // nothing here can signal it. Reported as the session's, which is the
        // process that would actually have to be stopped.
        Ok(AppHandle {
            pid: self
                .session
                .info()
                .await?
                .and_then(|info| info.pid)
                .unwrap_or(0),
            command,
        })
    }

    /// Android windows, with package identity filled in.
    async fn windows(&self) -> Result<Vec<WindowInfo>, StepError> {
        let windows = self.inner.windows().await?;
        Ok(self.identity.label(windows).await)
    }

    async fn focus(&self, window: WindowKey) -> Result<(), StepError> {
        self.inner.focus(window).await
    }

    async fn key(&self, window: WindowKey, stroke: &str) -> Result<(), StepError> {
        self.session.thaw().await?;
        self.inner.key(window, stroke).await
    }

    async fn text(&self, window: WindowKey, text: &str) -> Result<(), StepError> {
        self.session.thaw().await?;
        self.inner.text(window, text).await
    }

    async fn pointer(
        &self,
        window: WindowKey,
        pointing: crate::stage::Pointing,
    ) -> Result<(), StepError> {
        // Thawed first, every time. A frozen container accepts input events and
        // does nothing with them, so the step reports success against a screen
        // that could not have changed — the single most confusing failure this
        // subsystem can produce.
        self.session.thaw().await?;
        self.inner.pointer(window, pointing).await
    }

    async fn scroll(
        &self,
        window: WindowKey,
        at: Rect,
        amount: i32,
        axis: crate::program::Axis,
    ) -> Result<(), StepError> {
        self.session.thaw().await?;
        self.inner.scroll(window, at, amount, axis).await
    }

    async fn gesture(&self, window: WindowKey, gesture: &Gesture) -> Result<(), StepError> {
        self.session.thaw().await?;
        self.inner.gesture(window, gesture).await
    }

    async fn hover(&self, window: WindowKey, at: Rect) -> Result<(), StepError> {
        self.session.thaw().await?;
        self.inner.hover(window, at).await
    }

    async fn press(&self, window: WindowKey, at: Rect, button: u32) -> Result<(), StepError> {
        self.session.thaw().await?;
        self.inner.press(window, at, button).await
    }

    async fn release(&self, window: WindowKey, button: u32) -> Result<(), StepError> {
        self.inner.release(window, button).await
    }

    async fn drag(
        &self,
        window: WindowKey,
        from: Rect,
        to: Rect,
        button: u32,
        modifiers: crate::keys::Modifiers,
    ) -> Result<(), StepError> {
        self.session.thaw().await?;
        self.inner.drag(window, from, to, button, modifiers).await
    }

    async fn key_hold(
        &self,
        window: WindowKey,
        stroke: &str,
        pressed: bool,
    ) -> Result<(), StepError> {
        self.session.thaw().await?;
        self.inner.key_hold(window, stroke, pressed).await
    }

    /// The container's clipboard is not the stage's.
    ///
    /// Android keeps its own, inside the container, and the Wayland selection
    /// the stage owns is invisible to it. Answering with the stage's clipboard
    /// would hand back whatever the *host* copied and present it as the phone's
    /// — so this refuses until there is a bridge call for it.
    async fn clipboard_get(&self) -> Result<Option<String>, StepError> {
        Err(StepError::Backend(
            "the Android container keeps its own clipboard, which the stage cannot read".into(),
        ))
    }

    async fn clipboard_set(&self, _text: &str) -> Result<(), StepError> {
        Err(StepError::Backend(
            "the Android container keeps its own clipboard, which the stage cannot write".into(),
        ))
    }

    async fn relax(&self, window: WindowKey) -> Result<(), StepError> {
        self.inner.relax(window).await
    }

    async fn capture(&self, window: Option<WindowKey>) -> Result<Frame, StepError> {
        self.inner.capture(window).await
    }

    fn seat(&self) -> SeatCaps {
        self.inner.seat()
    }

    fn bridge(&self) -> Option<Arc<bridge::Bridge>> {
        self.bridge.clone()
    }

    fn damage(&self) -> tokio::sync::broadcast::Receiver<Damage> {
        self.inner.damage()
    }

    async fn shutdown(&self) -> Result<(), StepError> {
        // The session first: stopping the stage underneath a live container
        // leaves Android rendering into a socket that has gone, which it
        // survives but does not enjoy — and leaves the host-wide lease held by a
        // stage that no longer exists.
        let stopped = self.session.shutdown().await;
        let inner = self.inner.shutdown().await;
        stopped.and(inner)
    }
}
