//! Structural outline rendering against mnemonic anchors.
//!
//! `artist-ast` answers "which byte ranges are declarations"; this module turns
//! that into something the model can act on. Two things make it more than a
//! pretty-printer:
//!
//! **It speaks anchors, not line numbers.** Upstream renders `21: fn foo`.
//! Line numbers are positional, so any insert above invalidates them — the
//! reason this project addresses lines by content hash in the first place. An
//! outline carrying anchors feeds `edit` directly: the model can replace a whole
//! function straight from the outline with no intervening `read`.
//!
//! **It has a budget, not switches.** Upstream's altitude control is five
//! booleans (`--no-private`, `--no-fields`, …), which is a coarse choice made
//! before you know how big the file is. This folds everything, then unfolds
//! outer-then-inner until the output reaches a target line count, reverting the
//! step that would breach a hard ceiling. Shape follows the budget you have.
//!
//! The caller must reconcile anchors over the *whole* file and pass the full
//! line map here. Reconciling over only the declaration lines would free the
//! handles for every line the outline does not show.

use artist_ast::core::Declaration;
use hashline_tools::AnchoredLine;
use std::collections::HashMap;

/// How much outline to produce.
#[derive(Debug, Clone)]
pub struct OutlineOptions {
    /// Target visible declaration count. Unfolding stops once reached.
    pub budget: usize,
    /// Hard ceiling. An unfold step that would push past this is reverted and
    /// the walk stops, so one wide class cannot blow the budget open.
    pub ceiling: usize,
    /// Include declarations the adapter marked non-public.
    pub include_private: bool,
}

impl Default for OutlineOptions {
    fn default() -> Self {
        // 120 lines is roughly a screen and change: enough to see the shape of
        // a large file without approaching what a full `read` would have cost.
        // The ceiling is double, matching the upstream convention of allowing
        // an overshoot rather than truncating mid-level.
        Self {
            budget: 120,
            ceiling: 240,
            include_private: true,
        }
    }
}

/// A declaration selected for display, flattened out of the tree.
struct Row<'a> {
    decl: &'a Declaration,
    depth: usize,
}

/// Map 1-indexed line numbers to the anchors already issued for them.
pub fn anchor_map(lines: &[AnchoredLine]) -> HashMap<usize, &str> {
    lines
        .iter()
        .map(|l| (l.line_number, l.anchor.as_str()))
        .collect()
}

/// Render an outline of `decls`, addressing every row by anchor.
///
/// `anchors` must cover the whole file — see the module note on reconciliation.
pub fn render(
    decls: &[Declaration],
    anchors: &HashMap<usize, &str>,
    opts: &OutlineOptions,
) -> String {
    let rows = select(decls, opts);
    let mut out = String::new();
    for row in &rows {
        let Some(start) = anchors.get(&row.decl.start_line) else {
            // A declaration whose start line has no issued anchor cannot be
            // addressed, so showing it would invite an edit that fails. Skip.
            continue;
        };
        let end = anchors.get(&row.decl.end_line);
        match end {
            Some(end) if row.decl.end_line != row.decl.start_line => {
                out.push_str(&format!("{start}..{end}: "));
            }
            _ => out.push_str(&format!("{start}: ")),
        }
        for _ in 0..row.depth {
            out.push_str("  ");
        }
        out.push_str(row.decl.signature.trim());
        out.push('\n');
    }
    if out.is_empty() {
        out.push_str("(no declarations)\n");
    }
    out
}

/// Fold everything, then unfold outer-then-inner until the budget is met.
///
/// Breadth-first so an unfold step reveals a whole sibling group rather than
/// descending one branch to the exclusion of the rest — a file's shape is the
/// top two levels far more often than one deep spine.
fn select<'a>(decls: &'a [Declaration], opts: &OutlineOptions) -> Vec<Row<'a>> {
    let visible_at_root: Vec<&Declaration> = decls
        .iter()
        .filter(|d| opts.include_private || is_public(d))
        .collect();

    let mut rows: Vec<Row<'a>> = visible_at_root
        .iter()
        .map(|d| Row { decl: d, depth: 0 })
        .collect();

    // Queue of declarations whose children have not been revealed yet.
    let mut queue: std::collections::VecDeque<(&'a Declaration, usize)> =
        visible_at_root.iter().map(|d| (*d, 0usize)).collect();

    while let Some((decl, depth)) = queue.pop_front() {
        if rows.len() >= opts.budget {
            break;
        }
        let children: Vec<&Declaration> = decl
            .children
            .iter()
            .filter(|c| opts.include_private || is_public(c))
            .collect();
        if children.is_empty() {
            continue;
        }
        if rows.len() + children.len() > opts.ceiling {
            // Revert this step rather than half-revealing a sibling group: a
            // partial class body reads as a complete one and misleads.
            continue;
        }
        for child in children {
            rows.push(Row {
                decl: child,
                depth: depth + 1,
            });
            queue.push_back((child, depth + 1));
        }
    }

    // Restore source order; BFS produced level order, which reads oddly when
    // the rows are meant to mirror the file.
    rows.sort_by_key(|r| r.decl.start_line);
    rows
}

/// Adapters populate `visibility` with language-native words. Treat anything
/// explicitly private-ish as private and default to public — an adapter that
/// leaves it blank should not have its declarations silently dropped.
fn is_public(decl: &Declaration) -> bool {
    !matches!(
        decl.visibility.as_str(),
        "private" | "protected" | "internal" | "fileprivate"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_ast::core::DeclarationKind;

    fn decl(name: &str, start: usize, end: usize, children: Vec<Declaration>) -> Declaration {
        Declaration {
            kind: DeclarationKind::Function,
            name: name.to_string(),
            signature: format!("fn {name}()"),
            start_line: start,
            end_line: end,
            children,
            ..Default::default()
        }
    }

    fn anchors_for(pairs: &[(usize, &'static str)]) -> HashMap<usize, &'static str> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn multi_line_declaration_renders_a_span() {
        let decls = vec![decl("alpha", 1, 5, vec![])];
        let anchors = anchors_for(&[(1, "time"), (5, "beta")]);
        let out = render(&decls, &anchors, &OutlineOptions::default());
        assert_eq!(out, "time..beta: fn alpha()\n");
    }

    #[test]
    fn single_line_declaration_renders_one_anchor() {
        let decls = vec![decl("alpha", 3, 3, vec![])];
        let anchors = anchors_for(&[(3, "time")]);
        let out = render(&decls, &anchors, &OutlineOptions::default());
        assert_eq!(out, "time: fn alpha()\n");
    }

    /// Two-word handles contain a space, so a space-separated span would be
    /// ambiguous with a single handle. `..` cannot occur inside a mnemonic.
    #[test]
    fn two_word_anchors_stay_unambiguous() {
        let decls = vec![decl("alpha", 1, 4, vec![])];
        let anchors = anchors_for(&[(1, "time beta"), (4, "nod deep")]);
        let out = render(&decls, &anchors, &OutlineOptions::default());
        assert_eq!(out, "time beta..nod deep: fn alpha()\n");
    }

    #[test]
    fn unanchored_declaration_is_skipped_not_guessed() {
        let decls = vec![decl("alpha", 1, 2, vec![]), decl("beta", 9, 9, vec![])];
        let anchors = anchors_for(&[(9, "sam")]);
        let out = render(&decls, &anchors, &OutlineOptions::default());
        assert_eq!(out, "sam: fn beta()\n");
    }

    #[test]
    fn children_are_indented_and_in_source_order() {
        let decls = vec![decl(
            "outer",
            1,
            9,
            vec![decl("inner_a", 2, 3, vec![]), decl("inner_b", 5, 6, vec![])],
        )];
        let anchors = anchors_for(&[
            (1, "aa"),
            (9, "bb"),
            (2, "cc"),
            (3, "dd"),
            (5, "ee"),
            (6, "ff"),
        ]);
        let out = render(&decls, &anchors, &OutlineOptions::default());
        assert_eq!(
            out,
            "aa..bb: fn outer()\ncc..dd:   fn inner_a()\nee..ff:   fn inner_b()\n"
        );
    }

    #[test]
    fn budget_stops_the_unfold_before_children_appear() {
        let decls = vec![decl("outer", 1, 9, vec![decl("inner", 2, 3, vec![])])];
        let anchors = anchors_for(&[(1, "aa"), (9, "bb"), (2, "cc"), (3, "dd")]);
        let opts = OutlineOptions {
            budget: 1,
            ..Default::default()
        };
        let out = render(&decls, &anchors, &opts);
        assert_eq!(out, "aa..bb: fn outer()\n");
    }

    /// A sibling group that would breach the ceiling is reverted whole. Half a
    /// class body reads as a complete one, which is worse than omitting it.
    #[test]
    fn ceiling_reverts_a_whole_sibling_group() {
        let kids: Vec<Declaration> = (0..5)
            .map(|i| decl(&format!("k{i}"), 10 + i, 10 + i, vec![]))
            .collect();
        let decls = vec![decl("outer", 1, 40, kids)];
        let mut pairs: Vec<(usize, &'static str)> = vec![(1, "aa"), (40, "bb")];
        for (i, a) in ["c0", "c1", "c2", "c3", "c4"].iter().enumerate() {
            pairs.push((10 + i, a));
        }
        let anchors = anchors_for(&pairs);
        let opts = OutlineOptions {
            budget: 100,
            ceiling: 3, // 1 visible + 5 children would breach it
            ..Default::default()
        };
        let out = render(&decls, &anchors, &opts);
        assert_eq!(out, "aa..bb: fn outer()\n");
    }

    #[test]
    fn private_declarations_drop_when_asked() {
        let mut private = decl("hidden", 5, 5, vec![]);
        private.visibility = "private".into();
        let decls = vec![decl("shown", 1, 1, vec![]), private];
        let anchors = anchors_for(&[(1, "aa"), (5, "bb")]);
        let opts = OutlineOptions {
            include_private: false,
            ..Default::default()
        };
        let out = render(&decls, &anchors, &opts);
        assert_eq!(out, "aa: fn shown()\n");
    }

    #[test]
    fn blank_visibility_counts_as_public() {
        let decls = vec![decl("shown", 1, 1, vec![])];
        let anchors = anchors_for(&[(1, "aa")]);
        let opts = OutlineOptions {
            include_private: false,
            ..Default::default()
        };
        assert_eq!(render(&decls, &anchors, &opts), "aa: fn shown()\n");
    }

    #[test]
    fn empty_input_says_so_rather_than_returning_nothing() {
        let out = render(&[], &HashMap::new(), &OutlineOptions::default());
        assert_eq!(out, "(no declarations)\n");
    }
}
