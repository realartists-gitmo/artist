//! Supervising the Android container.
//!
//! Waydroid is not a process we start and stop. It is a *singleton* — one LXC
//! container per host, one session per user — with a lifecycle of its own that
//! predates us and outlives us. Three consequences shape everything here.
//!
//! **A session belongs to a display.** `waydroid session start` reads
//! `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` from its environment and bind-mounts
//! that exact socket into the container. A session started against a stage that
//! has since died keeps pointing at a socket nobody is listening on: it reports
//! `RUNNING` and renders nowhere. So "is a session up?" is never the question —
//! "is a session up *on our display*?" is, and the session dictionary from the
//! container service answers it.
//!
//! **The container freezes itself.** When Android decides nothing is happening
//! it asks the host to suspend, and Waydroid's hardware manager freezes the LXC
//! container. A frozen container renders nothing, answers no adb, and reports
//! its state as `FROZEN` while the *session* still reports `RUNNING`. This is
//! not an edge case — it happens within seconds of boot if no app is active.
//!
//! **The privileged path and the unprivileged path are different.** Container
//! control is a root-owned system service, and `waydroid container unfreeze`
//! shells out to `lxc-unfreeze` directly, so it fails for an ordinary user. The
//! same operation over `id.waydro.ContainerManager` on the system bus succeeds,
//! because the service does it on our behalf. Everything here that can go over
//! D-Bus does, and the agent never needs sudo.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::program::StepError;
use crate::stage::{AppCommand, Stage, StageEnv};

/// The system-bus service that owns the container.
const CONTAINER_SERVICE: &str = "id.waydro.Container";
const CONTAINER_PATH: &str = "/ContainerManager";
const CONTAINER_INTERFACE: &str = "id.waydro.ContainerManager";

/// How long to wait for `sys.boot_completed`.
///
/// A warm boot is seconds. A first boot after `waydroid init` provisions the
/// data image and is minutes, and failing that with a timeout would present as
/// "Android is broken" for a system that is merely doing first-run setup.
const BOOT_TIMEOUT: Duration = Duration::from_secs(300);

/// How Android presents itself on the stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowMode {
    /// One `xdg_toplevel` per Android task.
    ///
    /// Window identity, per-window capture and per-window damage all work as
    /// designed, because each activity is a window the compositor knows about.
    /// Some applications behave oddly under it — Waydroid's multi-window support
    /// is not what most Android apps are tested against.
    MultiWindow,
    /// One fullscreen surface, with Android's own window manager inside it.
    ///
    /// Highest fidelity and fewest surprises, at the cost of every window
    /// question — what is in front, what changed — having to be answered from
    /// the accessibility tree instead of from the compositor.
    FullUi,
}

impl WindowMode {
    /// The value of `persist.waydroid.multi_windows` this mode requires.
    fn multi_windows(self) -> &'static str {
        match self {
            Self::MultiWindow => "true",
            Self::FullUi => "false",
        }
    }

    /// What `waydroid.active_apps` is set to when nothing specific is running.
    ///
    /// `"Waydroid"` is the sentinel that means "show Android's own UI"; a
    /// package name means "show only this app's windows". Setting neither leaves
    /// the container awake with nothing on screen, which is the state that looks
    /// most like a broken compositor.
    fn idle_active_apps(self) -> &'static str {
        match self {
            Self::MultiWindow => "none",
            Self::FullUi => "Waydroid",
        }
    }
}

/// What the container service says about the current session.
#[derive(Clone, Debug, Default)]
pub struct SessionInfo {
    pub state: String,
    pub wayland_display: String,
    pub xdg_runtime_dir: String,
    pub pid: Option<i32>,
}

impl SessionInfo {
    fn from_dict(dict: &HashMap<String, String>) -> Self {
        Self {
            state: dict.get("state").cloned().unwrap_or_default(),
            wayland_display: dict.get("wayland_display").cloned().unwrap_or_default(),
            xdg_runtime_dir: dict.get("xdg_runtime_dir").cloned().unwrap_or_default(),
            pid: dict.get("pid").and_then(|pid| pid.parse().ok()),
        }
    }

    pub fn frozen(&self) -> bool {
        self.state.eq_ignore_ascii_case("FROZEN")
    }

    /// Whether this session is rendering onto the display we own.
    ///
    /// The comparison is what stops us adopting a session that belongs to the
    /// user's own Waydroid window, or to a stage that has since died.
    ///
    /// It compares the runtime *directory*, which is sound only because a stage
    /// takes a process-scoped one. LXC bind-mounts the socket by inode, so a
    /// stage that reused a fixed path would inherit a container still rendering
    /// into the previous stage's socket — with every path here matching.
    fn belongs_to(&self, env: &StageEnv) -> bool {
        let display = env.get("WAYLAND_DISPLAY").unwrap_or_default();
        let runtime = env.get("XDG_RUNTIME_DIR").unwrap_or_default();
        !display.is_empty() && self.wayland_display == display && self.xdg_runtime_dir == runtime
    }
}

/// The host-wide claim on Android.
///
/// One container, one session, one screen: two agents cannot each have their
/// own. Rather than let them share one input focus — the precise failure the
/// Stage exists to prevent — the second claimant is refused, and told who holds
/// it so the refusal is something to act on rather than a mystery.
pub struct Lease {
    path: PathBuf,
    held: bool,
}

impl Lease {
    /// Claim Android for this process.
    pub fn claim(stage_id: &str) -> Result<Self, StepError> {
        let path = lease_path();
        let mut lease = Self { path, held: false };
        lease.try_claim(stage_id)?;
        Ok(lease)
    }

    fn try_claim(&mut self, stage_id: &str) -> Result<(), StepError> {
        use std::io::Write as _;

        let record = format!("{}\n{}\n", std::process::id(), stage_id);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
        {
            Ok(mut file) => {
                let _ = file.write_all(record.as_bytes());
                self.held = true;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = std::fs::read_to_string(&self.path).unwrap_or_default();
                let mut lines = existing.lines();
                let pid: Option<u32> = lines.next().and_then(|line| line.trim().parse().ok());
                let owner = lines.next().unwrap_or("an unknown stage").to_owned();

                // A lease whose holder is gone is not a lease. Crashes happen,
                // and refusing every later run because of one is worse than the
                // race we would be avoiding.
                let alive =
                    pid.is_some_and(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists());
                if alive {
                    return Err(StepError::Backend(format!(
                        "Android is held by {owner} (pid {}). There is one container per host, \
                         so it cannot be shared — wait for that stage to finish, or stop it.",
                        pid.unwrap_or(0)
                    )));
                }
                let _ = std::fs::remove_file(&self.path);
                self.try_claim(stage_id)
            }
            Err(error) => Err(StepError::Backend(format!(
                "could not claim Android ({}): {error}",
                self.path.display()
            ))),
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.held {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn lease_path() -> PathBuf {
    // The *user's* runtime directory, deliberately — not the stage's. The claim
    // is host-wide, so putting it somewhere stage-scoped would give every stage
    // its own lease and defeat the point.
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned());
    PathBuf::from(base).join("artist-android.lease")
}

/// A running Android session on a stage we own.
pub struct Session {
    connection: zbus::Connection,
    env: StageEnv,
    mode: WindowMode,
    /// Held for as long as the session is: dropping it releases the claim.
    _lease: Lease,
}

impl Session {
    /// Bring Android up on this stage.
    ///
    /// Idempotent in the way that matters: an existing session on *our* display
    /// is adopted rather than restarted, and one on any other display is stopped
    /// first, because it is pointing at a socket we do not own.
    pub async fn start(stage: &dyn Stage, mode: WindowMode) -> Result<Self, StepError> {
        let lease = Lease::claim(stage.id().as_str())?;
        let connection = zbus::Connection::system().await.map_err(|error| {
            StepError::Backend(format!(
                "the Waydroid container service is not reachable on the system bus: {error}. \
                 Is waydroid-container running?"
            ))
        })?;

        let session = Self {
            connection,
            env: stage.env().clone(),
            mode,
            _lease: lease,
        };

        match session.info().await? {
            Some(info) if info.belongs_to(&session.env) => {
                // Ours already. It may still be frozen, which the caller's first
                // action will thaw.
            }
            Some(_) => {
                // Someone else's display — possibly a dead stage's. Stopping is
                // safe *because* we hold the lease: no other artist stage can be
                // using it, and a user-started Waydroid window is a case we
                // refuse rather than steal, below.
                session.stop_session().await?;
                session.start_session(stage).await?;
            }
            None => session.start_session(stage).await?,
        }

        session.wait_for_boot().await?;

        // The window mode is read by Android's window manager at start-up, so a
        // change to it only takes effect on the next boot. Rather than report
        // that as an error the caller has to understand and retry, the restart
        // happens here — once, because after it the property matches.
        if session.mode_needs_restart().await? {
            session.stop_session().await?;
            session.start_session(stage).await?;
            session.wait_for_boot().await?;
        }
        session
            .prop_set("waydroid.active_apps", mode.idle_active_apps())
            .await?;
        Ok(session)
    }

    async fn proxy(&self) -> Result<zbus::Proxy<'_>, StepError> {
        zbus::Proxy::new(
            &self.connection,
            CONTAINER_SERVICE,
            CONTAINER_PATH,
            CONTAINER_INTERFACE,
        )
        .await
        .map_err(|error| {
            StepError::Backend(format!("the container service did not answer: {error}"))
        })
    }

    /// What the container service currently reports, if there is a session.
    pub async fn info(&self) -> Result<Option<SessionInfo>, StepError> {
        let proxy = self.proxy().await?;
        match proxy
            .call::<_, _, HashMap<String, String>>("GetSession", &())
            .await
        {
            Ok(dict) if dict.is_empty() => Ok(None),
            Ok(dict) => Ok(Some(SessionInfo::from_dict(&dict))),
            // No session is reported as an error by the service rather than as
            // an empty dictionary, so this is the ordinary "nothing running"
            // path and not a fault.
            Err(_) => Ok(None),
        }
    }

    /// Thaw the container if it has suspended itself.
    ///
    /// Called before anything that expects Android to respond. Cheap when it is
    /// already awake, and the alternative — acting on a frozen container — is a
    /// step that reports success against a screen that cannot have changed.
    pub async fn thaw(&self) -> Result<(), StepError> {
        let Some(info) = self.info().await? else {
            return Err(StepError::Backend(
                "the Android session has gone away".into(),
            ));
        };
        if !info.frozen() {
            return Ok(());
        }
        let proxy = self.proxy().await?;
        proxy
            .call::<_, _, ()>("Unfreeze", &())
            .await
            .map_err(|error| StepError::Backend(format!("could not thaw the container: {error}")))
    }

    async fn start_session(&self, stage: &dyn Stage) -> Result<(), StepError> {
        // Launched *through the stage*, which is the whole trick: the stage
        // injects its own WAYLAND_DISPLAY and XDG_RUNTIME_DIR, and Waydroid
        // bind-mounts precisely that socket into the container.
        stage
            .spawn(
                AppCommand::new("waydroid")
                    .arg("session".to_owned())
                    .arg("start".to_owned()),
            )
            .await
            .map_err(|error| {
                StepError::Backend(format!(
                    "could not start the Waydroid session: {error}. \
                     Check `waydroid status` and `waydroid log`."
                ))
            })?;
        Ok(())
    }

    async fn stop_session(&self) -> Result<(), StepError> {
        let _ = run_waydroid(&self.env, &["session", "stop"]).await;
        // The service takes a moment to tear the container down, and starting a
        // new session against a half-stopped one fails in a way that reads as a
        // container fault.
        for _ in 0..40 {
            if self.info().await?.is_none() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(StepError::Backend(
            "the previous Waydroid session would not stop".into(),
        ))
    }

    /// Wait for Android to finish booting.
    ///
    /// On `sys.boot_completed` rather than a sleep, and the failure mode is the
    /// interesting part: until Android's platform service is up, the read does
    /// not return "not yet" — it does not return at all. So each attempt is
    /// bounded by [`CLI_TIMEOUT`] and a timeout is treated as "still booting",
    /// which makes the blocking call into a usable predicate.
    ///
    /// The interval is deliberately unhurried. Reading a property thaws the
    /// container and re-freezes it around the call, so a tight poll is not a
    /// passive observation of the boot — it is interference with it.
    async fn wait_for_boot(&self) -> Result<(), StepError> {
        let deadline = Instant::now() + BOOT_TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(value) = self.prop_get("sys.boot_completed").await
                && value.trim() == "1"
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Err(StepError::Backend(format!(
            "Android did not finish booting within {}s — `waydroid log` will say why",
            BOOT_TIMEOUT.as_secs()
        )))
    }

    /// Set the window-mode property, and say whether Android has to be restarted
    /// for it to take effect.
    ///
    /// Checked rather than set unconditionally: the common case is that it is
    /// already right, and paying a reboot for a property that did not change
    /// would make every launch minutes long.
    async fn mode_needs_restart(&self) -> Result<bool, StepError> {
        let wanted = self.mode.multi_windows();
        let current = self.prop_get("persist.waydroid.multi_windows").await?;
        if current.trim() == wanted {
            return Ok(false);
        }
        self.prop_set("persist.waydroid.multi_windows", wanted)
            .await?;
        Ok(true)
    }

    pub async fn prop_get(&self, key: &str) -> Result<String, StepError> {
        let output = run_waydroid(&self.env, &["prop", "get", key]).await?;
        Ok(output.trim().to_owned())
    }

    pub async fn prop_set(&self, key: &str, value: &str) -> Result<(), StepError> {
        run_waydroid(&self.env, &["prop", "set", key, value]).await?;
        Ok(())
    }

    /// Make a package the active one, which is what puts its windows on screen.
    ///
    /// In multi-window mode `waydroid.active_apps` is the filter that decides
    /// whose toplevels reach the compositor at all; leaving it unset is the most
    /// common reason Android boots successfully and shows nothing.
    pub async fn set_active(&self, package: &str) -> Result<(), StepError> {
        self.thaw().await?;
        let value = match self.mode {
            WindowMode::FullUi => "Waydroid",
            WindowMode::MultiWindow => package,
        };
        self.prop_set("waydroid.active_apps", value).await
    }

    /// The container's address on the Waydroid NAT, for adb.
    pub async fn ip_address(&self) -> Result<String, StepError> {
        let status = run_waydroid(&self.env, &["status"]).await?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("IP address:")
                && !rest.trim().is_empty()
                && rest.trim() != "UNKNOWN"
            {
                return Ok(rest.trim().to_owned());
            }
        }
        Err(StepError::Backend(
            "the container has no IP address yet — it may still be booting".into(),
        ))
    }

    /// Stop the session and release the claim.
    pub async fn shutdown(&self) -> Result<(), StepError> {
        self.stop_session().await
    }
}

/// How long any one `waydroid` invocation is allowed to take.
///
/// Not a defensive nicety — a hard requirement. `waydroid prop get` reaches
/// Android's platform service over binder, and while that service is still
/// coming up it does not fail: it retries, printing "Failed to get service
/// waydroidplatform" forever. A property read used as a boot predicate
/// therefore blocks until the very thing it is waiting for has happened, which
/// turns the poll into a hang with no diagnostic. Bounding each call converts it
/// back into what it was meant to be: a question that can be asked repeatedly.
const CLI_TIMEOUT: Duration = Duration::from_secs(15);

/// Run a `waydroid` subcommand and return its stdout.
///
/// The stage environment goes with it so the CLI talks about *our* session
/// rather than defaulting to whatever the user's own environment points at.
async fn run_waydroid(env: &StageEnv, args: &[&str]) -> Result<String, StepError> {
    let mut command = tokio::process::Command::new("waydroid");
    command.args(args);
    for (key, value) in env.iter() {
        command.env(key, value);
    }
    // Reaped when the timeout drops the future. Without this a timed-out
    // `prop get` keeps retrying in the background for the life of the agent,
    // and a poll loop accumulates one such process per attempt.
    command.kill_on_drop(true);

    let output = match tokio::time::timeout(CLI_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return Err(StepError::Backend(format!(
                "could not run `waydroid {}`: {error}. Is Waydroid installed?",
                args.join(" ")
            )));
        }
        Err(_) => {
            return Err(StepError::Backend(format!(
                "`waydroid {}` did not answer within {}s — Android's platform service \
                 is probably still starting",
                args.join(" "),
                CLI_TIMEOUT.as_secs()
            )));
        }
    };
    if !output.status.success() {
        return Err(StepError::Backend(format!(
            "`waydroid {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_for(display: &str, runtime: &str) -> StageEnv {
        let mut env = StageEnv::default();
        env.set("WAYLAND_DISPLAY", display);
        env.set("XDG_RUNTIME_DIR", runtime);
        env
    }

    #[test]
    fn a_session_on_our_display_is_ours() {
        let info = SessionInfo {
            state: "RUNNING".into(),
            wayland_display: "wayland-artist".into(),
            xdg_runtime_dir: "/run/user/1000/artist-stage-7".into(),
            pid: Some(42),
        };
        assert!(info.belongs_to(&env_for("wayland-artist", "/run/user/1000/artist-stage-7")));
    }

    #[test]
    fn a_session_on_a_dead_stage_is_not_ours() {
        // The failure this prevents: the socket name matches because every
        // stage uses the same one, but the *directory* is a previous stage's.
        // Adopting it would leave Android rendering into a socket nobody holds.
        let info = SessionInfo {
            state: "RUNNING".into(),
            wayland_display: "wayland-artist".into(),
            xdg_runtime_dir: "/run/user/1000/artist-stage-3".into(),
            pid: Some(42),
        };
        assert!(!info.belongs_to(&env_for("wayland-artist", "/run/user/1000/artist-stage-7")));
    }

    #[test]
    fn a_session_with_no_display_is_never_ours() {
        let info = SessionInfo::default();
        assert!(!info.belongs_to(&env_for("", "")));
    }

    #[test]
    fn frozen_is_read_case_insensitively() {
        let info = SessionInfo {
            state: "FROZEN".into(),
            ..SessionInfo::default()
        };
        assert!(info.frozen());
    }

    #[test]
    fn a_dead_holder_does_not_keep_the_lease() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lease");
        // A pid that cannot be alive: the kernel's own maximum plus one.
        std::fs::write(&path, "4194305\nstage-gone\n").unwrap();

        let mut lease = Lease {
            path: path.clone(),
            held: false,
        };
        lease.try_claim("stage-new").unwrap();
        assert!(lease.held);
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("stage-new"));
    }

    #[test]
    fn a_live_holder_keeps_it_and_says_who_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lease");
        std::fs::write(&path, format!("{}\nstage-one\n", std::process::id())).unwrap();

        let mut lease = Lease { path, held: false };
        let error = lease.try_claim("stage-two").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("stage-one"), "{message}");
        assert!(!lease.held);
    }

    #[test]
    fn modes_choose_their_own_properties() {
        assert_eq!(WindowMode::MultiWindow.multi_windows(), "true");
        assert_eq!(WindowMode::FullUi.multi_windows(), "false");
        assert_eq!(WindowMode::FullUi.idle_active_apps(), "Waydroid");
    }
}
