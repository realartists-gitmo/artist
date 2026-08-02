//! Rungs 0 and 1 for anything Chromium: browsers and Electron applications.
//!
//! Two surfaces per browser, at different rungs, because a browser really is two
//! different things:
//!
//! * **Chrome** — the tab strip, the omnibox, navigation. These are *programmatic*
//!   calls: `Target.createTarget`, `Page.navigate`, `Target.closeTarget`. Rung 0.
//!   It is a common mistake to drive them through the accessibility tree, which
//!   is both fragile and unnecessary — Chromium only exposes its own widgets to
//!   AT-SPI when it detects an assistive technology, and we never need it to.
//! * **Content** — the page. Rung 1, from `Accessibility.getFullAXTree`, whose
//!   `backendDOMNodeId` gives exactly the stable per-element identity anchors
//!   need.
//!
//! Native dialogs (a GTK file chooser, print) are separate toplevels and get
//! probed independently. Electron has no chrome surface at all.
//!
//! **The browser is never launched by this module.** `Browser::launch` would
//! spawn it with the harness's environment — the user's session — which is the
//! one thing the Stage exists to prevent. The browser is started by
//! `Stage::spawn` with the stage environment and *connected to* here.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use chromiumoxide::Browser;
use chromiumoxide::cdp::browser_protocol::dom::BackendNodeId;
use chromiumoxide::cdp::browser_protocol::network::RequestId;
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use futures::StreamExt;

use crate::model::{Caps, Node, NodeState, Role, Rung, Snapshot, SurfaceId};
use crate::program::{Settle, SettleKind, SettleOutcome, Step, StepError};
use crate::surface::{SettleWatch, Surface};

/// How long to wait for the browser to write its DevTools port.
const PORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Connect to a browser the stage already launched.
///
/// Chromium writes `DevToolsActivePort` into its user-data dir once its listener
/// binds. Line 1 is the port and **line 2 is the browser's websocket path**, so
/// the endpoint can be assembled directly with no HTTP request at all.
///
/// That matters: the obvious route is `GET /json/version`, but Chromium's
/// DevTools HTTP endpoint holds the connection open regardless of
/// `Connection: close`, so a naive read-to-end blocks forever. Reading the file
/// avoids the whole problem and one dependency with it.
///
/// Polling for the file is also how the port is learned without ever specifying
/// one — a fixed port collides with the user's own browser and with a second
/// stage.
pub async fn connect(user_data_dir: &std::path::Path) -> Result<Browser, StepError> {
    let port_file = user_data_dir.join("DevToolsActivePort");
    let deadline = tokio::time::Instant::now() + PORT_TIMEOUT;

    let endpoint = loop {
        if let Ok(contents) = tokio::fs::read_to_string(&port_file).await {
            let mut lines = contents.lines();
            if let (Some(port), Some(path)) = (lines.next(), lines.next())
                && let Ok(port) = port.trim().parse::<u16>()
            {
                break format!("ws://127.0.0.1:{port}{}", path.trim());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(StepError::Backend(format!(
                "browser never wrote {}; was it launched with --remote-debugging-port=0?",
                port_file.display()
            )));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };

    let (browser, mut handler) = Browser::connect(&endpoint)
        .await
        .map_err(|error| StepError::Backend(format!("connect to {endpoint}: {error}")))?;
    // The handler stream drives every CDP message; dropping it silently stops
    // all traffic, so it is spawned and kept alive for the browser's lifetime.
    tokio::spawn(async move { while handler.next().await.is_some() {} });
    Ok(browser)
}

/// Connect to a browser the user already has open.
///
/// The whole value of this is credentials. The stage's browser starts with an
/// empty profile: no sessions, no cookies, no saved passwords, and every task
/// that touches a logged-in site begins at a login page. A browser the user
/// already has open is already logged into everything they use — which is
/// exactly the reasoning behind sharing `$HOME` rather than sandboxing it.
///
/// **This is the one place the isolation property does not hold, and it is not
/// a leak — it is what was asked for.** A user attaching to their own browser
/// has said so explicitly by launching it with a debugging port; that is not a
/// thing that happens by accident. The stage's guarantees about focus and input
/// still hold, because a CDP page is driven by protocol message and never
/// through the seat. What does *not* hold is the private accessibility tree:
/// this browser is the user's, its tabs are the user's, and closing one closes
/// theirs.
///
/// Endpoint rather than profile directory, because a running browser's
/// `DevToolsActivePort` may not be readable by us and the port is the thing the
/// user actually knows.
pub async fn attach_to_endpoint(endpoint: &str) -> Result<Browser, StepError> {
    // A bare port, because that is the thing a person actually knows. They
    // started the browser with `--remote-debugging-port=9222`; nobody knows the
    // websocket path, and requiring it would mean digging through
    // `chrome://inspect` before the agent could do anything. Resolved here
    // rather than at the tool boundary so every caller gets it.
    let endpoint = if let Ok(port) = endpoint.trim().parse::<u16>() {
        &resolve_port(port).await?
    } else {
        endpoint
    };

    // Loopback only, for the same reason adapters are: a debugging endpoint is
    // total control of a browser, and a remote one is somebody else's.
    let host_ok = endpoint
        .strip_prefix("ws://")
        .map(|rest| {
            rest.starts_with("127.0.0.1:")
                || rest.starts_with("localhost:")
                || rest.starts_with("[::1]:")
        })
        .unwrap_or(false);
    if !host_ok {
        return Err(StepError::Backend(format!(
            "{endpoint:?} is not a loopback ws:// devtools endpoint. Attaching means \
             taking control of a browser, and that is only offered for one on this machine."
        )));
    }

    let (browser, mut handler) = Browser::connect(endpoint).await.map_err(|error| {
        StepError::Backend(format!(
            "connect to {endpoint}: {error}. Start the browser with \
             --remote-debugging-port=<port> and pass the ws:// url it prints."
        ))
    })?;
    tokio::spawn(async move { while handler.next().await.is_some() {} });
    Ok(browser)
}

/// Turn a debugging port into the browser's websocket endpoint.
///
/// `GET /json/version` carries `webSocketDebuggerUrl`. Read with a hand-rolled
/// request and a hard read limit rather than an HTTP client: Chromium's DevTools
/// endpoint holds the connection open regardless of `Connection: close`, so a
/// read-to-end never returns — the same trap that made `connect` read
/// `DevToolsActivePort` from disk instead.
async fn resolve_port(port: u16) -> Result<String, StepError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|error| {
            StepError::Backend(format!(
                "nothing is listening on port {port}: {error}. Start the browser with \
                 --remote-debugging-port={port} and try again."
            ))
        })?;
    socket
        .write_all(b"GET /json/version HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .await
        .map_err(|error| {
            StepError::Backend(format!("ask port {port} for its endpoint: {error}"))
        })?;

    // Bounded read, bounded wait. Both matter: this is talking to something we
    // have not identified yet, and it may not be a browser at all.
    let mut body = Vec::new();
    let mut chunk = [0u8; 4096];
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let read = tokio::time::timeout_at(deadline, socket.read(&mut chunk))
            .await
            .map_err(|_| {
                StepError::Backend(format!(
                    "port {port} did not answer like a devtools endpoint"
                ))
            })?
            .map_err(|error| StepError::Backend(format!("read from port {port}: {error}")))?;
        if read == 0 || body.len() > 64 * 1024 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
        if body.windows(4).any(|window| window == b"}\r\n\r") || body.ends_with(b"}") {
            break;
        }
    }

    let text = String::from_utf8_lossy(&body);
    let json = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or(&text);
    let parsed: serde_json::Value = serde_json::from_str(json.trim()).map_err(|_| {
        StepError::Backend(format!(
            "port {port} is listening but is not a browser devtools endpoint"
        ))
    })?;
    parsed
        .get("webSocketDebuggerUrl")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            StepError::Backend(format!(
                "the devtools endpoint on port {port} named no websocket url"
            ))
        })
}

/// The set of requests a page currently has outstanding.
///
/// A set rather than a counter. Chromium emits an extra `requestWillBeSent` per
/// redirect hop with no matching completion, so an incrementing counter drifts
/// permanently upward — after a few redirects it never returns below the idle
/// threshold and every `quiet` settle burns its full timeout.
type InFlight = Arc<Mutex<HashSet<RequestId>>>;

/// Requests still allowed in flight for a page to count as idle.
///
/// Not zero: analytics beacons, long-poll channels and streaming connections
/// never finish, and a page with one of those would never settle. Two is the
/// conventional tolerance and matches what headless testing tools use.
const IDLE_INFLIGHT: u32 = 2;
/// How long the in-flight count must stay at or below the threshold.
const IDLE_QUIET: std::time::Duration = std::time::Duration::from_millis(500);

/// A page's content, driven through CDP.
pub struct CdpPage {
    id: SurfaceId,
    page: chromiumoxide::Page,
    /// Requests started but not yet finished or failed.
    ///
    /// Maintained from the CDP event stream rather than inferred, which is what
    /// makes `networkIdle` mean what it says: `document.readyState` reaches
    /// `complete` once the initial document is parsed and says nothing at all
    /// about the XHR an SPA fires immediately afterwards.
    inflight: InFlight,
    /// Held so the connection outlives the surface.
    ///
    /// Dropping the `Browser` closes the websocket and every page with it, so
    /// ownership has to live somewhere. Here is the honest place: the surface is
    /// exactly what needs the connection, and tying the two together means a
    /// closed surface releases the connection rather than leaking it.
    _browser: Option<Arc<Browser>>,
}

impl CdpPage {
    pub async fn attach(
        id: impl Into<String>,
        page: chromiumoxide::Page,
    ) -> Result<Self, StepError> {
        Self::build(id, page, None).await
    }

    /// Attach and take responsibility for keeping the connection open.
    pub async fn attach_owned(
        id: impl Into<String>,
        page: chromiumoxide::Page,
        browser: Arc<Browser>,
    ) -> Result<Self, StepError> {
        Self::build(id, page, Some(browser)).await
    }

    async fn build(
        id: impl Into<String>,
        page: chromiumoxide::Page,
        browser: Option<Arc<Browser>>,
    ) -> Result<Self, StepError> {
        let inflight: InFlight = Arc::new(Mutex::new(HashSet::new()));
        // Best-effort: a page whose network domain will not enable still works,
        // it just falls back to readiness polling for `quiet`.
        if let Err(error) = track_network(&page, Arc::clone(&inflight)).await {
            eprintln!("artist: network tracking unavailable for this page: {error}");
        }
        Ok(Self {
            id: SurfaceId::new(id),
            page,
            inflight,
            _browser: browser,
        })
    }

    /// Requests currently in flight, for diagnostics and the inspector.
    pub fn inflight(&self) -> u32 {
        self.inflight.lock().unwrap().len() as u32
    }

    pub fn page(&self) -> &chromiumoxide::Page {
        &self.page
    }
}

/// Enable the network domain and track which requests are outstanding.
///
/// Four listeners. A request leaves flight either by finishing or by failing, so
/// a page that only watched completions would never settle after a blocked or
/// aborted request; and a main-frame navigation abandons whatever was still
/// outstanding, which nothing else will ever resolve.
async fn track_network(page: &chromiumoxide::Page, inflight: InFlight) -> Result<(), StepError> {
    use chromiumoxide::cdp::browser_protocol::network::{
        EnableParams, EventLoadingFailed, EventLoadingFinished, EventRequestWillBeSent,
    };
    use chromiumoxide::cdp::browser_protocol::page::EventFrameNavigated;

    page.execute(EnableParams::default())
        .await
        .map_err(|error| StepError::Backend(format!("enable network events: {error}")))?;

    let started = page
        .event_listener::<EventRequestWillBeSent>()
        .await
        .map_err(|error| StepError::Backend(format!("listen for requests: {error}")))?;
    let finished = page
        .event_listener::<EventLoadingFinished>()
        .await
        .map_err(|error| StepError::Backend(format!("listen for completions: {error}")))?;
    let failed = page
        .event_listener::<EventLoadingFailed>()
        .await
        .map_err(|error| StepError::Backend(format!("listen for failures: {error}")))?;
    let navigated = page
        .event_listener::<EventFrameNavigated>()
        .await
        .map_err(|error| StepError::Backend(format!("listen for navigations: {error}")))?;

    tokio::spawn({
        let set = Arc::clone(&inflight);
        async move {
            let mut started = started;
            while let Some(event) = started.next().await {
                note_started(&set, &event.request_id, event.redirect_response.is_some());
            }
        }
    });
    tokio::spawn({
        let set = Arc::clone(&inflight);
        async move {
            let mut finished = finished;
            while let Some(event) = finished.next().await {
                note_settled(&set, &event.request_id);
            }
        }
    });
    tokio::spawn({
        let set = Arc::clone(&inflight);
        async move {
            let mut failed = failed;
            while let Some(event) = failed.next().await {
                note_settled(&set, &event.request_id);
            }
        }
    });
    tokio::spawn({
        let set = Arc::clone(&inflight);
        async move {
            let mut navigated = navigated;
            while let Some(event) = navigated.next().await {
                note_navigated(&set, event.frame.parent_id.is_none());
            }
        }
    });
    Ok(())
}

/// A request has been announced.
///
/// Chromium re-announces a request for every redirect hop, and the hop that
/// carries `redirectResponse` never gets its own completion. Counting it was an
/// unmatched increment: after a login or OAuth flow the count sat permanently
/// above the idle threshold and `quiet` could never be satisfied again — every
/// settle from then on burned its full timeout.
///
/// A set makes the whole class of drift impossible: the same request id
/// re-inserted is still one request.
fn note_started(inflight: &InFlight, id: &RequestId, is_redirect_hop: bool) {
    if is_redirect_hop {
        return;
    }
    inflight.lock().unwrap().insert(id.clone());
}

/// A request finished or failed. Both leave flight.
fn note_settled(inflight: &InFlight, id: &RequestId) {
    inflight.lock().unwrap().remove(id);
}

/// A frame navigated.
///
/// A main-frame navigation discards the old document, so anything still
/// outstanding for it will never complete and would otherwise keep the page
/// "busy" for the rest of the session. Subframe navigations do not have that
/// effect and must not clear the set.
fn note_navigated(inflight: &InFlight, is_main_frame: bool) {
    if is_main_frame {
        inflight.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod inflight_tests {
    use super::*;

    fn book() -> InFlight {
        Arc::new(Mutex::new(HashSet::new()))
    }

    fn id(value: &str) -> RequestId {
        RequestId::new(value)
    }

    fn count(inflight: &InFlight) -> usize {
        inflight.lock().unwrap().len()
    }

    #[test]
    fn a_redirect_chain_leaves_nothing_outstanding() {
        // What Chromium actually emits for `/a -> /b -> /c -> 200`: the same
        // request id announced four times, the first three carrying a
        // `redirectResponse`, and exactly one completion at the end.
        let inflight = book();
        note_started(&inflight, &id("req-1"), false);
        for _ in 0..3 {
            note_started(&inflight, &id("req-1"), true);
        }
        assert_eq!(count(&inflight), 1, "a redirect chain is one request");

        note_settled(&inflight, &id("req-1"));
        assert_eq!(
            count(&inflight),
            0,
            "the counter used to sit at 4 here, permanently above the idle threshold"
        );
    }

    #[test]
    fn a_completion_for_a_request_we_never_saw_start_is_harmless() {
        // Attaching mid-flight is normal: the page was already loading when the
        // surface connected. A counter went negative here, or saturated at zero
        // and then under-counted the next real request.
        let inflight = book();
        note_settled(&inflight, &id("before-we-attached"));
        assert_eq!(count(&inflight), 0);

        note_started(&inflight, &id("req-1"), false);
        assert_eq!(count(&inflight), 1);
    }

    #[test]
    fn a_failure_leaves_flight_exactly_like_a_completion() {
        let inflight = book();
        note_started(&inflight, &id("req-1"), false);
        note_settled(&inflight, &id("req-1"));
        assert_eq!(count(&inflight), 0, "a blocked request must not pin a page");
    }

    #[test]
    fn a_main_frame_navigation_abandons_what_the_old_document_left_behind() {
        let inflight = book();
        note_started(&inflight, &id("long-poll"), false);
        note_started(&inflight, &id("analytics"), false);
        assert_eq!(count(&inflight), 2);

        // A subframe navigating says nothing about the top-level document.
        note_navigated(&inflight, false);
        assert_eq!(count(&inflight), 2);

        // The main frame navigating means those two will never complete.
        note_navigated(&inflight, true);
        assert_eq!(count(&inflight), 0);
    }

    #[test]
    fn independent_requests_are_counted_independently() {
        let inflight = book();
        note_started(&inflight, &id("a"), false);
        note_started(&inflight, &id("b"), false);
        note_started(&inflight, &id("c"), false);
        assert_eq!(count(&inflight), 3);
        note_settled(&inflight, &id("b"));
        assert_eq!(count(&inflight), 2);
    }
}

/// A browser's own chrome: tabs and navigation, at rung 0.
///
/// Rung 0 rather than 2 because `Target.*` and `Page.navigate` are ordinary
/// programmatic calls. Chromium only exposes its tab strip and omnibox to
/// AT-SPI when it detects an assistive technology, and driving those widgets by
/// clicking would be fragile for no gain — closing a tab is a method call, not a
/// gesture.
pub struct CdpChrome {
    id: SurfaceId,
    browser: Arc<Browser>,
}

impl CdpChrome {
    pub fn new(id: impl Into<String>, browser: Arc<Browser>) -> Self {
        Self {
            id: SurfaceId::new(id),
            browser,
        }
    }
}

#[async_trait::async_trait]
impl Surface for CdpChrome {
    fn id(&self) -> &SurfaceId {
        &self.id
    }

    fn rung(&self) -> Rung {
        Rung::Programmatic
    }

    fn caps(&self) -> Caps {
        Caps {
            // "Clicking" a tab here means activating it — a method call.
            click: true,
            type_text: false,
            key: false,
            scroll: false,
            pixels: false,
        }
    }

    fn title(&self) -> String {
        "browser".to_owned()
    }

    /// Open tabs, one node each.
    async fn snapshot(&self) -> Result<Snapshot, StepError> {
        let pages = self
            .browser
            .pages()
            .await
            .map_err(|error| StepError::Backend(format!("list tabs: {error}")))?;

        let mut nodes = Vec::new();
        for page in pages {
            let target = page.target_id().inner().clone();
            // A tab with no title yet is still a tab; falling back to the URL
            // keeps it addressable rather than nameless.
            let name = match page.get_title().await {
                Ok(Some(title)) if !title.trim().is_empty() => title,
                _ => page
                    .url()
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "untitled".to_owned()),
            };
            nodes.push(
                Node::new(format!("cdp:target:{target}"), Role::Tab, name)
                    .with_actions(["activate", "close"]),
            );
        }
        Ok(Snapshot::new(nodes))
    }

    async fn watch(&self, _settle: &Settle) -> Result<SettleWatch, StepError> {
        // Tab operations complete when their call returns.
        Ok(SettleWatch::ready(SettleOutcome::Settled { after_ms: 0 }))
    }

    async fn apply(&self, step: &Step, node: Option<&Node>) -> Result<(), StepError> {
        // `navigate` names no element: it acts on whichever tab is current, and
        // is the whole reason this rung exists. Requiring a tab anchor for it
        // would mean an observation just to open a URL.
        if let Step::Navigate { url } = step {
            return self.navigate(url).await;
        }

        let Some(node) = node else {
            return Err(StepError::Backend("this step needs a tab".into()));
        };
        let target = node
            .binding
            .as_str()
            .strip_prefix("cdp:target:")
            .ok_or_else(|| StepError::Backend("not a browser tab".into()))?
            .to_owned();

        match step {
            Step::Click(_) => self.tab_action(&target, "activate").await,
            // The verbs the tab nodes advertise. Before this they were rendered
            // and could never be run.
            Step::Invoke { action, .. } => self.tab_action(&target, action).await,
            other => Err(StepError::Unsupported {
                anchor: other
                    .target()
                    .map(|target| target.anchor.clone())
                    .unwrap_or_default(),
                role: node.role.label().to_owned(),
                name: node.name.clone(),
                action: other.action(),
            }),
        }
    }
}

impl CdpChrome {
    /// Open a URL in the current tab, or in a new one if there is none.
    async fn navigate(&self, url: &str) -> Result<(), StepError> {
        use chromiumoxide::cdp::browser_protocol::page::NavigateParams;

        let pages = self
            .browser
            .pages()
            .await
            .map_err(|error| StepError::Backend(format!("list tabs: {error}")))?;
        match pages.into_iter().next() {
            Some(page) => page
                .execute(NavigateParams::new(url.to_owned()))
                .await
                .map(|_| ())
                .map_err(|error| StepError::Backend(format!("navigate to {url}: {error}"))),
            None => self
                .browser
                .new_page(url)
                .await
                .map(|_| ())
                .map_err(|error| StepError::Backend(format!("open {url}: {error}"))),
        }
    }

    async fn tab_action(&self, target: &str, action: &str) -> Result<(), StepError> {
        use chromiumoxide::cdp::browser_protocol::target::{
            ActivateTargetParams, CloseTargetParams,
        };

        let pages = self
            .browser
            .pages()
            .await
            .map_err(|error| StepError::Backend(format!("list tabs: {error}")))?;
        let page = pages
            .into_iter()
            .find(|page| page.target_id().inner() == target)
            .ok_or_else(|| StepError::Backend(format!("no tab {target}")))?;
        let id = page.target_id().clone();

        match action.trim().to_ascii_lowercase().as_str() {
            "activate" | "click" | "focus" => page
                .execute(ActivateTargetParams::new(id))
                .await
                .map(|_| ())
                .map_err(|error| StepError::Backend(format!("activate tab: {error}"))),
            "close" => page
                .execute(CloseTargetParams::new(id))
                .await
                .map(|_| ())
                .map_err(|error| StepError::Backend(format!("close tab: {error}"))),
            other => Err(StepError::Backend(format!(
                "a tab has no action {other:?} — it declares activate and close"
            ))),
        }
    }
}

/// Map a CDP accessibility role onto our normalized vocabulary.
fn role_for(role: &str) -> Role {
    match role {
        "button" | "PushButton" => Role::Button,
        "link" => Role::Link,
        "textbox" | "searchbox" | "TextField" => Role::TextBox,
        "checkbox" => Role::CheckBox,
        "radio" => Role::RadioButton,
        "combobox" | "listbox" => Role::ComboBox,
        "listitem" | "option" => Role::ListItem,
        "menuitem" => Role::MenuItem,
        "tab" => Role::Tab,
        "heading" => Role::Heading,
        "img" | "image" => Role::Image,
        "row" => Role::Row,
        "StaticText" | "text" | "paragraph" => Role::Text,
        other => Role::Other(other.to_owned()),
    }
}

/// Whether a node is something text can be typed into.
///
/// Roles rather than a capability probe, because the answer is needed *before*
/// dispatch — the point is to refuse with a useful message rather than let the
/// browser refuse with an unhelpful one. `Other` is allowed through: a custom
/// element with a role we do not model may still be an editable host, and
/// guessing "no" there would make the tool refuse things that work.
fn accepts_text(node: &Node) -> bool {
    matches!(
        node.role,
        Role::TextBox | Role::ComboBox | Role::Other(_) | Role::Window
    )
}

/// Every frame below the root, depth-first.
///
/// Used to ask for each frame's accessibility tree in turn. Returns an empty
/// list rather than an error when the frame tree cannot be read: a page with no
/// frames is the common case and indistinguishable from a failure here, and
/// neither is worth failing an observation over.
async fn child_frames(
    page: &chromiumoxide::Page,
) -> Vec<chromiumoxide::cdp::browser_protocol::page::FrameId> {
    use chromiumoxide::cdp::browser_protocol::page::{FrameTree, GetFrameTreeParams};

    let Ok(tree) = page.execute(GetFrameTreeParams::default()).await else {
        return Vec::new();
    };

    fn walk(node: &FrameTree, out: &mut Vec<chromiumoxide::cdp::browser_protocol::page::FrameId>) {
        for child in node.child_frames.iter().flatten() {
            out.push(child.frame.id.clone());
            walk(child, out);
        }
    }

    let mut frames = Vec::new();
    walk(&tree.result.frame_tree, &mut frames);
    frames
}

/// The accessibility nodes in document order.
///
/// `getFullAXTree` returns them **breadth-first**: every child of the root, then
/// every grandchild. So a page of `<h2>Refund policy</h2><p>…</p><h2>Delivery…`
/// arrives as *heading, heading, heading, text, text, text* — and anything that
/// reads a node together with its neighbour reads the wrong neighbour.
///
/// That broke `extract` in a way no unit test could see, because the synthetic
/// snapshots those tests build are already in document order: a heading's
/// "section" was whatever followed it, which in breadth-first order is the next
/// heading, so every section came out empty. Found by asking a real page for its
/// refund policy and getting one sentence of it.
///
/// Walked from the root through `child_ids`, so the order is the document's.
/// Nodes unreachable from the root are appended rather than dropped — an
/// unreachable node is still a node, and losing one silently would be a worse
/// bug than mis-ordering it.
fn document_order(
    nodes: &[chromiumoxide::cdp::browser_protocol::accessibility::AxNode],
) -> Vec<&chromiumoxide::cdp::browser_protocol::accessibility::AxNode> {
    use std::collections::HashMap;

    let by_id: HashMap<_, _> = nodes.iter().map(|node| (&node.node_id, node)).collect();
    let mut ordered = Vec::with_capacity(nodes.len());
    let mut seen = std::collections::HashSet::new();

    // The root is the node nothing claims as a child. Falling back to the first
    // node keeps a malformed tree usable rather than empty.
    let children: std::collections::HashSet<_> = nodes
        .iter()
        .flat_map(|node| node.child_ids.iter().flatten())
        .collect();
    let roots: Vec<_> = nodes
        .iter()
        .filter(|node| !children.contains(&node.node_id))
        .collect();

    let mut stack: Vec<_> = roots.into_iter().rev().collect();
    while let Some(node) = stack.pop() {
        if !seen.insert(&node.node_id) {
            continue;
        }
        ordered.push(node);
        // Reversed onto the stack so they pop in the order the document has
        // them; without this every sibling list comes out backwards.
        for child in node.child_ids.iter().flatten().rev() {
            if let Some(child) = by_id.get(child) {
                stack.push(child);
            }
        }
    }

    for node in nodes {
        if !seen.contains(&node.node_id) {
            ordered.push(node);
        }
    }
    ordered
}

/// Pull the accessibility tree and normalize it into nodes.
///
/// `backendDOMNodeId` is the binding: stable for the life of the element, which
/// is exactly the identity contract anchors need, and quite unlike a CSS path
/// or an index that shifts when the page re-renders.
async fn ax_nodes(page: &chromiumoxide::Page) -> Result<Vec<Node>, StepError> {
    use chromiumoxide::cdp::browser_protocol::accessibility::GetFullAxTreeParams;

    // Every frame, not just the root one. `getFullAXTree` defaults to the root
    // frame's document, so an `<iframe>` contributes only *itself* — the frame
    // element — and nothing inside it. That silently hides the contents of every
    // payment widget, embedded editor, consent dialog and OAuth flow on the web:
    // the tree looks healthy, the button is simply not in it, and the model
    // concludes the page does not have one.
    //
    // Found by a test that put a button in an iframe and looked for it. The
    // `file://` fixtures could not have found it, because a frame loaded from
    // another path is another origin.
    let mut trees = Vec::new();
    match page.execute(GetFullAxTreeParams::default()).await {
        Ok(tree) => trees.push(tree),
        Err(error) => {
            return Err(StepError::Backend(format!("accessibility tree: {error}")));
        }
    }
    for frame in child_frames(page).await {
        // A frame that fails is skipped rather than fatal: a cross-origin frame
        // may be a separate target we cannot read, and losing one frame's
        // contents is far better than losing the page.
        if let Ok(tree) = page
            .execute(GetFullAxTreeParams::builder().frame_id(frame).build())
            .await
        {
            trees.push(tree);
        }
    }

    let mut nodes = Vec::new();
    for ax in trees
        .iter()
        .flat_map(|tree| document_order(&tree.result.nodes))
    {
        if ax.ignored {
            continue;
        }
        let Some(backend_id) = ax.backend_dom_node_id else {
            continue;
        };
        let role = ax
            .role
            .as_ref()
            .and_then(|value| value.value.as_ref())
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let name = ax
            .name
            .as_ref()
            .and_then(|value| value.value.as_ref())
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_owned();
        let value = ax
            .value
            .as_ref()
            .and_then(|value| value.value.as_ref())
            .and_then(|value| value.as_str())
            .map(str::to_owned);

        let role = role_for(role);
        // A nameless, valueless static text node is invisible to the model and
        // would only consume a handle.
        if !role.is_interactive() && name.is_empty() && value.is_none() {
            continue;
        }

        let mut node = Node::new(format!("cdp:node:{}", backend_id.inner()), role, name);
        if let Some(value) = value {
            node = node.with_value(value);
        }
        nodes.push(node.with_state(state_of(ax)));
    }
    Ok(nodes)
}

/// Read element state out of the AX node's properties.
///
/// This is what makes a click verifiable. `state` feeds [`Node::digest`], so
/// without it checking a checkbox, focusing a field or disabling a submit button
/// changes nothing the model can see: no flags on the line and no `~` delta.
/// The model then cannot tell whether its click landed, and a click on a
/// disabled control reports `ok`.
///
/// The properties are already in the `getFullAXTree` response — this costs
/// nothing but reading them.
fn state_of(ax: &chromiumoxide::cdp::browser_protocol::accessibility::AxNode) -> NodeState {
    use chromiumoxide::cdp::browser_protocol::accessibility::AxPropertyName;

    let mut state = NodeState::default();
    let Some(properties) = &ax.properties else {
        return state;
    };
    for property in properties {
        let raw = property.value.value.as_ref();
        // ARIA states are tristate: `checked` arrives as the *string* "true",
        // "false" or "mixed", not as a JSON bool. Treating "mixed" as unchecked
        // is right — a partially-checked box is not checked.
        let truthy = match raw {
            Some(serde_json::Value::Bool(value)) => *value,
            Some(serde_json::Value::String(value)) => value == "true",
            _ => false,
        };
        match property.name {
            AxPropertyName::Focused => state.focused = truthy,
            AxPropertyName::Disabled => state.disabled = truthy,
            AxPropertyName::Checked | AxPropertyName::Pressed => state.checked = truthy,
            AxPropertyName::Expanded => state.expanded = truthy,
            AxPropertyName::Selected => state.selected = truthy,
            AxPropertyName::Hidden => state.offscreen = truthy,
            _ => {}
        }
    }
    state
}

#[async_trait::async_trait]
impl Surface for CdpPage {
    fn id(&self) -> &SurfaceId {
        &self.id
    }

    fn rung(&self) -> Rung {
        Rung::Engine
    }

    fn caps(&self) -> Caps {
        Caps {
            click: true,
            type_text: true,
            key: true,
            scroll: true,
            pixels: true,
        }
    }

    fn title(&self) -> String {
        self.id.as_str().to_owned()
    }

    async fn snapshot(&self) -> Result<Snapshot, StepError> {
        Ok(Snapshot::new(ax_nodes(&self.page).await?))
    }

    /// Rung 3, from the engine rather than the compositor.
    ///
    /// A page renders inside a window that may also contain browser chrome, so
    /// capturing the window would hand the model a picture with a tab strip in
    /// it. `Page.captureScreenshot` is the viewport alone, and it works for a
    /// browser we merely attached to, with no stage involved at all.
    async fn pixels(&self) -> Result<Option<crate::model::Frame>, StepError> {
        use chromiumoxide::cdp::browser_protocol::page::{
            CaptureScreenshotFormat, CaptureScreenshotParams,
        };

        let shot = self
            .page
            .execute(
                CaptureScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Png)
                    .build(),
            )
            .await
            .map_err(|error| StepError::Backend(format!("capture screenshot: {error}")))?;

        let png = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &shot.data)
            .map_err(|error| StepError::Backend(format!("decode screenshot: {error}")))?;

        Ok(Some(
            crate::model::Frame::from_png(&png).map_err(StepError::Backend)?,
        ))
    }

    async fn watch(&self, settle: &Settle) -> Result<SettleWatch, StepError> {
        let settle = settle.clone();
        let page = self.page.clone();
        let inflight = Arc::clone(&self.inflight);
        let started = std::time::Instant::now();

        Ok(SettleWatch(Box::new(Box::pin(async move {
            match settle.until {
                SettleKind::None => return SettleOutcome::Settled { after_ms: 0 },
                SettleKind::Anchor(_) => return SettleOutcome::Unsupported,
                SettleKind::Quiet | SettleKind::NetworkIdle => {}
            }
            // Both the document being parsed *and* the network having drained.
            // `readyState` alone reaches `complete` as soon as the initial
            // document is done and is blind to the XHR an SPA fires immediately
            // after — which is exactly the interaction most of a real session
            // consists of. `wait_for_navigation` has the same blind spot.
            let deadline = started + std::time::Duration::from_millis(settle.timeout_ms);
            let mut stable_since: Option<std::time::Instant> = None;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let ready = page
                    .execute(
                        EvaluateParams::builder()
                            .expression("document.readyState")
                            .build()
                            .expect("static expression"),
                    )
                    .await
                    .ok()
                    .and_then(|result| result.result.result.value.clone())
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default();

                let quiet =
                    ready == "complete" && inflight.lock().unwrap().len() as u32 <= IDLE_INFLIGHT;

                if quiet {
                    let since = *stable_since.get_or_insert_with(std::time::Instant::now);
                    if since.elapsed() >= IDLE_QUIET {
                        return SettleOutcome::Settled {
                            after_ms: started.elapsed().as_millis() as u64,
                        };
                    }
                } else {
                    stable_since = None;
                }

                if std::time::Instant::now() >= deadline {
                    return SettleOutcome::TimedOut {
                        after_ms: started.elapsed().as_millis() as u64,
                    };
                }
            }
        }))))
    }

    async fn apply(&self, step: &Step, node: Option<&Node>) -> Result<(), StepError> {
        let backend_id = |node: Option<&Node>| -> Result<i64, StepError> {
            node.and_then(|node| node.binding.as_str().strip_prefix("cdp:node:"))
                .and_then(|id| id.parse::<i64>().ok())
                .ok_or_else(|| StepError::Backend("step needs a page element".into()))
        };

        match step {
            Step::Click(_) => {
                // Scroll into view, then click at the element's own centre —
                // computed here from CDP geometry, never supplied by the model.
                let id = backend_id(node)?;
                self.click_backend_node(id).await
            }
            Step::Type { text, clear, .. } => {
                // Refused here rather than by the browser. A page labels its
                // input with a `<label>`, and in document order that label's
                // text comes *first* — so naming "Email" and typing into the
                // first match hits the label, not the field. CDP answers that
                // with `Error -32000: Node is not an Element`, which tells the
                // model nothing it can act on. This says what the element is
                // and what to do instead.
                if let Some(target) = node
                    && !accepts_text(target)
                {
                    return Err(StepError::Unsupported {
                        anchor: target.binding.as_str().to_owned(),
                        role: target.role.label().to_owned(),
                        name: target.name.clone(),
                        action: "type",
                    });
                }
                let id = backend_id(node)?;
                self.focus_backend_node(id).await?;
                if *clear {
                    self.clear_backend_node(id).await?;
                }
                self.page
                    .execute(
                        chromiumoxide::cdp::browser_protocol::input::InsertTextParams::builder()
                            .text(text)
                            .build()
                            .map_err(StepError::Backend)?,
                    )
                    .await
                    .map(|_| ())
                    .map_err(|error| StepError::Backend(format!("type: {error}")))
            }
            Step::Key(press) => self.press_key(press.chord()).await,
            // Scrolling the named container when there is one. A virtualized
            // list, a chat log and a modal body all scroll independently of the
            // document, so scrolling the window instead moved nothing and
            // reported `ok`.
            Step::Scroll { amount, .. } => match node {
                None => {
                    let expression = format!("window.scrollBy(0, {})", amount * 100);
                    self.page
                        .execute(
                            EvaluateParams::builder()
                                .expression(expression)
                                .build()
                                .map_err(StepError::Backend)?,
                        )
                        .await
                        .map(|_| ())
                        .map_err(|error| StepError::Backend(format!("scroll: {error}")))
                }
                Some(_) => {
                    let id = backend_id(node)?;
                    self.scroll_backend_node(id, amount * 100).await
                }
            },
            Step::Navigate { url } => {
                use chromiumoxide::cdp::browser_protocol::page::NavigateParams;

                self.page
                    .execute(NavigateParams::new(url.clone()))
                    .await
                    .map(|_| ())
                    .map_err(|error| StepError::Backend(format!("navigate to {url}: {error}")))
            }
            Step::Back { .. } => self.history(-1).await,
            Step::Forward { .. } => self.history(1).await,
            // The page rung has no declared per-element verbs — every element
            // is reached the same way — so `invoke` here is a routing mistake
            // rather than a missing feature, and says so.
            Step::Invoke { target, action } => Err(StepError::Unsupported {
                anchor: target.anchor.clone(),
                role: node
                    .map(|node| node.role.label().to_owned())
                    .unwrap_or_default(),
                name: node.map(|node| node.name.clone()).unwrap_or_default(),
                action: "invoke",
            })
            .map_err(|error| match error {
                StepError::Unsupported {
                    anchor, role, name, ..
                } => StepError::Backend(format!(
                    "{anchor} ({role} {name:?}) has no action {action:?} — page elements are \
                     driven with click, type, key and scroll"
                )),
                other => other,
            }),
        }
    }
}

impl CdpPage {
    /// Empty a form control before typing into it.
    ///
    /// Set through the DOM rather than by sending `ctrl+a` then Delete: the
    /// keyboard route needs a working modifier path, is at the mercy of the
    /// page's own key handlers, and costs three extra round trips. Dispatching
    /// `input` and `change` afterwards is what makes frameworks notice — React
    /// in particular ignores a value assignment that fires no event.
    async fn clear_backend_node(&self, backend_id: i64) -> Result<(), StepError> {
        use chromiumoxide::cdp::browser_protocol::dom::{BackendNodeId, ResolveNodeParams};
        use chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnParams;

        let resolved = self
            .page
            .execute(
                ResolveNodeParams::builder()
                    .backend_node_id(BackendNodeId::new(backend_id))
                    .build(),
            )
            .await
            .map_err(|error| StepError::Backend(format!("resolve element: {error}")))?;
        let Some(object_id) = resolved.result.object.object_id.clone() else {
            return Err(StepError::Backend(
                "the element could not be resolved to clear it".into(),
            ));
        };

        const CLEAR: &str = r#"function () {
            if (this.isContentEditable) { this.textContent = ''; }
            else if ('value' in this) { this.value = ''; }
            this.dispatchEvent(new Event('input', { bubbles: true }));
            this.dispatchEvent(new Event('change', { bubbles: true }));
        }"#;

        self.page
            .execute(
                CallFunctionOnParams::builder()
                    .function_declaration(CLEAR)
                    .object_id(object_id)
                    .build()
                    .map_err(StepError::Backend)?,
            )
            .await
            .map(|_| ())
            .map_err(|error| StepError::Backend(format!("clear element: {error}")))
    }

    /// Scroll one element by a pixel delta.
    ///
    /// Walks up to the nearest actually-scrollable ancestor first. The
    /// accessibility tree names the thing a person would point at — a row, a
    /// message — while the element with the overflow is usually a container a
    /// few levels up that has no accessible name at all and so no anchor the
    /// model could ever cite.
    async fn scroll_backend_node(&self, backend_id: i64, delta: i32) -> Result<(), StepError> {
        use chromiumoxide::cdp::browser_protocol::dom::{BackendNodeId, ResolveNodeParams};
        use chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnParams;

        let resolved = self
            .page
            .execute(
                ResolveNodeParams::builder()
                    .backend_node_id(BackendNodeId::new(backend_id))
                    .build(),
            )
            .await
            .map_err(|error| StepError::Backend(format!("resolve element: {error}")))?;
        let Some(object_id) = resolved.result.object.object_id.clone() else {
            return Err(StepError::Backend(
                "the element could not be resolved to scroll it".into(),
            ));
        };

        const SCROLL: &str = r#"function (delta) {
            let node = this;
            while (node && node !== document.body) {
                const style = getComputedStyle(node);
                const scrollable = /auto|scroll|overlay/.test(style.overflowY)
                    && node.scrollHeight > node.clientHeight;
                if (scrollable) { break; }
                node = node.parentElement;
            }
            const target = node && node !== document.body ? node : null;
            if (target) {
                const before = target.scrollTop;
                target.scrollTop += delta;
                return target.scrollTop !== before;
            }
            const before = window.scrollY;
            window.scrollBy(0, delta);
            return window.scrollY !== before;
        }"#;

        let outcome = self
            .page
            .execute(
                CallFunctionOnParams::builder()
                    .function_declaration(SCROLL)
                    .object_id(object_id)
                    .argument(
                        chromiumoxide::cdp::js_protocol::runtime::CallArgument::builder()
                            .value(serde_json::json!(delta))
                            .build(),
                    )
                    .return_by_value(true)
                    .build()
                    .map_err(StepError::Backend)?,
            )
            .await
            .map_err(|error| StepError::Backend(format!("scroll element: {error}")))?;

        // Reporting `ok` for a scroll that moved nothing is how a model ends up
        // paging forever through a list that was already at the bottom.
        if outcome.result.result.value == Some(serde_json::Value::Bool(false)) {
            return Err(StepError::Backend(
                "nothing scrolled — the element and its ancestors are already at that end".into(),
            ));
        }
        Ok(())
    }

    /// Step through session history.
    ///
    /// `Page.navigateToHistoryEntry` takes an absolute entry, so the current
    /// index has to be read first — there is no relative form.
    async fn history(&self, delta: i64) -> Result<(), StepError> {
        use chromiumoxide::cdp::browser_protocol::page::{
            GetNavigationHistoryParams, NavigateToHistoryEntryParams,
        };

        let history = self
            .page
            .execute(GetNavigationHistoryParams::default())
            .await
            .map_err(|error| StepError::Backend(format!("read history: {error}")))?;
        let wanted = history.current_index as i64 + delta;
        let entry = usize::try_from(wanted)
            .ok()
            .and_then(|index| history.entries.get(index))
            .ok_or_else(|| {
                StepError::Backend(
                    if delta < 0 {
                        "there is nothing to go back to"
                    } else {
                        "there is nothing to go forward to"
                    }
                    .to_owned(),
                )
            })?;

        self.page
            .execute(NavigateToHistoryEntryParams::new(entry.id))
            .await
            .map(|_| ())
            .map_err(|error| StepError::Backend(format!("navigate history: {error}")))
    }

    async fn focus_backend_node(&self, backend_id: i64) -> Result<(), StepError> {
        use chromiumoxide::cdp::browser_protocol::dom::FocusParams;

        self.page
            .execute(
                FocusParams::builder()
                    .backend_node_id(BackendNodeId::new(backend_id))
                    .build(),
            )
            .await
            .map(|_| ())
            .map_err(|error| StepError::Backend(format!("focus element: {error}")))
    }

    async fn click_backend_node(&self, backend_id: i64) -> Result<(), StepError> {
        use chromiumoxide::cdp::browser_protocol::dom::{
            GetBoxModelParams, ScrollIntoViewIfNeededParams,
        };
        use chromiumoxide::cdp::browser_protocol::input::{
            DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
        };

        let node_id = BackendNodeId::new(backend_id);
        let _ = self
            .page
            .execute(
                ScrollIntoViewIfNeededParams::builder()
                    .backend_node_id(node_id)
                    .build(),
            )
            .await;

        let box_model = self
            .page
            .execute(
                GetBoxModelParams::builder()
                    .backend_node_id(node_id)
                    .build(),
            )
            .await
            .map_err(|error| StepError::Backend(format!("element geometry: {error}")))?;

        let quad = &box_model.result.model.content;
        // A content quad is four corner pairs; the centre is the mean of the
        // first and third.
        let (x, y) = (
            (quad.inner()[0] + quad.inner()[4]) / 2.0,
            (quad.inner()[1] + quad.inner()[5]) / 2.0,
        );

        for kind in [
            DispatchMouseEventType::MousePressed,
            DispatchMouseEventType::MouseReleased,
        ] {
            self.page
                .execute(
                    DispatchMouseEventParams::builder()
                        .r#type(kind)
                        .x(x)
                        .y(y)
                        .button(MouseButton::Left)
                        .click_count(1)
                        .build()
                        .map_err(StepError::Backend)?,
                )
                .await
                .map_err(|error| StepError::Backend(format!("click: {error}")))?;
        }
        Ok(())
    }

    async fn press_key(&self, key: &str) -> Result<(), StepError> {
        use chromiumoxide::cdp::browser_protocol::input::InsertTextParams;
        use chromiumoxide::cdp::browser_protocol::input::{
            DispatchKeyEventParams, DispatchKeyEventType,
        };

        let chord = crate::keys::parse(key)?;
        let (dom_key, code) = chord.key.dom();
        let modifiers = chord.modifiers.cdp_bits();

        // An unmodified printable character is text, and `insertText` is the
        // only way to get one into a contenteditable reliably. *With* a
        // modifier it is a shortcut and must be dispatched as a key event —
        // routing it to insertText is what made `ctrl+a` type a literal "a".
        if modifiers == 0
            && let crate::keys::Key::Char(character) = chord.key
        {
            return self
                .page
                .execute(
                    InsertTextParams::builder()
                        .text(character.to_string())
                        .build()
                        .map_err(StepError::Backend)?,
                )
                .await
                .map(|_| ())
                .map_err(|error| StepError::Backend(format!("type: {error}")));
        }

        for kind in [DispatchKeyEventType::KeyDown, DispatchKeyEventType::KeyUp] {
            self.page
                .execute(
                    DispatchKeyEventParams::builder()
                        .r#type(kind)
                        .key(dom_key.clone())
                        .windows_virtual_key_code(code)
                        .modifiers(modifiers)
                        .build()
                        .map_err(StepError::Backend)?,
                )
                .await
                .map_err(|error| StepError::Backend(format!("key: {error}")))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdp_roles_normalize_to_the_shared_vocabulary() {
        assert_eq!(role_for("button"), Role::Button);
        assert_eq!(role_for("textbox"), Role::TextBox);
        assert_eq!(role_for("link"), Role::Link);
        assert_eq!(role_for("StaticText"), Role::Text);
        // Unknown roles pass through rather than being flattened away, so the
        // model still learns what the page called it.
        assert_eq!(role_for("figure"), Role::Other("figure".into()));
    }

    #[tokio::test]
    async fn attaching_is_offered_only_for_a_browser_on_this_machine() {
        // A devtools endpoint is total control of a browser. A remote one is
        // somebody else's browser, and there is no version of that request that
        // is what the user meant.
        for refused in [
            "ws://example.com:9222/devtools/browser/abc",
            "ws://10.0.0.5:9222/devtools/browser/abc",
            "wss://127.0.0.1:9222/devtools/browser/abc",
            "http://127.0.0.1:9222/",
            "127.0.0.1:9222",
        ] {
            let error = attach_to_endpoint(refused).await.unwrap_err().to_string();
            assert!(
                error.contains("loopback"),
                "{refused} should be refused as non-loopback, got: {error}"
            );
        }
    }

    #[tokio::test]
    async fn a_loopback_endpoint_with_nothing_behind_it_says_how_to_start_one() {
        // Port 1 is never a devtools endpoint, so this exercises the connect
        // failure rather than the host check — and the message has to name the
        // flag, because "connection refused" tells a user nothing they can act on.
        let error = attach_to_endpoint("ws://127.0.0.1:1/devtools/browser/x")
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("loopback"), "wrong branch: {error}");
        assert!(
            error.contains("--remote-debugging-port"),
            "the recovery must be named: {error}"
        );
    }

    #[tokio::test]
    async fn a_port_with_nothing_on_it_says_what_to_start() {
        // Port 1 is never a devtools endpoint. The message has to name the flag,
        // because "connection refused" tells a user nothing they can act on.
        let error = attach_to_endpoint("1").await.unwrap_err().to_string();
        assert!(error.contains("--remote-debugging-port"), "{error}");
        assert!(error.contains("nothing is listening"), "{error}");
    }

    #[tokio::test]
    async fn a_bare_port_is_accepted_where_a_url_would_be() {
        // The usability point: nobody knows their devtools websocket path, but
        // everybody knows the port they typed. Reaching the "nothing listening"
        // branch proves the port form was parsed rather than rejected as a
        // malformed url.
        let error = attach_to_endpoint("65535").await.unwrap_err().to_string();
        assert!(
            !error.contains("loopback"),
            "a bare port must not be read as a non-loopback url: {error}"
        );
    }
}

#[cfg(test)]
mod typeable_tests {
    use super::*;

    #[test]
    fn a_label_is_not_a_field() {
        // The mistake this catches: a page labels its input with a `<label>`,
        // and in document order that label's text comes *first*. Naming "Email"
        // and typing into the first match hits the label. The browser answers
        // `Node is not an Element`, which tells the model nothing.
        assert!(!accepts_text(&Node::new("t", Role::Text, "Email")));
        assert!(!accepts_text(&Node::new("h", Role::Heading, "Email")));
        assert!(!accepts_text(&Node::new("b", Role::Button, "Send")));
        assert!(!accepts_text(&Node::new("l", Role::Link, "Email us")));
    }

    #[test]
    fn the_things_text_actually_goes_into_are_allowed() {
        assert!(accepts_text(&Node::new("f", Role::TextBox, "Email")));
        assert!(accepts_text(&Node::new("c", Role::ComboBox, "Country")));
        // A custom element with a role we do not model may still be an editable
        // host; guessing "no" there would refuse things that work.
        assert!(accepts_text(&Node::new(
            "x",
            Role::Other("textbox-ish".into()),
            "Notes"
        )));
    }
}
