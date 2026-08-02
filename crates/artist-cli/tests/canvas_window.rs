//! The window lifecycle, against the binary that actually ships.
//!
//! Everything else about windows is unit-tested with a stub child, because
//! `Windows::open` re-execs `current_exe` and under a test harness that is the
//! harness. So the parts that only exist once a real `artist` is on the other
//! end went unverified: whether the webview comes up at all, whether it renders
//! the canvas it was pointed at, and whether the child records where it was
//! when the session ends. Those are exactly the parts that were broken.
//!
//! This test lives in `artist-cli` rather than `artist-canvas` because
//! `CARGO_BIN_EXE_artist` is only set for the package that defines the binary.

use std::{path::PathBuf, time::Duration};

use artist_canvas::{
    server::Server,
    window::{self, Opened, Windows},
};

/// The real `artist`. The point of the whole test: the child half of every
/// assertion below runs in a different program from the one asserting.
const ARTIST: &str = env!("CARGO_BIN_EXE_artist");

const ENTRY: &str = r#"
import { createRoot } from "react-dom/client";
function App() { return <h1>window probe</h1>; }
createRoot(document.getElementById("root")).render(<App />);
"#;

/// A geometry no window could have: the size is under the floor `sane()`
/// enforces and the position is off every monitor, so both are discarded on
/// the way in. Anything that comes back out was therefore measured from the
/// live window rather than copied from this file.
const IMPOSSIBLE: &str = r#"{"width":100.0,"height":100.0,"x":-99000.0,"y":140.0}"#;

/// Wait for the page to say it mounted.
///
/// A webview takes seconds, and how many depends on the machine — so this polls
/// to a generous ceiling rather than sleeping a fixed guess, and the ceiling is
/// only ever paid by a genuine failure.
async fn mounted_page(server: &Server, slug: &str) -> Option<serde_json::Value> {
    for _ in 0..60 {
        if let Some(digest) = server.request_digest(slug).await
            && digest.get("mounted").and_then(|m| m.as_bool()) == Some(true)
        {
            return Some(digest);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    None
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_opens_renders_and_records_where_it_was() {
    if !window::available() {
        // Not a silent pass: a headless run should say why it proved nothing.
        eprintln!("skipping: no display to open a window on");
        return;
    }

    let project = tempfile::tempdir().expect("tempdir");
    let root = project.path().to_path_buf();
    let canvas = root.join(".artist/canvas/probe");
    std::fs::create_dir_all(&canvas).expect("canvas dir");
    std::fs::write(canvas.join("canvas.toml"), "title = \"Window probe\"").expect("manifest");
    std::fs::write(canvas.join("main.jsx"), ENTRY).expect("entry");

    let geometry = window::geometry_path(&root, "probe");
    std::fs::write(&geometry, IMPOSSIBLE).expect("seed geometry");

    let server = Server::start(root.clone()).await.expect("server starts");
    let url = server.url("probe");
    let windows = Windows::with_executable(PathBuf::from(ARTIST));

    assert_eq!(
        windows
            .open(&root, "probe", &url, "Window probe")
            .expect("a window should open"),
        Opened::Spawned
    );

    // Nothing before this proves a webview exists. `open` returning `Spawned`
    // only means a process outlived the startup grace; this is the page itself
    // reporting what it put on screen.
    let digest = mounted_page(&server, "probe")
        .await
        .expect("the window never reported a mounted page");
    let text = digest
        .get("text")
        .map(|text| text.to_string())
        .unwrap_or_default();
    assert!(
        text.contains("window probe"),
        "the window came up showing something else: {text}"
    );

    // "Open it" never meant "open another one".
    assert_eq!(
        windows
            .open(&root, "probe", &url, "Window probe")
            .expect("a second open should be answered"),
        Opened::Already
    );
    assert_eq!(windows.showing(), ["probe"]);

    // The exit that used to forget. Clearing the seed first means anything
    // present afterwards was written on the way out rather than left over.
    std::fs::remove_file(&geometry).expect("clear the seed");
    windows.close_all();

    let saved = std::fs::read_to_string(&geometry)
        .expect("closing the session should have recorded where the window was");
    let saved: serde_json::Value = serde_json::from_str(&saved).expect("valid geometry json");

    // Deliberately loose: a window manager may impose its own size, and this
    // is testing provenance, not layout. The seed said 100x100 at x=-99000, so
    // a size at or above the floor and any position at all can only have come
    // from the window itself.
    assert!(
        saved["width"].as_f64().is_some_and(|width| width >= 420.0),
        "width was not measured from the window: {saved}"
    );
    assert!(
        saved["height"]
            .as_f64()
            .is_some_and(|height| height >= 320.0),
        "height was not measured from the window: {saved}"
    );
    assert!(
        saved["x"].as_f64().is_some_and(|x| x > -20_000.0),
        "the discarded off-monitor position came back: {saved}"
    );
}
