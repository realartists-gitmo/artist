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
    collections::{BTreeMap, VecDeque},
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use dashmap::DashMap;

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
    bridge::CanvasHost,
    registry::Registry,
    state::StateStore,
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

/// Pushed to every open page.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Signal {
    Reload {
        slug: String,
    },
    /// One module changed. The page re-imports just that module and lets React
    /// Refresh swap the components, so scroll position, focus and state all
    /// survive an edit — the difference between a preview and a workbench.
    Update {
        slug: String,
        /// Canvas-relative, matching the URL the page originally imported.
        path: String,
    },
    /// Shared state changed.
    ///
    /// Carries only the keys that moved, plus the revision. It used to say that
    /// and send the whole map, so every consumer re-rendered on every write to
    /// any key, and a large state meant a large frame per keystroke.
    State {
        slug: String,
        rev: u64,
        changed: serde_json::Value,
    },
    /// The set of open questions changed.
    Ask {
        questions: Vec<artist_session::ask::Question>,
    },
    /// An agent event, forwarded verbatim.
    Agent {
        event: serde_json::Value,
    },
    /// Asks any open page to describe what it is showing.
    Digest {
        slug: String,
    },
}

impl Signal {
    /// Which canvas should see this, or `None` for everyone.
    fn addressed_to(&self) -> Option<&str> {
        match self {
            Signal::Reload { slug }
            | Signal::Update { slug, .. }
            | Signal::Digest { slug }
            | Signal::State { slug, .. } => Some(slug),
            Signal::Ask { .. } | Signal::Agent { .. } => None,
        }
    }

    fn event_name(&self) -> &'static str {
        match self {
            Signal::Reload { .. } => "reload",
            Signal::Update { .. } => "update",
            Signal::State { .. } => "state",
            Signal::Ask { .. } => "ask",
            Signal::Agent { .. } => "agent",
            Signal::Digest { .. } => "digest",
        }
    }
}

struct Inner {
    project: PathBuf,
    key: String,
    addr: SocketAddr,
    signals: broadcast::Sender<Signal>,
    reports: Mutex<VecDeque<Report>>,
    /// The last thing each canvas said it was showing.
    digests: Mutex<std::collections::HashMap<String, serde_json::Value>>,
    /// One store per canvas, opened lazily and kept for the process lifetime so
    /// two tabs of the same canvas share one revision counter.
    states: DashMap<String, Arc<StateStore>>,
    /// The windows this session put on screen, so they can be closed with it.
    windows: crate::window::Windows,
    host: Arc<dyn CanvasHost>,
}

/// A running canvas server.
#[derive(Clone)]
pub struct Server {
    inner: Arc<Inner>,
    addr: SocketAddr,
}

/// A server that has not been started yet, and may never be.
///
/// Most sessions never touch a canvas, and the eager version made every one of
/// them bind a port and stand up an RPC surface holding a session key anyway.
/// That is attack surface nobody asked for, on a machine where the user is
/// mostly editing Rust. It now costs nothing until the model actually reaches
/// for the tool.
pub struct Lazy {
    project: PathBuf,
    host: Arc<dyn CanvasHost>,
    started: tokio::sync::Mutex<Option<Arc<Server>>>,
}

impl Lazy {
    pub fn new(project: PathBuf, host: Arc<dyn CanvasHost>) -> Arc<Self> {
        Arc::new(Lazy {
            project,
            host,
            started: tokio::sync::Mutex::new(None),
        })
    }

    /// The running server, starting it if this is the first ask.
    ///
    /// A failed bind is not cached: it is usually transient — a port exhausted,
    /// a sandbox not yet ready — and permanently disabling canvases for the
    /// session because of one bad moment would be the wrong trade.
    pub async fn server(&self) -> anyhow::Result<Arc<Server>> {
        let mut started = self.started.lock().await;
        if let Some(server) = started.as_ref() {
            return Ok(Arc::clone(server));
        }
        let server =
            Arc::new(Server::start_with_host(self.project.clone(), Arc::clone(&self.host)).await?);
        *started = Some(Arc::clone(&server));
        Ok(server)
    }

    /// The server if it is already up, without starting one.
    ///
    /// For the status bar and `/canvas`, which should report on what exists
    /// rather than bring it into being by asking about it.
    ///
    /// Never blocks, and so answers "not running" while a start is in flight
    /// and holding the lock. That is the right trade for a caller drawing a
    /// frame — a status bar must not stall on a bind — and the wrong one for
    /// anybody who can wait. Those want `started`.
    pub fn running(&self) -> Option<Arc<Server>> {
        self.started
            .try_lock()
            .ok()
            .and_then(|started| started.clone())
    }

    /// The server if it is already up, waiting out any start in flight.
    ///
    /// Same question as `running`, asked by a caller that can await the honest
    /// answer instead of the cheap one: a tool reporting "nothing is being
    /// served" because another task happened to be mid-bind would send the
    /// model off to open a canvas that was already opening.
    pub async fn started(&self) -> Option<Arc<Server>> {
        self.started.lock().await.clone()
    }

    pub fn project(&self) -> &Path {
        &self.project
    }
}

impl Server {
    /// Bind and start serving, detached from any agent.
    pub async fn start(project: PathBuf) -> anyhow::Result<Self> {
        Server::start_with_host(project, Arc::new(crate::bridge::DetachedHost)).await
    }

    /// Bind and start serving against a live session. Returns once the port is
    /// known, so a caller can hand out a URL without racing the accept loop.
    pub async fn start_with_host(
        project: PathBuf,
        host: Arc<dyn CanvasHost>,
    ) -> anyhow::Result<Self> {
        let key = session_key(&mut rand::rng());
        let (signals, _) = broadcast::channel(256);
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
            digests: Mutex::new(std::collections::HashMap::new()),
            states: DashMap::new(),
            windows: crate::window::Windows::default(),
            host,
        });

        let router = Router::new()
            .route("/c/{key}/{slug}/", get(serve_shell))
            .route("/c/{key}/{slug}/{*path}", get(serve_module))
            .route("/@artist/client.js", get(serve_client))
            .route("/@artist/ui.js", get(serve_ui))
            .route("/@artist/react.js", get(serve_hooks))
            .route("/@artist/refresh.js", get(serve_refresh))
            .route("/@vendor/{*path}", get(serve_vendor))
            .route("/@dep/{key}/{slug}/{specifier}", get(serve_dep))
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

    /// The project whose canvases this server hosts.
    pub fn project(&self) -> &Path {
        &self.inner.project
    }

    /// The URL to hand the user for one canvas.
    pub fn url(&self, slug: &str) -> String {
        format!("http://{}/c/{}/{}/", self.addr, self.inner.key, slug)
    }

    /// The shared state store for one canvas.
    pub fn state(&self, slug: &str) -> Arc<StateStore> {
        self.inner.state_for(slug)
    }

    /// Put `slug` on screen, or report that it already is.
    ///
    /// The window is owned by the server, so it closes when the session that
    /// serves it does — a webview showing a canvas whose server has gone is
    /// worse than no window, because every interaction fails silently.
    pub fn show(&self, slug: &str, title: &str) -> std::io::Result<crate::window::Opened> {
        self.inner
            .windows
            .open(&self.inner.project, slug, &self.url(slug), title)
    }

    /// Close one canvas's window.
    pub fn hide(&self, slug: &str) -> bool {
        self.inner.windows.close(slug)
    }

    /// Which canvases have a window up.
    pub fn showing(&self) -> Vec<String> {
        self.inner.windows.showing()
    }

    /// Write shared state from the harness side and push it to open pages.
    ///
    /// This is how the model feeds a canvas: it writes rows, the page rerenders.
    /// A refused write pushes nothing: the pages keep the state they have, and
    /// the caller gets the reason to pass on to whoever asked for the write.
    pub fn publish_state(
        &self,
        slug: &str,
        values: BTreeMap<String, serde_json::Value>,
    ) -> Result<u64, crate::state::OverLimit> {
        let changed = serde_json::to_value(&values).unwrap_or_default();
        let snapshot = self.inner.state_for(slug).merge(values)?;
        let _ = self.inner.signals.send(Signal::State {
            slug: slug.to_owned(),
            rev: snapshot.rev,
            changed,
        });
        Ok(snapshot.rev)
    }

    /// Tell open pages the pending-question set changed.
    pub fn publish_questions(&self, questions: Vec<artist_session::ask::Question>) {
        let _ = self.inner.signals.send(Signal::Ask { questions });
    }

    /// Forward an agent event to any page subscribed to the stream.
    pub fn publish_agent_event(&self, event: serde_json::Value) {
        let _ = self.inner.signals.send(Signal::Agent { event });
    }

    /// Ask any open page for a description of what it is showing, and wait
    /// briefly for the answer.
    ///
    /// Returns `None` when no page is open — which is itself worth telling the
    /// model, since it means nobody has looked at the canvas yet.
    pub async fn request_digest(&self, slug: &str) -> Option<serde_json::Value> {
        self.inner
            .digests
            .lock()
            .expect("digest lock poisoned")
            .remove(slug);
        if self
            .inner
            .signals
            .send(Signal::Digest {
                slug: slug.to_owned(),
            })
            .is_err()
        {
            // No subscribers: nothing has the canvas open.
            return None;
        }
        // A page answers in a frame or two; this is a tool call, so a short
        // wait is cheaper than making the model ask twice.
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if let Some(digest) = self
                .inner
                .digests
                .lock()
                .expect("digest lock poisoned")
                .remove(slug)
            {
                return Some(digest);
            }
        }
        None
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
        let project = self.inner.project.clone();
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
            if watcher
                .watch(&base, notify::RecursiveMode::Recursive)
                .is_err()
            {
                return;
            }
            // A single save produces several events. Collapse anything that
            // arrives in the same beat so one edit is one reload.
            while let Ok(event) = rx.recv() {
                let mut paths = collect_paths(event);
                while let Ok(next) = rx.recv_timeout(Duration::from_millis(60)) {
                    paths.extend(collect_paths(next));
                }
                let entries = Registry::discover(&project);
                let mut changed: Vec<(String, Option<String>)> = paths
                    .iter()
                    .filter(|path| !is_harness_written(path))
                    .filter_map(|path| {
                        let slug = slug_of(&base, path)?;
                        let entry = entries.get(&slug).map(|c| c.manifest.entry.clone());
                        match module_path(&base, &slug, path, entry.as_deref()) {
                            // Swappable in place.
                            Some(module) => Some((slug, Some(module))),
                            // Not swappable: the entry, something the page is
                            // built from, or a writer's leavings. Only the
                            // first two are worth rebuilding for.
                            None if is_entry_module(&base, &slug, path, entry.as_deref())
                                || rebuilds_page(path) =>
                            {
                                Some((slug, None))
                            }
                            None => None,
                        }
                    })
                    .collect();
                changed.sort();
                changed.dedup();
                for (slug, module) in changed {
                    // A module can be swapped in place. The manifest, a
                    // stylesheet or an asset changes the page itself, so the
                    // page has to be rebuilt.
                    let _ = match module {
                        Some(path) => signals.send(Signal::Update { slug, path }),
                        None => signals.send(Signal::Reload { slug }),
                    };
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

/// Did the harness write this, rather than the model or the user?
///
/// Shared state is persisted on every write, so a click that calls
/// `useCanvasState` lands a file change milliseconds later. Treating that as a
/// source edit made the page reload itself on every interaction — the canvas
/// tearing down and rebuilding in response to its own state, which showed as a
/// flash on every tab switch.
fn is_harness_written(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == crate::state::STATE_FILE || name.ends_with(".tmp"))
}

/// Is this the module the page mounts from?
///
/// `module_path` returns `None` for it, because re-importing it would call
/// `createRoot` a second time and throw the live tree away. That makes it look
/// like a non-module to the caller, so it needs naming separately — otherwise
/// the allowlist below, which knows nothing about `.jsx`, would discard an edit
/// to the entry as though it were a writer's temporary.
fn is_entry_module(base: &Path, slug: &str, path: &Path, entry: Option<&str>) -> bool {
    let Ok(relative) = path.strip_prefix(base.join(slug)) else {
        return false;
    };
    let relative = relative.to_string_lossy().replace('\\', "/");
    entry.is_some_and(|entry| entry.trim_start_matches("./") == relative)
}

/// Does changing this file change the *page*, as opposed to a module the page
/// can swap in place?
///
/// An allowlist, and that is the point. Treating every unrecognised file as
/// page-affecting meant a writer's temporary file forced a full reload — and
/// almost nothing writes a file in place. `sed -i` leaves a `sedXXXXXX`
/// beside the target; artist's own `edit` tool writes through
/// `NamedTempFile`, whose names *begin* with `.tmp` rather than ending with
/// it, so the suffix check above never caught them. The reload then landed
/// just after the hot swap and destroyed the state the swap had preserved,
/// which is the entire thing this machinery exists to protect.
///
/// Erring towards ignoring is deliberate: a missed reload costs the reader one
/// refresh, and a spurious one costs them whatever they had typed.
fn rebuilds_page(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name == crate::registry::MANIFEST_FILE {
        return true;
    }
    // Editors and tools scatter dotfiles: `.swp`, `.tmpXXXX`, `.goutputstream-*`.
    // None of them are the page.
    if name.starts_with('.') {
        return false;
    }
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension,
                "css" | "html" | "png" | "jpg" | "jpeg" | "svg" | "webp" | "gif" | "woff" | "woff2"
            )
        })
}

/// The canvas-relative module path for a change, or `None` if it is not a
/// module the page could re-import.
fn module_path(base: &Path, slug: &str, path: &Path, entry: Option<&str>) -> Option<String> {
    let relative = path.strip_prefix(base.join(slug)).ok()?;
    let extension = relative.extension()?.to_str()?;
    if !matches!(extension, "js" | "jsx" | "ts" | "tsx" | "mjs") {
        return None;
    }
    let relative = relative.to_string_lossy().replace('\\', "/");
    // The entry mounts the React root, so re-importing it would call
    // createRoot a second time and throw the live tree away — the state a hot
    // swap exists to preserve. Rebuild the page instead.
    if entry.is_some_and(|entry| entry.trim_start_matches("./") == relative) {
        return None;
    }
    Some(relative)
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
    // Reported on every load rather than at parse time: the model reads
    // `status`, and this is the kind of thing it needs told twice.
    let shadowed = assets::shadowed_specifiers(&manifest);
    if !shadowed.is_empty() {
        inner.push_report(Report {
            slug: slug.clone(),
            level: "style".into(),
            message: format!(
                "{} replaces a module shipped in the binary. The canvas now needs the network \
                 to load, and a version mismatch with the vendored React will not be obvious.",
                shadowed.join(", ")
            ),
            detail: None,
        });
    }
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
            refresh: true,
        },
    ) {
        Ok(output) => {
            // Reported, not prevented: the model corrects on its next `status`
            // through the loop that already carries compile errors.
            let drift = crate::drift::scan(&source);
            if !drift.is_empty() {
                inner.push_report(Report {
                    slug: slug.clone(),
                    level: "style".into(),
                    message: crate::drift::describe(&label.display().to_string(), &drift),
                    detail: None,
                });
            }
            raw(
                "text/javascript; charset=utf-8",
                assets::scope_refresh(&label.display().to_string(), &output.code).into_bytes(),
            )
        }
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
    raw(
        "text/javascript; charset=utf-8",
        assets::CLIENT.as_bytes().to_vec(),
    )
}

async fn serve_ui() -> Response {
    raw(
        "text/javascript; charset=utf-8",
        assets::compiled("ui.jsx", assets::UI).as_bytes().to_vec(),
    )
}

async fn serve_refresh() -> Response {
    raw(
        "text/javascript; charset=utf-8",
        assets::compiled("refresh.js", assets::REFRESH)
            .as_bytes()
            .to_vec(),
    )
}

async fn serve_hooks() -> Response {
    raw(
        "text/javascript; charset=utf-8",
        assets::compiled("hooks.js", assets::HOOKS)
            .as_bytes()
            .to_vec(),
    )
}

/// Serve a package a canvas declared under `[deps]`.
///
/// The URL comes from the manifest on disk, never from the request, so this
/// cannot be driven into fetching an arbitrary host.
async fn serve_dep(
    State(inner): Shared,
    UrlPath((key, slug, specifier)): UrlPath<(String, String, String)>,
) -> Response {
    if key != inner.key {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Scoped to the canvas that asked. Searching every canvas meant directory
    // order decided which version won when two pinned the same package, and a
    // canvas could resolve a specifier it had never declared.
    let declared = Registry::discover(&inner.project)
        .get(&slug)
        .and_then(|canvas| canvas.manifest.deps.get(&specifier).cloned());
    let Some(url) = declared else {
        return (
            StatusCode::NOT_FOUND,
            format!("`{specifier}` is not declared under [deps] in canvas `{slug}`"),
        )
            .into_response();
    };
    match crate::deps::fetch(&url).await {
        Ok(bytes) => raw("text/javascript; charset=utf-8", bytes),
        Err(error) => (StatusCode::BAD_GATEWAY, error.to_string()).into_response(),
    }
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
                let signal = signal.ok()?;
                // A signal addressed to another canvas is not this page's
                // business; an unaddressed one goes to everybody.
                if signal.addressed_to().is_some_and(|target| target != slug) {
                    return None;
                }
                let data = serde_json::to_string(&signal).ok()?;
                Some(Ok::<_, std::convert::Infallible>(
                    Event::default().event(signal.event_name()).data(data),
                ))
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
    // The key comes from a header here, not the query string. A URL leaks by
    // Referer, lands in logs and proxies, and sits in browser history; a custom
    // header does none of that and cannot be sent cross-origin without a
    // preflight this server never answers.
    //
    // Not because argv would expose it — `window::command` passes the URL to
    // the child through the environment for exactly that reason, and saying
    // otherwise here invites someone to "fix" it back into an argument.
    let presented = headers
        .get("x-artist-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if presented != inner.key {
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

    // The slug arrives as a query parameter, so it is attacker-controlled even
    // when the key is right. Everything downstream joins it onto a path or uses
    // it to select a permission set, so it must be resolved against the
    // registry here — not merely trusted because the shell handlers happened to
    // resolve their own copy of it.
    let registry = Registry::discover(&inner.project);
    let Some(canvas) = registry.get(&session.slug).cloned() else {
        return (
            StatusCode::NOT_FOUND,
            format!("no canvas named `{}`", session.slug),
        )
            .into_response();
    };
    let slug = canvas.slug.clone();
    let params = request.params;

    match request.method.as_str() {
        "canvas.report" => {
            let level = params
                .get("level")
                .and_then(|v| v.as_str())
                .unwrap_or("log");
            let message = params
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            inner.push_report(Report {
                slug,
                level: level.to_owned(),
                message: message.to_owned(),
                detail: params.get("detail").cloned().filter(|v| !v.is_null()),
            });
            ok(serde_json::json!({"ok": true}))
        }

        "canvas.state.get" => {
            let snapshot = inner.state_for(&slug).snapshot();
            ok(serde_json::json!({"rev": snapshot.rev, "entries": snapshot.plain()}))
        }

        "canvas.state.set" => {
            let Some(values) = params.get("entries").and_then(|v| v.as_object()) else {
                return bad("state.set needs an `entries` object");
            };
            let merged: BTreeMap<String, serde_json::Value> = values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            let notify = params
                .get("notify")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // Refused rather than truncated: a canvas that silently kept only
            // some of what it wrote would be far harder to debug than one told
            // plainly that it wrote nothing.
            let snapshot = match inner.state_for(&slug).merge(merged.clone()) {
                Ok(snapshot) => snapshot,
                Err(over) => {
                    // Reported as well as returned. The page sees the rejection
                    // synchronously, but the model is the one that has to fix
                    // it, and `status` is where it looks.
                    inner.push_report(Report {
                        slug: slug.clone(),
                        level: "error".into(),
                        message: over.to_string(),
                        detail: None,
                    });
                    return bad(&over.to_string());
                }
            };
            if notify {
                inner
                    .host
                    .state_changed(&slug, merged.keys().cloned().collect());
            }
            // Echo to every open tab, including the one that wrote: it needs
            // the revision to know its optimistic update was accepted.
            let _ = inner.signals.send(Signal::State {
                slug,
                rev: snapshot.rev,
                changed: serde_json::to_value(&merged).unwrap_or_default(),
            });
            ok(serde_json::json!({"rev": snapshot.rev}))
        }

        "canvas.send" => {
            let Some(text) = params.get("text").and_then(|v| v.as_str()) else {
                return bad("send needs `text`");
            };
            // No default: the page has to say whether it means to interrupt the
            // running turn or schedule a new one.
            let Some(mode) = params
                .get("mode")
                .and_then(|v| serde_json::from_value::<crate::bridge::SendMode>(v.clone()).ok())
            else {
                return bad(
                    "send needs `mode`: \"steer\" to correct a running turn, or \"queue\" to start one",
                );
            };
            let outcome = inner.host.send(text.to_owned(), mode).await;
            ok(serde_json::json!({"ok": true, "outcome": outcome}))
        }

        "canvas.call" => {
            let Some(tool) = params.get("tool").and_then(|v| v.as_str()) else {
                return bad("call needs `tool`");
            };
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::Value::Object(Default::default()));
            // Taken from the canvas this request resolved to. Looking it up by
            // the page's own slug string again would let a canvas name a more
            // permissive sibling and borrow its grants.
            let allowed = canvas.manifest.permissions.allow.clone();
            match inner
                .host
                .call_tool(tool.to_owned(), arguments, allowed)
                .await
            {
                Ok(output) => ok(serde_json::json!({"ok": true, "output": output})),
                Err(denied) => (
                    StatusCode::FORBIDDEN,
                    axum::Json(serde_json::json!({
                        "ok": false,
                        "error": denied.to_string(),
                        "denied": denied,
                    })),
                )
                    .into_response(),
            }
        }

        "canvas.ask.pending" => ok(serde_json::json!({
            "questions": inner.host.pending_questions()
        })),

        "canvas.ask.answer" => {
            let Ok(answer) = serde_json::from_value::<artist_session::ask::Answer>(params.clone())
            else {
                return bad("answer needs `question_id` and `selected`");
            };
            let accepted = inner
                .host
                .answer_question(answer, &format!("canvas:{slug}"));
            // Whether or not this surface won the race, the set changed.
            let _ = inner.signals.send(Signal::Ask {
                questions: inner.host.pending_questions(),
            });
            ok(serde_json::json!({"accepted": accepted}))
        }

        "canvas.digest" => {
            inner
                .digests
                .lock()
                .expect("digest lock poisoned")
                .insert(slug, params);
            ok(serde_json::json!({"ok": true}))
        }

        "canvas.markdown" => {
            let source = params.get("source").and_then(|v| v.as_str()).unwrap_or("");
            ok(serde_json::json!({"html": crate::markdown::render(source)}))
        }

        "canvas.edit" => {
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let line = params
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .map(|line| line as u32);
            match crate::editor::open(&inner.project, path, line) {
                Ok(program) => ok(serde_json::json!({"ok": true, "editor": program})),
                Err(error) => bad(&error.to_string()),
            }
        }

        "canvas.context" => ok(inner.host.context()),

        "canvas.highlight" => {
            let source = params.get("source").and_then(|v| v.as_str()).unwrap_or("");
            let language = params
                .get("language")
                .and_then(|v| v.as_str())
                .unwrap_or("txt");
            let dark = params.get("dark").and_then(|v| v.as_bool()).unwrap_or(true);
            ok(
                serde_json::to_value(crate::highlight::highlight(source, language, dark))
                    .unwrap_or_default(),
            )
        }

        other => bad(&format!("unknown canvas method: {other}")),
    }
}

fn ok(body: serde_json::Value) -> Response {
    axum::Json(body).into_response()
}

fn bad(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(serde_json::json!({"ok": false, "error": message})),
    )
        .into_response()
}

impl Inner {
    /// The state store for one canvas, opened on first use and then shared, so
    /// two tabs of the same canvas agree on the revision counter.
    fn state_for(&self, slug: &str) -> Arc<StateStore> {
        if let Some(existing) = self.states.get(slug) {
            return Arc::clone(existing.value());
        }
        let root = self.project.join(crate::registry::CANVAS_DIR).join(slug);
        let store = Arc::new(StateStore::open(&root));
        self.states
            .entry(slug.to_owned())
            .or_insert(store)
            .value()
            .clone()
    }

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
        assert_eq!(
            resolve_within(root, "./main.jsx"),
            Some(root.join("main.jsx"))
        );
    }

    /// A canvas writing its own shared state must not be mistaken for someone
    /// editing the canvas. Reloading on it made every click that touched
    /// `useCanvasState` rebuild the page.
    #[test]
    fn the_harness_writing_state_is_not_an_edit() {
        let base = Path::new("/p/.artist/canvas/demo");
        assert!(is_harness_written(&base.join("state.json")));
        // The atomic write lands a temp file next to it first.
        assert!(is_harness_written(&base.join("state.json.tmp")));

        // What the model and the user write still counts.
        assert!(!is_harness_written(&base.join("main.jsx")));
        assert!(!is_harness_written(&base.join("canvas.toml")));
        assert!(!is_harness_written(&base.join("data.json")));
    }

    /// The bug this whole hot-reload path existed to avoid, arriving by the
    /// back door.
    ///
    /// Almost nothing writes a file in place. `sed -i` leaves a `sedXXXXXX`
    /// beside the target, and artist's own `edit` writes through
    /// `NamedTempFile`, whose names *begin* with `.tmp` rather than ending
    /// with it. Treating an unrecognised file as page-affecting meant every
    /// one of those forced a reload — which landed just after the hot swap and
    /// destroyed the state the swap had preserved. Verified live: state reset
    /// on every edit before this, and survives after.
    #[test]
    fn a_writers_temporary_file_does_not_rebuild_the_page() {
        let base = Path::new("/p/.artist/canvas/demo");
        // `sed -i`.
        assert!(!rebuilds_page(&base.join("sedM4vCtq")));
        // `tempfile::NamedTempFile`, which is what `edit` uses.
        assert!(!rebuilds_page(&base.join(".tmpA1b2C3")));
        // Editors of various habits.
        assert!(!rebuilds_page(&base.join(".App.jsx.swp")));
        assert!(!rebuilds_page(&base.join("App.jsx~")));
        assert!(!rebuilds_page(&base.join(".goutputstream-XYZ12")));
        assert!(!rebuilds_page(&base.join("4913")));
    }

    /// The other half: things that genuinely are the page still rebuild it.
    /// An allowlist that admits nothing would trade one silent failure for
    /// another.
    #[test]
    fn the_page_itself_still_rebuilds() {
        let base = Path::new("/p/.artist/canvas/demo");
        assert!(rebuilds_page(&base.join("canvas.toml")));
        assert!(rebuilds_page(&base.join("theme.css")));
        assert!(rebuilds_page(&base.join("logo.svg")));
        assert!(rebuilds_page(&base.join("hero.png")));
        assert!(rebuilds_page(&base.join("Inter.woff2")));
    }

    /// The entry is a `.jsx`, so the asset allowlist alone would discard it as
    /// though it were a temporary — but re-importing it would call
    /// `createRoot` twice, so it has to rebuild rather than swap.
    #[test]
    fn the_entry_module_still_rebuilds_the_page() {
        let base = Path::new("/p/.artist/canvas");
        let entry = Some("main.jsx");
        assert!(is_entry_module(
            base,
            "demo",
            &base.join("demo/main.jsx"),
            entry
        ));
        // A component beside it is swappable, not a rebuild.
        assert!(!is_entry_module(
            base,
            "demo",
            &base.join("demo/App.jsx"),
            entry
        ));
        // And the manifest's `./` prefix must not defeat the comparison.
        assert!(is_entry_module(
            base,
            "demo",
            &base.join("demo/main.jsx"),
            Some("./main.jsx")
        ));
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
        assert!(
            of(EventKind::Modify(ModifyKind::Metadata(
                MetadataKind::AccessTime
            )))
            .is_empty()
        );

        assert_eq!(
            of(EventKind::Modify(ModifyKind::Data(DataChange::Content))),
            [path.clone()]
        );
        assert_eq!(of(EventKind::Create(CreateKind::File)), [path.clone()]);
        assert_eq!(of(EventKind::Remove(RemoveKind::File)), [path]);
    }

    /// A module can be swapped in place; anything else changes the page itself
    /// and has to be rebuilt. Getting this backwards either loses state on
    /// every edit, or silently serves a stale page after a manifest change.
    #[test]
    fn only_modules_are_hot_swapped() {
        let base = Path::new("/p/.artist/canvas");

        let entry = Some("main.jsx");

        assert_eq!(
            module_path(
                base,
                "demo",
                Path::new("/p/.artist/canvas/demo/parts/Chart.tsx"),
                entry
            ),
            Some("parts/Chart.tsx".to_owned())
        );

        // The entry mounts the React root. Re-importing it would call
        // createRoot again and discard the very state a hot swap protects, so
        // it is deliberately a full reload.
        assert_eq!(
            module_path(
                base,
                "demo",
                Path::new("/p/.artist/canvas/demo/main.jsx"),
                entry
            ),
            None
        );
        assert_eq!(
            module_path(
                base,
                "demo",
                Path::new("/p/.artist/canvas/demo/main.jsx"),
                Some("./main.jsx")
            ),
            None
        );

        for whole_page in ["canvas.toml", "styles.css", "logo.svg", "state.json"] {
            let path = Path::new("/p/.artist/canvas/demo").join(whole_page);
            assert_eq!(
                module_path(base, "demo", &path, entry),
                None,
                "{whole_page} should force a reload"
            );
        }
        // A file with no extension is not a module either.
        assert_eq!(
            module_path(
                base,
                "demo",
                Path::new("/p/.artist/canvas/demo/README"),
                entry
            ),
            None
        );
    }

    /// Two modules both exporting `App` must not overwrite each other in the
    /// refresh runtime's family registry.
    #[test]
    fn refresh_registrations_are_namespaced_per_module() {
        let first = crate::assets::scope_refresh("main.jsx", "const a = 1;");
        let second = crate::assets::scope_refresh("parts/Chart.jsx", "const a = 1;");

        assert!(first.contains("\"main.jsx\""), "{first}");
        assert!(second.contains("\"parts/Chart.jsx\""), "{second}");
        // The previous pair is saved and restored, so modules do not leak their
        // registrar into whatever evaluates next.
        assert!(first.contains("__artistPrevReg"));
        assert!(
            first
                .trim_end()
                .ends_with("window.$RefreshSig$ = __artistPrevSig;")
        );
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
            digests: Mutex::new(std::collections::HashMap::new()),
            states: DashMap::new(),
            windows: crate::window::Windows::default(),
            host: Arc::new(crate::bridge::DetachedHost),
        };

        assert!(
            origin_is_ours(&headers, &inner),
            "same-origin sends no Origin"
        );

        headers.insert(header::ORIGIN, "http://127.0.0.1:54321".parse().unwrap());
        assert!(origin_is_ours(&headers, &inner));

        headers.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert!(!origin_is_ours(&headers, &inner));

        // A host that merely starts with our loopback name is still foreign.
        headers.insert(
            header::ORIGIN,
            "http://127.0.0.1.evil.example".parse().unwrap(),
        );
        assert!(!origin_is_ours(&headers, &inner));

        // Another server on loopback is foreign too — the port is part of the
        // origin, and the key can leak through a Referer.
        headers.insert(header::ORIGIN, "http://127.0.0.1:9999".parse().unwrap());
        assert!(!origin_is_ours(&headers, &inner));
    }
}
