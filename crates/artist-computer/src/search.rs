//! Finding one element without rendering the whole surface.
//!
//! The cheapest useful thing in the crate, because the expensive half of an
//! observation is not the *collection* — it is the rendering. [`AnchorBook`]
//! already holds every element on the surface; `observe` only decides how much
//! of it to write out. So locating one button costs a scan over data we are
//! already keeping, and the model reads three lines instead of six hundred.
//!
//! **Searches the current anchor epoch, not a fresh snapshot.** That is the
//! honest semantic: the anchors returned are exactly the ones that will resolve
//! when the model uses them, and if the surface has moved on since, that is what
//! `observe` is for. It also keeps `find` non-destructive — a shell's primary
//! screen hands over text once and advances, so re-observing inside a search
//! would consume output the model had not seen yet.

use crate::anchors::AnchorBook;
use crate::model::Node;
use crate::program::normalize;

/// How many matches are worth returning before the list stops being a shortlist.
const MATCH_CAP: usize = 25;

/// One hit, with enough context to tell it apart from its neighbours.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub anchor: String,
    pub node: Node,
    /// Lower is better. 0 is an exact name match.
    pub distance: usize,
}

/// Search the live anchor set for elements whose name or value mentions `query`.
///
/// Ranked rather than filtered, because a query like "save" on a real screen
/// matches "Save", "Save as…", "Autosave enabled" and a paragraph explaining
/// what saving does. Exact names first, then names containing the query, then
/// values — a hit in a text field's *contents* is a weaker signal than a hit in
/// a button's label, and burying the button under it would waste the round trip
/// this call exists to save.
pub fn find(book: &AnchorBook, query: &str) -> Vec<Match> {
    let wanted = normalize(query);
    if wanted.is_empty() {
        return Vec::new();
    }

    let mut hits: Vec<Match> = book
        .anchors()
        .filter_map(|(anchor, node)| {
            let name = normalize(&node.name);
            let value = node.value.as_deref().map(normalize).unwrap_or_default();

            let distance = if name == wanted {
                0
            } else if name.starts_with(&wanted) {
                1
            } else if name.contains(&wanted) {
                2
            } else if value.contains(&wanted) {
                3
            } else {
                return None;
            };

            Some(Match {
                anchor: anchor.clone(),
                node: node.clone(),
                distance,
            })
        })
        .collect();

    // Rank, then break ties the way a person scans: down the screen, then
    // across. Without the positional tie-break the order comes from a HashMap
    // and changes between identical calls, which makes the tool's output
    // irreproducible for no reason.
    hits.sort_by(|a, b| {
        a.distance.cmp(&b.distance).then_with(|| {
            let (left, right) = (a.node.bounds, b.node.bounds);
            match (left, right) {
                (Some(left), Some(right)) => (left.y, left.x).cmp(&(right.y, right.x)),
                _ => a.anchor.cmp(&b.anchor),
            }
        })
    });
    hits.truncate(MATCH_CAP);
    hits
}

/// Render matches for the model.
///
/// Deliberately not the observation format: an observation describes a surface,
/// and this answers a question. It carries the anchor, the role, the name, and
/// the depth as a breadcrumb — enough to tell two "Delete"s apart — and nothing
/// else.
pub fn render(surface: &str, query: &str, hits: &[Match], total: usize) -> String {
    if hits.is_empty() {
        return format!(
            "<find surface=\"{surface}\" query=\"{query}\">\n\
             nothing on this surface matches. It has {total} element(s); \
             observe it if your picture of it may be stale.\n"
        );
    }

    let mut out = format!(
        "<find surface=\"{surface}\" query=\"{query}\" matches=\"{}\">\n",
        hits.len()
    );
    for hit in hits {
        for _ in 0..hit.node.depth.min(6) {
            out.push_str("  ");
        }
        out.push_str(hit.node.role.label());
        if !hit.node.name.is_empty() {
            out.push_str(&format!(
                " {:?}",
                crate::render::truncate_public(&hit.node.name)
            ));
        }
        if let Some(value) = &hit.node.value {
            out.push_str(&format!(" = {:?}", crate::render::truncate_public(value)));
        }
        let flags = hit.node.state.flags();
        if !flags.is_empty() {
            out.push_str(&format!(" [{}]", flags.join(",")));
        }
        out.push_str(&format!(" ({})\n", hit.anchor));
    }
    if hits.len() == MATCH_CAP {
        out.push_str("… more matches were cut. Narrow the query.\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Rect, Role, Snapshot};

    fn at(binding: &str, role: Role, name: &str, y: i32) -> Node {
        Node::new(binding, role, name).with_bounds(Rect {
            x: 0,
            y,
            width: 40,
            height: 10,
        })
    }

    fn book_of(nodes: Vec<Node>) -> AnchorBook {
        let mut book = AnchorBook::new();
        book.observe(&Snapshot::new(nodes), true);
        book
    }

    #[test]
    fn an_exact_name_outranks_a_partial_one() {
        let book = book_of(vec![
            at("a", Role::Text, "Autosave is enabled", 10),
            at("b", Role::Button, "Save", 20),
            at("c", Role::Button, "Save as…", 30),
        ]);
        let hits = find(&book, "Save");

        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].node.name, "Save", "exact first: {hits:?}");
        // "Save as…" starts with the query; the sentence merely contains it.
        assert_eq!(hits[1].node.name, "Save as…");
        assert_eq!(hits[2].node.name, "Autosave is enabled");
    }

    #[test]
    fn a_hit_in_a_value_ranks_below_every_hit_in_a_name() {
        let book = book_of(vec![
            at("field", Role::TextBox, "Recipient", 10).with_value("delete@example.com"),
            at("button", Role::Button, "Delete", 20),
        ]);
        let hits = find(&book, "delete");
        assert_eq!(
            hits[0].node.name, "Delete",
            "the button must come first: {hits:?}"
        );
        assert_eq!(hits[1].node.name, "Recipient");
    }

    #[test]
    fn the_order_is_stable_across_identical_calls() {
        // The live set is a HashMap, so without an explicit tie-break the output
        // order varies between runs and the tool is irreproducible.
        let book = book_of(
            (0..12)
                .map(|index| at(&format!("n{index}"), Role::Button, "Delete", index * 10))
                .collect(),
        );
        let first = find(&book, "Delete");
        for _ in 0..8 {
            assert_eq!(find(&book, "Delete"), first, "search order must be stable");
        }
        // And it is reading order, not hash order.
        let ys: Vec<i32> = first.iter().map(|hit| hit.node.bounds.unwrap().y).collect();
        assert!(ys.windows(2).all(|pair| pair[0] <= pair[1]), "{ys:?}");
    }

    #[test]
    fn every_returned_anchor_actually_resolves() {
        // The whole point: a hit is directly usable. If `find` invented anchors
        // or returned retired ones, the model would get an unresolvable token
        // from a tool whose job is to hand it a usable one.
        let book = book_of(vec![
            at("a", Role::Button, "Save", 10),
            at("b", Role::Button, "Cancel", 20),
        ]);
        for hit in find(&book, "Save") {
            assert!(
                book.resolve(&hit.anchor).is_ok(),
                "{} did not resolve",
                hit.anchor
            );
        }
    }

    #[test]
    fn a_query_that_matches_nothing_says_so_and_says_what_to_do() {
        let book = book_of(vec![at("a", Role::Button, "Save", 10)]);
        let hits = find(&book, "Postpone");
        assert!(hits.is_empty());

        let rendered = render("win:1", "Postpone", &hits, 1);
        assert!(
            rendered.contains("nothing on this surface matches"),
            "{rendered}"
        );
        assert!(
            rendered.contains("observe"),
            "the recovery must be named: {rendered}"
        );
    }

    #[test]
    fn matches_are_capped_and_say_they_were() {
        let book = book_of(
            (0..MATCH_CAP + 20)
                .map(|index| at(&format!("n{index}"), Role::Button, "Delete", index as i32))
                .collect(),
        );
        let hits = find(&book, "Delete");
        assert_eq!(hits.len(), MATCH_CAP);

        let rendered = render("win:1", "Delete", &hits, MATCH_CAP + 20);
        assert!(rendered.contains("Narrow the query"), "{rendered}");
    }

    #[test]
    fn searching_uses_the_same_normalization_as_everything_else() {
        // `&Save` and `Save…` are the same button as far as every other part of
        // the crate is concerned, and a search that disagreed would be a second
        // vocabulary.
        let book = book_of(vec![at("a", Role::Button, "&Save…", 10)]);
        assert_eq!(find(&book, "save").len(), 1);
        assert_eq!(find(&book, "SAVE").len(), 1);
    }

    #[test]
    fn an_empty_query_matches_nothing_rather_than_everything() {
        let book = book_of(vec![at("a", Role::Button, "Save", 10)]);
        assert!(find(&book, "").is_empty());
        assert!(find(&book, "   ").is_empty());
    }
}
