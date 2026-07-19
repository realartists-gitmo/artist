//! End-to-end tests for the wasm plugin tier, driven against the fixture
//! guest (`tests/fixtures/rule-guest`, built for wasm32-wasip2 with plain
//! cargo). Run with:
//!
//! ```sh
//! cargo test -p artist-rules --features wasm --test wasm_plugin
//! ```
#![cfg(feature = "wasm")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use artist_rules::matcher::StreamMatcher;
use artist_rules::types::RuleId;

/// Build (once per test process) and locate the fixture guest component. The
/// `OnceLock` serializes the build so parallel tests can't race concurrent
/// `cargo build` invocations on the same target dir.
fn guest_wasm() -> PathBuf {
    static ARTIFACT: OnceLock<PathBuf> = OnceLock::new();
    ARTIFACT
        .get_or_init(|| {
            let fixture =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rule-guest");
            let artifact = fixture.join("target/wasm32-wasip2/release/rule_guest.wasm");
            if !artifact.exists() {
                let status = std::process::Command::new("cargo")
                    .args(["build", "--release", "--target", "wasm32-wasip2"])
                    .current_dir(&fixture)
                    .status()
                    .expect("run cargo for the fixture guest");
                assert!(
                    status.success(),
                    "fixture guest build failed — install the target with \
                     `rustup target add wasm32-wasip2`"
                );
            }
            artifact
        })
        .clone()
}

/// A rules dir containing the third-strike plugin + manifest.
fn rules_dir(temp: &Path) -> PathBuf {
    let dir = temp.join("rules");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(guest_wasm(), dir.join("third-strike.wasm")).unwrap();
    std::fs::write(
        dir.join("third-strike.toml"),
        r#"description = "Fires on the third strike-zone mention (stateful)"
prefilter = ['strike zone']
targets = ["assistant-text"]
fire = "per-turn"
"#,
    )
    .unwrap();
    dir
}

fn discover(dir: &Path) -> (Arc<artist_rules::matcher::RuleSet>, Vec<String>) {
    let mut diagnostics = Vec::new();
    let (rules, wasm) = artist_rules::discovery::discover_all(&[dir.to_owned()], &mut diagnostics);
    (
        Arc::new(artist_rules::matcher::RuleSet::compile(rules).with_wasm(wasm)),
        diagnostics,
    )
}

#[test]
fn stateful_plugin_fires_on_third_prefilter_hit() {
    let temp = tempfile::tempdir().unwrap();
    let (rules, diagnostics) = discover(&rules_dir(temp.path()));
    assert!(diagnostics.is_empty(), "diagnostics: {diagnostics:?}");
    let id = RuleId("wasm:third-strike".into());
    assert!(rules.get(&id).is_some(), "wasm rule registered");

    let mut matcher = StreamMatcher::new(Arc::clone(&rules));
    let armed = |_: &RuleId| true;
    for attempt in 1..=3u32 {
        let firing = matcher
            .push_text("into the strike zone again\n", &armed)
            .expect("prefilter hits every time");
        matcher.reset_turn();
        let verdict = rules.verdict(firing, attempt);
        if attempt < 3 {
            assert!(verdict.is_none(), "strike {attempt} must pass");
        } else {
            let firing = verdict.expect("third strike fires");
            assert_eq!(firing.rule, id);
            assert!(firing.reminder.contains("Third strike"));
        }
    }
}

#[test]
fn infinite_loop_traps_on_deadline_and_poisons() {
    let temp = tempfile::tempdir().unwrap();
    let (rules, _) = discover(&rules_dir(temp.path()));
    let id = RuleId("wasm:third-strike".into());
    let firing = artist_rules::types::Firing {
        rule: id.clone(),
        target: artist_rules::types::MatchTarget::AssistantText,
        tool: None,
        matched: "INFINITE_LOOP strike zone".into(),
        reminder: String::new(),
        persistence: Default::default(),
        fire: Default::default(),
    };
    let started = std::time::Instant::now();
    assert!(rules.verdict(firing.clone(), 1).is_none());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "epoch deadline must interrupt the guest promptly"
    );
    assert_eq!(rules.poisoned(), vec![id]);
    // Poisoned: subsequent hits are suppressed without calling the guest.
    assert!(rules.verdict(firing, 2).is_none());
}

#[test]
fn memory_bomb_hits_the_store_limit_and_poisons() {
    let temp = tempfile::tempdir().unwrap();
    let (rules, _) = discover(&rules_dir(temp.path()));
    let id = RuleId("wasm:third-strike".into());
    let firing = artist_rules::types::Firing {
        rule: id.clone(),
        target: artist_rules::types::MatchTarget::AssistantText,
        tool: None,
        matched: "MEMORY_BOMB strike zone".into(),
        reminder: String::new(),
        persistence: Default::default(),
        fire: Default::default(),
    };
    assert!(rules.verdict(firing, 1).is_none());
    assert_eq!(rules.poisoned(), vec![id]);
}

#[test]
fn mismatched_manifest_name_is_a_diagnostic() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("rules");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(guest_wasm(), dir.join("wrong-name.wasm")).unwrap();
    std::fs::write(
        dir.join("wrong-name.toml"),
        "description = \"d\"\nprefilter = ['x']\n",
    )
    .unwrap();
    let (rules, diagnostics) = discover(&dir);
    assert!(rules.get(&RuleId("wasm:wrong-name".into())).is_none());
    assert!(
        diagnostics
            .iter()
            .any(|line| line.contains("third-strike") && line.contains("wrong-name")),
        "diagnostics: {diagnostics:?}"
    );
}
