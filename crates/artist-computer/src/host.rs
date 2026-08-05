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
use crate::model::Rung;
use crate::program::StepError;
use crate::stage::bus::StageBus;
use crate::surface::Surface;

/// The stage screen size used when nothing configures one.
///
/// Matches `[computer] screen`'s default so the two cannot drift apart.
pub const DEFAULT_SCREEN: (i32, i32) = (1920, 1080);

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

/// Where a Chromium launch's browser profile comes from, and whether the
/// session's state is written back for the next one.
///
/// E1/E2: identity continuity is the point of running against the user's real
/// profile, not an afterthought. The default (`Auto`) finds this family's real
/// profile under `$XDG_CONFIG_HOME` and clones it in, and every session writes
/// its accumulated state back to a working profile under `$XDG_CACHE_HOME`, so
/// a returning visitor is actually the same visitor. An empty profile is only
/// used when isolation is asked for explicitly.
#[derive(Debug, Clone, Default)]
pub enum BrowserProfile {
    /// Carry this directory (normally the user's real profile) as the seed, and
    /// persist the session's state for the next launch.
    From(std::path::PathBuf),
    /// Find this family's real profile automatically; if there is none, use an
    /// empty one. Either way the session's state is persisted.
    #[default]
    Auto,
    /// Start clean and persist nothing: a one-session isolation.
    Fresh,
}

/// The user's real profile directory for a Chromium-family binary.
///
/// Mirrors the on-disk layout Chromium itself uses. Only families with known
/// layouts are recognised; anything else has no discoverable profile and falls
/// back to a fresh one.
fn real_profile_dir(program: &str) -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    let leaf = match program
        .rsplit('/')
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase()
        .as_str()
    {
        "chromium" | "chromium-browser" => "chromium",
        "google-chrome" | "google-chrome-stable" => "google-chrome",
        "brave" | "brave-browser" => "BraveSoftware/Brave-Browser",
        "msedge" | "microsoft-edge" => "microsoft-edge",
        _ => return None,
    };
    let dir = config.join(leaf);
    dir.is_dir().then_some(dir)
}

/// A stable, filesystem-safe name for a source profile directory.
///
/// Two launches of the same real profile must land on the *same* working
/// profile or there is no continuity; two different sources must not collide.
fn profile_slug(path: &std::path::Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    canonical
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// The persistent working profile for a source, under
/// `$XDG_CACHE_HOME/artist/computers/`.
///
/// Unlike the stage's runtime directory — tmpfs, wiped on reboot — this lives
/// on real disk, so the identity it holds survives the process.
fn working_profile_dir(source: &std::path::Path) -> Option<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(
        cache
            .join("artist")
            .join("computers")
            .join(profile_slug(source)),
    )
}

/// Create `path` and every missing parent with `0700`.
///
/// A credentials-bearing directory must not be world-traversable at any point
/// while it is being created, so the mode is set as the directories are made —
/// the same reasoning as [`create_private_dir`], extended to several levels.
fn create_private_tree(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(path)
}

/// Whether a directory already carries state, i.e. is not empty.
fn directory_has_state(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

/// Establish the session's live profile from the seed, writing into the given
/// `working` profile (which is `None` when persistence does not apply), and
/// return the working profile that should be written back to.
///
/// On a first sight the working profile is cloned from the seed, giving a
/// stable baseline later sessions reuse; on a returning sight the *working*
/// profile is the seed, so state accumulated last time carries over.
fn seed_profile(
    seed: &std::path::Path,
    live: &std::path::Path,
    working: Option<PathBuf>,
) -> Result<Option<PathBuf>, StepError> {
    if let Some(working) = &working {
        create_private_tree(working.parent().unwrap_or(working.as_path())).map_err(|error| {
            StepError::Backend(format!(
                "create working profile dir {}: {error}",
                working.display()
            ))
        })?;
    }

    match &working {
        Some(working) if directory_has_state(working) => {
            // Returning visitor: pick up where the last session left off.
            clone_profile(working, live)?;
        }
        _ => {
            // First sight: seed the live profile from the user's real one, and
            // keep a baseline copy to write back over.
            clone_profile(seed, live)?;
            if let Some(working) = &working {
                let _ = std::fs::remove_dir_all(working);
                create_private_tree(working).map_err(|error| {
                    StepError::Backend(format!(
                        "create working profile {}: {error}",
                        working.display()
                    ))
                })?;
                clone_profile(live, working)?;
            }
        }
    }
    Ok(working)
}

/// Write the session's live profile back to the working profile once the
/// browser exits, so the identity the next session inherits includes what this
/// one logged into.
fn watch_profile_writeback(live: PathBuf, working: Option<PathBuf>, pid: i32) {
    let Some(working) = working else {
        return;
    };
    let proc = format!("/proc/{pid}");
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if !std::path::Path::new(&proc).exists() {
                break;
            }
        }
        // The browser has exited and released its locks; only now is a copy
        // coherent. Best effort: the harness may be dying itself, and losing
        // the write-back then is the correct amount of degradation.
        let _ = std::fs::remove_dir_all(&working);
        let _ = create_private_tree(&working);
        let _ = clone_profile(&live, &working);
    });
}

fn clone_profile(src: &std::path::Path, dst: &std::path::Path) -> Result<(), StepError> {
    use std::fs;
    if !src.is_dir() {
        return Err(StepError::Backend(
            "browser profile must be a directory".into(),
        ));
    }
    fn copy(s: &std::path::Path, d: &std::path::Path) -> std::io::Result<()> {
        fs::create_dir_all(d)?;
        for e in fs::read_dir(s)? {
            let e = e?;
            let n = e.file_name();
            let name = n.to_string_lossy();
            if name.starts_with("Singleton") || name == "DevToolsActivePort" {
                continue;
            }
            let t = e.file_type()?;
            if t.is_symlink() {
                continue;
            }
            let target = d.join(&n);
            if t.is_dir() {
                copy(&e.path(), &target)?;
            } else if t.is_file() {
                fs::copy(e.path(), target)?;
            }
        }
        Ok(())
    }
    copy(src, dst).map_err(|e| StepError::Backend(format!("clone browser profile: {e}")))
}

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
    /// The stage's virtual screen size, from `[computer] screen`.
    ///
    /// Worth configuring rather than fixed: viewport size genuinely changes what
    /// an application shows — a responsive page lays out differently and a list
    /// renders a different number of rows — so a task that depends on seeing a
    /// wide table needs a way to ask for one.
    screen: (i32, i32),
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
    /// The Android container, once something has asked for it.
    ///
    /// Lazy because bringing it up costs a container boot and a host-wide
    /// exclusive claim — neither of which a session that only wants a browser
    /// should pay for.
    #[cfg(all(target_os = "linux", feature = "stage-wayland"))]
    android: Option<Arc<crate::android::AndroidStage>>,
    runtime_dir: PathBuf,
}

/// Why one rung could not drive a surface, and what would change that.
///
/// The reason alone is not worth much — Adam's note on this was exact: an
/// attributed decline is only useful *"as a way of motivating FIXING the better
/// routes, rather than just accepting the first wall."* So a decline carries a
/// remedy whenever one exists. "rung 0 declined" is a shrug; "no adapter matches
/// `zenity` — write one at `~/.artist/computer/adapters/zenity.toml`" is a task.
#[derive(Clone, Debug, PartialEq)]
pub struct Decline {
    pub rung: Rung,
    pub reason: String,
    /// What would promote this surface to that rung. `None` when nothing would
    /// — a program that is genuinely not a browser will never be rung 1, and
    /// pretending otherwise would be busywork dressed as a to-do.
    pub remedy: Option<String>,
}

impl Decline {
    fn new(rung: Rung, reason: impl Into<String>, remedy: Option<&str>) -> Self {
        Self {
            rung,
            reason: reason.into(),
            remedy: remedy.map(str::to_owned),
        }
    }

    /// One line, with the remedy attached when there is one.
    pub fn line(&self) -> String {
        match &self.remedy {
            Some(remedy) => format!(
                "rung {} declined ({}) — {remedy}",
                self.rung.as_u8(),
                self.reason
            ),
            None => format!("rung {} declined ({})", self.rung.as_u8(), self.reason),
        }
    }
}

/// A surface, plus the rungs that could not have it.
pub struct Launched {
    pub surface: Arc<dyn Surface>,
    /// Empty when the best possible rung was reached.
    pub declined: Vec<Decline>,
}

impl Launched {
    fn plain(surface: Arc<dyn Surface>) -> Self {
        Self {
            surface,
            declined: Vec::new(),
        }
    }
}

impl Host {
    pub fn new(state_dir: Option<PathBuf>, adapters: AdapterSet) -> Self {
        Self::sized(state_dir, adapters, DEFAULT_SCREEN)
    }

    pub fn sized(state_dir: Option<PathBuf>, adapters: AdapterSet, screen: (i32, i32)) -> Self {
        Self {
            state_dir,
            adapters,
            screen,
            stage: tokio::sync::Mutex::new(None),
            pending_chrome: tokio::sync::Mutex::new(None),
        }
    }

    pub fn screen(&self) -> (i32, i32) {
        self.screen
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
        let mut wayland = StageWayland::start_sized(
            StageId(id.clone()),
            &runtime_dir,
            self.screen.0,
            self.screen.1,
        )?;
        let mut env = crate::stage::StageEnv::default();
        bus.apply_to(&mut env);
        // Applied to every stage, not only Android ones. It has to be in place
        // *before* the stage starts, and a stage does not know in advance
        // whether Android will later be asked for; setting one variable that
        // nothing else reads is cheaper than the alternative, which is a
        // container that refuses to start with an error about audio.
        if let Some(host_runtime) = self.state_dir.as_ref().and_then(|dir| dir.parent()) {
            crate::android::prepare_env(&mut env, host_runtime);
        }
        wayland.extend_env(&env);

        let socket = wayland.socket_name().to_owned();
        *slot = Some(StageHandle {
            bus,
            wayland: Arc::new(wayland),
            android: None,
            runtime_dir,
        });
        Ok(socket)
    }

    /// Bring the Android container up on this stage, if it is not already.
    ///
    /// Returns the stage everything should then be launched against: once
    /// Android is running, its wrapper is what fills in package identity and
    /// thaws the container before acting, so bypassing it would silently lose
    /// both.
    #[cfg(all(target_os = "linux", feature = "stage-wayland"))]
    pub async fn ensure_android(
        &self,
        mode: crate::android::WindowMode,
    ) -> Result<Arc<crate::android::AndroidStage>, StepError> {
        self.ensure_stage().await?;

        // Built outside the lock. `AndroidStage::open` boots a container, which
        // is minutes on a cold start — holding the stage lock across it would
        // block every unrelated surface operation for the duration.
        let wayland = {
            let slot = self.stage.lock().await;
            let handle = slot
                .as_ref()
                .ok_or_else(|| StepError::Backend("stage vanished".into()))?;
            if let Some(android) = handle.android.as_ref() {
                return Ok(Arc::clone(android));
            }
            Arc::clone(&handle.wayland)
        };

        let android = Arc::new(
            crate::android::AndroidStage::open(wayland as Arc<dyn crate::stage::Stage>, mode)
                .await?,
        );

        let mut slot = self.stage.lock().await;
        let handle = slot.as_mut().ok_or_else(|| {
            StepError::Backend("the stage went away while Android started".into())
        })?;
        // Another caller may have won the race while the container booted. Theirs
        // is the one already installed, and a second container cannot exist, so
        // hand back what is there rather than replacing it.
        Ok(Arc::clone(handle.android.get_or_insert(android)))
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
        cwd: Option<&std::path::Path>,
        browser_profile: BrowserProfile,
    ) -> Result<Launched, StepError> {
        use crate::stage::{AppCommand, Stage};

        self.ensure_stage().await?;
        let (stage, runtime_dir, a11y_address) = {
            let slot = self.stage.lock().await;
            let handle = slot
                .as_ref()
                .ok_or_else(|| StepError::Backend("stage vanished".into()))?;
            (
                handle
                    .android
                    .as_ref()
                    .map(|android| Arc::clone(android) as Arc<dyn Stage>)
                    .unwrap_or_else(|| Arc::clone(&handle.wayland) as Arc<dyn Stage>),
                handle.runtime_dir.clone(),
                handle.bus.a11y_address().map(str::to_owned),
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
            // Rung 0 is the top of the ladder; nothing was passed over.
            return Ok(Launched::plain(Arc::new(surface)));
        }

        let chromium = is_chromium(program);
        let profile = runtime_dir.join("chrome-profile");
        // E1/E2: resolve where this session's profile comes from — the user's
        // real profile by default, persisted across sessions — before the
        // browser starts, because the choice is baked into the command line.
        let working = if chromium {
            let source = match &browser_profile {
                BrowserProfile::From(path) => Some(path.clone()),
                BrowserProfile::Auto => real_profile_dir(program),
                BrowserProfile::Fresh => None,
            };
            match &source {
                Some(seed) => seed_profile(seed, &profile, working_profile_dir(seed))?,
                None => None,
            }
        } else {
            None
        };
        let mut command = AppCommand::new(program);
        // `cwd` was accepted on the tool and dropped here, so a GUI launch
        // silently ran wherever the harness happened to be — which for a file
        // manager or an editor is the difference between opening the right
        // directory and the wrong one.
        command.cwd = cwd.map(std::path::Path::to_owned);
        if chromium {
            // Port 0, never fixed: a fixed port collides with the user's own
            // browser and with a second stage.
            command = command
                .arg("--remote-debugging-port=0")
                .arg(format!("--user-data-dir={}", profile.display()))
                .arg("--force-renderer-accessibility")
                // Without this Chromium guesses its display backend, fails with
                // "The platform failed to initialize", and exits before writing
                // `DevToolsActivePort` — so the connect times out and the whole
                // browser rung is unreachable. It presented as "browser never
                // wrote DevToolsActivePort", which reads like a timing problem
                // and is not.
                //
                // This was hidden because the one test covering the path was
                // skipping for an unrelated reason: `tempfile::tempdir()`
                // respects the umask, so its state directory was mode 0755 and
                // the stage refused it. Two silent failures stacked, and the
                // suite was green.
                .arg("--ozone-platform=wayland")
                // The no-first-run flags below are what Chromium reads, but
                // `--enable-automation` is what it sets in return: it flips
                // `navigator.webdriver` and adds the "Chrome is being
                // controlled" infobar. Both are launch-time properties of the
                // automation build, so both are excluded here — the runtime
                // mask (in `CdpPage::build`) handles the getter for a browser
                // already open.
                .arg("--exclude-switches=enable-automation")
                .arg("--disable-blink-features=AutomationControlled")
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
            // E2: watch the browser; when it exits, write its accumulated state
            // back to the persistent working profile for the next session.
            watch_profile_writeback(profile.clone(), working, app.pid);
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
            return Ok(Launched {
                surface: Arc::new(surface),
                declined: vec![Decline::new(
                    Rung::Programmatic,
                    format!("no adapter matches {program:?}"),
                    Some(adapter_remedy(program).as_str()),
                )],
            });
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

        // Down the ladder, recording why each rung declined. A bare
        // "unsupported" tells the reader nothing; "no adapter matches this
        // program" is a to-do with a template attached.
        let mut declined: Vec<Decline> = Vec::new();
        if self.adapters.for_app(program).is_none() {
            declined.push(Decline::new(
                Rung::Programmatic,
                format!("no adapter matches {program:?}"),
                Some(adapter_remedy(program).as_str()),
            ));
        }
        if !chromium {
            // No remedy: a program that is not Chromium-based will never speak
            // CDP, and inventing a to-do here would be noise in every report.
            declined.push(Decline::new(Rung::Engine, "not a Chromium process", None));
        }

        // Rung 2, Android first. An Android window's accessibility tree lives
        // inside the container and reaches us over the bridge, never over
        // AT-SPI — so trying AT-SPI first would find nothing and drop a
        // perfectly driveable window to pixels.
        #[cfg(all(target_os = "linux", feature = "stage-wayland"))]
        if let Some(bridge) = stage.bridge() {
            match bridge.ping().await {
                Ok(()) => {
                    // The package, which `AndroidStage::windows` has already
                    // resolved from the container. It narrows a tree that spans
                    // every window to the one this launch meant.
                    let package = window
                        .as_ref()
                        .map(|window| window.app_id.clone())
                        .unwrap_or_default();
                    let surface = crate::android::surface::AndroidSurface::new(
                        artist_tools::short_id("android"),
                        bridge,
                        package,
                    );
                    return Ok(Launched {
                        surface: Arc::new(surface),
                        declined,
                    });
                }
                Err(error) => declined.push(Decline::new(
                    Rung::Accessibility,
                    error.to_string(),
                    Some(
                        "build and install the bridge: \
                         `bash scripts/build-android-service.sh`, then \
                         `artist computer doctor` for how to enable it",
                    ),
                )),
            }
        }

        // Rung 2. The application is on the stage's *private* a11y bus, which
        // is what makes attribution exact: only what the agent launched is on
        // it, so an accessible found by pid belongs unambiguously to this
        // window rather than to whatever the user happens to have open.
        if let Some(address) = &a11y_address {
            match attach_accessible(address, app.pid).await {
                Ok(surface) => {
                    return Ok(Launched {
                        surface: Arc::new(surface),
                        declined,
                    });
                }
                Err(error) => declined.push(Decline::new(
                    Rung::Accessibility,
                    error.to_string(),
                    Some(A11Y_REMEDY),
                )),
            }
        } else {
            declined.push(Decline::new(
                Rung::Accessibility,
                "this stage has no accessibility bus",
                Some("run `artist computer doctor`; the at-spi packages are probably missing"),
            ));
        }

        // Rung 3. Not a dead end any more: the harness reads the screen, finds
        // the text and mints the anchors, so the model names things exactly as
        // it does everywhere else and never sees a coordinate.
        #[cfg(feature = "ocr")]
        if let Some(window) = window.as_ref() {
            match crate::ocr::Ocr::load_default() {
                Ok(ocr) => {
                    // Landing here means a *better* rung was available in
                    // principle and could not be reached. Silence would make
                    // that indistinguishable from "this really is a pixel
                    // application", which is the difference between a bug to
                    // fix and a limitation to accept.
                    if !declined.is_empty() {
                        eprintln!(
                            "artist: {program} fell to rung 3 (pixels); {}",
                            declined
                                .iter()
                                .map(Decline::line)
                                .collect::<Vec<_>>()
                                .join("; ")
                        );
                    }
                    let surface = crate::surface::screen::ScreenSurface::new(
                        artist_tools::short_id("screen"),
                        Arc::clone(&stage),
                        window.key,
                        ocr,
                    );
                    return Ok(Launched {
                        surface: Arc::new(surface),
                        declined,
                    });
                }
                Err(error) => declined.push(Decline::new(
                    Rung::Pixels,
                    error.to_string(),
                    Some("text recognition weights are missing; `artist computer doctor` says where they go"),
                )),
            }
        }

        // Every rung reported why, rather than one bare "unsupported". The
        // difference matters: "no adapter matches this program" is a to-do with
        // a template attached, while "something went wrong" is a dead end.
        Err(StepError::Backend(format!(
            "launched {program} (pid {}), but nothing can drive it. The probe wanted rung {:?}; \
             {}.",
            app.pid,
            attachment.rung,
            declined
                .iter()
                .map(Decline::line)
                .collect::<Vec<_>>()
                .join("; ")
        )))
    }

    #[cfg(not(all(target_os = "linux", feature = "stage-wayland")))]
    pub async fn launch(
        &self,
        _program: &str,
        _args: &[String],
        _cwd: Option<&std::path::Path>,
    ) -> Result<Launched, StepError> {
        Err(StepError::Backend(
            "this build has no stage backend; only terminal surfaces are available".into(),
        ))
    }

    /// The running Wayland stage, starting one if needed.
    ///
    /// Exposed for the viewer, which needs the concrete type: only this stage
    /// can export a render target, and a `dyn Stage` returning `None` for the
    /// other implementations would be pretending otherwise.
    #[cfg(all(target_os = "linux", feature = "stage-wayland"))]
    pub async fn wayland_stage(
        &self,
    ) -> Result<Arc<crate::stage::wayland::StageWayland>, StepError> {
        self.ensure_stage().await?;
        let slot = self.stage.lock().await;
        slot.as_ref()
            .map(|handle| Arc::clone(&handle.wayland))
            .ok_or_else(|| StepError::Backend("stage vanished".into()))
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

/// The AT-SPI registry's root, whose children are the applications on the bus.
#[cfg(all(target_os = "linux", feature = "stage-wayland"))]
const A11Y_ROOT: (&str, &str) = ("org.a11y.atspi.Registry", "/org/a11y/atspi/accessible/root");

/// Attach a rung-2 surface to the application we just launched.
///
/// The match is on **pid**, resolved from each accessible's own bus connection
/// via `GetConnectionUnixProcessID`. That is authoritative: an application's
/// D-Bus name is owned by the process that registered it. Matching on the
/// accessible's *name* instead would break the moment two windows share a
/// title, which is the whole class of bug anchors exist to prevent — and the
/// stage bus makes the pid route available where the user's bus would not.
///
/// Descendants count, because the process that registers the a11y bridge is
/// often not the one we spawned: shell wrappers and re-execing toolkits are
/// ordinary.
#[cfg(all(target_os = "linux", feature = "stage-wayland"))]
async fn attach_accessible(
    address: &str,
    pid: i32,
) -> Result<crate::surface::atspi::AtspiSurface, StepError> {
    use atspi_proxies::accessible::AccessibleProxy;

    let connection = zbus::connection::Builder::address(address)
        .map_err(|error| StepError::Backend(format!("a11y bus address: {error}")))?
        .build()
        .await
        .map_err(|error| StepError::Backend(format!("connect to the a11y bus: {error}")))?;

    let dbus = zbus::fdo::DBusProxy::new(&connection)
        .await
        .map_err(|error| StepError::Backend(format!("bus proxy: {error}")))?;

    // A toolkit registers its bridge some time after the process starts, so
    // this polls rather than looking once.
    let deadline = tokio::time::Instant::now() + A11Y_TIMEOUT;
    loop {
        let root = AccessibleProxy::builder(&connection)
            .destination(A11Y_ROOT.0)
            .and_then(|builder| builder.path(A11Y_ROOT.1))
            .map_err(|error: zbus::Error| StepError::Backend(format!("a11y registry: {error}")))?
            .build()
            .await
            .map_err(|error: zbus::Error| StepError::Backend(format!("a11y registry: {error}")))?;

        if let Ok(applications) = root.get_children().await {
            for application in applications {
                // An application with no unique bus name is not something we can
                // attribute to a process, and attributing by anything weaker is
                // the guess this whole design refuses to make.
                let Some(unique) = application.name() else {
                    continue;
                };
                let destination = unique.as_str().to_owned();
                let Ok(owner) = dbus
                    .get_connection_unix_process_id(unique.clone().into())
                    .await
                else {
                    continue;
                };
                if !owned_by(Some(owner as i32), pid) {
                    continue;
                }
                let surface = crate::surface::atspi::AtspiSurface::new(
                    artist_tools::short_id("win"),
                    connection.clone(),
                    destination,
                    application.path().as_str().to_owned(),
                );
                // A bridge that has registered but not yet built its tree is a
                // one-node stub, and attaching to it hands the model an empty
                // screen it cannot act on. Wait for something real.
                if usable_tree(&surface).await {
                    return Ok(surface);
                }
            }
        }

        if tokio::time::Instant::now() >= deadline {
            return Err(StepError::Backend(format!(
                "no application on the stage's accessibility bus belongs to pid {pid} \
                 (or its tree is still empty after {A11Y_TIMEOUT:?})"
            )));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Whether a candidate tree has enough in it to be worth attaching to.
///
/// A toolkit registers its accessibility bridge before it has built a tree, so
/// attaching too early gives the model a blank surface it cannot distinguish
/// from an application that genuinely has no controls. This is the guard against
/// that — and it has to measure *usefulness*, not size.
///
/// It originally required eight nodes, which was a number carried over from the
/// design sketch with nothing behind it. A live test found the cost: a `zenity
/// --info` dialog has **six** — a window, a label, its text and a button — and
/// was rejected as a stub despite being perfectly driveable. The launch then
/// spent fifteen seconds retrying before falling down a rung.
///
/// What actually separates a stub from a real tree is not node count but whether
/// anything in it can be *named and acted on*. A freshly-registered bridge
/// exposes an application root and nothing else.
#[cfg(all(target_os = "linux", feature = "stage-wayland"))]
async fn usable_tree(surface: &crate::surface::atspi::AtspiSurface) -> bool {
    use crate::surface::Surface;

    let Ok(snapshot) = surface.snapshot().await else {
        return false;
    };
    snapshot.nodes.iter().any(|node| {
        !node.name.trim().is_empty() && (node.role.is_interactive() || !node.actions.is_empty())
    })
}

/// How long to wait for a toolkit to put its accessibility tree on the bus.
#[cfg(all(target_os = "linux", feature = "stage-wayland"))]
const A11Y_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

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

pub(crate) async fn first_page(
    browser: &chromiumoxide::Browser,
) -> Result<chromiumoxide::Page, StepError> {
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

/// The remedy for a missing rung-0 adapter: name the file to write.
///
/// A path the reader can act on beats a category. This is the single highest-value
/// remedy in the ladder, because rung 0 is where the real wins are — a D-Bus call
/// instead of a window.
fn adapter_remedy(program: &str) -> String {
    format!(
        "write an adapter at `$ARTIST_CONFIG_DIR/computer/adapters/{}.toml` if this \
         program has a D-Bus, CLI or localhost-HTTP interface — rung 0 is faster and \
         far more reliable than driving its window",
        program.rsplit('/').next().unwrap_or(program)
    )
}

/// Why an accessibility tree is usually missing, in the order worth checking.
const A11Y_REMEDY: &str = "the toolkit may not have been told accessibility is on — \
     check `artist computer doctor`, and note that GTK needs GTK_A11Y=atspi, Qt needs \
     QT_ACCESSIBILITY=1, and Electron needs --force-renderer-accessibility";

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

    #[test]
    fn profile_slug_is_stable_and_filesystem_safe() {
        // The same source must always map to the same working profile, and
        // hostile paths must produce something a directory name can hold.
        let first = profile_slug(std::path::Path::new("/home/me/.config/chromium"));
        let second = profile_slug(std::path::Path::new("/home/me/.config/chromium"));
        assert_eq!(first, second);
        assert!(first.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        assert!(!first.is_empty());
    }

    #[test]
    fn the_working_profile_is_seeded_once_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let seed = dir.path().join("real-profile");
        let live = dir.path().join("live-profile");
        let working = dir.path().join("working-profile");

        // A real profile with some state in it.
        std::fs::create_dir_all(seed.join("Default")).unwrap();
        std::fs::write(seed.join("Default/Cookies"), b"session one").unwrap();

        // First sight: the live profile is seeded from it, and a baseline
        // working copy is established.
        let returned = seed_profile(&seed, &live, Some(working.clone())).unwrap();
        assert_eq!(returned, Some(working.clone()));
        assert!(live.join("Default/Cookies").exists());

        // The session now logs in somewhere new — state lands in the live
        // profile, not in the seed (the user's real profile is untouched).
        std::fs::write(live.join("Default/Cookies"), b"session two, now logged in").unwrap();

        // On exit the watcher writes the live profile back over the working one.
        std::fs::remove_dir_all(&working).unwrap();
        clone_profile(&live, &working).unwrap();

        // A returning sight: the *working* profile is the seed, so what the
        // last session gained carries over instead of being re-cloned away.
        let live2 = dir.path().join("live-profile-2");
        seed_profile(&seed, &live2, Some(working.clone())).unwrap();
        assert_eq!(
            std::fs::read_to_string(live2.join("Default/Cookies")).unwrap(),
            "session two, now logged in"
        );
        // And the user's real profile was never overwritten.
        assert_eq!(
            std::fs::read_to_string(seed.join("Default/Cookies")).unwrap(),
            "session one"
        );
    }

    #[test]
    fn no_working_profile_means_plain_clone_without_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let seed = dir.path().join("real-profile");
        std::fs::create_dir_all(seed.join("Default")).unwrap();
        std::fs::write(seed.join("Default/Cookies"), b"session one").unwrap();

        let live = dir.path().join("live-profile");
        let returned = seed_profile(&seed, &live, None).unwrap();
        assert_eq!(returned, None);
        assert!(live.join("Default/Cookies").exists());
    }
}
