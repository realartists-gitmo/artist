//! The half of the project map that keeps the other half honest.
//!
//! The skeleton is issued once, on the turn that opens a session, and never
//! reissued — reissuing it would move content the model has already been given
//! and reprice every cached token behind it. That leaves a gap: a session long
//! enough to change the architecture is running on a map that quietly stopped
//! being true.
//!
//! These cover the delta that closes it. Written against real files and a real
//! dependency graph rather than a hand-built fixture, because the two previous
//! bugs in this subsystem — an outline that misreported its own coverage, and a
//! fan-in of zero across twelve crates — both passed complete unit suites while
//! producing wrong output. Structure-shaped assertions do not catch a wrong
//! claim.

use std::path::Path;

/// A workspace of two crates, `app` depending on nothing yet.
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // A project marker, so root discovery stops here rather than climbing out
    // into whatever contains the temp directory.
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();

    for unit in ["core", "app"] {
        std::fs::create_dir_all(root.join(unit).join("src")).unwrap();
        std::fs::write(
            root.join(unit).join("Cargo.toml"),
            format!("[package]\nname = \"{unit}\"\n"),
        )
        .unwrap();
    }
    std::fs::write(
        root.join("core/src/lib.rs"),
        "//! Shared primitives.\npub fn helper() {}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("app/src/lib.rs"),
        "//! The application.\npub fn run() {}\n",
    )
    .unwrap();
    dir
}

/// Rewrite a file and hand back its path.
fn rewrite(root: &Path, relative: &str, body: &str) -> std::path::PathBuf {
    let path = root.join(relative);
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn a_new_dependency_between_units_is_reported() {
    let dir = workspace();
    let root = dir.path();
    let mut baseline = artist_tools::skeleton::baseline(root).expect("a baseline");

    let file = rewrite(
        root,
        "app/src/lib.rs",
        "//! The application.\nuse core::helper;\npub fn run() { helper() }\n",
    );
    let notes = baseline.observe(root, &file);

    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("app"), "{notes:?}");
    assert!(notes[0].contains("core"), "{notes:?}");
}

/// The common case, and the one that has to stay free: most edits do not move
/// the architecture, and a note that fires constantly stops being read.
#[test]
fn an_edit_that_changes_no_dependency_says_nothing() {
    let dir = workspace();
    let root = dir.path();
    let mut baseline = artist_tools::skeleton::baseline(root).expect("a baseline");

    let file = rewrite(
        root,
        "app/src/lib.rs",
        "//! The application.\npub fn run() { let _ = 1 + 1; }\n",
    );
    assert!(baseline.observe(root, &file).is_empty());
}

/// A dependency that was already there when the session opened is not news.
#[test]
fn a_pre_existing_dependency_is_not_reported() {
    let dir = workspace();
    let root = dir.path();
    rewrite(
        root,
        "app/src/lib.rs",
        "//! The application.\nuse core::helper;\npub fn run() { helper() }\n",
    );

    // Baseline captured *after* the dependency exists.
    let mut baseline = artist_tools::skeleton::baseline(root).expect("a baseline");
    let file = rewrite(
        root,
        "app/src/lib.rs",
        "//! The application.\nuse core::helper;\npub fn run() { helper(); helper() }\n",
    );
    assert!(
        baseline.observe(root, &file).is_empty(),
        "an edge present at session start is not a change"
    );
}

/// Announced once. Repeating it on every subsequent commit would make the
/// channel noise, which is the failure mode that kills a hook like this.
#[test]
fn a_reported_dependency_is_absorbed_and_not_repeated() {
    let dir = workspace();
    let root = dir.path();
    let mut baseline = artist_tools::skeleton::baseline(root).expect("a baseline");

    let file = rewrite(
        root,
        "app/src/lib.rs",
        "//! The application.\nuse core::helper;\npub fn run() { helper() }\n",
    );
    assert_eq!(baseline.observe(root, &file).len(), 1, "first sighting");
    assert!(
        baseline.observe(root, &file).is_empty(),
        "the same edge must not be reported twice"
    );
}

/// A closed loop is a different fact from a new edge, and the more important
/// one: a unit inside a ring cannot be lifted out of it alone.
#[test]
fn closing_a_loop_is_reported_as_mutual_dependence() {
    let dir = workspace();
    let root = dir.path();

    // app → core already exists when the session opens.
    rewrite(
        root,
        "app/src/lib.rs",
        "//! The application.\nuse core::helper;\npub fn run() { helper() }\n",
    );
    let mut baseline = artist_tools::skeleton::baseline(root).expect("a baseline");

    // core → app closes the ring.
    let file = rewrite(
        root,
        "core/src/lib.rs",
        "//! Shared primitives.\nuse app::run;\npub fn helper() { run() }\n",
    );
    let notes = baseline.observe(root, &file);

    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].contains("depend on each other"),
        "a cycle should not be reported as a plain new edge: {notes:?}"
    );
}

/// An edit outside every unit has no architecture to report on, and must not
/// panic or invent one.
#[test]
fn an_edit_outside_any_unit_is_silent() {
    let dir = workspace();
    let root = dir.path();
    let mut baseline = artist_tools::skeleton::baseline(root).expect("a baseline");

    let file = rewrite(root, "README.md", "# notes\n");
    assert!(baseline.observe(root, &file).is_empty());
}

/// A project with nothing to describe has no baseline, and the caller must get
/// `None` rather than an empty map it would then report deltas against.
#[test]
fn a_single_unit_project_has_no_architecture() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"solo\"\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "//! Alone.\npub fn x() {}\n").unwrap();

    assert!(artist_tools::skeleton::baseline(root).is_none());
}

/// What a unit says when a session first arrives in it.
///
/// The skeleton names every unit in one line. This is that line expanded, and
/// it is affordable precisely because it fires where the model actually went
/// rather than everywhere it might.
#[test]
fn a_unit_introduces_itself_with_its_role_and_reach() {
    let dir = workspace();
    let root = dir.path();
    rewrite(
        root,
        "app/src/lib.rs",
        "//! The application.\nuse core::helper;\npub fn run() { helper() }\n",
    );
    std::fs::write(root.join("app/src/extra.rs"), "pub fn more() {}\n").unwrap();

    let baseline = artist_tools::skeleton::baseline(root).expect("a baseline");

    let note = baseline
        .region_note(&root.join("core/src/lib.rs"))
        .expect("a note for core");
    assert!(note.contains("entering core"), "{note}");
    assert!(note.contains("Shared primitives"), "role is carried: {note}");
    assert!(
        note.contains("break if it changes"),
        "the number that changes how careful to be: {note}"
    );

    // `app` depends on core rather than the other way round, so it introduces
    // itself with what it uses instead.
    let app = baseline
        .region_note(&root.join("app/src/lib.rs"))
        .expect("a note for app");
    assert!(app.contains("entering app"), "{app}");
    assert!(app.contains("it uses core"), "{app}");
}

#[test]
fn a_file_outside_every_unit_introduces_nothing() {
    let dir = workspace();
    let baseline = artist_tools::skeleton::baseline(dir.path()).expect("a baseline");
    assert!(baseline.region_note(&dir.path().join("README.md")).is_none());
}

/// Recent work is a prior about what the session is for, and it is available
/// before anything has been touched.
#[test]
fn recent_git_activity_points_at_the_units_in_hand() {
    let dir = workspace();
    let root = dir.path();
    // `workspace()` leaves an untracked tree, which is uncommitted by
    // definition — enough for `git status` to name both units.
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    };
    if !run(&["init", "-q"]) {
        return; // no git available; the feature is optional by construction
    }

    let skeleton =
        artist_tools::skeleton::build(root, artist_tools::skeleton::DEFAULT_BUDGET).expect("shape");
    assert!(
        skeleton.active.contains(&"app".to_owned())
            || skeleton.active.contains(&"core".to_owned()),
        "uncommitted work should be named: {:?}",
        skeleton.active
    );
    assert!(skeleton.render().contains("recent work is in:"));
}

/// The property that keeps the introduction from becoming noise: once per unit
/// per session, however many of its files are opened.
///
/// Held on the workspace rather than computed per call, and shared across
/// clones — a subagent working in the same crate must not be introduced to it
/// again.
#[test]
fn a_unit_introduces_itself_only_once_per_session() {
    let dir = workspace();
    let state = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("core/src/extra.rs"), "pub fn more() {}\n").unwrap();

    let workspace =
        artist_tools::Workspace::open(dir.path(), state.path(), "test").expect("workspace");

    let first = workspace.region_note_for_test(&dir.path().join("core/src/lib.rs"));
    assert!(first.is_some(), "the first file in a unit introduces it");

    let second = workspace.region_note_for_test(&dir.path().join("core/src/extra.rs"));
    assert!(
        second.is_none(),
        "a second file in the same unit must say nothing: {second:?}"
    );

    // A different unit is a different region, and still worth a sentence.
    let other = workspace.region_note_for_test(&dir.path().join("app/src/lib.rs"));
    assert!(other.is_some(), "a new unit still introduces itself");

    // A clone is the same session.
    let subagent = workspace.clone();
    assert!(
        subagent
            .region_note_for_test(&dir.path().join("core/src/lib.rs"))
            .is_none(),
        "a cloned workspace shares what has already been entered"
    );
}

/// A unit that disappears invalidates everything the map said about it and
/// everything that depended on it. Missing this is the most consequential
/// silence available.
#[test]
fn a_deleted_unit_is_reported() {
    let dir = workspace();
    let root = dir.path();
    rewrite(
        root,
        "app/src/lib.rs",
        "//! The application.\nuse core::helper;\npub fn run() { helper() }\n",
    );
    let mut baseline = artist_tools::skeleton::baseline(root).expect("a baseline");

    std::fs::remove_dir_all(root.join("core")).unwrap();
    let notes = baseline.observe(root, &root.join("app/src/lib.rs"));

    assert!(
        notes.iter().any(|n| n.contains("core") && n.contains("gone")),
        "{notes:?}"
    );
    // Reported once: the unit is dropped from the baseline as it is announced.
    let again = baseline.observe(root, &root.join("app/src/lib.rs"));
    assert!(
        !again.iter().any(|n| n.contains("gone")),
        "a deletion must not be re-announced: {again:?}"
    );
}

/// A crate created mid-session is part of the architecture the opening map
/// described, and the map never mentioned it.
#[test]
fn a_new_unit_is_reported() {
    let dir = workspace();
    let root = dir.path();
    let mut baseline = artist_tools::skeleton::baseline(root).expect("a baseline");

    std::fs::create_dir_all(root.join("extra/src")).unwrap();
    std::fs::write(
        root.join("extra/src/lib.rs"),
        "//! Something new.\npub fn fresh() {}\n",
    )
    .unwrap();
    let manifest = rewrite(root, "extra/Cargo.toml", "[package]\nname = \"extra\"\n");

    let notes = baseline.observe(root, &manifest);
    assert!(
        notes.iter().any(|n| n.contains("extra") && n.contains("new")),
        "{notes:?}"
    );
    assert!(
        !baseline
            .observe(root, &manifest)
            .iter()
            .any(|n| n.contains("new to the project")),
        "a new unit must not be re-announced"
    );
}

/// Removing the last import of a dependency changes the architecture as surely
/// as adding one, and the frozen map still claims the edge exists.
#[test]
fn a_removed_dependency_is_reported() {
    let dir = workspace();
    let root = dir.path();
    rewrite(
        root,
        "app/src/lib.rs",
        "//! The application.\nuse core::helper;\npub fn run() { helper() }\n",
    );
    let mut baseline = artist_tools::skeleton::baseline(root).expect("a baseline");

    let file = rewrite(
        root,
        "app/src/lib.rs",
        "//! The application.\npub fn run() {}\n",
    );
    let notes = baseline.observe(root, &file);
    assert!(
        notes.iter().any(|n| n.contains("no longer depends on core")),
        "{notes:?}"
    );
}

/// A unit-level edge survives while any file still carries the import. Dropping
/// it from one file of two is not a change to the architecture, and saying so
/// would be a false alarm.
#[test]
fn a_dependency_still_used_elsewhere_in_the_unit_is_not_reported_as_removed() {
    let dir = workspace();
    let root = dir.path();
    rewrite(
        root,
        "app/src/lib.rs",
        "//! The application.\nmod other;\nuse core::helper;\npub fn run() { helper() }\n",
    );
    std::fs::write(
        root.join("app/src/other.rs"),
        "use core::helper;\npub fn again() { helper() }\n",
    )
    .unwrap();
    let mut baseline = artist_tools::skeleton::baseline(root).expect("a baseline");

    // One of the two files stops importing it; the other still does.
    let file = rewrite(
        root,
        "app/src/lib.rs",
        "//! The application.\nmod other;\npub fn run() {}\n",
    );
    let notes = baseline.observe(root, &file);
    assert!(
        !notes.iter().any(|n| n.contains("no longer depends")),
        "the unit still imports core through other.rs: {notes:?}"
    );
}

/// Detection is driven by the fingerprint, not by the list of things that can
/// be described.
///
/// That list is one I wrote, and a list I wrote is one I can be wrong about. So
/// detection never consults it: the derived architecture is hashed, and any
/// movement in that hash is reported. Whether a readable summary exists is a
/// separate question from whether the change is announced at all.
///
/// As it happens the summariser currently covers everything the fingerprint
/// includes, so the "cannot describe it" fallback does not fire here. That is
/// the desirable state, not a reason to drop the fallback — it is what holds
/// the property when the fingerprint grows a field the summariser has not been
/// taught about.
#[test]
fn any_change_to_the_derived_architecture_is_reported() {
    let dir = workspace();
    let root = dir.path();
    let mut baseline = artist_tools::skeleton::baseline(root).expect("a baseline");

    // Reach past the described categories: mutate the recorded architecture
    // directly, so the fingerprint moves without any edge diff explaining it.
    baseline.corrupt_for_test();

    let notes = baseline.observe(root, &root.join("app/src/lib.rs"));
    assert!(
        !notes.is_empty(),
        "a change the summariser cannot describe must not pass in silence"
    );
    assert!(
        notes.iter().any(|n| n.contains("a-unit-that-was-never-here")),
        "and it should describe it where it can: {notes:?}"
    );
}

/// The common case stays free. A fingerprint that moved on every commit would
/// make the channel noise, which is what kills a hook like this.
#[test]
fn an_unrelated_edit_leaves_the_fingerprint_alone() {
    let dir = workspace();
    let root = dir.path();
    let mut baseline = artist_tools::skeleton::baseline(root).expect("a baseline");

    for body in [
        "//! The application.\npub fn run() { let _ = 1; }\n",
        "//! The application.\npub fn run() { let _ = 2; }\npub fn other() {}\n",
    ] {
        let file = rewrite(root, "app/src/lib.rs", body);
        assert!(
            baseline.observe(root, &file).is_empty(),
            "an edit that moves no dependency must say nothing"
        );
    }
}
