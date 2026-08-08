//! Anchor minting, resolution, staleness and diffing — for every rung.
//!
//! This module is the reason `computer` is one tool rather than four. Backends
//! produce [`Node`]s carrying opaque [`Binding`]s; nothing below this layer ever
//! sees an anchor token, and nothing above it ever sees a binding. So a CDP DOM
//! node, an AT-SPI accessible and a terminal row all get identical naming,
//! identical delta semantics and identical error wording, by construction
//! rather than by four backends agreeing to behave.
//!
//! Resolution is **identity-only**. There is deliberately no fallback to
//! matching on role and name: two buttons both called "Delete" are exactly the
//! case anchors exist to disambiguate, and a re-rendered list where row 3 now
//! holds what row 4 held is exactly the case a path fallback would get wrong.
//! Anything that is not a live binding is stale, and the recovery is to observe
//! again.

use std::collections::{HashMap, HashSet, VecDeque};

use hashline_tools::AnchorTable;

use crate::model::{Binding, Node, Snapshot};

/// How many retired anchors stay remembered as tombstones.
///
/// Bounded because a churning list would otherwise grow this without limit; a
/// few hundred is far more than the handful of epochs a model can plausibly be
/// working from.
const RETIRED_MEMORY: usize = 512;

/// The share of a surface that must turn over before a delta stops being a
/// useful description of what happened.
///
/// Below this, a diff is exactly what the model wants. Above it — a navigation,
/// a modal replacing the page, a tab switch — the diff is longer than the thing
/// it describes and invites the model to reason about elements that are gone.
const CHURN_PROMOTES_TO_FULL: f32 = 0.6;

/// What changed about one node between two epochs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Changed,
    Removed,
}

/// One entry of a rendered observation.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub anchor: String,
    pub node: Node,
    pub change: Option<Change>,
}

/// The result of observing a surface: anchored nodes plus what moved.
#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    pub epoch: u64,
    /// False when this is a delta against the previous epoch.
    pub full: bool,
    pub entries: Vec<Entry>,
    /// Nodes that disappeared. Carried separately because they are not part of
    /// the current snapshot and so have no live anchor to render against.
    pub removed: Vec<(String, Node)>,
    /// Set when a requested delta was promoted to a full render because the
    /// surface turned over wholesale. See [`CHURN_PROMOTES_TO_FULL`].
    pub replaced: bool,
}

/// Why an anchor did not resolve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnchorError {
    /// Never issued for this surface — most often the model invented it, or
    /// used a token from a different surface.
    NotIssued(String),
    /// Issued once, but the element is gone.
    Stale(String),
}

impl AnchorError {
    pub fn message(&self, what: &str) -> String {
        match self {
            Self::NotIssued(handle) => hashline_tools::not_issued_message(handle, what),
            Self::Stale(handle) => hashline_tools::stale_anchor_message(handle, what),
        }
    }
}

/// Per-surface observation state: deterministic live addresses, prior digests, and
/// the live node set the current anchors point at.
#[derive(Debug, Default)]
pub struct AnchorBook {
    table: AnchorTable,
    /// Digest per binding as of the last observation, for change detection.
    digests: HashMap<Binding, [u8; 32]>,
    /// The nodes the currently issued anchors name.
    live: HashMap<String, Node>,
    /// The same anchors in the order the surface reported them.
    ///
    /// Kept because document order is real information every backend gives us
    /// and a `HashMap` throws away. It is what lets a label be paired with the
    /// value *beside* it — the common shape of a form or a spec table, where
    /// "Total" and "$42.00" are two sibling nodes and nothing but their
    /// adjacency relates them. Geometry cannot substitute: a PTY row and many
    /// CDP nodes have no bounds at all.
    order: Vec<String>,
    /// Exact addresses that named something and no longer do. Tombstones are
    /// diagnostic only and never participate in identity or address allocation.
    retired: HashSet<String>,
    retired_order: VecDeque<String>,
    epoch: u64,
}

impl AnchorBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Absorb a fresh snapshot and describe what changed.
    ///
    /// `full` forces the complete set to be rendered; otherwise everything after
    /// the first observation is a delta. Either way the *collection* is always
    /// complete — a delta is a rendering choice, not a partial read — so every
    /// epoch recomputes the live deterministic address set from complete bindings.
    pub fn observe(&mut self, snapshot: &Snapshot, full: bool) -> Observation {
        let nodes = dedupe(&snapshot.nodes);
        let bindings: Vec<String> = nodes
            .iter()
            .map(|node| node.binding.as_str().to_owned())
            .collect();
        let anchors = self.table.reconcile(&bindings);

        let first = self.epoch == 0;
        let previous_population = self.live.len();
        self.epoch += 1;

        let mut entries = Vec::with_capacity(nodes.len());
        let mut digests = HashMap::with_capacity(nodes.len());
        let mut live = HashMap::with_capacity(nodes.len());
        let mut order = Vec::with_capacity(nodes.len());

        for (node, anchor) in nodes.iter().zip(anchors) {
            // Re-issued to a live element, so it is no longer a tombstone.
            if self.retired.remove(&anchor) {
                self.retired_order.retain(|held| held != &anchor);
            }
            let digest = node.digest();
            let change = match self.digests.get(&node.binding) {
                None if first => None,
                None => Some(Change::Added),
                Some(previous) if *previous != digest => Some(Change::Changed),
                Some(_) => None,
            };
            digests.insert(node.binding.clone(), digest);
            live.insert(anchor.clone(), node.clone());
            order.push(anchor.clone());
            entries.push(Entry {
                anchor,
                node: node.clone(),
                change,
            });
        }

        // Removals are computed against the previous live set, so they are
        // reported by the exact address the model last saw, then not repeated.
        let mut removed = Vec::new();
        for (anchor, node) in &self.live {
            if digests.contains_key(&node.binding) || live.contains_key(anchor) {
                continue;
            }
            if !first {
                removed.push((anchor.clone(), node.clone()));
            }
            if self.retired.insert(anchor.clone()) {
                self.retired_order.push_back(anchor.clone());
            }
        }
        removed.sort_by(|a, b| a.0.cmp(&b.0));
        while self.retired_order.len() > RETIRED_MEMORY {
            if let Some(oldest) = self.retired_order.pop_front() {
                self.retired.remove(&oldest);
            }
        }

        self.digests = digests;
        self.live = live;
        self.order = order;

        // A navigation replaces every binding, so a "delta" becomes every new
        // node as `+` *and* every old one as `-`: two renders of two different
        // screens, costing roughly twice a full render and reading as a diff
        // between things that were never alternatives. Past a churn threshold
        // the honest description is that this is a different surface.
        let churned = entries
            .iter()
            .filter(|entry| entry.change == Some(Change::Added))
            .count()
            + removed.len();
        // Measured against whichever screen had more on it: a page that goes
        // from 500 nodes to 3 has churned just as completely as the reverse.
        let population = nodes.len().max(previous_population).max(1);
        let replaced =
            !full && !first && (churned as f32 / population as f32) >= CHURN_PROMOTES_TO_FULL;

        let full = full || first || replaced;
        if !full {
            entries.retain(|entry| entry.change.is_some());
        }
        Observation {
            epoch: self.epoch,
            full,
            entries,
            removed: if replaced { Vec::new() } else { removed },
            replaced,
        }
    }

    /// The element that currently holds keyboard focus, if a backend said so.
    ///
    /// What a `key` step actually aims at. Populated from the surface's own
    /// state — an AT-SPI state set, a CDP `focused` property — so a model that
    /// names what it expects Enter to activate can be checked against reality.
    pub fn focused(&self) -> Option<&Node> {
        self.live.values().find(|node| node.state.focused)
    }

    /// Resolve an anchor to the node it currently names.
    pub fn resolve(&self, anchor: &str) -> Result<&Node, AnchorError> {
        if let Some(node) = self.live.get(anchor) {
            return Ok(node);
        }
        // Exact opaque matching only. A retired exact address is stale; any byte-different
        // spelling was never issued for this surface.
        if self.retired.contains(anchor) || self.table.binding(anchor).is_some() {
            return Err(AnchorError::Stale(anchor.to_owned()));
        }
        Err(AnchorError::NotIssued(anchor.to_owned()))
    }

    /// Every anchor currently issued, for diagnostics and the inspector.
    pub fn anchors(&self) -> impl Iterator<Item = (&String, &Node)> {
        self.live.iter()
    }

    /// The same set, in the order the surface reported it.
    ///
    /// Prefer this anywhere the *relationship between neighbours* carries
    /// meaning. [`anchors`](Self::anchors) walks a `HashMap` and its order
    /// varies between identical calls, which is fine for a population count and
    /// wrong for anything that reads two adjacent elements together.
    pub fn in_order(&self) -> impl Iterator<Item = (&String, &Node)> {
        self.order
            .iter()
            .filter_map(|anchor| self.live.get(anchor).map(|node| (anchor, node)))
    }
}

/// Give repeated backend bindings the minimal occurrence rank needed to identify twins.
/// This is deterministic left-to-right symmetry breaking before v1 addressing.
fn dedupe(nodes: &[Node]) -> Vec<Node> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        let count = seen.entry(node.binding.as_str()).or_insert(0);
        *count += 1;
        let mut node = node.clone();
        if *count > 1 {
            node.binding = Binding::new(format!("{}#{count}", node.binding.as_str()));
        }
        out.push(node);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeState, Role};

    fn button(binding: &str, name: &str) -> Node {
        Node::new(binding, Role::Button, name)
    }

    fn snapshot(nodes: Vec<Node>) -> Snapshot {
        Snapshot::new(nodes)
    }

    #[test]
    fn first_observation_is_full_and_marks_nothing_as_changed() {
        let mut book = AnchorBook::new();
        let observed = book.observe(
            &snapshot(vec![button("a", "Save"), button("b", "Cancel")]),
            false,
        );

        assert!(observed.full, "first contact must render the whole surface");
        assert_eq!(observed.entries.len(), 2);
        assert!(observed.entries.iter().all(|entry| entry.change.is_none()));
        assert!(observed.removed.is_empty());
    }

    #[test]
    fn a_surviving_binding_keeps_its_anchor_across_epochs() {
        let mut book = AnchorBook::new();
        let first = book.observe(&snapshot(vec![button("a", "Save")]), false);
        let anchor = first.entries[0].anchor.clone();

        // A second element appears; the first must not be renamed.
        let second = book.observe(
            &snapshot(vec![button("a", "Save"), button("b", "Cancel")]),
            true,
        );
        let saved = second
            .entries
            .iter()
            .find(|entry| entry.node.binding.as_str() == "a")
            .unwrap();
        assert_eq!(saved.anchor, anchor, "a live element must never be renamed");
    }

    #[test]
    fn deltas_report_only_what_moved() {
        let mut book = AnchorBook::new();
        book.observe(
            &snapshot(vec![button("a", "Save"), button("b", "Cancel")]),
            false,
        );

        let delta = book.observe(
            &snapshot(vec![
                button("a", "Save"),
                button("b", "Close"),
                button("c", "Help"),
            ]),
            false,
        );

        assert!(!delta.full);
        let changes: Vec<_> = delta
            .entries
            .iter()
            .map(|entry| (entry.node.binding.as_str(), entry.change.clone()))
            .collect();
        assert_eq!(
            changes,
            vec![("b", Some(Change::Changed)), ("c", Some(Change::Added)),],
            "an unchanged node must not appear in a delta"
        );
    }

    #[test]
    fn removals_are_reported_by_the_name_the_model_last_saw() {
        let mut book = AnchorBook::new();
        let first = book.observe(
            &snapshot(vec![button("a", "Save"), button("b", "Cancel")]),
            false,
        );
        let cancel = first
            .entries
            .iter()
            .find(|entry| entry.node.binding.as_str() == "b")
            .unwrap()
            .anchor
            .clone();

        let delta = book.observe(&snapshot(vec![button("a", "Save")]), false);
        assert_eq!(delta.removed.len(), 1);
        assert_eq!(delta.removed[0].0, cancel);
    }

    #[test]
    fn bounds_changes_alone_do_not_produce_a_delta() {
        use crate::model::Rect;

        let mut book = AnchorBook::new();
        book.observe(&snapshot(vec![button("a", "Save")]), false);
        let moved = button("a", "Save").with_bounds(Rect {
            x: 40,
            y: 90,
            width: 10,
            height: 10,
        });
        let delta = book.observe(&snapshot(vec![moved]), false);

        assert!(
            delta.entries.is_empty(),
            "a reflow is not a change the model needs to hear about"
        );
    }

    #[test]
    fn state_changes_produce_a_delta() {
        let mut book = AnchorBook::new();
        book.observe(&snapshot(vec![button("a", "Save")]), false);
        let focused = button("a", "Save").with_state(NodeState {
            focused: true,
            ..NodeState::default()
        });
        let delta = book.observe(&snapshot(vec![focused]), false);
        assert_eq!(delta.entries.len(), 1);
        assert_eq!(delta.entries[0].change, Some(Change::Changed));
    }

    #[test]
    fn duplicate_bindings_never_share_an_anchor() {
        let mut book = AnchorBook::new();
        let observed = book.observe(
            &snapshot(vec![button("row", "Item"), button("row", "Item")]),
            false,
        );
        assert_eq!(observed.entries.len(), 2);
        assert_ne!(
            observed.entries[0].anchor, observed.entries[1].anchor,
            "two elements must never be addressable by one name"
        );
    }

    #[test]
    fn resolution_distinguishes_never_issued_from_stale() {
        let mut book = AnchorBook::new();
        let first = book.observe(&snapshot(vec![button("a", "Save")]), false);
        let anchor = first.entries[0].anchor.clone();

        assert!(book.resolve(&anchor).is_ok());
        assert_eq!(
            book.resolve("definitelynotaword"),
            Err(AnchorError::NotIssued("definitelynotaword".into()))
        );

        book.observe(&snapshot(vec![button("b", "Cancel")]), false);
        assert_eq!(
            book.resolve(&anchor),
            Err(AnchorError::Stale(anchor.clone())),
            "an element that is gone must fail loudly, not resolve to its neighbour"
        );
    }

    #[test]
    fn resolution_is_exact_and_opaque() {
        let mut book = AnchorBook::new();
        let observed = book.observe(&snapshot(vec![button("a", "Save")]), false);
        let anchor = observed.entries[0].anchor.clone();
        assert!(book.resolve(&anchor).is_ok());
        assert!(matches!(
            book.resolve(&format!(" {anchor}")),
            Err(AnchorError::NotIssued(_))
        ));
        let upper = anchor.to_uppercase();
        if upper != anchor {
            assert!(matches!(
                book.resolve(&upper),
                Err(AnchorError::NotIssued(_))
            ));
        }
        assert!(matches!(
            book.resolve(&format!("{anchor}x")),
            Err(AnchorError::NotIssued(_))
        ));
    }

    #[test]
    fn identical_names_stay_independently_addressable() {
        // The case the whole design exists for: two buttons both called
        // "Delete". A role+name fallback would conflate them.
        let mut book = AnchorBook::new();
        let observed = book.observe(
            &snapshot(vec![button("first", "Delete"), button("second", "Delete")]),
            false,
        );
        let first = &observed.entries[0];
        let second = &observed.entries[1];
        assert_ne!(first.anchor, second.anchor);
        assert_eq!(
            book.resolve(&first.anchor).unwrap().binding.as_str(),
            "first"
        );
        assert_eq!(
            book.resolve(&second.anchor).unwrap().binding.as_str(),
            "second"
        );
    }

    #[test]
    fn a_reissued_anchor_stops_being_a_tombstone() {
        // A later deterministic identity may produce the same shortest address.
        // When it does, the live address must resolve normally rather than remain stale.
        let mut book = AnchorBook::new();
        let first = book.observe(&snapshot(vec![button("a", "Save")]), false);
        let anchor = first.entries[0].anchor.clone();

        // Churn until the same one-component address occurs again.
        for epoch in 0..3000 {
            let nodes = (0..8)
                .map(|slot| button(&format!("e{epoch}s{slot}"), "Row"))
                .collect();
            let observed = book.observe(&snapshot(nodes), true);
            if let Some(entry) = observed.entries.iter().find(|entry| entry.anchor == anchor) {
                let binding = entry.node.binding.as_str().to_owned();
                assert_eq!(
                    book.resolve(&anchor).unwrap().binding.as_str(),
                    binding,
                    "a reissued anchor must name its new element, not stay a tombstone"
                );
                return;
            }
        }
    }

    #[test]
    fn churning_surfaces_keep_deterministic_live_addresses() {
        // A list that fully replaces its contents every epoch must remain bounded;
        // only the current live address map and bounded diagnostic tombstones persist.
        let mut book = AnchorBook::new();
        for epoch in 0..500 {
            let nodes = (0..20)
                .map(|row| button(&format!("e{epoch}r{row}"), "Row"))
                .collect();
            let observed = book.observe(&snapshot(nodes), true);
            assert_eq!(observed.entries.len(), 20);
        }
    }
}
