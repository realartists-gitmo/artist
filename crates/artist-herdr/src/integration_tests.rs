#![cfg(unix)]

use crate::{HerdrContext, HerdrIntegration, command::CommandRunner};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Fake {
    _dir: TempDir,
    log: PathBuf,
    context: HerdrContext,
}

fn fake(body: &str) -> Fake {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("calls.log");
    let binary = dir.path().join("herdr");
    fs::write(
        &binary,
        format!(
            "#!/bin/sh\n{body}\nprintf '%s\\n' \"$*\" >> '{}'\n",
            log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let context = HerdrContext {
        pane_id: Arc::from("9-9"),
        binary: Arc::new(binary),
        socket_path: None,
    };
    Fake {
        _dir: dir,
        log,
        context,
    }
}

fn lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

async fn wait_for(path: &Path, predicate: impl Fn(&[String]) -> bool) -> Vec<String> {
    for _ in 0..300 {
        let current = lines(path);
        if predicate(&current) {
            return current;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for fake Herdr calls: {:?}", lines(path));
}

#[tokio::test]
async fn preserves_rapid_distinct_transitions_in_sequence() {
    let fake = fake("");
    let integration = HerdrIntegration::start(fake.context);
    let handle = integration.handle();
    handle.report_session("session-1");
    handle.claim_idle();
    let turn = handle.start_turn();
    for _ in 0..10 {
        handle.set_blocked("question", true);
        handle.set_blocked("question", false);
    }
    turn.finish();

    let calls = wait_for(&fake.log, |calls| {
        calls
            .iter()
            .filter(|line| line.split_whitespace().nth(1) == Some("report-agent"))
            .count()
            == 23
    })
    .await;
    for _ in 0..100 {
        handle.claim_idle();
    }
    integration.shutdown().await;

    assert!(calls[0].contains("pane report-agent-session 9-9"));
    assert!(calls[0].contains("--source artist:extension --agent artist"));
    assert!(calls[0].contains("--agent-session-id session-1"));
    let states = calls
        .iter()
        .filter(|line| line.split_whitespace().nth(1) == Some("report-agent"))
        .map(|line| {
            let parts = line.split_whitespace().collect::<Vec<_>>();
            parts[parts.iter().position(|part| *part == "--state").unwrap() + 1]
        })
        .collect::<Vec<_>>();
    let mut expected = vec!["idle", "working"];
    for _ in 0..10 {
        expected.extend(["blocked", "working"]);
    }
    expected.push("idle");
    assert_eq!(states, expected);
    let sequences = lines(&fake.log)
        .iter()
        .map(|line| {
            let parts = line.split_whitespace().collect::<Vec<_>>();
            parts[parts.iter().position(|part| *part == "--seq").unwrap() + 1]
                .parse::<u64>()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(
        lines(&fake.log)
            .last()
            .unwrap()
            .contains("pane release-agent 9-9")
    );
}

#[tokio::test]
async fn retries_after_cli_failure_and_recovers() {
    let fake = fake("if [ -f \"$0.fail\" ]; then exit 1; fi");
    let marker = PathBuf::from(format!("{}.fail", fake.context.binary().display()));
    fs::write(&marker, "fail").unwrap();
    let integration = HerdrIntegration::start(fake.context);
    integration.handle().claim_idle();
    tokio::time::sleep(Duration::from_millis(50)).await;
    fs::remove_file(marker).unwrap();
    wait_for(&fake.log, |calls| {
        calls.iter().any(|line| line.contains("--state idle"))
    })
    .await;
    integration.shutdown().await;
}

#[tokio::test]
async fn release_retries_before_shutdown_returns() {
    let fake = fake(
        "case \"$*\" in *release-agent*) n=$(cat \"$0.count\" 2>/dev/null || echo 0); n=$((n+1)); echo $n > \"$0.count\"; [ $n -lt 3 ] && exit 1;; esac",
    );
    let count = PathBuf::from(format!("{}.count", fake.context.binary().display()));
    let integration = HerdrIntegration::start(fake.context);
    integration.handle().claim_idle();
    integration.shutdown().await;
    assert_eq!(fs::read_to_string(count).unwrap().trim(), "3");
    assert!(
        lines(&fake.log)
            .last()
            .unwrap()
            .contains("pane release-agent 9-9")
    );
}

#[tokio::test]
async fn command_timeout_and_missing_cli_are_nonfatal() {
    let fake = fake("sleep 2");
    let runner = CommandRunner::with_timeout(fake.context, Duration::from_millis(25));
    let integration = HerdrIntegration::start_with_runner(runner);
    integration.handle().claim_idle();
    let started = Instant::now();
    integration.shutdown().await;
    assert!(started.elapsed() < Duration::from_secs(1));

    let dir = tempfile::tempdir().unwrap();
    let context = HerdrContext {
        pane_id: Arc::from("9-9"),
        binary: Arc::new(dir.path().join("missing-herdr")),
        socket_path: None,
    };
    let integration = HerdrIntegration::start(context);
    integration.handle().claim_idle();
    integration.shutdown().await;
}
