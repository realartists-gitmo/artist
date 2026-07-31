//! Rung 2: the platform accessibility tree.
//!
//! The universal rung for native toolkits. GTK and Qt both expose their widgets
//! here, and — crucially — AT-SPI exposes **actions**, not just geometry. Most
//! buttons and menu items can be invoked by name with no coordinates involved at
//! all, which makes this rung both cheaper and far more robust than clicking.
//!
//! Everything is read from the *stage's private bus*, never the user's. That is
//! what makes attribution exact: only agent-launched applications are on it, so
//! a node belongs unambiguously to the window being driven rather than to
//! whatever else the user happens to have open.
//!
//! Anchors bind to the accessible's object path, which is stable for the life of
//! the widget — the same identity contract a CDP `backendDOMNodeId` provides.

use std::collections::{HashMap, VecDeque};

use atspi_proxies::accessible::AccessibleProxy;
use atspi_proxies::action::ActionProxy;
use atspi_proxies::editable_text::EditableTextProxy;

use crate::model::{Caps, Node, NodeState, Role, Rung, Snapshot, SurfaceId};
use crate::program::{Settle, SettleKind, SettleOutcome, Step, StepError};
use crate::surface::{SettleWatch, Surface};

/// How deep to walk. Real trees are shallow; a cycle or a pathological
/// container should not be able to hang an observation.
const MAX_DEPTH: u16 = 24;
/// Upper bound on nodes collected from one tree.
const MAX_NODES: usize = 2_000;
/// How often to re-read the tree while waiting for it to settle.
const POLL_MS: u64 = 60;

/// Map an AT-SPI role name onto the shared vocabulary.
///
/// Toolkits report these as human words ("push button", "entry"), and each has
/// its own spelling for the same concept — normalizing here is what lets the
/// model reason in one vocabulary across GTK, Qt and the web.
pub fn role_for(name: &str) -> Role {
    match name.trim().to_ascii_lowercase().as_str() {
        "push button" | "button" | "toggle button" => Role::Button,
        "link" => Role::Link,
        "entry" | "text" | "password text" | "spin button" => Role::TextBox,
        "check box" | "check menu item" => Role::CheckBox,
        "radio button" | "radio menu item" => Role::RadioButton,
        "combo box" => Role::ComboBox,
        "list item" | "table row" => Role::ListItem,
        "menu item" | "menu" => Role::MenuItem,
        "page tab" => Role::Tab,
        "heading" => Role::Heading,
        "label" | "static" | "paragraph" => Role::Text,
        "image" | "icon" => Role::Image,
        "frame" | "window" | "dialog" => Role::Window,
        other => Role::Other(other.to_owned()),
    }
}

/// Build an `org.a11y.atspi.Action` proxy for a specific accessible.
///
/// Factored out because the same three-step construction is needed both when
/// describing a node and when invoking it, and zbus's builder is fallible then
/// async — an awkward shape to inline twice.
async fn action_proxy<'a>(
    connection: &zbus::Connection,
    destination: &str,
    path: &str,
) -> Result<ActionProxy<'a>, zbus::Error> {
    ActionProxy::builder(connection)
        .destination(destination.to_owned())?
        .path(path.to_owned())?
        .build()
        .await
}

/// Build a node from one accessible.
async fn node_for(
    proxy: &AccessibleProxy<'_>,
    binding: String,
    depth: u16,
) -> Result<Node, StepError> {
    let name = proxy.name().await.unwrap_or_default();
    let role = role_for(&proxy.get_role_name().await.unwrap_or_default());

    // Actions are the reason this rung is worth having: a node that declares
    // one can be invoked directly, with no geometry involved.
    let actions = match action_proxy(
        proxy.inner().connection(),
        proxy.inner().destination().as_str(),
        proxy.inner().path().as_str(),
    )
    .await
    {
        Ok(action) => action
            .get_actions()
            .await
            .map(|actions| actions.into_iter().map(|action| action.name).collect())
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    // The state set is a single call and is what makes an action verifiable:
    // without it, checking a checkbox or disabling a submit button changes
    // nothing in the digest, so the model sees no `~` line and cannot tell
    // whether its click landed.
    let state = proxy
        .get_state()
        .await
        .map(state_from)
        .unwrap_or_default();

    Ok(Node::new(binding, role, name.trim())
        .with_depth(depth)
        .with_actions(actions)
        .with_state(state))
}

/// Project an AT-SPI state set onto the shared vocabulary.
fn state_from(states: atspi_proxies::common::StateSet) -> NodeState {
    use atspi_proxies::common::State;
    NodeState {
        focused: states.contains(State::Focused),
        // AT-SPI states availability positively, twice. A control is disabled
        // when it has lost either: `Sensitive` is "can be interacted with",
        // `Enabled` is "would do something if it were".
        disabled: !states.contains(State::Sensitive) || !states.contains(State::Enabled),
        checked: states.contains(State::Checked),
        expanded: states.contains(State::Expanded),
        selected: states.contains(State::Selected),
        // `Showing` means on-screen right now; `Visible` only means not
        // explicitly hidden, so a widget scrolled out of a viewport is still
        // Visible. Showing is the one that answers "can the user see this".
        offscreen: !states.contains(State::Showing),
    }
}

/// One application window on the stage's accessibility bus.
pub struct AtspiSurface {
    id: SurfaceId,
    connection: zbus::Connection,
    /// The accessible that roots this surface.
    destination: String,
    root_path: String,
}

impl AtspiSurface {
    /// Attach to an application's accessible root on a given bus.
    pub fn new(
        id: impl Into<String>,
        connection: zbus::Connection,
        destination: impl Into<String>,
        root_path: impl Into<String>,
    ) -> Self {
        Self {
            id: SurfaceId::new(id),
            connection,
            destination: destination.into(),
            root_path: root_path.into(),
        }
    }

    async fn proxy(&self, path: &str) -> Result<AccessibleProxy<'_>, StepError> {
        accessible(&self.connection, &self.destination, path).await
    }

    async fn walk(&self) -> Result<Vec<Node>, StepError> {
        walk_tree(&self.connection, &self.destination, &self.root_path).await
    }
}

async fn accessible<'a>(
    connection: &zbus::Connection,
    destination: &str,
    path: &str,
) -> Result<AccessibleProxy<'a>, StepError> {
    AccessibleProxy::builder(connection)
        .destination(destination.to_owned())
        .and_then(|builder| builder.path(path.to_owned()))
        .map_err(|error: zbus::Error| StepError::Backend(format!("accessible proxy: {error}")))?
        .build()
        .await
        .map_err(|error: zbus::Error| StepError::Backend(format!("accessible proxy: {error}")))
}

/// Walk the tree breadth-first into a flat node list.
///
/// Breadth-first so that a budget cut removes the deepest detail rather than a
/// whole branch of the interface — losing the last few rows of a list is far
/// better than losing an entire dialog.
///
/// A free function rather than a method because [`AtspiSurface::watch`] needs to
/// walk from a `'static` future, and everything it needs is cheaply cloneable.
async fn walk_tree(
    connection: &zbus::Connection,
    destination: &str,
    root_path: &str,
) -> Result<Vec<Node>, StepError> {
    let mut nodes = Vec::new();
    // A `Vec` popped from the back is a stack, and a stack is depth-first —
    // which is the opposite of what the budget needs. Hitting `MAX_NODES`
    // depth-first drops every branch after the one being descended, so a deep
    // sidebar can cost the entire dialog next to it.
    let mut queue = VecDeque::from([(root_path.to_owned(), 0u16)]);
    let mut seen = HashMap::new();

    while let Some((path, depth)) = queue.pop_front() {
        if nodes.len() >= MAX_NODES || depth > MAX_DEPTH {
            continue;
        }
        // Guard against a tree that refers back into itself.
        if seen.insert(path.clone(), ()).is_some() {
            continue;
        }
        let Ok(proxy) = accessible(connection, destination, &path).await else {
            continue;
        };
        if let Ok(node) = node_for(&proxy, format!("atspi:{path}"), depth).await {
            let interesting =
                node.role.is_interactive() || !node.name.is_empty() || !node.actions.is_empty();
            if interesting {
                nodes.push(node);
            }
        }
        if let Ok(children) = proxy.get_children().await {
            for child in children {
                queue.push_back((child.path().to_string(), depth + 1));
            }
        }
    }
    Ok(nodes)
}

/// A digest of the whole tree, for settle detection.
fn tree_digest(nodes: &[Node]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for node in nodes {
        hasher.update(&node.digest());
    }
    *hasher.finalize().as_bytes()
}

#[async_trait::async_trait]
impl Surface for AtspiSurface {
    fn id(&self) -> &SurfaceId {
        &self.id
    }

    fn rung(&self) -> Rung {
        Rung::Accessibility
    }

    /// What this rung can actually do — not what it would be nice to offer.
    ///
    /// `caps` is read by the model to decide how to phrase a program, so a
    /// capability that is advertised and then rejected costs a whole round trip
    /// and teaches the model nothing. AT-SPI invokes actions and edits text; it
    /// has no keyboard and no pointer, so key and scroll belong to the stage.
    fn caps(&self) -> Caps {
        Caps {
            click: true,
            type_text: true,
            key: false,
            scroll: false,
            pixels: false,
        }
    }

    fn title(&self) -> String {
        self.id.as_str().to_owned()
    }

    async fn snapshot(&self) -> Result<Snapshot, StepError> {
        Ok(Snapshot::new(self.walk().await?))
    }

    /// Tree-hash stability.
    ///
    /// AT-SPI does emit change signals, but a toolkit that batches them would
    /// settle early; comparing what we can actually read is the honest measure.
    ///
    /// This has to be implemented rather than reported `Unsupported`, because
    /// `Quiet` is the *default*: an unsupported default meant no AT-SPI program
    /// ever waited, and every one of them snapshotted the tree as it was before
    /// the click — the exact failure `settle` exists to prevent.
    async fn watch(&self, settle: &Settle) -> Result<SettleWatch, StepError> {
        if matches!(settle.until, SettleKind::None) {
            return Ok(SettleWatch::ready(SettleOutcome::Settled { after_ms: 0 }));
        }
        let (connection, destination, root_path) = (
            self.connection.clone(),
            self.destination.clone(),
            self.root_path.clone(),
        );
        let settle = settle.clone();
        let started = std::time::Instant::now();
        // Read the pre-action tree while arming, so a change that lands before
        // the first poll still counts as a change.
        let armed = walk_tree(&connection, &destination, &root_path)
            .await
            .map(|nodes| tree_digest(&nodes))
            .unwrap_or_default();

        Ok(SettleWatch(Box::new(Box::pin(async move {
            match &settle.until {
                SettleKind::Quiet | SettleKind::Anchor(_) => {}
                // There is no network at this rung, and pretending otherwise
                // would settle instantly on a page that is still loading.
                SettleKind::NetworkIdle => return SettleOutcome::Unsupported,
                SettleKind::None => return SettleOutcome::Settled { after_ms: 0 },
            }

            let deadline = started + std::time::Duration::from_millis(settle.timeout_ms);
            let mut previous = armed;
            let mut changed = false;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
                let current = walk_tree(&connection, &destination, &root_path)
                    .await
                    .map(|nodes| tree_digest(&nodes))
                    .unwrap_or_default();

                // Settled means something happened and then stopped happening.
                // Requiring the change first is what keeps a slow toolkit from
                // reporting "settled" on the pre-action tree.
                changed |= current != armed;
                if changed && current == previous {
                    return SettleOutcome::Settled {
                        after_ms: started.elapsed().as_millis() as u64,
                    };
                }
                previous = current;
                if std::time::Instant::now() >= deadline {
                    return SettleOutcome::TimedOut {
                        after_ms: started.elapsed().as_millis() as u64,
                    };
                }
            }
        }))))
    }

    async fn apply(&self, step: &Step, node: Option<&Node>) -> Result<(), StepError> {
        let Some(node) = node else {
            return Err(StepError::Backend(
                "this step needs an element from the accessibility tree".into(),
            ));
        };
        let path = node
            .binding
            .as_str()
            .strip_prefix("atspi:")
            .ok_or_else(|| StepError::Backend("not an accessibility node".into()))?;

        match step {
            Step::Click(target) => {
                // Invoke the declared action rather than synthesizing a click.
                // No coordinates, no focus juggling, and it works for elements
                // that are scrolled out of view.
                let action = action_proxy(&self.connection, &self.destination, path)
                    .await
                    .map_err(|error| StepError::Backend(format!("action proxy: {error}")))?;

                let actions = action.get_actions().await.unwrap_or_default();
                // Prefer the conventional activation verb; otherwise the first.
                let index = actions
                    .iter()
                    .position(|declared| {
                        matches!(
                            declared.name.to_ascii_lowercase().as_str(),
                            "click" | "activate" | "press" | "jump"
                        )
                    })
                    .unwrap_or(0);
                if actions.is_empty() {
                    return Err(StepError::Unsupported {
                        anchor: target.anchor.clone(),
                        role: node.role.label().to_owned(),
                        name: node.name.clone(),
                        action: "click",
                    });
                }
                action
                    .do_action(index as i32)
                    .await
                    .map(|_| ())
                    .map_err(|error| StepError::Backend(format!("do_action: {error}")))
            }
            // `EditableText` sets the whole field in one call, which is both
            // more reliable than typing and the right default: filling a
            // pre-populated field should replace it, not append to it.
            Step::Type { target, text } => {
                let editable = EditableTextProxy::builder(&self.connection)
                    .destination(self.destination.clone())
                    .and_then(|builder| builder.path(path.to_owned()))
                    .map_err(|error: zbus::Error| {
                        StepError::Backend(format!("editable text proxy: {error}"))
                    })?
                    .build()
                    .await
                    .map_err(|_| StepError::Unsupported {
                        anchor: target.anchor.clone(),
                        role: node.role.label().to_owned(),
                        name: node.name.clone(),
                        action: "type",
                    })?;

                match editable.set_text_contents(text).await {
                    // The call is `-> bool`: a toolkit that refuses returns
                    // false rather than an error, and reporting that as `ok`
                    // would be the silent-success failure this design exists to
                    // avoid.
                    Ok(true) => Ok(()),
                    Ok(false) => Err(StepError::Backend(format!(
                        "{:?} refused the text — it may be read-only",
                        node.name
                    ))),
                    Err(error) => Err(StepError::Backend(format!("set text: {error}"))),
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolkit_role_names_normalize_to_one_vocabulary() {
        // GTK and Qt spell the same widget differently; the model should not
        // have to know which toolkit it is looking at.
        assert_eq!(role_for("push button"), Role::Button);
        assert_eq!(role_for("toggle button"), Role::Button);
        assert_eq!(role_for("entry"), Role::TextBox);
        assert_eq!(role_for("password text"), Role::TextBox);
        assert_eq!(role_for("check box"), Role::CheckBox);
        assert_eq!(role_for("page tab"), Role::Tab);
        assert_eq!(role_for("frame"), Role::Window);
    }

    #[test]
    fn role_names_are_case_and_space_insensitive() {
        assert_eq!(role_for("  Push Button "), Role::Button);
    }

    #[test]
    fn an_unknown_role_passes_through_rather_than_being_flattened() {
        // The model still learns what the toolkit called it, which is better
        // than every unfamiliar widget becoming an indistinguishable blob.
        assert_eq!(
            role_for("color chooser"),
            Role::Other("color chooser".into())
        );
    }
}
