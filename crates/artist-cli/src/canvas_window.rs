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

/// Show `url` in a window until the user closes it.
///
/// Only returns when the window is gone, so the child process exits with it.
#[cfg(feature = "webview")]
pub fn run(url: &str, title: &str) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use tao::{
        dpi::LogicalSize,
        event::{Event, WindowEvent},
        event_loop::{ControlFlow, EventLoopBuilder},
        window::WindowBuilder,
    };
    use wry::WebViewBuilder;

    let event_loop = EventLoopBuilder::new().build();
    let window = WindowBuilder::new()
        .with_title(title)
        .with_inner_size(LogicalSize::new(1100.0, 780.0))
        // A canvas is a workbench, not a dialog: it should be resizable and
        // start large enough for a table beside a chart.
        .with_min_inner_size(LogicalSize::new(420.0, 320.0))
        .build(&event_loop)?;

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
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
}

#[cfg(not(feature = "webview"))]
pub fn run(_url: &str, _title: &str) -> anyhow::Result<()> {
    anyhow::bail!("this build of artist has no webview support (built without the `webview` feature)")
}

/// Is a window actually reachable from here?
///
/// Both halves have to hold: the binary must carry a webview, and the session
/// must have a display to put it on.
pub fn supported() -> bool {
    cfg!(feature = "webview") && artist_canvas::window::available()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build without the feature must never claim a window is available,
    /// whatever the display environment says.
    #[test]
    fn support_requires_both_the_feature_and_a_display() {
        if !cfg!(feature = "webview") {
            assert!(!supported());
        } else {
            assert_eq!(supported(), artist_canvas::window::available());
        }
    }
}
