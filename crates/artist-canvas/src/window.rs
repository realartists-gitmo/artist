//! Opening a canvas in a window artist owns.
//!
//! # Why this re-execs instead of opening a window in-process
//!
//! A webview needs a platform event loop, and on macOS that loop must own the
//! main thread. Artist's main thread is the TUI: a blocking crossterm poll and
//! ratatui's inline viewport. Those two cannot share a thread, and running the
//! window loop off-thread is allowed on Linux and Windows but not on macOS —
//! so it would work in development here and fail for half the users.
//!
//! Re-exec sidesteps the conflict entirely. Artist is one binary, so it can
//! launch itself with a hidden subcommand and let the child give the event loop
//! the main thread it wants. The window then dies with its process, the TUI
//! never yields its own thread, and the platform differences stay in one place.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

/// The hidden subcommand the child runs. Not in `--help`: it is an
/// implementation detail of `canvas open`, not something to invoke by hand.
pub const WINDOW_SUBCOMMAND: &str = "__canvas-window";

/// How the child receives the URL, which contains the session key.
pub const URL_VAR: &str = "ARTIST_CANVAS_URL";

/// Where the child reads and writes its size and position.
pub const GEOMETRY_VAR: &str = "ARTIST_CANVAS_GEOMETRY";

/// Where a canvas's window geometry is remembered.
///
/// Beside the canvas rather than in a global config: geometry is a property of
/// this canvas in this project — a wide table wants a wide window, and it wants
/// it again tomorrow.
pub fn geometry_path(project: &Path, slug: &str) -> PathBuf {
    project
        .join(".artist/canvas")
        .join(slug)
        .join(".window.json")
}

/// What `Windows::open` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Opened {
    /// A new window went up.
    Spawned,
    /// One was already showing this canvas, so nothing was spawned.
    Already,
}

/// The windows this session put on screen.
///
/// Exists for two reasons, both of which were bugs. Without it `open` dropped
/// the child handle, so windows outlived the session that promised to close
/// them — artist quit and left a webview showing a canvas whose server was
/// gone. And calling `open` twice put up two windows for the same canvas, which
/// is never what anyone meant by "open it".
#[derive(Default)]
pub struct Windows {
    live: Mutex<HashMap<String, Child>>,
}

impl Windows {
    /// Show `slug`, unless it is already showing.
    pub fn open(&self, project: &Path, slug: &str, url: &str, title: &str) -> io::Result<Opened> {
        let mut live = self.live.lock().expect("window registry poisoned");

        if let Some(child) = live.get_mut(slug) {
            // `try_wait` is the only honest test: the user may have closed the
            // window, and a stale entry would then refuse to reopen it.
            match child.try_wait() {
                Ok(None) => return Ok(Opened::Already),
                _ => {
                    live.remove(slug);
                }
            }
        }

        let mut child = command(
            std::env::current_exe()?,
            url,
            title,
            &geometry_path(project, slug),
        )
        .spawn()?;

        // Spawning proves only that the binary exists. The child re-execs
        // artist and dispatches on argv, so it can still die at once: a build
        // without the `webview` feature bails on purpose, and a box without
        // WebKitGTK fails to link before it reaches main. Reporting `Spawned`
        // in either case told the user a window was up while they watched
        // nothing happen — and with stderr discarded, silently.
        //
        // `available()` cannot cover this. It reads the environment, and the
        // half that goes wrong here is compiled into the binary or installed
        // on the system. Outliving the grace period is the only honest test.
        if let Some(reason) = failed_to_start(&mut child) {
            // A child that exited was reaped by the `try_wait` that saw it. One
            // we failed to *check on* was not, and dropping the handle here
            // would leave it defunct for the life of the session — the same
            // leak `close_all` reaps for. Both calls are harmless on a child
            // already collected.
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(format!("the window {reason}")));
        }

        live.insert(slug.to_owned(), child);
        Ok(Opened::Spawned)
    }

    /// Close one window, if it is ours and still up.
    pub fn close(&self, slug: &str) -> bool {
        let mut live = self.live.lock().expect("window registry poisoned");
        match live.remove(slug) {
            Some(mut child) => {
                stop(&mut child);
                true
            }
            None => false,
        }
    }

    /// Close everything. Called when the session ends.
    pub fn close_all(&self) {
        let mut live = self.live.lock().expect("window registry poisoned");
        for (_, mut child) in live.drain() {
            stop(&mut child);
        }
    }

    /// Which canvases are on screen right now.
    pub fn showing(&self) -> Vec<String> {
        let mut live = self.live.lock().expect("window registry poisoned");
        live.retain(|_, child| matches!(child.try_wait(), Ok(None)));
        let mut slugs: Vec<_> = live.keys().cloned().collect();
        slugs.sort();
        slugs
    }
}

/// Windows do not survive the session that opened them. The tool's own message
/// says so, and before this that was simply untrue.
impl Drop for Windows {
    fn drop(&mut self) {
        self.close_all();
    }
}

/// How long a window gets to close itself before it is killed.
///
/// Paid once per open window when the session ends, so it cannot be generous —
/// but a window that answers does so in single-digit milliseconds, and the
/// grace is a deadline rather than a delay: the loop below returns the moment
/// the child is gone.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

/// How often to check, so a window that closes at once is not waited on.
const SHUTDOWN_POLL: Duration = Duration::from_millis(5);

/// Ask a window to close; kill it if it will not.
///
/// This used to be a bare `kill`, which is unanswerable — and the child has
/// something to say on the way out: it records the window's size and position
/// so reopening the canvas does not mean re-dragging it. SIGKILL meant that
/// only ever happened when the *user* clicked the titlebar X, so the exit path
/// artist itself takes at the end of every session was the one that forgot.
///
/// Closing the child's stdin is the request. The kill stays as the backstop
/// for a window that is wedged, or old enough not to be listening.
fn stop(child: &mut Child) {
    // The child is reading this and nothing else ever writes to it, so
    // dropping the handle is an EOF the child cannot miss.
    drop(child.stdin.take());

    let deadline = Instant::now() + SHUTDOWN_GRACE;
    loop {
        match child.try_wait() {
            // Gone, and reaped by the `try_wait` that saw it.
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() >= deadline => break,
            Ok(None) => std::thread::sleep(SHUTDOWN_POLL),
            Err(_) => break,
        }
    }

    let _ = child.kill();
    // Reaped rather than left as a zombie: artist may be a long-lived process,
    // and a session that opens canvases repeatedly would otherwise accumulate
    // defunct children for its whole life.
    let _ = child.wait();
}

/// The command `Windows::open` will spawn.
///
/// Separated so a test can inspect it without launching a window.
fn command(executable: PathBuf, url: &str, title: &str, geometry: &Path) -> Command {
    let mut command = Command::new(executable);
    // The URL carries the session key, and an argument is world-readable in
    // `ps` and `/proc/<pid>/cmdline`. Another process's environment is not, so
    // the child reads it from there instead.
    command
        .arg(WINDOW_SUBCOMMAND)
        .arg(title)
        .env(URL_VAR, url)
        .env(GEOMETRY_VAR, geometry)
        // The child must not write to the terminal the TUI is drawing in.
        // A stray line from a GTK warning would corrupt the viewport. stderr
        // is piped rather than discarded for the same reason it used to be
        // discarded — it never reaches the terminal either way — but a pipe
        // can be read back, which is what turns "it did not open" into a
        // reason. `failed_to_start` owns that pipe from here.
        //
        // stdin is a pipe nothing is ever written to. Closing it is how
        // [`stop`] asks the window to go, and because the kernel closes it
        // too when this process dies, the child also learns about a crash
        // immediately rather than inferring it from a dead socket six seconds
        // later. A signal would have been the obvious alternative and is
        // worse: artist runs on a multi-threaded tokio runtime that is
        // already up before the window subcommand is dispatched, so SIGTERM
        // would be delivered to whichever thread has not blocked it and take
        // the process down before the window could record anything.
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

/// How long a doomed child gets to prove it is doomed.
///
/// Nothing that is going to succeed finishes inside this — a webview takes far
/// longer than a quarter second to put pixels up — and nothing that is going to
/// fail takes anywhere near it: a bail or a link error lands in single-digit
/// milliseconds. So the success path pays this in full and the failure path
/// almost never does, which is the right way round for a window a user just
/// asked for.
const STARTUP_GRACE: Duration = Duration::from_millis(250);

/// How often to look, so a failure is reported as soon as it happens rather
/// than at the end of the grace period.
const STARTUP_POLL: Duration = Duration::from_millis(10);

/// Wait briefly for a child to prove it can run; describe the failure if it
/// cannot.
///
/// `None` means the child outlived the grace period and a window is genuinely
/// on its way.
fn failed_to_start(child: &mut Child) -> Option<String> {
    let deadline = Instant::now() + STARTUP_GRACE;
    loop {
        match child.try_wait() {
            // Still running once the grace is up: as good as this gets without
            // waiting on the window itself, which can take seconds.
            Ok(None) if Instant::now() >= deadline => {
                discard_stderr(child);
                return None;
            }
            Ok(None) => std::thread::sleep(STARTUP_POLL),
            Ok(Some(status)) => {
                let reason = read_stderr(child)
                    .filter(|text| !text.is_empty())
                    // The child's own message is the useful one — "no webview
                    // support", the dynamic linker naming the library it could
                    // not find. The status is the fallback for a child that
                    // died without saying anything.
                    .unwrap_or_else(|| format!("exited with {status}"));
                return Some(format!("closed immediately: {reason}"));
            }
            // A child that cannot be waited on is not one to report as open.
            Err(error) => return Some(format!("could not be checked on: {error}")),
        }
    }
}

/// The child's last words, capped: this goes into a message for the model, and
/// a webview that failed noisily can produce a great deal of GTK output.
fn read_stderr(child: &mut Child) -> Option<String> {
    use std::io::Read;

    let mut stderr = child.stderr.take()?;
    let mut buffer = Vec::new();
    stderr.by_ref().take(2048).read_to_end(&mut buffer).ok()?;
    Some(String::from_utf8_lossy(&buffer).trim().replace('\n', "; "))
}

/// Drain a surviving child's stderr into nowhere.
///
/// The pipe still has to be read. A webview logs GTK warnings for as long as it
/// is up, and an undrained pipe fills its buffer and then blocks the child
/// mid-write — a window that freezes an hour in, for the sake of output nobody
/// wants. Reading and discarding costs one thread per window, and there is one
/// window per canvas.
fn discard_stderr(child: &mut Child) {
    let Some(mut stderr) = child.stderr.take() else {
        return;
    };
    std::thread::spawn(move || {
        let _ = io::copy(&mut stderr, &mut io::sink());
    });
}

/// Whether a windowing system is available at all.
///
/// Over SSH or on a headless box there is no display, and spawning would give
/// the user a silently dead child. Callers fall back to printing the URL.
pub fn available() -> bool {
    if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
        return true;
    }
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The child is dispatched by matching this exact string before clap runs,
    /// so it must stay stable and stay unlikely to collide with a real verb.
    #[test]
    fn the_subcommand_is_stable_and_clearly_internal() {
        assert_eq!(WINDOW_SUBCOMMAND, "__canvas-window");
        assert!(WINDOW_SUBCOMMAND.starts_with("__"));
    }

    /// The URL contains the session key, so it must not become an argument:
    /// `/proc/<pid>/cmdline` is world-readable and `ps` shows the whole thing.
    #[test]
    fn the_url_travels_out_of_band() {
        let url = "http://127.0.0.1:4242/c/supersecretkey/demo/";
        let spawned = command(
            PathBuf::from("/usr/bin/artist"),
            url,
            "Demo",
            Path::new("/p/.artist/canvas/demo/.window.json"),
        );

        let args: Vec<_> = spawned
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args.iter().any(|arg| arg.contains("supersecretkey")),
            "the key reached the command line: {args:?}"
        );
        assert_eq!(args, [WINDOW_SUBCOMMAND, "Demo"]);

        let passed = spawned
            .get_envs()
            .find(|(name, _)| *name == std::ffi::OsStr::new(URL_VAR))
            .and_then(|(_, value)| value)
            .expect("the child needs the url");
        assert_eq!(passed.to_string_lossy(), url);
    }

    /// A child that dies on the spot is not an open window, and saying it was
    /// is the failure this whole check exists for: the user is told a window
    /// went up, sees nothing, and has no URL to fall back to.
    #[test]
    fn a_child_that_dies_at_once_is_reported_as_a_failure() {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("echo 'no webview support' >&2; exit 1")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn");

        let reason = failed_to_start(&mut child).expect("an immediate exit must be a failure");
        // The child's own explanation has to survive: "could not open a window"
        // alone leaves the user with nothing to act on.
        assert!(reason.contains("no webview support"), "{reason}");
    }

    /// The opposite error is just as bad — refusing to report a window that is
    /// coming up fine, because a webview is slow to appear.
    #[test]
    fn a_child_that_keeps_running_is_reported_as_started() {
        let mut child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn");

        assert_eq!(failed_to_start(&mut child), None);

        let _ = child.kill();
        let _ = child.wait();
    }

    /// The window records where it was on the way out, so closing it has to be
    /// a request it can answer rather than a kill it cannot.
    #[test]
    fn a_window_is_asked_to_close_before_it_is_killed() {
        // `cat` stands in for the window: it reads stdin and ends at EOF,
        // which is exactly what the child's hangup watcher does.
        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");

        let started = Instant::now();
        stop(&mut child);
        assert!(
            started.elapsed() < SHUTDOWN_GRACE,
            "a window that closes itself should not be waited on for the full grace"
        );
    }

    /// The kill has to stay. A wedged window, or one from a build that predates
    /// the hangup watcher, would otherwise keep the session from ending.
    #[test]
    fn a_window_that_will_not_close_is_still_killed() {
        // `sleep` never reads stdin, so closing the pipe means nothing to it.
        let mut child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");

        let started = Instant::now();
        stop(&mut child);
        assert!(
            started.elapsed() >= SHUTDOWN_GRACE,
            "the grace should have been paid in full before killing"
        );
        // Reaped, not merely signalled: `stop` is what stands between a
        // long-lived artist and a pile of defunct children.
        assert!(
            matches!(child.try_wait(), Err(_) | Ok(Some(_))),
            "the killed window was left unreaped"
        );
    }

    /// Geometry belongs to the canvas, not to the machine: a canvas showing a
    /// wide table wants a wide window again tomorrow.
    #[test]
    fn geometry_is_remembered_per_canvas() {
        let one = geometry_path(Path::new("/p"), "wide-table");
        let two = geometry_path(Path::new("/p"), "narrow-form");
        assert_ne!(one, two);
        assert!(one.starts_with("/p/.artist/canvas/wide-table"), "{one:?}");
        // A dotfile inside the canvas directory, so it does not read as
        // something the model should be editing.
        assert!(
            one.file_name().unwrap().to_string_lossy().starts_with('.'),
            "{one:?}"
        );
    }

    /// Opening a canvas that is already on screen must not put a second window
    /// up. "Open it" never meant "open another one".
    #[test]
    fn a_second_open_does_not_spawn_a_second_window() {
        let windows = Windows::default();
        let temporary = tempfile::tempdir().expect("tempdir");

        // `true` stands in for the window child: it is spawnable everywhere and
        // this is testing the registry, not the webview.
        let mut live = windows.live.lock().expect("lock");
        live.insert(
            "demo".to_owned(),
            Command::new("sleep")
                .arg("30")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn"),
        );
        drop(live);

        assert_eq!(
            windows
                .open(
                    temporary.path(),
                    "demo",
                    "http://127.0.0.1/c/k/demo/",
                    "Demo"
                )
                .expect("open"),
            Opened::Already
        );
        assert_eq!(windows.showing(), ["demo"]);

        assert!(windows.close("demo"), "close should report it closed one");
        assert!(windows.showing().is_empty());
        assert!(!windows.close("demo"), "closing twice is not closing two");
    }

    /// The tool tells the user the window closes when the session ends. Before
    /// the registry that was simply false — the child handle was dropped.
    #[test]
    fn dropping_the_registry_closes_the_windows() {
        let child = {
            let windows = Windows::default();
            let mut live = windows.live.lock().expect("lock");
            let child = Command::new("sleep")
                .arg("30")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn");
            let pid = child.id();
            live.insert("demo".to_owned(), child);
            drop(live);
            pid
        };

        // The process is gone, so signalling it finds nothing. `kill -0` is the
        // portable existence check and does not itself terminate anything.
        let alive = Command::new("kill")
            .arg("-0")
            .arg(child.to_string())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        assert!(!alive, "the window outlived the registry that owns it");
    }

    /// A headless session must fall back to printing a URL rather than
    /// spawning a child that dies without saying why.
    #[test]
    fn availability_follows_the_display_environment() {
        // SAFETY: single-threaded test, restoring both variables before it ends.
        let wayland = std::env::var_os("WAYLAND_DISPLAY");
        let x11 = std::env::var_os("DISPLAY");

        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
            std::env::remove_var("DISPLAY");
        }
        if cfg!(target_os = "linux") {
            assert!(!available(), "no display should mean no window");
            unsafe { std::env::set_var("DISPLAY", ":0") };
            assert!(available(), "X11 display should be enough");
        }

        unsafe {
            match wayland {
                Some(value) => std::env::set_var("WAYLAND_DISPLAY", value),
                None => std::env::remove_var("WAYLAND_DISPLAY"),
            }
            match x11 {
                Some(value) => std::env::set_var("DISPLAY", value),
                None => std::env::remove_var("DISPLAY"),
            }
        }
    }
}
