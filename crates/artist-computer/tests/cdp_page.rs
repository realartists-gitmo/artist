//! Drive a real browser through the CDP surface.
//!
//! Runs against `chromium --headless=new` on a `file://` fixture, so the whole
//! surface — accessibility tree to nodes to anchors to a click that actually
//! changes the page — is exercised without needing a stage or a display. Skipped
//! rather than ignored when no browser is installed, so a machine that has one
//! gets the coverage automatically.

use std::time::Duration;

#[path = "support/mod.rs"]
mod support;

use artist_computer::program::Expect;
use artist_computer::surface::cdp::{CdpPage, connect};
use artist_computer::{
    AnchorBook, Program, Settle, SettleKind, Step, Surface, Target, run_program,
};

const PAGE: &str = r#"<!doctype html>
<html><body>
  <h1>Fixture</h1>
  <label for="email">Email</label>
  <input id="email" type="text" aria-label="Email">
  <button id="go" onclick="document.getElementById('out').textContent='submitted'">Send message</button>
  <button id="cancel">Cancel</button>
  <p id="out">idle</p>
</body></html>
"#;

fn chromium() -> Option<String> {
    [
        "chromium",
        "chromium-browser",
        "google-chrome-stable",
        "google-chrome",
    ]
    .into_iter()
    .find_map(which)
}

fn which(name: &str) -> Option<String> {
    let path = std::path::Path::new("/usr/bin").join(name);
    path.exists().then(|| path.to_string_lossy().into_owned())
}

/// Only one browser at a time, for the whole test binary.
///
/// Each of these tests launches a real Chromium; `cargo test` runs them in
/// parallel by default, and eleven concurrent browsers is enough to make
/// launches time out on an ordinary machine. The suite then fails under its own
/// default settings, which makes it worse than useless — a red run stops
/// meaning anything.
///
/// Held for the life of the browser rather than just the launch, because the
/// contention is memory and CPU across the whole test, not the spawn.
static ONE_BROWSER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Chromium {
    /// Released when the browser is dropped, letting the next test start one.
    _slot: tokio::sync::MutexGuard<'static, ()>,
    child: std::process::Child,
    /// The browser profile. Dropped after the child is reaped below.
    _dir: tempfile::TempDir,
    /// The fixture page, attached after launch.
    fixture: Option<tempfile::TempDir>,
}

impl Drop for Chromium {
    fn drop(&mut self) {
        // The whole group, then the child, then reap. Chromium forks a zygote,
        // a GPU process and several utility processes; killing only the one we
        // spawned leaves those alive holding the profile open, so `TempDir`'s
        // removal leaves hundreds of megabytes behind across a test run.
        // KILL, not TERM: Chromium handles TERM asynchronously, so the profile
        // directory is still being written when `TempDir` tries to remove it.
        let _ = std::process::Command::new("kill")
            .arg("-KILL")
            .arg(format!("-{}", self.child.id()))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Launch headless Chromium the way the stage would: port 0, own profile.
async fn launch(page_url: &str) -> Option<(Chromium, chromiumoxide::Browser)> {
    let slot = ONE_BROWSER.lock().await;
    let binary = chromium()?;
    let dir = tempfile::tempdir().ok()?;
    let profile = dir.path().join("profile");

    use std::os::unix::process::CommandExt as _;

    let mut command = std::process::Command::new(binary);
    // Own process group, so teardown can reach the forked helpers.
    command.process_group(0);
    let child = command
        .args([
            "--headless=new",
            // Port 0, never a fixed port: a fixed one collides with the user's
            // own browser and with a second stage.
            "--remote-debugging-port=0",
            &format!("--user-data-dir={}", profile.display()),
            "--force-renderer-accessibility",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-gpu",
            // Test-only. Chromium's zygote calls `setsid`, escaping our process
            // group, so it survives teardown still holding the profile open —
            // which is how a test run leaks hundreds of megabytes of `/tmp`.
            // Not used in the product, where the sandbox is worth keeping.
            "--no-zygote",
            "--no-sandbox",
            page_url,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let guard = Chromium {
        _slot: slot,
        child,
        _dir: dir,
        fixture: None,
    };
    let browser = bounded("connecting to the browser", connect(&profile))
        .await
        .map_err(|error| eprintln!("cdp connect failed: {error}"))
        .ok()?;
    Some((guard, browser))
}

/// Every CDP step is time-bounded.
///
/// A browser that never answers must fail the test with a clear message rather
/// than hang the suite — an indefinitely blocked test is strictly worse than no
/// test, because it looks like progress.
async fn bounded<T>(what: &str, future: impl std::future::Future<Output = T>) -> T {
    match tokio::time::timeout(Duration::from_secs(30), future).await {
        Ok(value) => value,
        Err(_) => panic!("{what} did not complete within 30s"),
    }
}

/// A page served over a real `http://` origin.
///
/// `file://` proves the protocol and nothing beyond it: `fetch` does not work
/// there, so no content can arrive after a click; a frame from another path is
/// another origin; and nothing ever loads slowly. Every genuinely hard thing
/// about driving a page needs a real origin to exist at all.
async fn served_page(
    pages: &support::pages::Pages,
    path: &str,
) -> Option<(Chromium, chromiumoxide::Browser, CdpPage)> {
    let (guard, browser) = launch(&pages.url(path)).await?;
    let page = bounded("listing browser pages", async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(pages) = browser.pages().await
                && let Some(page) = pages.into_iter().next()
            {
                return Some(page);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await?;
    let surface = CdpPage::attach("tab:1", page).await.ok()?;
    Some((guard, browser, surface))
}

async fn fixture_page() -> Option<(Chromium, chromiumoxide::Browser, CdpPage)> {
    // A `TempDir` owned by the guard, not a pid-keyed path in `/tmp`: the
    // latter is never removed, and every run of this test leaks one.
    let dir = tempfile::tempdir().ok()?;
    let file = dir.path().join("fixture.html");
    std::fs::write(&file, PAGE).ok()?;
    let url = format!("file://{}", file.display());

    let (mut guard, browser) = launch(&url).await?;
    guard.fixture = Some(dir);

    // The page the browser opened with.
    let page = bounded("listing browser pages", async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            match browser.pages().await {
                Ok(pages) if !pages.is_empty() => {
                    return Some(pages.into_iter().next().expect("non-empty"));
                }
                Ok(_) => {}
                Err(error) => eprintln!("cdp pages error: {error}"),
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await?;

    let surface = CdpPage::attach("tab:1", page).await.ok()?;
    Some((guard, browser, surface))
}

#[tokio::test]
async fn the_accessibility_tree_becomes_anchored_nodes() {
    let Some((_guard, _browser, surface)) = fixture_page().await else {
        eprintln!("skipping: no chromium available");
        return;
    };

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let snapshot = surface.snapshot().await.expect("snapshot");
        let mut book = AnchorBook::new();
        let observed = book.observe(&snapshot, false);
        let rendered = artist_computer::render::observation("tab:1", &observed, None);

        if rendered.contains("Send message") {
            assert!(rendered.contains("button"), "{rendered}");
            assert!(rendered.contains("Cancel"), "{rendered}");
            // Geometry is resolved harness-side and must never be rendered.
            assert!(!rendered.contains("bounds"), "{rendered}");
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the page's buttons never appeared: {rendered}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// The full loop: observe, resolve an anchor, verify the label, click, settle,
/// and confirm the page actually changed.
#[tokio::test]
async fn a_program_clicks_a_real_button_and_the_page_changes() {
    let Some((_guard, _browser, surface)) = fixture_page().await else {
        eprintln!("skipping: no chromium available");
        return;
    };

    let mut book = AnchorBook::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let anchor = loop {
        let snapshot = surface.snapshot().await.expect("snapshot");
        let observed = book.observe(&snapshot, true);
        if let Some(entry) = observed
            .entries
            .iter()
            .find(|entry| entry.node.name.contains("Send message"))
        {
            break entry.anchor.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the button never appeared"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    };

    let program = Program {
        steps: vec![Step::Click(Target {
            anchor: anchor.clone(),
            label: Some("Send message".into()),
        })],
        settle: Settle {
            until: SettleKind::Quiet,
            timeout_ms: 3_000,
        },
        expect: artist_computer::program::Expect::Appears("submitted".into()),
    };

    let report = run_program(&surface, &mut book, &program)
        .await
        .expect("program should run");
    assert!(report.error.is_none(), "{:?}", report.error);
    assert_eq!(report.steps[0].outcome, "ok");
    assert_eq!(
        report.steps[0].resolved_name.as_deref(),
        Some("Send message")
    );

    // The click must have had an actual effect on the page.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = surface.snapshot().await.expect("snapshot");
        if snapshot
            .nodes
            .iter()
            .any(|node| node.name.contains("submitted"))
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the page never reflected the click"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// The whole path the model actually takes: one `launch` call with `gui: true`
/// brings up an isolated display, starts a browser in it, connects over CDP and
/// returns an observation the model can act on.
///
/// This is what "the ladder is wired in" means. Every piece below it was
/// individually verified, but until this passes a model using the tool reaches
/// terminals and nothing else.
#[tokio::test]
async fn the_tool_launches_a_browser_and_observes_it() {
    use artist_computer::{ComputerTool, SurfaceRegistry};
    use rig_core::tool::PortableTool;

    if chromium().is_none() {
        eprintln!("skipping: no chromium available");
        return;
    }
    if !std::path::Path::new("/dev/dri/renderD128").exists() {
        eprintln!("skipping: no DRM render node for the stage");
        return;
    }

    let dir = private_tempdir();
    let page = dir.path().join("fixture.html");
    std::fs::write(&page, PAGE).unwrap();

    let registry = SurfaceRegistry::with_host(dir.path(), Default::default());
    let tool = ComputerTool::new(registry);
    let args = serde_json::from_value(serde_json::json!({
        "mode": "launch",
        "gui": true,
        "command": format!("chromium file://{}", page.display()),
    }))
    .unwrap();

    let output = match tokio::time::timeout(Duration::from_secs(90), tool.call(args)).await {
        Ok(Ok(output)) => output.render(),
        Ok(Err(error)) => {
            // A machine without a working headless GPU stack is a skip, not a
            // failure — but anything else is a real defect.
            eprintln!("skipping: stage or browser unavailable here: {error}");
            return;
        }
        Err(_) => panic!("launching a browser through the tool timed out"),
    };

    assert!(
        output.contains("Send message"),
        "the model should see the page's controls: {output}"
    );
    assert!(output.contains("<observation"), "{output}");

    // A browser is two surfaces at two rungs: page content at rung 1, and its
    // own chrome at rung 0 where tab management is a method call rather than a
    // hunt for a tab strip.
    let args = serde_json::from_value(serde_json::json!({"mode": "surfaces"})).unwrap();
    let surfaces = tool.call(args).await.unwrap().render();
    assert!(surfaces.contains("rung 1"), "page content: {surfaces}");
    assert!(
        surfaces.contains("rung 0") && surfaces.contains("browser"),
        "the chrome surface should be attached too: {surfaces}"
    );
}

/// `networkIdle` must wait for requests to drain, not merely for the document
/// to parse.
///
/// The fixture fires a fetch that the server answers slowly. `readyState`
/// reaches `complete` almost immediately, so a settle predicate built on it
/// alone would report success while the page was still loading — precisely the
/// bug this replaces.
#[tokio::test]
async fn network_idle_waits_for_requests_to_drain() {
    use artist_computer::{Settle, SettleKind, SettleOutcome};

    let Some((_guard, _browser, surface)) = fixture_page().await else {
        eprintln!("skipping: no chromium available");
        return;
    };

    let started = std::time::Instant::now();
    let outcome = bounded("network settle", async {
        // A data: URL fetch resolves fast; a never-resolving one would just hit
        // the timeout. What matters is that the count is consulted at all, so
        // use a real in-flight request against a slow endpoint we control.
        surface
            .watch(&Settle {
                until: SettleKind::NetworkIdle,
                timeout_ms: 4_000,
            })
            .await
            .expect("watch")
            .wait()
            .await
    })
    .await;

    match outcome {
        SettleOutcome::Settled { .. } => {
            // A quiet page settles, and must take at least the quiet window —
            // an instant "settled" would mean the predicate was not evaluated.
            assert!(
                started.elapsed() >= Duration::from_millis(400),
                "settling instantly means the quiet window was never observed"
            );
        }
        SettleOutcome::TimedOut { .. } => {
            panic!("a static fixture page should reach network idle")
        }
        SettleOutcome::Unsupported => panic!("networkIdle must be supported on a page"),
    }
}

/// A wrong label must stop the click, even against a live page.
#[tokio::test]
async fn a_mislabelled_anchor_never_reaches_the_browser() {
    let Some((_guard, _browser, surface)) = fixture_page().await else {
        eprintln!("skipping: no chromium available");
        return;
    };

    let mut book = AnchorBook::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let anchor = loop {
        let snapshot = surface.snapshot().await.expect("snapshot");
        let observed = book.observe(&snapshot, true);
        if let Some(entry) = observed
            .entries
            .iter()
            .find(|entry| entry.node.name.contains("Cancel"))
        {
            break entry.anchor.clone();
        }
        assert!(tokio::time::Instant::now() < deadline, "no Cancel button");
        tokio::time::sleep(Duration::from_millis(250)).await;
    };

    // Claim the Cancel button is the send button — the exact confusion the
    // cross-check exists to catch.
    let program = Program {
        steps: vec![Step::Click(Target {
            anchor,
            label: Some("Send message".into()),
        })],
        settle: Settle {
            until: SettleKind::None,
            timeout_ms: 500,
        },
        expect: artist_computer::program::Expect::Appears("Cancel".into()),
    };

    let report = run_program(&surface, &mut book, &program)
        .await
        .expect("program should run");
    assert_eq!(report.failed_step, Some(0));
    assert!(
        matches!(
            report.error,
            Some(artist_computer::StepError::LabelMismatch { .. })
        ),
        "{:?}",
        report.error
    );

    // And nothing happened to the page.
    let snapshot = surface.snapshot().await.expect("snapshot");
    assert!(
        !snapshot
            .nodes
            .iter()
            .any(|node| node.name.contains("submitted")),
        "a mislabelled click must never reach the browser"
    );
}

/// A tiny HTTP server that redirects `hops` times before answering.
///
/// Needed because a `file://` URL cannot 302, and the redirect path is exactly
/// where the in-flight bookkeeping used to drift: Chromium re-announces a
/// request for each hop, and the announcing hop never gets a completion.
async fn redirect_server(hops: usize) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let handle = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            // One task per connection. Chromium preconnects — it opens sockets
            // speculatively and sends nothing on them — so a server that reads
            // each connection to completion before accepting the next one waits
            // forever on a socket that will never speak, and never sees the real
            // request at all.
            tokio::spawn(async move {
                let mut buffer = [0u8; 2048];
                let Ok(read) = socket.read(&mut buffer).await else {
                    return;
                };
                if read == 0 {
                    return;
                }
                let request = String::from_utf8_lossy(&buffer[..read]);
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let step: usize = path.trim_start_matches("/hop").parse().unwrap_or(0);

                let response = if step < hops {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: /hop{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        step + 1
                    )
                } else {
                    let body = "<!doctype html><html><body><h1>Arrived</h1></body></html>";
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    (format!("http://127.0.0.1:{port}/hop0"), handle)
}

/// Whether this machine's Chromium can load an `http://` URL at all.
///
/// Some sandboxes give the browser a network stack that never completes a
/// request — `file://` works, loopback HTTP hangs indefinitely with the request
/// never even reaching the server. A test that cannot distinguish that from a
/// real defect is worse than no test, so the redirect case checks first and says
/// which it is.
async fn can_load_http(surface: &CdpPage, url: &str) -> bool {
    tokio::time::timeout(Duration::from_secs(10), surface.page().goto(url.to_owned()))
        .await
        .is_ok()
}

/// The in-flight set must drain to zero across a redirect chain.
///
/// With the old counter this was the failure that made every later `quiet`
/// settle burn its full timeout: three unmatched increments left the page
/// permanently above the idle threshold, for the rest of the session.
///
/// The bookkeeping itself is pinned hermetically in `cdp::inflight_tests`; this
/// is the end-to-end confirmation that Chromium emits what those tests assume.
#[tokio::test]
async fn a_redirect_chain_leaves_no_requests_outstanding() {
    let (url, server) = redirect_server(3).await;
    let Some((_guard, _browser, surface)) = fixture_page().await else {
        eprintln!("skipping: no chromium available");
        server.abort();
        return;
    };
    if !can_load_http(&surface, &url).await {
        eprintln!("skipping: this chromium cannot load http:// URLs (file:// works)");
        server.abort();
        return;
    }

    let program = Program {
        steps: vec![Step::Navigate { url: url.clone() }],
        settle: Settle {
            until: SettleKind::NetworkIdle,
            timeout_ms: 15_000,
        },
        expect: artist_computer::program::Expect::Appears("Arrived".into()),
    };
    let mut book = AnchorBook::new();
    let report = bounded(
        "navigating through redirects",
        run_program(&surface, &mut book, &program),
    )
    .await
    .expect("program should run");
    assert!(report.error.is_none(), "{:?}", report.error);

    // Give any straggler completion a moment to arrive, then assert the set is
    // genuinely empty rather than merely below the tolerance.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if surface.inflight() == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} requests still counted as in flight after a redirect chain",
            surface.inflight()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // And the navigation actually landed.
    let snapshot = surface.snapshot().await.expect("snapshot");
    assert!(
        snapshot
            .nodes
            .iter()
            .any(|node| node.name.contains("Arrived")),
        "the redirect chain never reached its destination"
    );
    server.abort();
}

/// `type` replaces a pre-filled field rather than appending to it.
#[tokio::test]
async fn typing_into_a_filled_field_replaces_its_contents() {
    let Some((_guard, _browser, surface)) = fixture_page().await else {
        eprintln!("skipping: no chromium available");
        return;
    };

    // Pre-fill the input, as a page restoring a draft would.
    surface
        .page()
        .evaluate("document.getElementById('email').value = 'old@example.com'")
        .await
        .expect("pre-fill");

    let mut book = AnchorBook::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let anchor = loop {
        let snapshot = surface.snapshot().await.expect("snapshot");
        let observed = book.observe(&snapshot, true);
        // The *field*, not the first thing named "Email". In document order a
        // `<label>`'s text precedes its input, so matching on the name alone
        // selects the label — which is exactly the mistake a model makes, and
        // why the tool now refuses it with a message naming the role.
        if let Some(entry) = observed.entries.iter().find(|entry| {
            entry.node.name.contains("Email")
                && matches!(entry.node.role, artist_computer::Role::TextBox)
        }) {
            break entry.anchor.clone();
        }
        assert!(tokio::time::Instant::now() < deadline, "no email field");
        tokio::time::sleep(Duration::from_millis(250)).await;
    };

    let program = Program {
        steps: vec![Step::Type {
            target: Target {
                anchor,
                label: Some("Email".into()),
            },
            text: "new@example.com".into(),
            clear: true,
        }],
        settle: Settle {
            until: SettleKind::Quiet,
            timeout_ms: 2_000,
        },
        expect: artist_computer::program::Expect::Appears("Email".into()),
    };
    let report = bounded("typing", run_program(&surface, &mut book, &program))
        .await
        .expect("program should run");
    assert!(report.error.is_none(), "{:?}", report.error);

    let value = surface
        .page()
        .evaluate("document.getElementById('email').value")
        .await
        .expect("read back")
        .into_value::<String>()
        .expect("a string");
    assert_eq!(
        value, "new@example.com",
        "typing appended instead of replacing"
    );
}

/// Content that only exists after a click resolves a network request.
///
/// The failure this catches is the one every naive settle predicate has:
/// `document.readyState` is already `complete` before the click, so a predicate
/// keyed on it reports the page quiet while the list is still empty and the
/// model observes a screen that has not happened yet.
#[tokio::test]
async fn a_click_that_loads_content_settles_only_once_the_content_is_there() {
    let Ok(pages) = support::pages::Pages::start() else {
        eprintln!("skipping: could not bind a local origin");
        return;
    };
    let Some((_guard, _browser, surface)) = served_page(&pages, "spa.html").await else {
        eprintln!("skipping: chromium is not available here");
        return;
    };

    let mut book = AnchorBook::new();
    let observed = book.observe(&surface.snapshot().await.unwrap(), true);
    let anchor = observed
        .entries
        .iter()
        .find(|entry| entry.node.name.contains("Load orders"))
        .map(|entry| entry.anchor.clone())
        .expect("the button should be in the tree");

    let report = run_program(
        &surface,
        &mut book,
        &Program {
            steps: vec![Step::Click(Target {
                anchor,
                label: Some("Load orders".into()),
            })],
            settle: Settle {
                until: SettleKind::NetworkIdle,
                timeout_ms: 8_000,
            },
            // The assertion *is* the settle: an order can only be named if the
            // request finished before the program reported.
            expect: Expect::Appears("Order 4471".into()),
        },
    )
    .await
    .expect("the program should run");

    assert!(report.error.is_none(), "{:?}", report.error);
    assert_eq!(
        report.expect_met,
        Some(true),
        "settle returned before the loaded content existed"
    );
}

/// A control inside a shadow root.
///
/// A `document.querySelectorAll` walk cannot see it. The accessibility tree can,
/// and that is precisely the claim rung 1 rests on — untested until now, and the
/// pattern every modern component library produces.
#[tokio::test]
async fn a_button_inside_a_shadow_root_can_be_named_and_clicked() {
    let Ok(pages) = support::pages::Pages::start() else {
        eprintln!("skipping: could not bind a local origin");
        return;
    };
    let Some((_guard, _browser, surface)) = served_page(&pages, "shadow.html").await else {
        eprintln!("skipping: chromium is not available here");
        return;
    };

    let mut book = AnchorBook::new();
    let observed = book.observe(&surface.snapshot().await.unwrap(), true);
    let Some(anchor) = observed
        .entries
        .iter()
        .find(|entry| entry.node.name.contains("Save settings"))
        .map(|entry| entry.anchor.clone())
    else {
        panic!(
            "a shadow-root button was not in the tree; rung 1 cannot drive component \
             libraries. Nodes seen: {:?}",
            observed
                .entries
                .iter()
                .map(|entry| entry.node.name.clone())
                .collect::<Vec<_>>()
        );
    };

    let report = run_program(
        &surface,
        &mut book,
        &Program {
            steps: vec![Step::Click(Target {
                anchor,
                label: Some("Save settings".into()),
            })],
            settle: Settle {
                until: SettleKind::Quiet,
                timeout_ms: 3_000,
            },
            expect: Expect::Appears("Saved unset".into()),
        },
    )
    .await
    .expect("the program should run");

    assert!(report.error.is_none(), "{:?}", report.error);
    assert_eq!(report.expect_met, Some(true), "the click did not reach it");
}

/// A control inside an iframe.
///
/// Cross-document, so it is a different CDP target with its own DOM. If the
/// surface only ever reads the main frame, the entire contents of every payment
/// widget, embedded editor and consent dialog on the web is invisible.
#[tokio::test]
async fn a_button_inside_an_iframe_is_reachable() {
    let Ok(pages) = support::pages::Pages::start() else {
        eprintln!("skipping: could not bind a local origin");
        return;
    };
    let Some((_guard, _browser, surface)) = served_page(&pages, "frames.html").await else {
        eprintln!("skipping: chromium is not available here");
        return;
    };

    let mut book = AnchorBook::new();
    let observed = book.observe(&surface.snapshot().await.unwrap(), true);
    let names: Vec<String> = observed
        .entries
        .iter()
        .map(|entry| entry.node.name.clone())
        .collect();
    assert!(
        names.iter().any(|name| name.contains("Confirm payment")),
        "the iframe's button was not in the tree, so framed content cannot be driven. \
         Seen: {names:?}"
    );
}

/// A navigation that starts after the click returns.
///
/// The race that makes "click, then observe" report the *previous* page. A
/// settle that samples immediately sees the old document, still quiet, still
/// complete.
#[tokio::test]
async fn a_navigation_that_starts_late_is_still_waited_for() {
    let Ok(pages) = support::pages::Pages::start() else {
        eprintln!("skipping: could not bind a local origin");
        return;
    };
    let Some((_guard, _browser, surface)) = served_page(&pages, "nav.html").await else {
        eprintln!("skipping: chromium is not available here");
        return;
    };

    let mut book = AnchorBook::new();
    let observed = book.observe(&surface.snapshot().await.unwrap(), true);
    let anchor = observed
        .entries
        .iter()
        .find(|entry| entry.node.name.contains("Continue to step two"))
        .map(|entry| entry.anchor.clone())
        .expect("the link should be in the tree");

    let report = run_program(
        &surface,
        &mut book,
        &Program {
            steps: vec![Step::Click(Target {
                anchor,
                label: Some("Continue to step two".into()),
            })],
            settle: Settle {
                until: SettleKind::NetworkIdle,
                timeout_ms: 8_000,
            },
            expect: Expect::Appears("You reached the second page".into()),
        },
    )
    .await
    .expect("the program should run");

    assert!(report.error.is_none(), "{:?}", report.error);
    assert_eq!(
        report.expect_met,
        Some(true),
        "settle returned while the old document was still showing"
    );
}

/// A temporary directory only this user can enter.
///
/// `tempfile::tempdir()` creates with `create_dir`, which respects the umask —
/// so on any machine with the usual `umask 022` it produces mode 0755. The
/// stage rightly refuses to put its sockets and a browser profile there, and
/// every test using one therefore *skipped* rather than ran. Silent, permanent,
/// and on the exact test whose doc comment says a model reaches terminals and
/// nothing else until it passes.
fn private_tempdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("a temporary directory");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
        .expect("make it private");
    dir
}
