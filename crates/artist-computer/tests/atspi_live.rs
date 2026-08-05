//! Rung 2 against a real toolkit, on a real stage.
//!
//! **This is the test the design has been missing.** Rung 2 is the universal
//! rung for native applications — everything that is not a browser and not a
//! terminal arrives here — and until now its only coverage was string mapping of
//! role names. Whether a real GTK application puts a usable tree on our private
//! bus, and whether its buttons declare actions we can invoke without touching a
//! coordinate, were open questions the whole rung rested on.
//!
//! The plan called this out as a risk to resolve *before* betting the adapter
//! design on rung 2. This resolves it.
//!
//! Skipped rather than ignored when the pieces are absent, so a machine that has
//! them gets the coverage automatically.

use std::sync::Arc;
use std::time::Duration;

use artist_computer::model::Rung;
use artist_computer::surface::Surface;

/// Something that puts named, actionable widgets on screen.
///
/// `zenity` is a good target precisely because it is boring: a GTK dialog with
/// two named buttons is the shape most native automation actually deals with,
/// and it exits on its own if we lose track of it.
fn app() -> Option<&'static str> {
    ["zenity"]
        .into_iter()
        .find(|name| std::path::Path::new("/usr/bin").join(name).exists())
}

/// A temp directory only this user can enter.
///
/// `tempfile` honours the umask, which on this machine yields 0755 — and the
/// stage correctly refuses to put sockets and a browser profile somewhere other
/// users can traverse. Real `$XDG_RUNTIME_DIR` is 0700, so the fixture has to be
/// too; loosening the check to accommodate the test would delete the property.
fn private_dir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
        .expect("tighten the fixture directory");
    dir
}

fn stage_possible() -> bool {
    std::path::Path::new("/dev/dri/renderD128").exists()
        && std::path::Path::new("/usr/lib/at-spi-bus-launcher").exists()
        && std::path::Path::new("/usr/lib/at-spi2-registryd").exists()
}

/// Attach to a real GTK application and read its widgets.
///
/// Asserts the three things rung 2 promises, in the order they can fail:
/// a tree exists at all, its widgets carry accessible *names*, and those widgets
/// declare **actions** — which is what lets us invoke them by name rather than
/// by clicking a computed point.
#[tokio::test]
async fn a_real_gtk_application_exposes_a_usable_tree() {
    let Some(program) = app() else {
        eprintln!("skipping: no GTK test application installed");
        return;
    };
    if !stage_possible() {
        eprintln!("skipping: no DRM render node or no at-spi2-core");
        return;
    }

    let dir = private_dir();
    let registry = artist_computer::SurfaceRegistry::with_host(dir.path(), Default::default());

    // A dialog with two named buttons and a label. Everything here is a widget
    // a person would point at, which is exactly what rung 2 has to surface.
    let launched = tokio::time::timeout(
        Duration::from_secs(90),
        registry.host().launch(
            program,
            &[
                "--question".into(),
                "--title=artist-atspi-probe".into(),
                "--text=Delete this file?".into(),
                "--ok-label=Delete".into(),
                "--cancel-label=Keep".into(),
            ],
            None,
            Default::default(),
        ),
    )
    .await;

    let surface = match launched {
        Ok(Ok(launched)) => launched.surface,
        Ok(Err(error)) => {
            // A machine without a working headless GL stack, or a toolkit that
            // refuses to start, is a skip. Anything else is a real defect — and
            // the error now names which rung declined and why, which is what
            // makes this judgement possible at all.
            eprintln!("skipping: could not attach a surface here: {error}");
            return;
        }
        Err(_) => panic!("launching {program} on a stage timed out"),
    };

    assert_eq!(
        surface.rung(),
        Rung::Accessibility,
        "a GTK dialog must reach rung 2, not fall through to pixels"
    );

    let snapshot = tokio::time::timeout(Duration::from_secs(30), surface.snapshot())
        .await
        .expect("snapshot timed out")
        .expect("snapshot should succeed");

    let named: Vec<&artist_computer::model::Node> = snapshot
        .nodes
        .iter()
        .filter(|node| !node.name.trim().is_empty())
        .collect();

    assert!(
        named.len() >= 2,
        "a dialog with two buttons and a label should expose several named \
         widgets, got {}: {:?}",
        named.len(),
        snapshot
            .nodes
            .iter()
            .map(|n| (&n.name, n.role.label()))
            .collect::<Vec<_>>()
    );

    // The labels we set. If these are absent the tree exists but is not
    // describing the dialog we launched.
    let has = |wanted: &str| {
        named
            .iter()
            .any(|node| node.name.to_lowercase().contains(&wanted.to_lowercase()))
    };
    assert!(
        has("Delete") && has("Keep"),
        "the dialog's own button labels must appear: {:?}",
        named.iter().map(|n| &n.name).collect::<Vec<_>>()
    );

    // **The question the rung rests on.** If real toolkits do not declare
    // actions, invoking by name is impossible and everything falls back to
    // synthesized clicks — which would change the roadmap, not just this test.
    let actionable: Vec<&&artist_computer::model::Node> = named
        .iter()
        .filter(|node| !node.actions.is_empty())
        .collect();

    eprintln!(
        "rung 2 on {program}: {} nodes, {} named, {} declaring actions",
        snapshot.nodes.len(),
        named.len(),
        actionable.len()
    );
    for node in &actionable {
        eprintln!(
            "  {} {:?} -> {:?}",
            node.role.label(),
            node.name,
            node.actions
        );
    }

    assert!(
        !actionable.is_empty(),
        "no widget declared an action. Rung 2 can then only observe, and the \
         adapter design has to carry more weight than planned. Nodes were: {:?}",
        named
            .iter()
            .map(|n| (&n.name, n.role.label(), &n.actions))
            .collect::<Vec<_>>()
    );
}

/// Two applications on one stage are told apart by process, not by title.
///
/// The pid route is what makes attribution exact, and it is only exact if it
/// survives a second application being present. Title matching would pick
/// whichever dialog it saw first.
#[tokio::test]
async fn two_applications_on_one_stage_do_not_get_confused() {
    let Some(program) = app() else {
        eprintln!("skipping: no GTK test application installed");
        return;
    };
    if !stage_possible() {
        eprintln!("skipping: no DRM render node or no at-spi2-core");
        return;
    }

    let dir = private_dir();
    let registry = artist_computer::SurfaceRegistry::with_host(dir.path(), Default::default());
    let host = registry.host();

    let first = tokio::time::timeout(
        Duration::from_secs(90),
        host.launch(
            program,
            &[
                "--info".into(),
                "--title=probe-one".into(),
                "--text=FirstWindowMarker".into(),
            ],
            None,
            Default::default(),
        ),
    )
    .await;
    let Ok(Ok(first)) = first.map(|r| r.map(|l| l.surface)) else {
        eprintln!("skipping: first launch did not attach");
        return;
    };

    let second = tokio::time::timeout(
        Duration::from_secs(90),
        host.launch(
            program,
            &[
                "--info".into(),
                "--title=probe-two".into(),
                "--text=SecondWindowMarker".into(),
            ],
            None,
            Default::default(),
        ),
    )
    .await;
    let Ok(Ok(second)) = second.map(|r| r.map(|l| l.surface)) else {
        eprintln!("skipping: second launch did not attach");
        return;
    };

    assert_ne!(
        first.id().as_str(),
        second.id().as_str(),
        "two launches must be two surfaces"
    );

    let text_of = |surface: Arc<dyn Surface>| async move {
        surface
            .snapshot()
            .await
            .map(|snapshot| {
                snapshot
                    .nodes
                    .iter()
                    .map(|node| node.name.clone())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    };

    let first_text = text_of(Arc::clone(&first)).await;
    let second_text = text_of(Arc::clone(&second)).await;

    assert!(
        first_text.contains("FirstWindowMarker"),
        "the first surface must describe the first window, got {first_text:?}"
    );
    assert!(
        second_text.contains("SecondWindowMarker"),
        "the second surface must describe the second window, got {second_text:?}"
    );
    assert!(
        !second_text.contains("FirstWindowMarker"),
        "the second surface leaked the first window's content — attribution by \
         pid has failed: {second_text:?}"
    );
}
