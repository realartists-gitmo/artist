#[cfg(target_os = "linux")]
use artist_session_host::{DaemonOptions, ServerPacket, SessionHostDaemon};
use artist_session_host::{
    EventJournal, HostCommand, HostCore, HostEvent, HostRegistry, HostRequest, RuntimeAction,
    RuntimePhase, RuntimeState,
};
#[cfg(target_os = "linux")]
use artist_session_host::{SeqPacket, SeqPacketListener};
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn registry_is_private_exclusive_and_recoverable() {
    let temp = tempfile::tempdir().unwrap();
    let registry = HostRegistry::new(temp.path().join("hosts")).unwrap();
    assert_eq!(
        fs::metadata(registry.root()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let lease = registry.acquire("root-1").unwrap();
    let record = registry.resolve("root-1").unwrap().unwrap();
    assert_eq!(record.token.len(), 32);
    assert!(registry.acquire("root-1").is_err());
    drop(lease);
    assert!(registry.resolve("root-1").unwrap().is_none());
}

#[test]
fn separate_roots_can_be_owned_concurrently() {
    let temp = tempfile::tempdir().unwrap();
    let registry = HostRegistry::new(temp.path().join("hosts")).unwrap();
    let first = registry.acquire("one").unwrap();
    let second = registry.acquire("two").unwrap();
    assert_ne!(first.record().token, second.record().token);
}

#[test]
fn dead_owner_is_removed_and_can_be_reclaimed() {
    let temp = tempfile::tempdir().unwrap();
    let registry = HostRegistry::new(temp.path().join("hosts")).unwrap();
    let record = serde_json::json!({
        "version": 1, "session": "stale", "pid": 4294967294_u32,
        "socket": registry.root().join("stale.sock"), "token": "old", "started_at_ms": 1
    });
    fs::write(
        registry.root().join("stale.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    assert!(registry.resolve("stale").unwrap().is_none());
    assert!(registry.acquire("stale").is_ok());
}

#[test]
fn reconnect_replays_after_ack_and_detects_replay_gap() {
    let mut journal = EventJournal::new(2);
    journal.push(HostEvent::AccessibilityPatch {
        lineage: "main".into(),
        object: "a".into(),
        patch: serde_json::json!({}),
    });
    journal.push(HostEvent::AccessibilityPatch {
        lineage: "main".into(),
        object: "b".into(),
        patch: serde_json::json!({}),
    });
    assert_eq!(journal.after(1).unwrap().len(), 1);
    journal.push(HostEvent::AccessibilityPatch {
        lineage: "main".into(),
        object: "c".into(),
        patch: serde_json::json!({}),
    });
    assert!(
        journal.after(0).is_none(),
        "caller must request a snapshot after a replay gap"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn seqpacket_preserves_messages_and_exposes_peer_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("host.sock");
    let listener = SeqPacketListener::bind(path.clone()).unwrap();
    let thread = std::thread::spawn(move || {
        let peer = listener.accept().unwrap();
        assert_eq!(peer.peer_credentials().unwrap().uid, unsafe {
            libc::geteuid()
        });
        let request: serde_json::Value = peer.receive().unwrap();
        assert_eq!(request, serde_json::json!({"message":"hello"}));
        peer.send(&serde_json::json!({"reply":"world"})).unwrap();
    });
    let client = SeqPacket::connect(&path).unwrap();
    client
        .send(&serde_json::json!({"message":"hello"}))
        .unwrap();
    let response: serde_json::Value = client.receive().unwrap();
    assert_eq!(response, serde_json::json!({"reply":"world"}));
    thread.join().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn seqpacket_transfers_descriptor_ownership_with_the_matching_packet() {
    use std::{io::Read, os::fd::AsRawFd};
    let temp = tempfile::tempdir().unwrap();
    let payload = temp.path().join("buffer");
    fs::write(&payload, b"dma-buf stand-in").unwrap();
    let file = fs::File::open(&payload).unwrap();
    let socket = temp.path().join("host-fd.sock");
    let listener = SeqPacketListener::bind(socket.clone()).unwrap();
    let thread = std::thread::spawn(move || {
        let peer = listener.accept().unwrap();
        peer.send_with_fds(&serde_json::json!({"buffer": 7}), &[file.as_raw_fd()])
            .unwrap();
    });
    let client = SeqPacket::connect(&socket).unwrap();
    let (packet, mut fds): (serde_json::Value, Vec<std::os::fd::OwnedFd>) =
        client.receive_with_fds().unwrap();
    assert_eq!(packet, serde_json::json!({"buffer": 7}));
    assert_eq!(fds.len(), 1);
    let mut received = fs::File::from(fds.pop().unwrap());
    let mut text = String::new();
    received.read_to_string(&mut text).unwrap();
    assert_eq!(text, "dma-buf stand-in");
    thread.join().unwrap();
}

#[test]
fn message_steers_running_agent_while_queue_is_explicit() {
    let mut host = HostCore::new("secret", 8);
    host.publish(HostEvent::RuntimeState(RuntimeState {
        lineage: "main".into(),
        profile: "default".into(),
        state: RuntimePhase::Running,
        queued_turns: 0,
    }));
    let request = |request_id, command| HostRequest {
        version: 1,
        request_id,
        token: "secret".into(),
        command,
    };
    let reply = host.handle(request(
        1,
        HostCommand::Message {
            lineage: "main".into(),
            text: "redirect".into(),
        },
    ));
    assert!(matches!(reply.action, Some(RuntimeAction::Steer { text, .. }) if text == "redirect"));
    let reply = host.handle(request(
        2,
        HostCommand::QueueNextTurn {
            lineage: "main".into(),
            text: "later".into(),
        },
    ));
    assert!(reply.action.is_none());
    assert!(
        matches!(host.take_next_turn("main"), Some(RuntimeAction::Start { text, .. }) if text == "later")
    );
}

#[test]
fn attach_authenticates_and_falls_back_to_snapshot_after_gap() {
    let mut host = HostCore::new("secret", 1);
    host.publish(HostEvent::AccessibilityPatch {
        lineage: "main".into(),
        object: "one".into(),
        patch: serde_json::json!({}),
    });
    host.publish(HostEvent::AccessibilityPatch {
        lineage: "main".into(),
        object: "two".into(),
        patch: serde_json::json!({}),
    });
    let denied = host.handle(HostRequest {
        version: 1,
        request_id: 1,
        token: "wrong".into(),
        command: HostCommand::Attach { after_seq: 0 },
    });
    assert!(denied.response.result.is_err());
    let reply = host.handle(HostRequest {
        version: 1,
        request_id: 2,
        token: "secret".into(),
        command: HostCommand::Attach { after_seq: 0 },
    });
    assert!(
        matches!(reply.replay.as_slice(), [event] if matches!(event.payload, HostEvent::Snapshot { .. }))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn daemon_keeps_run_events_across_client_disconnect_and_replays_them() {
    use std::{os::unix::fs::PermissionsExt, time::Duration};
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("fake-artist");
    fs::write(
        &executable,
        "#!/bin/sh\nsleep 0.15\necho '{\"type\":\"text_delta\",\"data\":\"survived\"}' >&2\nsleep 0.05\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let registry = HostRegistry::new(temp.path().join("hosts")).unwrap();
    let daemon = SessionHostDaemon::bind(
        &registry,
        DaemonOptions {
            session: "root-live".into(),
            project: temp.path().into(),
            executable,
            idle_timeout: Duration::from_secs(2),
        },
    )
    .unwrap();
    let record = registry.resolve("root-live").unwrap().unwrap();
    let thread = std::thread::spawn(move || daemon.run().unwrap());
    let first = SeqPacket::connect(&record.socket).unwrap();
    first
        .send(&HostRequest {
            version: 1,
            request_id: 1,
            token: record.token.clone(),
            command: HostCommand::Message {
                lineage: "main".into(),
                text: "go".into(),
            },
        })
        .unwrap();
    let _: ServerPacket = first.receive().unwrap();
    drop(first);
    std::thread::sleep(Duration::from_millis(350));
    let second = SeqPacket::connect(&record.socket).unwrap();
    second
        .send(&HostRequest {
            version: 1,
            request_id: 2,
            token: record.token,
            command: HostCommand::Attach { after_seq: 0 },
        })
        .unwrap();
    let _: ServerPacket = second.receive().unwrap();
    let mut saw_delta = false;
    for _ in 0..4 {
        let packet: ServerPacket = second.receive().unwrap();
        if matches!(packet, ServerPacket::Event(event) if matches!(event.payload, HostEvent::StreamingDelta { event: artist_ui_core::PromptEvent::TextDelta(ref text), .. } if text == "survived"))
        {
            saw_delta = true;
            break;
        }
    }
    assert!(saw_delta);
    drop(second);
    thread.join().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn one_root_host_runs_distinct_agent_lineages_concurrently() {
    use std::{os::unix::fs::PermissionsExt, time::Duration};
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("fake-artist");
    fs::write(
        &executable,
        "#!/bin/sh\nsleep 0.2\nprintf '{\"type\":\"text_delta\",\"data\":\"%s\"}\\n' \"$6\" >&2\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let registry = HostRegistry::new(temp.path().join("hosts")).unwrap();
    let daemon = SessionHostDaemon::bind(
        &registry,
        DaemonOptions {
            session: "root-parallel".into(),
            project: temp.path().into(),
            executable,
            idle_timeout: Duration::from_secs(2),
        },
    )
    .unwrap();
    let record = registry.resolve("root-parallel").unwrap().unwrap();
    let thread = std::thread::spawn(move || daemon.run().unwrap());
    let client = SeqPacket::connect(&record.socket).unwrap();
    for (request_id, lineage) in [(1, "main"), (2, "main/delegate-child")] {
        client
            .send(&HostRequest {
                version: 1,
                request_id,
                token: record.token.clone(),
                command: HostCommand::Message {
                    lineage: lineage.into(),
                    text: "go".into(),
                },
            })
            .unwrap();
        let _: ServerPacket = client.receive().unwrap();
    }
    drop(client);
    std::thread::sleep(Duration::from_millis(350));
    let replay = SeqPacket::connect(&record.socket).unwrap();
    replay
        .send(&HostRequest {
            version: 1,
            request_id: 3,
            token: record.token,
            command: HostCommand::Attach { after_seq: 0 },
        })
        .unwrap();
    let _: ServerPacket = replay.receive().unwrap();
    let mut lineages = std::collections::BTreeSet::new();
    for _ in 0..6 {
        let packet: ServerPacket = replay.receive().unwrap();
        if let ServerPacket::Event(event) = packet
            && let HostEvent::StreamingDelta {
                event: artist_ui_core::PromptEvent::TextDelta(text),
                ..
            } = event.payload
        {
            lineages.insert(text);
        }
    }
    assert_eq!(
        lineages,
        ["main".to_owned(), "main/delegate-child".to_owned()]
            .into_iter()
            .collect()
    );
    drop(replay);
    thread.join().unwrap();
}
