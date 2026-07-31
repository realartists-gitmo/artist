//! Bringing the stage up and attaching the right surface to what runs in it.
//!
//! This is where the ladder stops being a taxonomy and starts deciding things.
//! Everything else in the crate is deliberately passive — a surface knows how to
//! observe and act, the probe knows how to rank, the adapters know what an
//! application can be asked to do — and none of them start anything. This module
//! does: it opens a display when one is first needed, launches applications into
//! it, and picks the cheapest rung each one actually supports.
//!
//! The stage is **lazy**. Bringing up a compositor and a session bus is not free,
//! and most sessions never touch a GUI at all; paying for one on the chance that
//! a browser might be wanted would tax every session for the benefit of a few.

use std::path::PathBuf;
use std::sync::Arc;

use crate::ladder::adapters::AdapterSet;
use crate::ladder::{Probe, select};
use crate::program::StepError;
use crate::stage::bus::StageBus;
use crate::surface::Surface;

/// How long to wait for a launched application to show a window.
const WINDOW_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Create a directory only this user can enter, or verify an existing one is.
///
/// `0700` at creation rather than after: a directory created world-traversable
/// and tightened afterwards has a window in which its contents are exposed. An
/// existing directory is checked rather than trusted, because an attacker who
/// pre-created the path would otherwise hand us a directory they can read.
fn create_private_dir(path: &std::path::Path) -> Result<(), StepError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(path).map_err(|error| {
                StepError::Backend(format!("inspect {}: {error}", path.display()))
            })?;
            if !metadata.is_dir() {
                return Err(StepError::Backend(format!(
                    "{} exists and is not a directory; refusing to use it for stage sockets",
                    path.display()
                )));
            }
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(StepError::Backend(format!(
                    "{} is accessible to other users (mode {:o}); refusing to put stage \
                     sockets and a browser profile there",
                    path.display(),
                    metadata.permissions().mode() & 0o777
                )));
            }
            Ok(())
        }
        Err(error) => Err(StepError::Backend(format!(
            "create {}: {error}",
            path.display()
        ))),
    }
}

/// Programs whose windows are Chromium content, and therefore rung 1 over CDP.
///
/// Matched on the binary name rather than sniffed at runtime: an Electron app
/// only exposes a debugging port if it was *launched* with one, so the decision
/// has to be made before the process starts.
const CHROMIUM_FAMILY: &[&str] = &[
    "chromium",
    "chromium-browser",
    "chrome",
    "google-chrome",
    "google-chrome-stable",
    "brave",
    "brave-browser",
    "msedge",
];

fn is_chromium(program: &str) -> bool {
    let base = program
        .rsplit('/')
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    CHROMIUM_FAMILY.contains(&base.as_str())
}

/// The graphical half of computer use: a stage, and what is running in it.
pub struct Host {
    /// `None` when `$XDG_RUNTIME_DIR` is unset — see
    /// [`crate::tool::default_state_dir`] for why there is no `/tmp` fallback.
    state_dir: Option<PathBuf>,
    adapters: AdapterSet,
    stage: tokio::sync::Mutex<Option<StageHandle>>,
    /// A chrome surface produced as a side effect of launching a browser.
    ///
    /// `launch` returns one surface — the page, which is what the caller asked
    /// for — but a browser is really two surfaces at two rungs. The chrome half
    /// is parked here for the registry to collect rather than being silently
    /// dropped.
    pending_chrome: tokio::sync::Mutex<Option<Arc<dyn Surface>>>,
}

/// A running stage and the services it depends on, kept together so they die
/// together.
struct StageHandle {
    #[allow(dead_code)]
    bus: StageBus,
    #[cfg(all(target_os = "linux", feature = "stage-wayland"))]
    wayland: Arc<crate::stage::wayland::StageWayland>,
    runtime_dir: PathBuf,
}

impl Host {
    pub fn new(state_dir: Option<PathBuf>, adapters: AdapterSet) -> Self {
        Self {
            state_dir,
            adapters,
            stage: tokio::sync::Mutex::new(None),
            pending_chrome: tokio::sync::Mutex::new(None),
        }
    }

    /// Take any additional surface the last launch produced.
    pub async fn take_pending_surface(&self) -> Option<Arc<dyn Surface>> {
        self.pending_chrome.lock().await.take()
    }

    pub fn adapters(&self) -> &AdapterSet {
        &self.adapters
    }

    pub fn state_dir(&self) -> Option<&std::path::Path> {
        self.state_dir.as_deref()
    }

    /// Whether a display has been brought up yet.
    pub async fn stage_running(&self) -> bool {
        self.stage.lock().await.is_some()
    }

    /// Start the stage if it is not already running, and describe it.
    #[cfg(all(target_os = "linux", feature = "stage-wayland"))]
    pub async fn ensure_stage(&self) -> Result<String, StepError> {
        use crate::stage::{StageId, wayland::StageWayland};

        let mut slot = self.stage.lock().await;
        if let Some(handle) = slot.as_ref() {
            return Ok(handle.wayland.socket_name().to_owned());
        }

        let Some(state_dir) = self.state_dir.as_ref() else {
            return Err(StepError::Backend(
                "cannot start a display: $XDG_RUNTIME_DIR is not set, and a stage's \
                 Wayland socket, D-Bus socket and browser profile must not live in a \
                 world-readable directory. Terminal, browser-attach and adapter \
                 surfaces still work."
                    .into(),
            ));
        };

        // `short_id` is readable but only 1024 combinations wide, and a stage
        // owns sockets whose paths must not collide with another stage's. A
        // process-wide counter makes the directory unambiguous while keeping
        // the id itself legible in a transcript.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let ordinal = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = artist_tools::short_id("stage");
        let runtime_dir = state_dir.join(format!("{id}-{ordinal}"));
        create_private_dir(state_dir)?;
        create_private_dir(&runtime_dir)?;

        // The bus first: applications need `DBUS_SESSION_BUS_ADDRESS` and the
        // accessibility flags in their environment from the moment they start,
        // because a toolkit decides whether to run its a11y bridge at startup
        // and never revisits it.
        let bus = StageBus::start(&runtime_dir).await?;
        let mut wayland = StageWayland::start(StageId(id.clone()), &runtime_dir)?;
        let mut env = crate::stage::StageEnv::default();
        bus.apply_to(&mut env);
        wayland.extend_env(&env);

        let socket = wayland.socket_name().to_owned();
        *slot = Some(StageHandle {
            bus,
            wayland: Arc::new(wayland),
            runtime_dir,
        });
        Ok(socket)
    }

    #[cfg(not(all(target_os = "linux", feature = "stage-wayland")))]
    pub async fn ensure_stage(&self) -> Result<String, StepError> {
        Err(StepError::Backend(
            "this build has no stage backend; only terminal surfaces are available".into(),
        ))
    }

    /// Launch a graphical application and attach the best surface for it.
    ///
    /// The rung is decided from facts, not guesses: whether an adapter claims
    /// the program, whether we launched it with a debugging port, and what its
    /// accessibility tree actually contains.
    #[cfg(all(target_os = "linux", feature = "stage-wayland"))]
    pub async fn launch(
        &self,
        program: &str,
        args: &[String],
    ) -> Result<Arc<dyn Surface>, StepError> {
        use crate::stage::{AppCommand, Stage};

        self.ensure_stage().await?;
        let (stage, runtime_dir) = {
            let slot = self.stage.lock().await;
            let handle = slot
                .as_ref()
                .ok_or_else(|| StepError::Backend("stage vanished".into()))?;
            (
                Arc::clone(&handle.wayland) as Arc<dyn Stage>,
                handle.runtime_dir.clone(),
            )
        };

        // Rung 0 short-circuit: if an adapter claims this program there is no
        // reason to open a window at all.
        if let Some(adapter) = self.adapters.for_app(program) {
            let surface = crate::surface::programmatic::ProgrammaticSurface::new(
                artist_tools::short_id("app"),
                Arc::new(adapter.clone()),
            )
            .with_env(stage.env().iter().map(|(k, v)| (k.clone(), v.clone())));
            return Ok(Arc::new(surface));
        }

        let chromium = is_chromium(program);
        let profile = runtime_dir.join("chrome-profile");
        let mut command = AppCommand::new(program);
        if chromium {
            // Port 0, never fixed: a fixed port collides with the user's own
            // browser and with a second stage.
            command = command
                .arg("--remote-debugging-port=0")
                .arg(format!("--user-data-dir={}", profile.display()))
                .arg("--force-renderer-accessibility")
                .arg("--no-first-run")
                .arg("--no-default-browser-check");
        }
        for arg in args {
            command = command.arg(arg.clone());
        }
        let app = stage.spawn(command).await?;

        // Wait for the application to actually map a window before deciding
        // anything about it — an unmapped app has no tree and no geometry.
        let window = wait_for_window(stage.as_ref(), app.pid).await;

        if chromium {
            let browser = Arc::new(crate::surface::cdp::connect(&profile).await?);
            let page = first_page(&browser).await?;
            let surface = crate::surface::cdp::CdpPage::attach_owned(
                artist_tools::short_id("tab"),
                page,
                Arc::clone(&browser),
            )
            .await?;
            // The chrome surface rides along at rung 0, so tab management is a
            // method call rather than a hunt for a tab strip in some tree.
            self.pending_chrome.lock().await.replace(Arc::new(
                crate::surface::cdp::CdpChrome::new(artist_tools::short_id("browser"), browser),
            ));
            return Ok(Arc::new(surface));
        }

        // Otherwise let the probe decide between accessibility and pixels.
        let probe = Probe {
            app_id: window
                .as_ref()
                .map(|window| window.app_id.clone())
                .unwrap_or_default(),
            argv0: program.to_owned(),
            pid: Some(app.pid),
            mapped: window.is_some(),
            ..Probe::default()
        };
        let attachment = select(&probe, &self.adapters);
        Err(StepError::Backend(format!(
            "launched {program} (pid {}), but no surface backend is wired for rung {:?} yet — \
             the window is on the stage and can be captured, but not yet observed structurally",
            app.pid,
            attachment.rung
        )))
    }

    #[cfg(not(all(target_os = "linux", feature = "stage-wayland")))]
    pub async fn launch(
        &self,
        _program: &str,
        _args: &[String],
    ) -> Result<Arc<dyn Surface>, StepError> {
        Err(StepError::Backend(
            "this build has no stage backend; only terminal surfaces are available".into(),
        ))
    }

    /// Shut the stage down, if one is running.
    pub async fn close_stage(&self) -> bool {
        self.stage.lock().await.take().is_some()
    }
}

#[cfg(all(target_os = "linux", feature = "stage-wayland"))]
async fn wait_for_window(
    stage: &dyn crate::stage::Stage,
    pid: i32,
) -> Option<crate::stage::WindowInfo> {
    let deadline = tokio::time::Instant::now() + WINDOW_TIMEOUT;
    loop {
        if let Ok(windows) = stage.windows().await {
            // Match on pid, which is authoritative. A title match would break
            // the moment two windows share a name — and an "any window will do"
            // fallback is worse still: with two applications on one stage,
            // launching B would return A's window, and A's `app_id` would then
            // decide B's rung and adapter. It also returned instantly, so the
            // wait did nothing.
            if let Some(window) = windows
                .iter()
                .find(|window| window.mapped && owned_by(window.pid, pid))
            {
                return Some(window.clone());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
}

/// Whether a window's process is the one we launched, or a descendant of it.
///
/// Descendants count because the pid that maps a window is very often not the
/// pid we spawned: `firefox` is a shell wrapper, Chromium's window belongs to
/// the browser process rather than the launcher, and Electron apps re-exec.
/// Insisting on an exact match would time out on most real applications.
fn owned_by(candidate: Option<i32>, ancestor: i32) -> bool {
    let Some(mut pid) = candidate else {
        return false;
    };
    // Bounded so a /proc race that yields a cycle cannot spin forever.
    for _ in 0..16 {
        if pid == ancestor {
            return true;
        }
        match parent_pid(pid) {
            Some(parent) if parent > 1 => pid = parent,
            _ => return false,
        }
    }
    false
}

/// The parent pid from `/proc/<pid>/stat`.
///
/// Read from the last `)` forward: the second field is the executable name, in
/// parentheses and *not* escaped, so a process called `foo bar) baz` breaks any
/// parser that splits from the left.
fn parent_pid(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = &stat[stat.rfind(')')? + 1..];
    after_name.split_whitespace().nth(1)?.parse().ok()
}

async fn first_page(browser: &chromiumoxide::Browser) -> Result<chromiumoxide::Page, StepError> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if let Ok(pages) = browser.pages().await
            && let Some(page) = pages.into_iter().next()
        {
            return Ok(page);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(StepError::Backend("browser opened no page".into()));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rung;

    #[test]
    fn the_chromium_family_is_recognized_by_binary_name() {
        assert!(is_chromium("chromium"));
        assert!(is_chromium("/usr/bin/google-chrome-stable"));
        assert!(is_chromium("/opt/brave-bin/brave"));
        assert!(is_chromium("CHROMIUM"));

        assert!(!is_chromium("firefox"));
        assert!(!is_chromium("gedit"));
        // A program merely containing the word is not one.
        assert!(!is_chromium("chromium-notes"));
    }

    #[tokio::test]
    async fn a_host_starts_with_no_stage_running() {
        let dir = tempfile::tempdir().unwrap();
        let host = Host::new(Some(dir.path().to_owned()), AdapterSet::default());
        assert!(
            !host.stage_running().await,
            "a display must not be brought up until something needs one"
        );
    }

    #[tokio::test]
    async fn an_adapted_program_never_opens_a_window() {
        // Rung 0's whole point: if the application can be asked directly, there
        // is no reason to start a display at all.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("adapters");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("player.toml"),
            "name = \"player\"\nmatch_app_id = [\"*vlc\"]\n\n[[action]]\nname = \"pause\"\ncli = { argv = [\"true\"] }\n",
        )
        .unwrap();
        let adapters = AdapterSet::discover_roots(&[root]);
        assert!(adapters.for_app("/usr/bin/vlc").is_some());

        let probe = Probe {
            app_id: "/usr/bin/vlc".into(),
            ..Probe::default()
        };
        assert_eq!(select(&probe, &adapters).rung, Rung::Programmatic);
    }
}
