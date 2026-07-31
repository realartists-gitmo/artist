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

/// The set of requests a page currently has outstanding.
///
/// A set rather than a counter. Chromium emits an extra `requestWillBeSent` per
/// redirect hop with no matching completion, so an incrementing counter drifts
/// permanently upward — after a few redirects it never returns below the idle
/// threshold and every `quiet` settle burns its full timeout.
type InFlight = Arc<Mutex<HashSet<chromiumoxide::cdp::browser_protocol::network::RequestId>>>;

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
                // Chromium re-announces a request for every redirect hop, and
                // the hop that carries `redirectResponse` never gets its own
                // completion. Counting it was an unmatched increment: after a
                // login or OAuth flow the count sat permanently above the idle
                // threshold and `quiet` could never be satisfied again.
                //
                // A set makes the whole class of drift impossible: the same
                // request id re-inserted is still one request.
                if event.redirect_response.is_some() {
                    continue;
                }
                set.lock().unwrap().insert(event.request_id.clone());
            }
        }
    });
    tokio::spawn({
        let set = Arc::clone(&inflight);
        async move {
            let mut finished = finished;
            while let Some(event) = finished.next().await {
                set.lock().unwrap().remove(&event.request_id);
            }
        }
    });
    tokio::spawn({
        let set = Arc::clone(&inflight);
        async move {
            let mut failed = failed;
            while let Some(event) = failed.next().await {
                set.lock().unwrap().remove(&event.request_id);
            }
        }
    });
    tokio::spawn({
        let set = Arc::clone(&inflight);
        async move {
            let mut navigated = navigated;
            while let Some(event) = navigated.next().await {
                // A main-frame navigation discards the old document, so
                // anything still outstanding for it will never complete and
                // would otherwise keep the page "busy" for the rest of the
                // session. Subframe navigations do not have that effect.
                if event.frame.parent_id.is_none() {
                    set.lock().unwrap().clear();
                }
            }
        }
    });
    Ok(())
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
        let Some(node) = node else {
            return Err(StepError::Backend("this step needs a tab".into()));
        };
        let target = node
            .binding
            .as_str()
            .strip_prefix("cdp:target:")
            .ok_or_else(|| StepError::Backend("not a browser tab".into()))?;

        match step {
            Step::Click(_) => {
                use chromiumoxide::cdp::browser_protocol::target::ActivateTargetParams;

                let pages = self
                    .browser
                    .pages()
                    .await
                    .map_err(|error| StepError::Backend(format!("list tabs: {error}")))?;
                let page = pages
                    .into_iter()
                    .find(|page| page.target_id().inner() == target)
                    .ok_or_else(|| StepError::Backend(format!("no tab {target}")))?;
                page.execute(ActivateTargetParams::new(page.target_id().clone()))
                    .await
                    .map(|_| ())
                    .map_err(|error| StepError::Backend(format!("activate tab: {error}")))
            }
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

/// Pull the accessibility tree and normalize it into nodes.
///
/// `backendDOMNodeId` is the binding: stable for the life of the element, which
/// is exactly the identity contract anchors need, and quite unlike a CSS path
/// or an index that shifts when the page re-renders.
async fn ax_nodes(page: &chromiumoxide::Page) -> Result<Vec<Node>, StepError> {
    use chromiumoxide::cdp::browser_protocol::accessibility::GetFullAxTreeParams;

    let tree = page
        .execute(GetFullAxTreeParams::default())
        .await
        .map_err(|error| StepError::Backend(format!("accessibility tree: {error}")))?;

    let mut nodes = Vec::new();
    for ax in &tree.result.nodes {
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
        nodes.push(node.with_state(state_of(&ax)));
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

                let quiet = ready == "complete"
                    && inflight.lock().unwrap().len() as u32 <= IDLE_INFLIGHT;

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
            Step::Key(key) => self.press_key(key).await,
            Step::Scroll { amount, .. } => {
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
            Step::Navigate { url } => {
                use chromiumoxide::cdp::browser_protocol::page::NavigateParams;

                self.page
                    .execute(NavigateParams::new(url.clone()))
                    .await
                    .map(|_| ())
                    .map_err(|error| StepError::Backend(format!("navigate to {url}: {error}")))
            }
            Step::Back => self.history(-1).await,
            Step::Forward => self.history(1).await,
            // The page rung has no declared per-element verbs — every element
            // is reached the same way — so `invoke` here is a routing mistake
            // rather than a missing feature, and says so.
            Step::Invoke { target, action } => Err(StepError::Unsupported {
                anchor: target.anchor.clone(),
                role: node.map(|node| node.role.label().to_owned()).unwrap_or_default(),
                name: node.map(|node| node.name.clone()).unwrap_or_default(),
                action: "invoke",
            })
            .map_err(|error| match error {
                StepError::Unsupported { anchor, role, name, .. } => StepError::Backend(format!(
                    "{anchor} ({role} {name:?}) has no action {action:?} — page elements are \
                     driven with click, type, key and scroll"
                )),
                other => other,
            }),
        }
    }
}

impl CdpPage {
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
                    .backend_node_id(node_id.clone())
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
        use chromiumoxide::cdp::browser_protocol::input::{
            DispatchKeyEventParams, DispatchKeyEventType,
        };
        use chromiumoxide::cdp::browser_protocol::input::InsertTextParams;

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
}
