//! Canvases two people can be in at once.
//!
//! A shared canvas is Workbench with a second seat, and it is the one form of
//! sharing that does not need anybody to run a service. Two artist processes
//! dial each other by public key, hole-punch a direct QUIC connection, and
//! replicate the canvas between them. Discovery goes through pkarr on the
//! BitTorrent mainline DHT — a commons whose participants *are* the
//! infrastructure — with a relay only as the fallback for the roughly one
//! network in ten where hole punching fails.
//!
//! # Why artist does the networking and the page does not
//!
//! A browser cannot hole punch. The sandbox forbids sending UDP to an arbitrary
//! address, so iroh's browser build relays *every* connection, permanently,
//! which would put someone's server back in the middle of the thing that exists
//! to avoid one. Artist is native and has a socket, so the traversal happens
//! here and the page never leaves loopback:
//!
//! ```text
//! page → loopback → artist ⇄ hole-punched QUIC ⇄ artist → loopback → page
//! ```
//!
//! # What crosses the wire, and what cannot
//!
//! Files and state. That is the entire protocol, and the omission is the
//! security model: there is no message that invokes a tool, sends a prompt, or
//! answers a question, so a peer's page cannot reach the host's harness because
//! there is nothing for it to reach through. A peer renders the canvas in their
//! own artist, against their own agent, under their own permissions — which is
//! what a remote principal with an empty allow-set means in practice.
//!
//! Code flows one way. The host owns the source and peers receive it; a peer
//! editing files that the host's model is also editing would need real
//! conflict resolution over source text, and there is no reason to want it —
//! the model doing the writing lives on one machine. State flows both ways,
//! because it is already revision-stamped and coordination-free.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The protocol this speaks, versioned because a shared canvas outlives the
/// session that started it and two artists may be different builds.
pub const ALPN: &[u8] = b"artist/canvas/1";

/// How much of a canvas will be sent. Generous for source, small enough that a
/// peer cannot be made to buffer something enormous.
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;

/// How many files one canvas may share.
const MAX_FILES: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error("no canvas named `{0}`")]
    Unknown(String),
    #[error("this ticket is not one of ours: {0}")]
    BadTicket(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Protocol(String),
}

/// What a host hands out so someone can join.
///
/// Text rather than an opaque blob: it goes in a chat message, and being able
/// to see that it names an endpoint and a canvas and nothing else is worth more
/// than the few bytes an encoding would save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub endpoint: iroh::EndpointId,
    pub slug: String,
}

impl std::fmt::Display for Ticket {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "artist:{}/{}", self.endpoint, self.slug)
    }
}

impl std::str::FromStr for Ticket {
    type Err = PeerError;

    fn from_str(value: &str) -> Result<Self, PeerError> {
        let rest = value
            .trim()
            .strip_prefix("artist:")
            .ok_or_else(|| PeerError::BadTicket(value.to_owned()))?;
        let (endpoint, slug) = rest
            .split_once('/')
            .ok_or_else(|| PeerError::BadTicket(value.to_owned()))?;
        let endpoint = endpoint
            .parse()
            .map_err(|_| PeerError::BadTicket(value.to_owned()))?;
        if slug.is_empty() || !crate::registry::is_slug(slug) {
            return Err(PeerError::BadTicket(value.to_owned()));
        }
        Ok(Ticket {
            endpoint,
            slug: slug.to_owned(),
        })
    }
}

/// This machine's identity, generated once and kept.
///
/// Per machine rather than per project, which is a real trade and worth naming.
/// A ticket is a public key, so a stable key is what lets one you handed out
/// last week still work today — and that durability is most of why sharing is
/// worth having. The cost is that the same key identifies you across every
/// project you share from.
pub fn identity() -> Result<iroh::SecretKey, PeerError> {
    let path = identity_path();
    if let Ok(bytes) = std::fs::read(&path)
        && let Ok(bytes) = <[u8; 32]>::try_from(bytes.as_slice())
    {
        return Ok(iroh::SecretKey::from_bytes(&bytes));
    }

    let key = iroh::SecretKey::generate();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, key.to_bytes())?;
    // A private key readable by every process on the machine is a private key
    // in name only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(key)
}

fn identity_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("artist")
        .join("node.key")
}

/// One message. Framed as a JSON line, because the volume here is a canvas's
/// source and its state, not a stream where a binary encoding would earn its
/// complexity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Message {
    /// Peer to host, opening the exchange.
    Join { slug: String },
    /// Host to peer: the canvas itself. Code only ever travels this way.
    Canvas {
        slug: String,
        title: String,
        manifest: String,
        files: Vec<(String, String)>,
    },
    /// Either direction: shared state moved.
    ///
    /// Whole entries, not values. Each carries the revision and the writer it
    /// was stamped with, which is what lets the far side order it against its
    /// own history — send bare values and the receiver has to invent an
    /// ordering, which is where writes get lost.
    State {
        entries: std::collections::BTreeMap<String, crate::state::Entry>,
    },
    /// Host to peer, when the ticket names something that is not there.
    Refused { reason: String },
}

/// Everything a host will send for `slug`.
///
/// Separated from the transport so the decision about *what* is shareable can
/// be tested without a network: exactly the canvas's own directory, no
/// `exports/`, no `.window.json`, nothing beginning with a dot.
pub fn shareable(project: &Path, slug: &str) -> Result<Message, PeerError> {
    let registry = crate::Registry::discover(project);
    let canvas = registry
        .get(slug)
        .ok_or_else(|| PeerError::Unknown(slug.to_owned()))?;

    let mut files = Vec::new();
    collect(&canvas.root, &canvas.root, &mut files)?;
    files.sort();
    files.truncate(MAX_FILES);

    Ok(Message::Canvas {
        slug: slug.to_owned(),
        title: canvas.manifest.title.clone(),
        manifest: canvas.manifest.render(),
        files,
    })
}

/// Walk a canvas's own files, skipping what is not source.
fn collect(root: &Path, at: &Path, out: &mut Vec<(String, String)>) -> Result<(), PeerError> {
    for entry in std::fs::read_dir(at)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // Dotfiles are artist's own bookkeeping — remembered window geometry,
        // and whatever later adds itself beside it. `exports/` is derived and
        // would multiply the payload by every export ever taken.
        if name.starts_with('.') || name == "exports" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out)?;
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            // Binary assets are skipped rather than mangled. A canvas leaning
            // on one will look wrong to the peer, which is better than a peer
            // that fails to open at all.
            continue;
        };
        if text.len() <= MAX_FILE_BYTES {
            out.push((relative.to_string_lossy().replace('\\', "/"), text));
        }
    }
    Ok(())
}

/// Write a received canvas into a peer's project.
///
/// Under a name that says where it came from, so joining someone's canvas does
/// not quietly overwrite one of yours that happens to share a slug — and so the
/// user can tell, a week later, which canvases are theirs.
pub fn receive(project: &Path, message: &Message) -> Result<String, PeerError> {
    let Message::Canvas {
        slug,
        manifest,
        files,
        ..
    } = message
    else {
        return Err(PeerError::Protocol(
            "expected the canvas, got something else".into(),
        ));
    };

    let local = format!("shared-{slug}");
    let root = project.join(crate::registry::CANVAS_DIR).join(&local);
    std::fs::create_dir_all(&root)?;
    std::fs::write(root.join(crate::registry::MANIFEST_FILE), manifest)?;

    for (name, contents) in files {
        // The same jail the server uses for module requests. A peer is not
        // trusted to name a path: `../../.ssh/authorized_keys` arriving over
        // the wire has to land nowhere.
        let Some(target) = crate::server::resolve_within(&root, name) else {
            return Err(PeerError::Protocol(format!(
                "`{name}` points outside the canvas"
            )));
        };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, contents)?;
    }
    Ok(local)
}

/// A canvas this machine is sharing.
///
/// Holds the endpoint open for as long as the share lasts. Dropping it stops
/// answering, which is the only way to stop: there is no revocation list
/// because there is nothing persistent to revoke — a ticket is only useful
/// while the process that answers it is running.
pub struct Share {
    endpoint: iroh::Endpoint,
    pub ticket: Ticket,
    /// Peers currently connected, for presence.
    joined: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Share {
    /// Start answering for `slug`.
    pub async fn start(project: PathBuf, slug: String) -> Result<Self, PeerError> {
        // Fail before binding a socket if there is nothing to share: a ticket
        // for a canvas that does not exist is worse than no ticket, because it
        // fails on the other person's machine.
        let _ = shareable(&project, &slug)?;

        let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(identity()?)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .map_err(|error| PeerError::Protocol(format!("could not bind an endpoint: {error}")))?;

        let ticket = Ticket {
            endpoint: endpoint.id(),
            slug: slug.clone(),
        };
        let joined = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let accepting = endpoint.clone();
        let present = std::sync::Arc::clone(&joined);
        tokio::spawn(async move {
            while let Some(incoming) = accepting.accept().await {
                let project = project.clone();
                let slug = slug.clone();
                let present = std::sync::Arc::clone(&present);
                // One task per peer: a peer that stalls mid-transfer must not
                // stop anyone else joining.
                tokio::spawn(async move {
                    if let Ok(connection) = incoming.await {
                        let who = connection.remote_id().to_string();
                        present.lock().expect("presence lock").push(who.clone());
                        let _ = serve_peer(&project, &slug, &connection).await;
                        present
                            .lock()
                            .expect("presence lock")
                            .retain(|other| other != &who);
                    }
                });
            }
        });

        Ok(Share {
            endpoint,
            ticket,
            joined,
        })
    }

    /// Who is looking at this canvas right now.
    pub fn peers(&self) -> Vec<String> {
        self.joined.lock().expect("presence lock").clone()
    }

    /// This share's endpoint including the addresses it is reachable at.
    ///
    /// A ticket carries only a public key, which is the point — you dial a key
    /// and discovery finds the machine. But discovery is a DHT and a DNS
    /// lookup: it is asynchronous, it needs the network, and immediately after
    /// binding there is nothing published yet. Anything that needs to connect
    /// *now*, without waiting on the internet to agree, uses this.
    pub fn addr(&self) -> iroh::EndpointAddr {
        self.endpoint.addr()
    }
}

impl Drop for Share {
    fn drop(&mut self) {
        // `close` is a future, and a future dropped un-awaited does nothing —
        // the share would have stayed answering until the process exited. It
        // gets its own task because `Drop` cannot await, and closing politely
        // is worth a task: it tells connected peers the share is over instead
        // of leaving them on a socket that has stopped replying.
        let endpoint = self.endpoint.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move { endpoint.close().await });
        }
    }
}

/// Answer one peer for the life of its connection.
async fn serve_peer(
    project: &Path,
    slug: &str,
    connection: &iroh::endpoint::Connection,
) -> Result<(), PeerError> {
    let (mut send, mut recv) = connection
        .accept_bi()
        .await
        .map_err(|error| PeerError::Protocol(error.to_string()))?;

    let asked = read_message(&mut recv).await?;
    // The slug is the peer's, so it is checked against the one being shared
    // rather than trusted. Without this a ticket for one canvas would read
    // any canvas in the project.
    match asked {
        Message::Join { slug: wanted } if wanted == slug => {}
        Message::Join { .. } => {
            write_message(
                &mut send,
                &Message::Refused {
                    reason: "that ticket is for a different canvas".into(),
                },
            )
            .await?;
            return Ok(());
        }
        _ => return Err(PeerError::Protocol("expected a join".into())),
    }

    write_message(&mut send, &shareable(project, slug)?).await?;

    let store = crate::StateStore::open(&project.join(crate::registry::CANVAS_DIR).join(slug));
    write_message(
        &mut send,
        &Message::State {
            entries: store.snapshot().entries,
        },
    )
    .await?;

    // State flows both ways from here. Code does not: nothing in this loop
    // writes a file inside the canvas, so a peer cannot edit the host's source.
    //
    // `absorb` rather than `merge`: these entries were stamped on the peer's
    // machine and keep their own revision and writer, so each is ordered per
    // key against what is already here rather than restamped as a local write.
    // The previous version compared the peer's document revision against this
    // one's and dropped the whole update if it was behind — two unrelated
    // counters, so whichever side had written less quietly lost everything.
    while let Ok(Message::State { entries }) = read_message(&mut recv).await {
        let _ = store.absorb(entries);
    }
    Ok(())
}

/// Join a shared canvas by ticket, finding the host through discovery.
///
/// This is the one people use: a ticket is a public key, and pkarr on the
/// mainline DHT turns it into an address without anyone operating a directory.
pub async fn join(project: &Path, ticket: &Ticket) -> Result<String, PeerError> {
    join_at(
        project,
        iroh::EndpointAddr::new(ticket.endpoint),
        &ticket.slug,
    )
    .await
}

/// Join a host whose address is already known.
///
/// Skips discovery, which matters for two cases that have nothing to do with
/// each other: a test that must not depend on n0's DNS being reachable and
/// fast, and a peer on the same machine or LAN where waiting for a DHT to
/// propagate would be absurd.
pub async fn join_at(
    project: &Path,
    addr: iroh::EndpointAddr,
    slug: &str,
) -> Result<String, PeerError> {
    let ticket = Ticket {
        endpoint: addr.id,
        slug: slug.to_owned(),
    };
    join_inner(project, addr, &ticket).await
}

async fn join_inner(
    project: &Path,
    addr: iroh::EndpointAddr,
    ticket: &Ticket,
) -> Result<String, PeerError> {
    // A fresh key per join, deliberately, and not the machine's.
    //
    // Identity exists so someone can be *dialled*, and nobody dials a joiner —
    // its endpoint appears in no ticket. Using the machine key here bought
    // nothing and cost two things. It told every host you connect to which
    // machine you are, permanently and across projects, for no benefit. And
    // because both sides load the same file, two artists on one machine had
    // the same endpoint id and iroh refused the connection as self-dialling:
    // sharing a canvas between two of your own sessions was impossible.
    let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .secret_key(iroh::SecretKey::generate())
        .bind()
        .await
        .map_err(|error| PeerError::Protocol(format!("could not bind an endpoint: {error}")))?;

    let connection = endpoint
        .connect(addr, ALPN)
        .await
        .map_err(|error| PeerError::Protocol(format!("could not reach that peer: {error}")))?;
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|error| PeerError::Protocol(error.to_string()))?;

    write_message(
        &mut send,
        &Message::Join {
            slug: ticket.slug.clone(),
        },
    )
    .await?;

    let local = match read_message(&mut recv).await? {
        message @ Message::Canvas { .. } => receive(project, &message)?,
        Message::Refused { reason } => return Err(PeerError::Protocol(reason)),
        _ => return Err(PeerError::Protocol("expected the canvas".into())),
    };

    // Absorbed with the host's stamps intact. Restamping them as local writes
    // would make this machine claim authorship of every key it received, and
    // the next exchange would then look like the peer had overwritten the host.
    if let Ok(Message::State { entries }) = read_message(&mut recv).await {
        let store =
            crate::StateStore::open(&project.join(crate::registry::CANVAS_DIR).join(&local));
        let _ = store.absorb(entries);
    }

    // Awaited, unlike the version of this line that sat here doing nothing:
    // `close` is a future, and a joiner that returns without closing leaves the
    // host holding a connection to a peer that has stopped listening.
    endpoint.close().await;
    Ok(local)
}

/// Messages are newline-framed JSON, capped so a peer cannot make the other
/// side buffer without limit.
async fn write_message(
    send: &mut iroh::endpoint::SendStream,
    message: &Message,
) -> Result<(), PeerError> {
    let mut line =
        serde_json::to_vec(message).map_err(|error| PeerError::Protocol(error.to_string()))?;
    line.push(b'\n');
    send.write_all(&line)
        .await
        .map_err(|error| PeerError::Protocol(error.to_string()))?;
    Ok(())
}

async fn read_message(recv: &mut iroh::endpoint::RecvStream) -> Result<Message, PeerError> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while line.len() < MAX_FILES * MAX_FILE_BYTES {
        match recv.read_exact(&mut byte).await {
            Ok(()) if byte[0] == b'\n' => {
                return serde_json::from_slice(&line)
                    .map_err(|error| PeerError::Protocol(error.to_string()));
            }
            Ok(()) => line.push(byte[0]),
            Err(error) => return Err(PeerError::Protocol(error.to_string())),
        }
    }
    Err(PeerError::Protocol(
        "a peer sent an oversized message".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_round_trips_through_its_text() {
        let key = iroh::SecretKey::generate();
        let ticket = Ticket {
            endpoint: key.public(),
            slug: "sales-report".to_owned(),
        };
        let text = ticket.to_string();
        assert!(text.starts_with("artist:"), "{text}");
        assert!(text.ends_with("/sales-report"), "{text}");
        assert_eq!(text.parse::<Ticket>().expect("round trip"), ticket);
    }

    /// A ticket arrives by being pasted, so every way of pasting it wrong has
    /// to be a refusal rather than a confusing failure later on.
    #[test]
    fn a_malformed_ticket_is_refused() {
        for bad in [
            "",
            "artist:",
            "artist:notakey/demo",
            "https://example.com/demo",
            // A slug is a path segment on the receiving side.
            "artist:aaaa/../../etc",
        ] {
            assert!(
                bad.parse::<Ticket>().is_err(),
                "`{bad}` should not parse as a ticket"
            );
        }
    }

    /// Artist's own bookkeeping is not part of the canvas, and `exports/` is
    /// derived — a canvas shared after ten exports would otherwise send all ten.
    #[test]
    fn sharing_sends_the_source_and_nothing_else() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path().join(".artist/canvas/demo");
        std::fs::create_dir_all(root.join("exports")).expect("dirs");
        std::fs::write(root.join("canvas.toml"), "title = \"Demo\"").expect("manifest");
        std::fs::write(root.join("main.jsx"), "export default () => null;").expect("entry");
        std::fs::write(root.join(".window.json"), "{}").expect("geometry");
        std::fs::write(root.join("exports/old.html"), "<html></html>").expect("export");

        let Message::Canvas { files, .. } = shareable(project.path(), "demo").expect("shareable")
        else {
            panic!("expected a canvas");
        };
        let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
        assert!(names.contains(&"main.jsx"), "{names:?}");
        assert!(
            !names.iter().any(|name| name.contains("window")),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|name| name.contains("exports")),
            "{names:?}"
        );
    }

    /// A peer names the paths it sends, so it is exactly as trustworthy as a
    /// browser naming a module — which is to say not at all.
    #[test]
    fn a_peer_cannot_write_outside_the_canvas_it_shared() {
        let project = tempfile::tempdir().expect("tempdir");
        let message = Message::Canvas {
            slug: "demo".to_owned(),
            title: "Demo".to_owned(),
            manifest: "title = \"Demo\"".to_owned(),
            files: vec![("../../../../escaped.js".to_owned(), "pwned".to_owned())],
        };

        let refused = receive(project.path(), &message);
        assert!(
            matches!(refused, Err(PeerError::Protocol(_))),
            "a traversing path was accepted: {refused:?}"
        );
        assert!(
            !project.path().join("../escaped.js").exists(),
            "a file landed outside the project"
        );
    }

    /// Joining must not silently replace a canvas of the user's own that
    /// happens to share a name.
    #[test]
    fn a_received_canvas_lands_under_a_name_that_says_where_it_came_from() {
        let project = tempfile::tempdir().expect("tempdir");
        let message = Message::Canvas {
            slug: "demo".to_owned(),
            title: "Demo".to_owned(),
            manifest: "title = \"Demo\"".to_owned(),
            files: vec![(
                "main.jsx".to_owned(),
                "export default () => null;".to_owned(),
            )],
        };

        let local = receive(project.path(), &message).expect("receive");
        assert_eq!(local, "shared-demo");
        assert!(
            project
                .path()
                .join(".artist/canvas/shared-demo/main.jsx")
                .is_file()
        );
    }

    /// The identity has to survive a restart or every ticket ever handed out
    /// stops working, which is most of the point of a ticket.
    #[test]
    fn the_machine_keeps_one_identity() {
        let first = identity().expect("identity");
        let again = identity().expect("identity again");
        assert_eq!(first.public(), again.public());
    }

    /// Two artists on one machine, which is the same code path as two on
    /// different machines — the only difference is whether hole punching has
    /// anything to punch through.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_canvas_travels_between_two_artists() {
        let host = tempfile::tempdir().expect("host project");
        let peer = tempfile::tempdir().expect("peer project");

        let root = host.path().join(".artist/canvas/demo");
        std::fs::create_dir_all(&root).expect("dirs");
        std::fs::write(root.join("canvas.toml"), "title = \"Demo\"").expect("manifest");
        std::fs::write(root.join("main.jsx"), "export default () => null;").expect("entry");
        crate::StateStore::open(&root)
            .set("rows", serde_json::json!(42))
            .expect("state");

        let share = Share::start(host.path().to_owned(), "demo".to_owned())
            .await
            .expect("share starts");

        // Addressed directly rather than by ticket: dialling a bare key needs
        // pkarr to have published and DNS to have answered, which is the
        // internet's business and not this test's. Everything past the dial is
        // identical either way.
        let local = join_at(peer.path(), share.addr(), "demo")
            .await
            .expect("join");
        assert_eq!(local, "shared-demo");

        // The source arrived...
        let landed = peer.path().join(".artist/canvas/shared-demo/main.jsx");
        assert_eq!(
            std::fs::read_to_string(&landed).expect("entry"),
            "export default () => null;"
        );
        // ...and so did what the canvas was showing.
        let mirrored =
            crate::StateStore::open(&peer.path().join(".artist/canvas/shared-demo")).snapshot();
        assert_eq!(mirrored.plain().get("rows"), Some(&serde_json::json!(42)));
    }

    /// A ticket names one canvas. Holding one must not be a way to read the
    /// rest of someone's project.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_ticket_opens_only_the_canvas_it_names() {
        let host = tempfile::tempdir().expect("host project");
        let peer = tempfile::tempdir().expect("peer project");

        for slug in ["shared", "private"] {
            let root = host.path().join(".artist/canvas").join(slug);
            std::fs::create_dir_all(&root).expect("dirs");
            std::fs::write(root.join("canvas.toml"), "title = \"x\"").expect("manifest");
            std::fs::write(root.join("main.jsx"), format!("// {slug}")).expect("entry");
        }

        let share = Share::start(host.path().to_owned(), "shared".to_owned())
            .await
            .expect("share starts");

        // The honest ticket first. Without this the test would pass just as
        // well against a transport that never worked at all — an assertion
        // that something fails is only worth having next to one that succeeds.
        assert_eq!(
            join_at(peer.path(), share.addr(), "shared")
                .await
                .expect("the real ticket"),
            "shared-shared"
        );

        // The same endpoint, asked for the canvas the ticket does not name.
        let refused = join_at(peer.path(), share.addr(), "private").await;
        assert!(
            refused.is_err(),
            "a ticket for `shared` opened `private`: {refused:?}"
        );
        assert!(
            !peer.path().join(".artist/canvas/shared-private").exists(),
            "the other canvas landed anyway"
        );
    }
}
