//! The canvas window process.
//!
//! Runs in a child artist spawned via `artist_canvas::window::open`, so the
//! platform event loop gets the main thread the TUI is already using in the
//! parent. See that module for why re-exec rather than a thread.
//!
//! Everything here is behind the `webview` feature. WebKitGTK on Linux and
//! WebView2 on Windows are system libraries, not crates, and a user without
//! them should still be able to build artist — they just get the URL printed
//! instead of a window.

#[cfg(any(feature = "webview", test))]
use std::path::PathBuf;

/// A window's remembered size and position.
///
/// A canvas is reopened across a session and across days, and a window that
/// forgets is a window the user re-drags every time.
#[cfg(any(feature = "webview", test))]
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq)]
struct Geometry {
    width: f64,
    height: f64,
    x: Option<f64>,
    y: Option<f64>,
}

#[cfg(any(feature = "webview", test))]
impl Default for Geometry {
    fn default() -> Self {
        // Large enough for a table beside a chart, which is what a canvas is
        // usually for. A dialog-sized default trains everyone to resize first.
        Geometry {
            width: 1100.0,
            height: 780.0,
            x: None,
            y: None,
        }
    }
}

#[cfg(any(feature = "webview", test))]
impl Geometry {
    fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .map(Geometry::sane)
            .unwrap_or_default()
    }

    /// A saved geometry from a monitor that is no longer attached would put the
    /// window somewhere the user cannot reach it, so it is bounded on the way
    /// back in rather than trusted.
    fn sane(self) -> Self {
        Geometry {
            width: self.width.clamp(420.0, 8000.0),
            height: self.height.clamp(320.0, 8000.0),
            x: self.x.filter(|x| x.abs() < 20_000.0),
            y: self.y.filter(|y| y.abs() < 20_000.0),
        }
    }

    fn save(&self, path: &std::path::Path) {
        // Best effort: a window that cannot record where it was is a smaller
        // problem than a window that refuses to close.
        if let Ok(text) = serde_json::to_string(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

/// Record where the window is, so reopening this canvas puts it back.
///
/// Called from every path that ends the event loop rather than only from the
/// titlebar X. It used to hang off `CloseRequested` alone, which meant the
/// common exit — artist closing its own windows as the session ends — always
/// forgot, and the user's sizing survived only if they happened to close the
/// window by hand first.
#[cfg(feature = "webview")]
fn remember(window: &tao::window::Window, path: Option<&std::path::Path>) {
    let Some(path) = path else {
        return;
    };
    let scale = window.scale_factor();
    let size = window.inner_size().to_logical::<f64>(scale);
    let position = window
        .outer_position()
        .ok()
        .map(|point| point.to_logical::<f64>(scale));
    Geometry {
        width: size.width,
        height: size.height,
        x: position.map(|point| point.x),
        y: position.map(|point| point.y),
    }
    .save(path);
}

/// Stop when the parent closes the window's stdin.
///
/// That is artist asking for the window back — `canvas close`, or the session
/// ending — and it arrives as an EOF rather than a signal for a reason the
/// parent's `stop` explains. It also fires if artist dies without asking,
/// because the kernel closes the write end with it; that overlaps with
/// [`watch_parent`] and beats it by several seconds.
#[cfg(feature = "webview")]
fn stop_on_hangup(stop: impl Fn() + Send + 'static) {
    use std::io::Read;

    // Only a pipe can hang up. Run this subcommand any other way — by hand, or
    // from a shell that hands a background job `/dev/null` — and the first read
    // returns EOF at once, which would be indistinguishable from artist asking
    // for the window back. The window then vanished the instant it appeared.
    if !stdin_is_a_pipe() {
        return;
    }

    std::thread::spawn(move || {
        // Nothing is ever written, so this blocks until the pipe closes. A
        // read that somehow returns data is not a hangup and must not be
        // treated as one.
        let mut byte = [0u8; 1];
        while let Ok(1) = std::io::stdin().read(&mut byte) {}
        stop();
    });
}

/// Is stdin something that can deliver a hangup?
///
/// `Windows::open` always gives the child a pipe, so in the shipping path this
/// is true. It exists for every other path: a terminal, a redirect from a file,
/// or the `/dev/null` a shell hands a background job all read EOF immediately
/// or never, and neither means what the hangup watcher would take it to mean.
#[cfg(all(unix, feature = "webview"))]
fn stdin_is_a_pipe() -> bool {
    use std::os::{fd::AsRawFd, unix::fs::FileTypeExt};

    // SAFETY: fd 0 is valid for the life of the process, and `ManuallyDrop`
    // keeps the borrowed handle from closing it — this only ever stats it.
    let borrowed = std::mem::ManuallyDrop::new(unsafe {
        <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(std::io::stdin().as_raw_fd())
    });
    borrowed
        .metadata()
        .map(|metadata| metadata.file_type().is_fifo())
        .unwrap_or(false)
}

/// Windows has no `/dev/null`-shaped equivalent here, and the parent always
/// supplies a pipe, so there is nothing to distinguish.
#[cfg(all(not(unix), feature = "webview"))]
fn stdin_is_a_pipe() -> bool {
    true
}

/// Watch the parent's server, and stop when it stops.
///
/// The child is a separate process, so nothing about it dies automatically when
/// artist does — and a webview left showing a canvas whose server is gone is
/// worse than no window: every interaction fails silently. Polling the socket
/// is the portable version of "exit with my parent", and it also covers the
/// parent crashing rather than exiting cleanly.
#[cfg(feature = "webview")]
fn watch_parent(url: &str, stop: impl Fn() + Send + 'static) {
    use std::net::{TcpStream, ToSocketAddrs};

    let Some(authority) = url
        .split_once("://")
        .and_then(|(_, rest)| rest.split('/').next())
        .map(str::to_owned)
    else {
        return;
    };

    std::thread::spawn(move || {
        let mut misses = 0;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let reachable = authority
                .to_socket_addrs()
                .ok()
                .and_then(|mut addrs| addrs.next())
                .is_some_and(|addr| {
                    TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).is_ok()
                });
            // Three misses rather than one: a busy server can refuse a
            // connection for a moment, and closing the user's window over a
            // transient blip would be its own bug.
            misses = if reachable { 0 } else { misses + 1 };
            if misses >= 3 {
                stop();
                return;
            }
        }
    });
}

/// Show `url` in a window until the user closes it.
///
/// Only returns when the window is gone, so the child process exits with it.
#[cfg(feature = "webview")]
pub fn run(url: &str, title: &str) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use tao::{
        dpi::{LogicalPosition, LogicalSize},
        event::{Event, WindowEvent},
        event_loop::{ControlFlow, EventLoopBuilder},
        window::WindowBuilder,
    };
    use wry::WebViewBuilder;

    let remembered = std::env::var_os(crate::window::GEOMETRY_VAR).map(PathBuf::from);
    let geometry = remembered
        .as_deref()
        .map(Geometry::load)
        .unwrap_or_default();

    let event_loop = EventLoopBuilder::<()>::with_user_event().build();
    let mut builder = WindowBuilder::new()
        .with_title(title)
        .with_inner_size(LogicalSize::new(geometry.width, geometry.height))
        // A canvas is a workbench, not a dialog: it should be resizable and
        // start large enough for a table beside a chart.
        .with_min_inner_size(LogicalSize::new(420.0, 320.0));
    if let (Some(x), Some(y)) = (geometry.x, geometry.y) {
        builder = builder.with_position(LogicalPosition::new(x, y));
    }
    let window = builder.build(&event_loop)?;

    let proxy = event_loop.create_proxy();
    watch_parent(url, move || {
        let _ = proxy.send_event(());
    });
    let proxy = event_loop.create_proxy();
    stop_on_hangup(move || {
        let _ = proxy.send_event(());
    });

    let builder = WebViewBuilder::new()
        .with_url(url)
        // The page is served from loopback by the parent process. Devtools stay
        // on because a canvas is developed, not shipped — and the model reads
        // the console through the bridge either way.
        .with_devtools(true);

    // GTK has no raw window handle wry can attach to, so on Linux the webview
    // is built into the window's own vbox. Building from the handle compiles
    // fine and fails at runtime with `UnsupportedWindowHandle`.
    #[cfg(target_os = "linux")]
    let _webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        builder.build_gtk(
            window
                .default_vbox()
                .context("the canvas window has no GTK container")?,
        )?
    };
    #[cfg(not(target_os = "linux"))]
    let _webview = builder.build(&window)?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            // Three ways to the same end, and all three have to remember: the
            // user closing the window, artist asking for it back, and artist
            // vanishing out from under it. Only the first of those is the one
            // a person thinks of as "closing the window", and it is the rarest.
            Event::UserEvent(())
            | Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                remember(&window, remembered.as_deref());
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

#[cfg(not(feature = "webview"))]
pub fn run(_url: &str, _title: &str) -> anyhow::Result<()> {
    anyhow::bail!(
        "this build of artist has no webview support (built without the `webview` feature)"
    )
}

// There was a `supported()` here that answered "feature && display". Nothing
// could use it: the decision it informs is made in artist-agent, which cannot
// see a feature belonging to artist-cli. `Windows::open` now settles the
// question by spawning and checking the child survived, which also catches the
// case a feature flag never could — the feature compiled in, WebKitGTK missing
// from the box.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_round_trips_through_the_file() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let path = temporary.path().join(".window.json");

        let saved = Geometry {
            width: 1440.0,
            height: 900.0,
            x: Some(120.0),
            y: Some(64.0),
        };
        saved.save(&path);
        assert_eq!(Geometry::load(&path), saved);

        // Nothing written yet, or written by an older artist, falls back rather
        // than opening a zero-sized window.
        assert_eq!(
            Geometry::load(&temporary.path().join("absent.json")),
            Geometry::default()
        );
        std::fs::write(&path, "not json").expect("write");
        assert_eq!(Geometry::load(&path), Geometry::default());
    }

    /// A geometry saved on a monitor that is no longer attached would put the
    /// window somewhere with no way to drag it back.
    #[test]
    fn an_unreachable_geometry_is_bounded_on_the_way_back_in() {
        let offscreen = Geometry {
            width: 4.0,
            height: 2.0,
            x: Some(-99_000.0),
            y: Some(50.0),
        };
        let sane = offscreen.sane();
        assert!(sane.width >= 420.0 && sane.height >= 320.0, "{sane:?}");
        assert_eq!(sane.x, None, "an off-monitor position should be forgotten");
        assert_eq!(sane.y, Some(50.0), "a reachable coordinate should survive");
    }
}
