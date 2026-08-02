//! The whole stack, through the tool the model actually calls.
//!
//! Every layer below this is tested on its own, and until now the *stack* of
//! them was not. That gap is where the interesting failures live: a surface that
//! works when constructed directly but is never reachable from `launch`, an
//! anchor that resolves in a unit test but never reaches an observation, an
//! event that is recorded by a function nothing calls.
//!
//! This drives the same entry point the model does — `ComputerTool::call` with
//! JSON — and checks the three things a real task depends on end to end: an
//! application can be launched and observed, an anchor from that observation can
//! be acted on, and the whole thing lands in the event log that `artist computer
//! log` and `distill` read.

#![cfg(all(target_os = "linux", feature = "stage-wayland"))]

use std::time::Duration;

use artist_computer::{ComputerTool, SurfaceRegistry};
use rig_core::tool::PortableTool;

fn gui_app() -> Option<&'static str> {
    ["zenity"]
        .into_iter()
        .find(|name| std::path::Path::new("/usr/bin").join(name).exists())
}

fn stage_possible() -> bool {
    std::path::Path::new("/dev/dri/renderD128").exists()
        && std::path::Path::new("/usr/lib/at-spi-bus-launcher").exists()
}

/// `$XDG_RUNTIME_DIR` is 0700, and the stage refuses anything looser.
fn private_dir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
        .expect("tighten the fixture directory");
    dir
}

async fn call(tool: &ComputerTool, value: serde_json::Value) -> Result<String, String> {
    let args = serde_json::from_value(value).map_err(|error| error.to_string())?;
    tokio::time::timeout(Duration::from_secs(120), tool.call(args))
        .await
        .map_err(|_| "the tool call timed out".to_owned())?
        .map(|output| output.render())
        .map_err(|error| error.to_string())
}

/// A terminal task, start to finish, through the tool.
///
/// No display needed, so this runs everywhere and is the honest floor: if this
/// breaks, the tool is broken regardless of what any rung can do.
#[tokio::test]
async fn a_terminal_task_runs_end_to_end_through_the_tool() {
    let dir = private_dir();
    let log = artist_session::EventLogWriter::open(dir.path(), "e2e").unwrap();
    let (recorder, _writer) = artist_session::spawn_writer(log, None);
    let reader = artist_session::EventLogReader::new(dir.path());

    let registry = SurfaceRegistry::with_host(dir.path(), Default::default());
    let tool = ComputerTool::with_recorder(registry, recorder.clone(), None);

    // Launch. `printf` then `cat` keeps the process alive with known output on
    // screen, so the observation is deterministic.
    let launched = call(
        &tool,
        serde_json::json!({
            "mode": "launch",
            "command": "printf 'READY-MARKER\\n'; cat",
        }),
    )
    .await
    .expect("launch should succeed");

    assert!(launched.contains("launched"), "{launched}");
    let surface = launched
        .split_whitespace()
        .find(|word| word.starts_with("term"))
        .map(|word| word.trim_end_matches(['\n', ' ']).to_owned())
        .expect("the launch line names the surface");

    // Observe at least once through the tool, whatever `launch` already showed.
    // Seeding the loop with the launch output would let it exit immediately and
    // silently skip the `observe` path entirely — which is most of what this
    // test exists to cover.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut observation;
    loop {
        observation = call(
            &tool,
            serde_json::json!({"mode": "observe", "surface": surface, "full": true}),
        )
        .await
        .expect("observe should succeed");
        if observation.contains("READY-MARKER") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the program's output never appeared: {observation}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Act. A key step needs no anchor, which is what makes it the right first
    // thing to prove: it exercises dispatch without depending on resolution.
    let acted = call(
        &tool,
        serde_json::json!({
            "mode": "do",
            "surface": surface,
            "steps": [{"key": "Enter"}],
            "settle": {"until": "none"},
            "expect": {"appears": "READY-MARKER"}
        }),
    )
    .await
    .expect("the program should run");
    assert!(!acted.contains("failed step"), "{acted}");

    // And it must all be in the log, or `artist computer log` and `distill` have
    // nothing to read — which is exactly the state this subsystem was in before
    // the recorder was wired.
    recorder.flush().await;
    let events = reader.read_all().expect("read the log");

    let observed = events
        .iter()
        .filter(|envelope| {
            matches!(
                envelope.event(),
                artist_session::SessionEvent::ComputerObserved(_)
            )
        })
        .count();
    let acted_events: Vec<_> = events
        .iter()
        .filter_map(|envelope| match envelope.event() {
            artist_session::SessionEvent::ComputerActed(acted) => Some(acted),
            _ => None,
        })
        .collect();

    // One from `launch`, at least one from `observe`.
    assert!(observed >= 2, "every look must be recorded, got {observed}");
    assert_eq!(acted_events.len(), 1, "the program must be recorded once");

    let recorded = &acted_events[0];
    assert_eq!(recorded.surface, surface);
    assert_eq!(recorded.steps.len(), 1);
    assert_eq!(recorded.steps[0].action, "key");
    // The payload is what makes a distilled macro replayable rather than a
    // silent no-op.
    assert_eq!(recorded.steps[0].payload.as_deref(), Some("Enter"));
    assert_eq!(recorded.expect.as_deref(), Some("READY-MARKER"));
    assert_eq!(recorded.expect_met, Some(true));
}

/// A GUI task, start to finish, through the tool.
///
/// This is the path the plan's verification section describes and nothing had
/// ever walked: `launch` with `gui: true` brings up an isolated display, starts
/// a real application in it, picks a rung, and returns an observation whose
/// anchors the model can act on.
#[tokio::test]
async fn a_gui_task_runs_end_to_end_through_the_tool() {
    let Some(program) = gui_app() else {
        eprintln!("skipping: no GUI test application installed");
        return;
    };
    if !stage_possible() {
        eprintln!("skipping: no DRM render node or no at-spi2-core");
        return;
    }

    let dir = private_dir();
    let log = artist_session::EventLogWriter::open(dir.path(), "e2e-gui").unwrap();
    let (recorder, _writer) = artist_session::spawn_writer(log, None);
    let reader = artist_session::EventLogReader::new(dir.path());

    let registry = SurfaceRegistry::with_host(dir.path(), Default::default());
    let tool = ComputerTool::with_recorder(registry, recorder.clone(), None);

    let launched = match call(
        &tool,
        serde_json::json!({
            "mode": "launch",
            "gui": true,
            "command": format!(
                "{program} --question --title=e2e --text=Proceed? --ok-label=Approve --cancel-label=Reject"
            ),
        }),
    )
    .await
    {
        Ok(output) => output,
        Err(error) => {
            eprintln!("skipping: could not bring up a GUI here: {error}");
            return;
        }
    };

    // The observation must name the dialog's own buttons, with anchors.
    assert!(
        launched.contains("Approve") && launched.contains("Reject"),
        "the observation must describe the dialog: {launched}"
    );

    let surface = launched
        .split_whitespace()
        .find(|word| word.starts_with("win") || word.starts_with("screen"))
        .map(str::to_owned)
        .expect("the launch line names the surface");

    // Pull the anchor for "Approve" out of the rendered observation, exactly as
    // the model would: it is the parenthesised token at the end of the line.
    let anchor = launched
        .lines()
        .find(|line| line.contains("\"Approve\""))
        .and_then(|line| line.rsplit('(').next())
        .map(|tail| tail.trim_end_matches([')', ' ']).to_owned())
        .expect("the Approve button must carry an anchor");

    // Act on it by anchor and label, the way every rung is driven.
    let acted = call(
        &tool,
        serde_json::json!({
            "mode": "do",
            "surface": surface,
            "steps": [{"click": {"anchor": anchor, "label": "Approve"}}],
            "settle": {"until": "quiet", "timeoutMs": 3000},
            "expect": {"gone": "Approve"}
        }),
    )
    .await
    .expect("the program should run");

    eprintln!("gui program report:\n{acted}");
    assert!(
        !acted.contains("label mismatch"),
        "the anchor did not mean what the observation said: {acted}"
    );

    recorder.flush().await;
    let events = reader.read_all().expect("read the log");
    let acted_events: Vec<_> = events
        .iter()
        .filter_map(|envelope| match envelope.event() {
            artist_session::SessionEvent::ComputerActed(acted) => Some(acted),
            _ => None,
        })
        .collect();

    assert_eq!(acted_events.len(), 1, "the GUI program must be recorded");
    let recorded = &acted_events[0];
    assert_eq!(recorded.steps[0].action, "click");
    assert_eq!(recorded.steps[0].label.as_deref(), Some("Approve"));
    // The audit trail's whole point: what the model claimed, and what it was.
    assert!(
        recorded.steps[0]
            .resolved_name
            .as_deref()
            .is_some_and(|name| name.contains("Approve")),
        "the resolved name must be recorded: {:?}",
        recorded.steps[0].resolved_name
    );
}

/// A long session: hundreds of epochs of churn on one surface.
///
/// The anchor allocator has a hard exhaustion assert — it panics rather than
/// hand out a duplicate — and the tombstone list is bounded. Neither has ever
/// been driven past a handful of epochs, so "it works for a hundred steps" was
/// an assumption. A session that dies at step 300 with a panic inside the
/// harness is the worst failure mode available.
#[tokio::test]
async fn a_long_session_of_churn_does_not_exhaust_or_leak_anchors() {
    use artist_computer::{AnchorBook, Node, Role, Snapshot};

    let mut book = AnchorBook::new();
    let mut anchors_seen = std::collections::HashSet::new();

    for epoch in 0..500u32 {
        // Every element is new every time — the worst case for the allocator,
        // and what a re-rendering list actually does.
        let nodes: Vec<Node> = (0..40)
            .map(|index| {
                Node::new(
                    format!("e{epoch}:n{index}"),
                    Role::Button,
                    format!("Item {epoch}-{index}"),
                )
            })
            .collect();
        let observed = book.observe(&Snapshot::new(nodes), false);

        // Every anchor issued in this epoch must resolve in this epoch. A
        // duplicate would resolve to the wrong node and never be noticed.
        let mut live = std::collections::HashSet::new();
        for entry in &observed.entries {
            assert!(
                live.insert(entry.anchor.clone()),
                "epoch {epoch} issued {} twice",
                entry.anchor
            );
            assert!(
                book.resolve(&entry.anchor).is_ok(),
                "epoch {epoch}: {} did not resolve",
                entry.anchor
            );
        }
        anchors_seen.extend(live);
    }

    // The handle space is finite and reused; 500 epochs of 40 fresh elements is
    // 20,000 allocations. If it were leaking rather than reclaiming, the pool
    // would be exhausted long before here.
    assert!(
        anchors_seen.len() < 20_000,
        "handles are not being reclaimed: {} distinct anchors for 20,000 elements",
        anchors_seen.len()
    );
}

/// The stage dying underneath a surface.
///
/// Not hypothetical: the compositor thread can die on a GL error, and a stage is
/// killed on `Drop`. What must not happen is a hang — a tool call that never
/// returns takes the whole session with it, and is far worse than an error.
#[tokio::test]
async fn a_surface_whose_stage_died_reports_rather_than_hangs() {
    use artist_computer::stage::{Stage, StageId, wayland::StageWayland};

    if !std::path::Path::new("/dev/dri/renderD128").exists() {
        eprintln!("skipping: no DRM render node");
        return;
    }
    let dir = private_dir();
    let Ok(stage) = StageWayland::start(StageId("dying".into()), dir.path()) else {
        eprintln!("skipping: stage unavailable here");
        return;
    };

    stage.shutdown().await.expect("shutdown is not an error");
    // The compositor thread is gone. Every later call must fail promptly rather
    // than block on a channel nobody is reading.
    let answered = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let _ = stage.windows().await;
        let _ = stage.capture(None).await;
    })
    .await;
    assert!(
        answered.is_ok(),
        "a dead stage must report, not hang — a tool call that never returns \
         takes the session with it"
    );
}
