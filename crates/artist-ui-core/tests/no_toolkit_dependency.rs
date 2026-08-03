//! The load-bearing rule of the two-frontend split.
//!
//! `artist-ui-core` is what lets the TUI and the GUI be peers rather than one
//! being a port of the other. That only holds while the core cannot express a
//! toolkit's concepts — the moment `ratatui::Span` or `gpui::Element` is
//! reachable from here, the core starts absorbing one view's assumptions and the
//! other silently becomes second-class.
//!
//! Code review does not catch this reliably, because the offending line is
//! always a small convenience. A test does.

use std::collections::{HashMap, HashSet};

/// Toolkits the core must never be able to reach, transitively.
const FORBIDDEN: &[&str] = &["ratatui", "gpui", "crossterm", "wry", "tao", "iced", "egui"];

#[test]
fn core_cannot_reach_a_rendering_toolkit() {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .exec()
        .expect("cargo metadata runs");

    let resolve = metadata
        .resolve
        .as_ref()
        .expect("a resolved dependency graph");
    let root = resolve
        .root
        .as_ref()
        .expect("artist-ui-core is the root package");

    let edges: HashMap<_, _> = resolve
        .nodes
        .iter()
        .map(|node| (&node.id, &node.deps))
        .collect();
    let names: HashMap<_, _> = metadata
        .packages
        .iter()
        .map(|package| (&package.id, package.name.as_str()))
        .collect();

    // Walk only normal and build edges. A dev-dependency on a toolkit would be
    // harmless — it cannot reach the library — and excluding them keeps the rule
    // from blocking, say, a snapshot test helper.
    let mut seen = HashSet::new();
    let mut stack = vec![root];
    let mut found = Vec::new();

    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(&name) = names.get(id)
            && FORBIDDEN.contains(&name)
        {
            found.push(name);
        }
        for dep in edges.get(id).into_iter().flat_map(|deps| deps.iter()) {
            let reachable = dep.dep_kinds.iter().any(|kind| {
                matches!(
                    kind.kind,
                    cargo_metadata::DependencyKind::Normal | cargo_metadata::DependencyKind::Build
                )
            });
            if reachable {
                stack.push(&dep.pkg);
            }
        }
    }

    assert!(
        found.is_empty(),
        "artist-ui-core reached a rendering toolkit: {found:?}.\n\
         The core is shared by the ratatui and gpui frontends and must stay \
         toolkit-agnostic — move the offending code into the view that needs it, \
         and add a semantic type here instead if both views need to agree."
    );
}
