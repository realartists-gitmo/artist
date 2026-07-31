//! The local server a canvas is served from.
//!
//! One server per `artist` process, bound to loopback on an ephemeral port,
//! with every canvas multiplexed underneath it.
//!
//! # Why the session key is a path segment
//!
//! Canvas URLs look like `/c/<key>/<slug>/`. Putting the key in the path rather
//! than a query parameter means every relative import the page makes — `./App`,
//! `./components/Chart` — inherits it for free. A query parameter would be
//! dropped by the module resolver on the first relative specifier, and a cookie
//! would be sent by any other page on loopback, which is exactly what the key
//! exists to prevent.

use std::{
    collections::VecDeque,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    extract::{Path as UrlPath, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures::StreamExt as _;
use rand::{RngExt as _, rngs::ThreadRng};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::{
    assets,
    registry::Registry,
    transform::{self, Options},
};

/// How much of a canvas's own noise to keep for `canvas status`. The model
/// reads this; an unbounded buffer would eventually be the whole context.
const REPORT_CAPACITY: usize = 200;

/// Anything the page wants the harness to know.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub slug: String,
    pub level: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

#[derive(Clone, Debug)]
enum Signal {
    Reload { slug: String },
}

struct Inner {
    project: PathBuf,
    key: String,
    addr: SocketAddr,
    signals: broadcast::Sender<Signal>,
    reports: Mutex<VecDeque<Report>>,
}

/// A running canvas server.
#[derive(Clone)]
pub struct Server {
    inner: Arc<Inner>,
    addr: SocketAddr,
}

impl Server {
    /// Bind and start serving. Returns once the port is known, so a caller can
    /// hand out a URL immediately without racing the accept loop.
    pub async fn start(project: PathBuf) -> anyhow::Result<Self> {
        let key = session_key(&mut rand::rng());
        let (signals, _) = broadcast::channel(64);
        // Bind first: the origin check compares against our own port, so the
        // address has to be known before anything can serve a request.
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let inner = Arc::new(Inner {
            project,
            key,
            addr,
            signals,
            reports: Mutex::new(VecDeque::new()),
        });

        let router = Router::new()
            .route("/c/{key}/{slug}/", get(serve_shell))
            .route("/c/{key}/{slug}/{*path}", get(serve_module))
            .route("/@artist/client.js", get(serve_client))
            .route("/@vendor/{*path}", get(serve_vendor))
            .route("/_artist/events", get(serve_events))
            .route("/_artist/rpc", post(serve_rpc))
            .with_state(Arc::clone(&inner));

        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        let server = Server { inner, addr };
        server.spawn_watcher();
        Ok(server)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The URL to hand the user for one canvas.
    pub fn url(&self, slug: &str) -> String {
        format!("http://{}/c/{}/{}/", self.addr, self.inner.key, slug)
    }

    /// Everything the page has reported since the last drain.
    pub fn take_reports(&self, slug: Option<&str>) -> Vec<Report> {
        let mut reports = self.inner.reports.lock().expect("report lock poisoned");
        match slug {
            None => reports.drain(..).collect(),
            Some(slug) => {
                let (mine, theirs): (VecDeque<_>, VecDeque<_>) =
                    reports.drain(..).partition(|report| report.slug == slug);
                *reports = theirs;
                mine.into()
            }
        }
    }

    /// Watch the canvas tree and tell open pages to reload.
    ///
    /// One watcher covers every canvas: they share a parent directory, and a
    /// watcher per canvas would mean re-registering on every create.
    fn spawn_watcher(&self) {
        let root = self.inner.project.join(crate::registry::CANVAS_DIR);
        let signals = self.inner.signals.clone();
        let base = root.clone();
        std::thread::spawn(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            let mut watcher = match notify::recommended_watcher(tx) {
                Ok(watcher) => watcher,
                Err(_) => return,
            };
            use notify::Watcher;
            // The directory may not exist yet; the first `canvas create` makes
            // it, so create it here rather than giving up on watching.
            let _ = std::fs::create_dir_all(&base);
            if watcher.watch(&base, notify::RecursiveMode::Recursive).is_err() {
                return;
            }
            // A single save produces several events. Collapse anything that
            // arrives in the same beat so one edit is one reload.
            while let Ok(event) = rx.recv() {
                let mut paths = collect_paths(event);
                while let Ok(next) = rx.recv_timeout(Duration::from_millis(60)) {
                    paths.extend(collect_paths(next));
                }
                let mut slugs: Vec<String> = paths
                    .iter()
                    .filter_map(|path| slug_of(&base, path))
                    .collect();
                slugs.sort();
                slugs.dedup();
                for slug in slugs {
                    let _ = signals.send(Signal::Reload { slug });
                }
            }
        });
    }
}

/// Paths from an event that actually changed the canvas.
///
/// Reads must be filtered out, not merely debounced. Serving a module reads it
/// from disk, which inotify reports as an access and a metadata (atime) change;
/// treating those as edits meant serving a module scheduled a reload, the
/// reload re-requested the module, and the page never stopped refreshing.
fn collect_paths(event: Result<notify::Event, notify::Error>) -> Vec<PathBuf> {
    use notify::event::{EventKind, ModifyKind};

    let Ok(event) = event else {
        return Vec::new();
    };
    match event.kind {
        EventKind::Create(_) | EventKind::Remove(_) => event.paths,
        EventKind::Modify(ModifyKind::Metadata(_)) => Vec::new(),
        EventKind::Modify(_) => event.paths,
        EventKind::Access(_) | EventKind::Any | EventKind::Other => Vec::new(),
    }
}

/// Which canvas does a changed path belong to?
fn slug_of(base: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(base).ok()?;
    match relative.components().next()? {
        Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
        _ => None,
    }
}

// ------------------------------------------------------------------ handlers

type Shared = State<Arc<Inner>>;

async fn serve_shell(
    State(inner): Shared,
    UrlPath((key, slug)): UrlPath<(String, String)>,
) -> Response {
    if key != inner.key {
        return StatusCode::NOT_FOUND.into_response();
    }
    let manifest = Registry::discover(&inner.project)
        .get(&slug)
        .map(|canvas| canvas.manifest.clone());
    let Some(manifest) = manifest else {
        return (StatusCode::NOT_FOUND, format!("no canvas named {slug}")).into_response();
    };
    html(assets::shell(&slug, &manifest, &inner.key))
}

async fn serve_module(
    State(inner): Shared,
    UrlPath((key, slug, path)): UrlPath<(String, String, String)>,
) -> Response {
    if key != inner.key {
        return StatusCode::NOT_FOUND.into_response();
    }
    let registry = Registry::discover(&inner.project);
    let Some(canvas) = registry.get(&slug) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(target) = resolve_within(&canvas.root, &path) else {
        // A traversal attempt is indistinguishable from a typo to the caller.
        return StatusCode::NOT_FOUND.into_response();
    };

    let Ok(bytes) = tokio::fs::read(&target).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    if !matches!(
        target.extension().and_then(|e| e.to_str()),
        Some("js" | "jsx" | "ts" | "tsx" | "mjs")
    ) {
        return raw(assets::content_type(&path), bytes);
    }

    let Ok(source) = String::from_utf8(bytes) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    // Transform against the canvas-relative path: the development JSX runtime
    // bakes the file name into every element, and an absolute path would put
    // the user's home directory into page source the browser can read.
    let label = PathBuf::from(relative_label(&canvas.root, &target));
    match transform::transform(
        &label,
        &source,
        Options {
            development: true,
            ..Options::default()
        },
    ) {
        Ok(output) => raw("text/javascript; charset=utf-8", output.code.into_bytes()),
        Err(error) => {
            let first = error.diagnostics.first();
            let detail = serde_json::json!({
                "path": label.display().to_string(),
                "message": first.map(|d| d.message.clone()).unwrap_or_default(),
                "line": first.map(|d| d.line),
                "column": first.map(|d| d.column),
            });
            inner.push_report(Report {
                slug: slug.clone(),
                level: "build-error".into(),
                message: error.to_string(),
                detail: Some(detail.clone()),
            });
            // 200 with a throwing module: the browser reports the failure through
            // the page's own error path, and the overlay explains it in place.
            raw(
                "text/javascript; charset=utf-8",
                format!(
                    "const detail = {detail};\n\
                     console.error(`canvas build error in ${{detail.path}}:${{detail.line}}:${{detail.column}}\\n${{detail.message}}`);\n\
                     throw new Error(detail.message);\n"
                )
                .into_bytes(),
            )
        }
    }
}

async fn serve_client() -> Response {
    raw("text/javascript; charset=utf-8", assets::CLIENT.as_bytes().to_vec())
}

async fn serve_vendor(UrlPath(path): UrlPath<String>) -> Response {
    match assets::vendored(&path) {
        Some(bytes) => raw(assets::content_type(&path), bytes.to_vec()),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct Session {
    #[serde(default)]
    k: String,
    #[serde(default)]
    slug: String,
}

async fn serve_events(State(inner): Shared, Query(session): Query<Session>) -> Response {
    if session.k != inner.key {
        return StatusCode::NOT_FOUND.into_response();
    }
    let slug = session.slug;
    let stream = tokio_stream::wrappers::BroadcastStream::new(inner.signals.subscribe())
        .filter_map(move |signal| {
            let slug = slug.clone();
            async move {
                match signal {
                    Ok(Signal::Reload { slug: changed }) if changed == slug => {
                        Some(Ok::<_, std::convert::Infallible>(
                            Event::default().event("reload").data("{}"),
                        ))
                    }
                    _ => None,
                }
            }
        });
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

#[derive(Debug, Deserialize)]
struct RpcRequest {
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

async fn serve_rpc(
    State(inner): Shared,
    Query(session): Query<Session>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if session.k != inner.key {
        return StatusCode::NOT_FOUND.into_response();
    }
    // A page on another origin can POST here without reading the response, so
    // the key alone is not enough — it is in a URL that could leak by Referer.
    if !origin_is_ours(&headers, inner.as_ref()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(request) = serde_json::from_str::<RpcRequest>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    match request.method.as_str() {
        "canvas.report" => {
            let level = request.params.get("level").and_then(|v| v.as_str()).unwrap_or("log");
            let message = request
                .params
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            inner.push_report(Report {
                slug: session.slug,
                level: level.to_owned(),
                message: message.to_owned(),
                detail: request.params.get("detail").cloned().filter(|v| !v.is_null()),
            });
            axum::Json(serde_json::json!({"ok": true})).into_response()
        }
        other => (
            StatusCode::BAD_REQUEST,
            format!("unknown canvas method: {other}"),
        )
            .into_response(),
    }
}

impl Inner {
    fn push_report(&self, report: Report) {
        let mut reports = self.reports.lock().expect("report lock poisoned");
        if reports.len() >= REPORT_CAPACITY {
            reports.pop_front();
        }
        reports.push_back(report);
    }
}

/// Only our own pages may drive the bridge.
///
/// A missing `Origin` is accepted because a same-origin `fetch` from a module
/// is not required to send one; a *present and foreign* origin is refused —
/// including another server on loopback, which the port comparison excludes.
fn origin_is_ours(headers: &HeaderMap, inner: &Inner) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let port = inner.addr.port();
    origin == format!("http://127.0.0.1:{port}") || origin == format!("http://localhost:{port}")
}

// -------------------------------------------------------------------- helpers

/// The per-process capability that gates every canvas URL.
///
/// 32 characters of a 36-symbol alphabet is ~165 bits — far past guessing, and
/// it stays copy-pasteable and shell-safe, which matters because it rides in a
/// URL the user is handed.
fn session_key(rng: &mut ThreadRng) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    (0..32)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect()
}

/// Resolve a request path inside a canvas, refusing anything that climbs out.
fn resolve_within(root: &Path, requested: &str) -> Option<PathBuf> {
    let mut resolved = root.to_path_buf();
    for segment in requested.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return None,
            other if other.contains('\\') => return None,
            other => resolved.push(other),
        }
    }
    // Symlinks can still point outside; compare canonical forms when both
    // sides exist, and fall back to the lexical result when they do not.
    match (resolved.canonicalize(), root.canonicalize()) {
        (Ok(target), Ok(base)) if !target.starts_with(&base) => None,
        _ => Some(resolved),
    }
}

fn relative_label(root: &Path, target: &Path) -> String {
    target
        .strip_prefix(root)
        .unwrap_or(target)
        .display()
        .to_string()
}

fn html(body: String) -> Response {
    raw("text/html; charset=utf-8", body.into_bytes())
}

fn raw(content_type: &str, body: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            // A canvas is recompiled on every request; a cached module would
            // defeat the reload the watcher just triggered.
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_out_of_a_canvas_is_refused() {
        let root = Path::new("/tmp/project/.artist/canvas/demo");
        assert!(resolve_within(root, "../../../etc/passwd").is_none());
        assert!(resolve_within(root, "components/../../../etc/passwd").is_none());
        assert!(resolve_within(root, "a\\..\\b").is_none());
        assert_eq!(
            resolve_within(root, "components/Chart.jsx"),
            Some(root.join("components/Chart.jsx"))
        );
        assert_eq!(resolve_within(root, "./main.jsx"), Some(root.join("main.jsx")));
    }

    /// Serving a module reads it, and a read is an access plus an atime bump.
    /// Counting either as an edit made every page request schedule a reload
    /// that caused the next page request — the tab refreshed forever.
    #[test]
    fn reading_a_file_is_not_an_edit() {
        use notify::event::{
            AccessKind, CreateKind, DataChange, Event, EventKind, MetadataKind, ModifyKind,
            RemoveKind,
        };

        let path = PathBuf::from("/p/.artist/canvas/demo/main.jsx");
        let of = |kind| {
            collect_paths(Ok(Event {
                kind,
                paths: vec![path.clone()],
                attrs: Default::default(),
            }))
        };

        assert!(of(EventKind::Access(AccessKind::Read)).is_empty());
        assert!(of(EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime))).is_empty());

        assert_eq!(of(EventKind::Modify(ModifyKind::Data(DataChange::Content))), [path.clone()]);
        assert_eq!(of(EventKind::Create(CreateKind::File)), [path.clone()]);
        assert_eq!(of(EventKind::Remove(RemoveKind::File)), [path]);
    }

    #[test]
    fn a_changed_file_maps_back_to_its_canvas() {
        let base = Path::new("/p/.artist/canvas");
        assert_eq!(
            slug_of(base, Path::new("/p/.artist/canvas/perf/components/A.jsx")),
            Some("perf".to_owned())
        );
        assert_eq!(
            slug_of(base, Path::new("/p/.artist/canvas/perf")),
            Some("perf".to_owned())
        );
        assert_eq!(slug_of(base, Path::new("/elsewhere/x.jsx")), None);
    }

    #[test]
    fn a_foreign_origin_cannot_drive_the_bridge() {
        let mut headers = HeaderMap::new();
        let inner = Inner {
            project: PathBuf::new(),
            key: String::new(),
            addr: "127.0.0.1:54321".parse().expect("loopback addr"),
            signals: broadcast::channel(1).0,
            reports: Mutex::new(VecDeque::new()),
        };

        assert!(origin_is_ours(&headers, &inner), "same-origin sends no Origin");

        headers.insert(header::ORIGIN, "http://127.0.0.1:54321".parse().unwrap());
        assert!(origin_is_ours(&headers, &inner));

        headers.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert!(!origin_is_ours(&headers, &inner));

        // A host that merely starts with our loopback name is still foreign.
        headers.insert(header::ORIGIN, "http://127.0.0.1.evil.example".parse().unwrap());
        assert!(!origin_is_ours(&headers, &inner));

        // Another server on loopback is foreign too — the port is part of the
        // origin, and the key can leak through a Referer.
        headers.insert(header::ORIGIN, "http://127.0.0.1:9999".parse().unwrap());
        assert!(!origin_is_ours(&headers, &inner));
    }
}
