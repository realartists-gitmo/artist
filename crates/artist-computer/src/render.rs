//! Turning an observation into the text the model reads.
//!
//! The only place in the crate that produces model-facing prose, and the only
//! place that decides what to leave out. `bounds` never appears here — see
//! [`crate::model`].
//!
//! The budget is not a nicety. A hundred-step session against a real
//! application will render a hundred observations; if each one is a full tree
//! dump the context is gone by step twenty and the agent forgets the task. So:
//! interactive nodes always survive, static text is coalesced and truncated,
//! and the overflow is named rather than silently dropped.

use crate::anchors::{Change, Entry, Observation};

/// Longest rendered value before it is cut.
const VALUE_CAP: usize = 120;
/// How many static-text lines survive before the rest is summarized.
const TEXT_BUDGET: usize = 40;

/// The sentinel opening every observation.
///
/// Load-bearing beyond display: the decay pass keys on it to recognize a stale
/// observation it may elide, which is why it is emitted even when a surface has
/// nothing to report.
pub fn sentinel(surface: &str, epoch: u64, elided: bool, image: Option<&str>) -> String {
    let mut tag = format!("<observation surface=\"{surface}\" epoch=\"{epoch}\"");
    if elided {
        tag.push_str(" elided=\"true\"");
    }
    if let Some(digest) = image {
        tag.push_str(&format!(" img=\"{digest}\""));
    }
    tag.push('>');
    tag
}

/// Render one observation for the model.
pub fn observation(surface: &str, observed: &Observation, image: Option<&str>) -> String {
    let mut out = sentinel(surface, observed.epoch, false, image);
    out.push('\n');

    if observed.replaced {
        out.push_str("(surface replaced — full view)\n");
    }
    if observed.full {
        out.push_str(&render_full(&observed.entries));
    } else {
        out.push_str(&render_delta(observed));
    }
    out
}

fn render_full(entries: &[Entry]) -> String {
    let mut out = String::new();
    let mut text_shown = 0usize;
    let mut text_hidden = 0usize;

    for entry in entries {
        if entry.node.role.is_interactive() {
            out.push_str(&line(entry, None));
            out.push('\n');
            continue;
        }
        // A node with neither a name nor a value shows nothing and is dropped
        // whether or not there is budget left. Counting it toward the overflow
        // inflated "N more text nodes" with nodes that would never have been
        // rendered, so the model was told to re-observe for content that does
        // not exist.
        if entry.node.name.trim().is_empty() && entry.node.value.is_none() {
            continue;
        }
        // Static text is the compressible part: it is usually most of a screen
        // and almost never what the next action targets.
        if text_shown < TEXT_BUDGET {
            out.push_str(&line(entry, None));
            out.push('\n');
            text_shown += 1;
        } else {
            text_hidden += 1;
        }
    }

    if text_hidden > 0 {
        out.push_str(&format!(
            "… {text_hidden} more text node(s) — observe with full=true after scrolling, or expand a container\n"
        ));
    }
    if out.is_empty() {
        out.push_str("(no actionable elements)\n");
    }
    out
}

fn render_delta(observed: &Observation) -> String {
    let mut out = String::new();
    // The same budget a full render applies, for the same reason. An unbudgeted
    // delta is not cheaper than a full render just because it is a diff: a page
    // that appends a log or expands a tree emits arbitrarily many static-text
    // lines, and a single 5 kB removed text node used to print in full.
    let mut text_shown = 0usize;
    let mut text_hidden = 0usize;

    for entry in &observed.entries {
        let marker = match entry.change {
            Some(Change::Added) => "+",
            Some(Change::Changed) => "~",
            _ => " ",
        };
        if !entry.node.role.is_interactive() {
            if text_shown >= TEXT_BUDGET {
                text_hidden += 1;
                continue;
            }
            text_shown += 1;
        }
        out.push_str(&line(entry, Some(marker)));
        out.push('\n');
    }

    let mut removed_hidden = 0usize;
    for (anchor, node) in &observed.removed {
        if !node.role.is_interactive() {
            if text_shown >= TEXT_BUDGET {
                removed_hidden += 1;
                continue;
            }
            text_shown += 1;
        }
        // Truncated like every other rendered name. A removal line skipping
        // `truncate` meant the one node the model can no longer act on was the
        // one allowed to print without limit.
        out.push_str(&format!(
            "- {} {:?} ({anchor})\n",
            node.role.label(),
            truncate(&node.name, VALUE_CAP)
        ));
    }

    let hidden = text_hidden + removed_hidden;
    if hidden > 0 {
        out.push_str(&format!(
            "… {hidden} more changed text node(s) — observe with full=true to see them\n"
        ));
    }
    if out.is_empty() {
        out.push_str("(no change)\n");
    }
    out
}

fn line(entry: &Entry, marker: Option<&str>) -> String {
    let mut out = String::new();
    if let Some(marker) = marker {
        out.push_str(marker);
        out.push(' ');
    }
    // Depth is rendered for containers so the model can tell nesting apart, but
    // capped: a deeply nested DOM would otherwise indent everything off-screen.
    for _ in 0..entry.node.depth.min(6) {
        out.push_str("  ");
    }
    out.push_str(entry.node.role.label());

    if !entry.node.name.is_empty() {
        out.push_str(&format!(" {:?}", truncate(&entry.node.name, VALUE_CAP)));
    }
    if let Some(value) = &entry.node.value {
        out.push_str(&format!(" = {:?}", truncate(value, VALUE_CAP)));
    }
    let flags = entry.node.state.flags();
    if !flags.is_empty() {
        out.push_str(&format!(" [{}]", flags.join(",")));
    }
    out.push_str(&format!(" ({})", entry.anchor));
    out
}

/// Flatten to one line and cap the length.
///
/// Line breaks and tabs become single spaces — a multi-line accessible name has
/// to fit on one rendered row — but runs of spaces are preserved. In a terminal
/// row the spacing *is* the content: collapsing `PID  COMMAND` to `PID COMMAND`
/// destroys the column alignment the model needs to read a table.
fn truncate(value: &str, cap: usize) -> String {
    let flattened: String = value
        .chars()
        .map(|character| match character {
            '\n' | '\r' | '\t' => ' ',
            other => other,
        })
        .collect();
    let flattened = flattened.trim_end();
    if flattened.chars().count() <= cap {
        return flattened.to_owned();
    }
    let kept: String = flattened.chars().take(cap.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// `truncate`, for the one other module that renders model-facing text.
///
/// `search.rs` answers a question rather than describing a surface, so it has
/// its own format — but a name must be cut the same way in both, or the same
/// element reads differently depending on which call surfaced it.
pub(crate) fn truncate_public(value: &str) -> String {
    truncate(value, VALUE_CAP)
}

/// The stub a decayed observation leaves behind.
pub fn elided(surface: &str, epoch: u64, image: Option<&str>) -> String {
    format!(
        "{}\n[elided to save context — call computer with mode=\"observe\", full=true on {surface} to see it again]",
        sentinel(surface, epoch, true, image)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchors::AnchorBook;
    use crate::model::{Node, NodeState, Role, Snapshot};

    fn book_with(nodes: Vec<Node>) -> (AnchorBook, Observation) {
        let mut book = AnchorBook::new();
        let observed = book.observe(&Snapshot::new(nodes), false);
        (book, observed)
    }

    #[test]
    fn a_full_observation_names_every_interactive_element() {
        let (_book, observed) = book_with(vec![
            Node::new("a", Role::Button, "Save"),
            Node::new("b", Role::TextBox, "Email").with_value("adam@example.com"),
        ]);
        let text = observation("win:1", &observed, None);

        assert!(text.starts_with("<observation surface=\"win:1\" epoch=\"1\">"));
        assert!(text.contains("button \"Save\""));
        assert!(text.contains("textbox \"Email\" = \"adam@example.com\""));
    }

    #[test]
    fn coordinates_never_reach_the_model() {
        use crate::model::Rect;
        let (_book, observed) = book_with(vec![Node::new("a", Role::Button, "Save").with_bounds(
            Rect {
                x: 743,
                y: 219,
                width: 80,
                height: 24,
            },
        )]);
        let text = observation("win:1", &observed, None);
        for forbidden in ["743", "219", "bounds"] {
            assert!(
                !text.contains(forbidden),
                "rendered text leaked geometry ({forbidden}): {text}"
            );
        }
    }

    /// A surface with enough unchanged nodes that swapping one is a genuine
    /// diff rather than a replacement.
    fn stable_screen(last: &str) -> Snapshot {
        let mut nodes: Vec<Node> = (0..8)
            .map(|index| Node::new(format!("s{index}"), Role::Button, format!("Item {index}")))
            .collect();
        nodes.push(Node::new(last, Role::Button, last));
        Snapshot::new(nodes)
    }

    #[test]
    fn a_delta_renders_change_markers_and_removals() {
        let mut book = AnchorBook::new();
        book.observe(&stable_screen("Cancel"), false);
        let delta = book.observe(&stable_screen("Help"), false);
        let text = observation("win:1", &delta, None);

        assert!(text.contains("+ button \"Help\""), "{text}");
        assert!(text.contains("- button \"Cancel\""), "{text}");
        assert!(
            !text.contains("\"Item 0\""),
            "an unchanged element must not be repeated: {text}"
        );
    }

    #[test]
    fn a_surface_that_turns_over_wholesale_renders_full_instead_of_a_double_diff() {
        // Navigation rebinds everything, so the "delta" is every new node as
        // `+` plus every old one as `-` — two screens rendered to describe one,
        // costing about twice a full render.
        let mut book = AnchorBook::new();
        book.observe(
            &Snapshot::new(
                (0..20)
                    .map(|index| {
                        Node::new(format!("old{index}"), Role::Button, format!("Old {index}"))
                    })
                    .collect(),
            ),
            false,
        );
        let after = book.observe(
            &Snapshot::new(
                (0..20)
                    .map(|index| {
                        Node::new(format!("new{index}"), Role::Button, format!("New {index}"))
                    })
                    .collect(),
            ),
            false,
        );

        assert!(after.full, "a wholesale replacement must render full");
        assert!(after.replaced);
        let text = observation("win:1", &after, None);
        assert!(text.contains("(surface replaced — full view)"), "{text}");
        assert!(text.contains("\"New 0\""), "{text}");
        assert!(
            !text.contains("Old 0"),
            "the previous screen must not be rendered a second time: {text}"
        );
    }

    #[test]
    fn a_delta_budgets_its_text_the_same_way_a_full_render_does() {
        let mut book = AnchorBook::new();
        let base: Vec<Node> = (0..200)
            .map(|index| Node::new(format!("t{index}"), Role::Text, "quiet"))
            .collect();
        book.observe(&Snapshot::new(base.clone()), false);

        // Every text node changes at once — a log pane appending, a tree
        // expanding. Unbudgeted, this printed all 200.
        let churned: Vec<Node> = base
            .iter()
            .enumerate()
            .map(|(index, node)| {
                Node::new(node.binding.as_str(), Role::Text, format!("line {index}"))
            })
            .collect();
        let delta = book.observe(&Snapshot::new(churned), false);
        assert!(!delta.full, "changed-in-place is not a replacement");

        let text = observation("win:1", &delta, None);
        assert!(text.contains("more changed text node(s)"), "{text}");
        assert!(
            text.lines().count() < 60,
            "a delta must be budgeted, got {} lines",
            text.lines().count()
        );
    }

    #[test]
    fn a_removed_node_with_a_huge_name_is_truncated_like_any_other() {
        let mut book = AnchorBook::new();
        let mut nodes = stable_screen("Cancel").nodes;
        nodes.push(Node::new("blob", Role::Text, "x".repeat(5_000)));
        book.observe(&Snapshot::new(nodes), false);

        let delta = book.observe(&stable_screen("Cancel"), false);
        let text = observation("win:1", &delta, None);
        assert!(
            text.len() < 1_000,
            "a removal line skipped truncation entirely: {} bytes",
            text.len()
        );
    }

    #[test]
    fn static_text_is_budgeted_but_never_silently_dropped() {
        let nodes = (0..TEXT_BUDGET + 25)
            .map(|index| Node::new(format!("t{index}"), Role::Text, format!("line {index}")))
            .collect();
        let (_book, observed) = book_with(nodes);
        let text = observation("win:1", &observed, None);

        assert!(
            text.contains("25 more text node(s)"),
            "the overflow must be named: {text}"
        );
    }

    #[test]
    fn interactive_elements_survive_the_budget() {
        let mut nodes: Vec<Node> = (0..200)
            .map(|index| Node::new(format!("t{index}"), Role::Text, format!("line {index}")))
            .collect();
        nodes.push(Node::new("go", Role::Button, "Submit"));
        let (_book, observed) = book_with(nodes);
        let text = observation("win:1", &observed, None);

        assert!(
            text.contains("button \"Submit\""),
            "a button must never be truncated away: {text}"
        );
    }

    #[test]
    fn long_values_are_collapsed_and_capped() {
        let sprawling = "a ".repeat(400);
        let (_book, observed) = book_with(vec![
            Node::new("a", Role::TextBox, "Notes").with_value(sprawling),
        ]);
        let text = observation("win:1", &observed, None);
        assert!(text.contains('…'));
        assert!(text.lines().all(|line| line.chars().count() < 400));
    }

    #[test]
    fn flags_render_when_notable() {
        let (_book, observed) = book_with(vec![
            Node::new("a", Role::CheckBox, "Remember me").with_state(NodeState {
                checked: true,
                disabled: true,
                ..NodeState::default()
            }),
        ]);
        let text = observation("win:1", &observed, None);
        assert!(text.contains("[disabled,checked]"));
    }

    #[test]
    fn the_elided_stub_keeps_the_sentinel_and_the_image_digest() {
        let stub = elided("win:3", 7, Some("ab12cd"));
        assert!(stub.contains("elided=\"true\""));
        assert!(stub.contains("img=\"ab12cd\""));
        assert!(stub.contains("mode=\"observe\""));
    }
}
