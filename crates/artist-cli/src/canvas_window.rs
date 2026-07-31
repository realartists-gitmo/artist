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

use std::path::PathBuf;

/// A window's remembered size and position.
///
/// A canvas is reopened across a session and across days, and a window that
/// forgets is a window the user re-drags every time.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq)]
struct Geometry {
    width: f64,
    height: f64,
    x: Option<f64>,
    y: Option<f64>,
}

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

    let remembered = std::env::var_os(artist_canvas::window::GEOMETRY_VAR).map(PathBuf::from);
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
            // The parent's server stopped answering, so artist is gone and this
            // window can no longer do anything.
            Event::UserEvent(()) => *control_flow = ControlFlow::Exit,
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                if let Some(path) = remembered.as_deref() {
                    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                    let position = window
                        .outer_position()
                        .ok()
                        .map(|point| point.to_logical::<f64>(window.scale_factor()));
                    Geometry {
                        width: size.width,
                        height: size.height,
                        x: position.map(|point| point.x),
                        y: position.map(|point| point.y),
                    }
                    .save(path);
                }
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
