//! Rung 2 on Android.
//!
//! The strongest rung this platform has. Android's accessibility tree is a
//! first-class product surface rather than a bolt-on — TalkBack is built on it
//! and app developers are held to it — so it is both richer and better
//! maintained than most desktop toolkits' AT-SPI support.
//!
//! Two things make this different from [`crate::surface::atspi`], and both come
//! from the container boundary:
//!
//! * **Acting goes through the tree, not the seat.** `performAction(ACTION_CLICK)`
//!   is delivered by Android to the view that owns it, which is more reliable
//!   than a synthetic tap at the view's centre: it cannot miss, cannot land on an
//!   overlay that appeared in between, and works on views that are scrolled
//!   partly out of sight. The pointer is still there for anything the tree
//!   refuses.
//! * **Settle comes from the service.** Android's damage tells us nothing —
//!   measured, see `docs/computer-use-platforms.md` — but the accessibility
//!   service pushes a notification whenever window content changes, which is a
//!   *better* signal than damage: it is semantic rather than pixel-level, so an
//!   animation that changes nothing structurally does not defeat it.

use std::sync::Arc;

use crate::model::{Caps, Node, Rect, Role, Rung, Snapshot, SurfaceId};
use crate::program::{Settle, SettleKind, SettleOutcome, Step, StepError};
use crate::surface::{SettleWatch, Surface};

use super::bridge::{Bridge, BridgeNode};

/// How long a screen must be structurally quiet before it counts as settled.
const QUIET_MS: u64 = 200;

pub struct AndroidSurface {
    id: SurfaceId,
    bridge: Arc<Bridge>,
    /// The package this surface is for, so a tree covering several windows can
    /// be narrowed to the one the caller means.
    package: String,
}

impl AndroidSurface {
    pub fn new(id: impl Into<String>, bridge: Arc<Bridge>, package: impl Into<String>) -> Self {
        Self {
            id: SurfaceId::new(id),
            bridge,
            package: package.into(),
        }
    }
}

/// Turn one Android node into the shared vocabulary.
///
/// The name is chosen in the order a person would read it: visible text first,
/// then the content description a developer wrote for screen readers, then the
/// hint a field shows when empty. The resource id is deliberately *last* — it is
/// the most stable but the least human, and a model naming `com.a:id/btn_ok_2`
/// is naming something the screen never showed it.
fn to_node(node: &BridgeNode) -> Node {
    let name = [&node.text, &node.desc, &node.hint]
        .into_iter()
        .find(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| {
            node.res_id
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned()
        });

    let mut actions: Vec<&str> = Vec::new();
    if node.clickable {
        actions.push("click");
    }
    if node.long_clickable {
        actions.push("longPress");
    }
    if node.editable {
        actions.push("type");
    }
    if node.scrollable {
        actions.push("scroll");
    }

    let mut built = Node::new(node.id.clone(), role_of(node), name).with_actions(actions);
    if let Some(bounds) = bounds_of(node) {
        built = built.with_bounds(bounds);
    }
    built
}

/// `[left, top, right, bottom]` as a rectangle, when it is a real one.
///
/// Android reports zero-size bounds for nodes that are not laid out, and
/// negative ones for nodes scrolled off-screen. Neither is somewhere to click,
/// and a node with no bounds is honestly unpositioned rather than at the origin.
fn bounds_of(node: &BridgeNode) -> Option<Rect> {
    let [left, top, right, bottom] = node.bounds[..] else {
        return None;
    };
    let (width, height) = (right - left, bottom - top);
    (width > 0 && height > 0).then_some(Rect {
        x: left,
        y: top,
        width: width as u32,
        height: height as u32,
    })
}

/// Android's class names, normalized to the shared role vocabulary.
///
/// Matched on the suffix rather than the whole name because every widget is
/// subclassed: `androidx.appcompat.widget.AppCompatButton` and
/// `android.widget.Button` are the same thing to a model, and a table of exact
/// names would miss every app that uses the support library.
fn role_of(node: &BridgeNode) -> Role {
    let class = node.class.rsplit('.').next().unwrap_or_default();
    let lower = class.to_ascii_lowercase();

    if node.editable || lower.contains("edittext") {
        return Role::TextBox;
    }
    if lower.contains("checkbox") {
        return Role::CheckBox;
    }
    if lower.contains("radio") {
        return Role::RadioButton;
    }
    if lower.contains("switch") || lower.contains("togglebutton") {
        return Role::CheckBox;
    }
    if lower.contains("spinner") {
        return Role::ComboBox;
    }
    if lower.contains("imagebutton") {
        return Role::Button;
    }
    if lower.contains("button") {
        return Role::Button;
    }
    if lower.contains("image") {
        return Role::Image;
    }
    if lower.contains("textview") {
        return Role::Text;
    }
    // A clickable node of any class is a button as far as anything acting on it
    // is concerned. Compose in particular reports almost everything as a generic
    // `View`, so classifying only by class name would leave a whole modern UI
    // toolkit looking like a page of undifferentiated boxes.
    if node.clickable {
        return Role::Button;
    }
    Role::Other(class.to_owned())
}

#[async_trait::async_trait]
impl Surface for AndroidSurface {
    fn id(&self) -> &SurfaceId {
        &self.id
    }

    fn rung(&self) -> Rung {
        Rung::Accessibility
    }

    fn caps(&self) -> Caps {
        Caps {
            click: true,
            type_text: true,
            key: true,
            scroll: true,
            // The tree carries no pixels. A caller that wants a picture asks the
            // stage, which is where the frame actually lives.
            pixels: false,
        }
    }

    fn title(&self) -> String {
        self.package.clone()
    }

    async fn snapshot(&self) -> Result<Snapshot, StepError> {
        let tree = self.bridge.tree().await?;
        let nodes = tree
            .nodes
            .iter()
            // Invisible nodes are excluded rather than reported as present:
            // Android keeps offscreen list items in the tree, and offering them
            // as things to click means offering things a person cannot see.
            .filter(|node| node.visible && node.enabled)
            .filter(|node| self.package.is_empty() || node.package == self.package)
            .map(to_node)
            .filter(|node| !node.name.trim().is_empty() || !node.actions.is_empty())
            .collect();
        Ok(Snapshot::new(nodes))
    }

    /// Settle on the service going quiet.
    ///
    /// Structural rather than visual: Android tells us when *content* changed,
    /// so a spinner animating forever does not keep this awake the way pixel
    /// damage would.
    async fn watch(&self, settle: &Settle) -> Result<SettleWatch, StepError> {
        if matches!(settle.until, SettleKind::None) {
            return Ok(SettleWatch::ready(SettleOutcome::Settled { after_ms: 0 }));
        }
        if matches!(settle.until, SettleKind::NetworkIdle) {
            return Ok(SettleWatch::ready(SettleOutcome::Unsupported));
        }

        // Subscribed before the action dispatches, which is the whole reason
        // `watch` is separate from `apply`.
        let mut events = self.bridge.events();
        let timeout = settle.timeout_ms;
        let started = std::time::Instant::now();

        Ok(SettleWatch(Box::new(Box::pin(async move {
            let deadline = started + std::time::Duration::from_millis(timeout);
            let mut moved = false;
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return if moved {
                        SettleOutcome::Settled {
                            after_ms: started.elapsed().as_millis() as u64,
                        }
                    } else {
                        SettleOutcome::TimedOut {
                            after_ms: started.elapsed().as_millis() as u64,
                        }
                    };
                }
                let quiet = std::time::Duration::from_millis(QUIET_MS).min(remaining);
                match tokio::time::timeout(quiet, events.recv()).await {
                    // Something changed; the quiet window restarts.
                    Ok(Ok(())) => moved = true,
                    Ok(Err(_)) => return SettleOutcome::Unsupported,
                    // Quiet for a full window. Only a settle if something
                    // happened first — otherwise this reports "settled" for an
                    // action that has not taken effect yet.
                    Err(_) if moved => {
                        return SettleOutcome::Settled {
                            after_ms: started.elapsed().as_millis() as u64,
                        };
                    }
                    Err(_) => {}
                }
            }
        }))))
    }

    async fn apply(
        &self,
        step: &Step,
        node: Option<&Node>,
        _secondary: Option<&Node>,
    ) -> Result<Option<String>, StepError> {
        let binding = |action: &'static str| -> Result<String, StepError> {
            node.map(|node| node.binding.as_str().to_owned())
                .ok_or_else(|| {
                    StepError::Backend(format!("{action} on an Android surface needs an element"))
                })
        };

        match step {
            Step::Click { .. } => {
                let id = binding("a click")?;
                self.bridge.act(&id, "click").await
            }
            Step::LongPress(_) => {
                let id = binding("a long press")?;
                self.bridge.act(&id, "longClick").await
            }
            Step::Type { text, clear, .. } => {
                let id = binding("typing")?;
                // ACTION_SET_TEXT replaces the field's whole contents, so
                // appending has to be built rather than asked for.
                let value = if *clear {
                    text.clone()
                } else {
                    let existing = node.map(|node| node.name.clone()).unwrap_or_default();
                    format!("{existing}{text}")
                };
                self.bridge.set_text(&id, &value).await
            }
            Step::Scroll { amount, .. } => {
                let id = binding("scrolling")?;
                let action = if *amount >= 0 {
                    "scrollForward"
                } else {
                    "scrollBackward"
                };
                self.bridge.act(&id, action).await
            }
            other => Err(StepError::Backend(format!(
                "an Android accessibility surface cannot {:?}. Keys and swipes go to the \
                 stage's seat rather than through the tree.",
                other.action()
            ))),
        }
        // No verb on this rung reports a value of its own; the observation
        // afterwards is what says what happened.
        .map(|()| None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(class: &str) -> BridgeNode {
        serde_json::from_str(&format!(r#"{{"id":"1:0","class":"{class}"}}"#)).unwrap()
    }

    #[test]
    fn support_library_widgets_get_the_same_role_as_plain_ones() {
        // The failure this prevents: an exact-name table that recognizes
        // android.widget.Button and misses every app built with AppCompat.
        assert_eq!(role_of(&node("android.widget.Button")), Role::Button);
        assert_eq!(
            role_of(&node("androidx.appcompat.widget.AppCompatButton")),
            Role::Button
        );
    }

    #[test]
    fn an_editable_node_is_a_textbox_whatever_its_class() {
        let mut editable = node("android.view.View");
        editable.editable = true;
        assert_eq!(role_of(&editable), Role::TextBox);
    }

    #[test]
    fn a_clickable_compose_view_is_a_button() {
        // Compose reports almost everything as a bare View. Classifying by class
        // alone leaves a whole modern toolkit as undifferentiated boxes.
        let mut view = node("android.view.View");
        view.clickable = true;
        assert_eq!(role_of(&view), Role::Button);
    }

    #[test]
    fn an_unclickable_unknown_class_keeps_its_name() {
        assert_eq!(
            role_of(&node("com.example.FancyThing")),
            Role::Other("FancyThing".into())
        );
    }

    #[test]
    fn the_name_prefers_what_is_on_screen() {
        let mut button = node("android.widget.Button");
        button.text = "Save".into();
        button.desc = "Save the document".into();
        button.res_id = "com.a:id/save_btn".into();
        assert_eq!(to_node(&button).name, "Save");
    }

    #[test]
    fn a_node_with_no_text_falls_back_to_its_description_then_its_id() {
        let mut icon = node("android.widget.ImageButton");
        icon.desc = "Close".into();
        assert_eq!(to_node(&icon).name, "Close");

        let mut nameless = node("android.widget.ImageButton");
        nameless.res_id = "com.a:id/close_button".into();
        assert_eq!(to_node(&nameless).name, "close_button");
    }

    #[test]
    fn actions_follow_what_the_node_says_it_accepts() {
        let mut field = node("android.widget.EditText");
        field.editable = true;
        field.clickable = true;
        let built = to_node(&field);
        assert!(built.actions.iter().any(|action| action == "type"));
        assert!(built.actions.iter().any(|action| action == "click"));
        assert!(!built.actions.iter().any(|action| action == "scroll"));
    }

    #[test]
    fn zero_size_bounds_are_no_bounds() {
        // Android reports these for nodes that were never laid out. Treating
        // them as a rectangle puts a clickable target at the origin.
        let mut hidden = node("android.widget.Button");
        hidden.bounds = vec![0, 0, 0, 0];
        assert!(bounds_of(&hidden).is_none());
    }

    #[test]
    fn offscreen_negative_bounds_are_no_bounds() {
        let mut scrolled = node("android.widget.Button");
        scrolled.bounds = vec![100, 50, 40, 10];
        assert!(bounds_of(&scrolled).is_none());
    }

    #[test]
    fn real_bounds_convert() {
        let mut button = node("android.widget.Button");
        button.bounds = vec![10, 20, 110, 70];
        let rect = bounds_of(&button).unwrap();
        assert_eq!((rect.x, rect.y, rect.width, rect.height), (10, 20, 100, 50));
    }
}
