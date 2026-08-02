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
/// How much of the file's shape an outline actually shows.
///
/// Returned rather than inferred, because a caller that claims to be showing a
/// whole file has to be able to check. `read` appends an outline under a header
/// saying "shape of the whole file"; on a declaration-dense file the budget cut
/// that short and said nothing, so the model was told it had the complete shape
/// while a fifth of it was missing.
pub struct Coverage {
    pub shown: usize,
    pub total: usize,
}

impl Coverage {
    pub fn is_complete(&self) -> bool {
        self.shown >= self.total
    }
}

/// Total declarations in a tree, including nested ones.
pub fn count_declarations(decls: &[Declaration]) -> usize {
    decls
        .iter()
        .map(|d| 1 + count_declarations(&d.children))
        .sum()
}

pub fn render(
    decls: &[Declaration],
    anchors: &HashMap<usize, &str>,
    opts: &OutlineOptions,
    lang: &str,
) -> String {
    render_with_coverage(decls, anchors, opts, lang).0
}

/// As [`render`], plus how much of the tree the result covers.
pub fn render_with_coverage(
    decls: &[Declaration],
    anchors: &HashMap<usize, &str>,
    opts: &OutlineOptions,
    lang: &str,
) -> (String, Coverage) {
    let rows = select(decls, opts, lang);
    let mut emitted = 0usize;
    let mut out = String::new();
    let mut previous_start: Option<usize> = None;
    for row in &rows {
        let Some(start) = anchors.get(&row.decl.start_line) else {
            // A declaration whose start line has no issued anchor cannot be
            // addressed, so showing it would invite an edit that fails. Skip.
            continue;
        };
        // Several declarations can begin on one physical line — `struct T { pub a: u32 }`
        // yields both the struct and the field at the same line. They genuinely
        // share an anchor, but repeating it down the gutter reads as a bug. Print
        // it once and indent the rest under it.
        if previous_start == Some(row.decl.start_line) {
            out.push_str(&" ".repeat(start.chars().count()));
            out.push_str("· ");
        } else {
            let end = anchors.get(&row.decl.end_line);
            match end {
                Some(end) if row.decl.end_line != row.decl.start_line => {
                    out.push_str(&format!("{start}..{end}: "));
                }
                _ => out.push_str(&format!("{start}: ")),
            }
        }
        previous_start = Some(row.decl.start_line);
        for _ in 0..row.depth {
            out.push_str("  ");
        }
        out.push_str(row.decl.signature.trim());
        out.push('\n');
        emitted += 1;
    }
    if out.is_empty() {
        out.push_str("(no declarations)\n");
    }
    (
        out,
        Coverage {
            shown: emitted,
            total: count_declarations(decls),
        },
    )
}

/// Fold everything, then unfold outer-then-inner until the budget is met.
///
/// Breadth-first so an unfold step reveals a whole sibling group rather than
/// descending one branch to the exclusion of the rest — a file's shape is the
/// top two levels far more often than one deep spine.
fn select<'a>(decls: &'a [Declaration], opts: &OutlineOptions, lang: &str) -> Vec<Row<'a>> {
    let visible_at_root: Vec<&Declaration> = decls
        .iter()
        .filter(|d| opts.include_private || is_public(d, lang))
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
            .filter(|c| opts.include_private || is_public(c, lang))
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

    // The budget above only gates *descent*: `rows` is seeded with every
    // top-level declaration before the loop runs, so a file that is 300 flat
    // functions emitted 300 rows however small the budget was. That made
    // `budget` mean "how long to keep unfolding" rather than what its name
    // says, and let a read append an unbounded outline.
    //
    // Truncating after the sort below would keep an arbitrary slice, so cut
    // here and let the caller report the shortfall via `Coverage`.
    if rows.len() > opts.budget {
        rows.sort_by_key(|r| r.decl.start_line);
        rows.truncate(opts.budget);
    }

    // Restore source order; BFS produced level order, which reads oddly when
    // the rows are meant to mirror the file.
    rows.sort_by_key(|r| r.decl.start_line);
    rows
}

/// Is this declaration part of its module's outward surface?
///
/// `Declaration.visibility` is whatever the adapter found, and the languages do
/// not agree on how visibility is expressed: Rust uses a `pub` modifier, Go the
/// case of the identifier, Python a leading underscore, the C-family an explicit
/// keyword. The adapters normalise most of that — `_go_visibility` emits
/// `"public"`/`"private"`, `_visibility_for_name` emits `"private"` for
/// `_name` — so explicit markers can be trusted across the board.
///
/// The empty string is the case that actually differs, and it is why this
/// predicate needs the language. Rust has no `priv` keyword, so *absence* of
/// `pub` is precisely what private means. Everywhere else a blank field means
/// the adapter had nothing to report, which is not evidence of privacy.
/// Reading blank as public across the board — as this function first did —
/// silently turned `includePrivate: false` into a no-op on Rust.
fn is_public(decl: &Declaration, lang: &str) -> bool {
    let visibility = decl.visibility.as_str();
    if visibility.eq_ignore_ascii_case("public") {
        return true;
    }
    if matches!(
        visibility,
        "private" | "protected" | "internal" | "fileprivate"
    ) {
        return false;
    }
    // Rust: `pub`, `pub(crate)`, `pub(super)`. A restricted `pub` is still a
    // deliberate export, so it counts as surface.
    if visibility.starts_with("pub") {
        return true;
    }
    lang != "rust"
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
        let out = render(&decls, &anchors, &OutlineOptions::default(), "rust");
        assert_eq!(out, "time..beta: fn alpha()\n");
    }

    #[test]
    fn single_line_declaration_renders_one_anchor() {
        let decls = vec![decl("alpha", 3, 3, vec![])];
        let anchors = anchors_for(&[(3, "time")]);
        let out = render(&decls, &anchors, &OutlineOptions::default(), "rust");
        assert_eq!(out, "time: fn alpha()\n");
    }

    /// Two-word handles contain a space, so a space-separated span would be
    /// ambiguous with a single handle. `..` cannot occur inside a mnemonic.
    #[test]
    fn two_word_anchors_stay_unambiguous() {
        let decls = vec![decl("alpha", 1, 4, vec![])];
        let anchors = anchors_for(&[(1, "time beta"), (4, "nod deep")]);
        let out = render(&decls, &anchors, &OutlineOptions::default(), "rust");
        assert_eq!(out, "time beta..nod deep: fn alpha()\n");
    }

    #[test]
    fn unanchored_declaration_is_skipped_not_guessed() {
        let decls = vec![decl("alpha", 1, 2, vec![]), decl("beta", 9, 9, vec![])];
        let anchors = anchors_for(&[(9, "sam")]);
        let out = render(&decls, &anchors, &OutlineOptions::default(), "rust");
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
        let out = render(&decls, &anchors, &OutlineOptions::default(), "rust");
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
        let out = render(&decls, &anchors, &opts, "rust");
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
        let out = render(&decls, &anchors, &opts, "rust");
        assert_eq!(out, "aa..bb: fn outer()\n");
    }

    #[test]
    fn private_declarations_drop_when_asked() {
        let mut private = decl("hidden", 5, 5, vec![]);
        private.visibility = "private".into();
        let mut shown = decl("shown", 1, 1, vec![]);
        // Explicit, because under Rust an unmarked item is private — which is
        // exactly what this test used to get wrong.
        shown.visibility = "pub".into();
        let decls = vec![shown, private];
        let anchors = anchors_for(&[(1, "aa"), (5, "bb")]);
        let opts = OutlineOptions {
            include_private: false,
            ..Default::default()
        };
        let out = render(&decls, &anchors, &opts, "rust");
        assert_eq!(out, "aa: fn shown()\n");
    }

    // `blank_visibility_counts_as_public` used to live here, asserting that an
    // unmarked Rust declaration was public. That was the bug, not the contract:
    // see `rust_blank_visibility_is_private` and
    // `blank_visibility_is_public_outside_rust`, which split the case by
    // language instead of assuming one answer fits all of them.

    #[test]
    fn empty_input_says_so_rather_than_returning_nothing() {
        let out = render(&[], &HashMap::new(), &OutlineOptions::default(), "rust");
        assert_eq!(out, "(no declarations)\n");
    }

    // --- visibility, per language ----------------------------------------

    fn with_visibility(name: &str, line: usize, visibility: &str) -> Declaration {
        let mut d = decl(name, line, line, vec![]);
        d.visibility = visibility.to_string();
        d
    }

    fn public_only(
        decls: Vec<Declaration>,
        lang: &str,
        anchors: &[(usize, &'static str)],
    ) -> String {
        let opts = OutlineOptions {
            include_private: false,
            ..Default::default()
        };
        render(&decls, &anchors_for(anchors), &opts, lang)
    }

    /// The regression this predicate exists for. Rust has no `priv` keyword, so
    /// the adapter reports a private item as an empty string; reading blank as
    /// public made `includePrivate: false` a silent no-op.
    #[test]
    fn rust_blank_visibility_is_private() {
        let decls = vec![
            with_visibility("shown", 1, "pub"),
            with_visibility("hidden", 2, ""),
        ];
        let out = public_only(decls, "rust", &[(1, "aa"), (2, "bb")]);
        assert!(out.contains("fn shown"), "pub item was dropped:\n{out}");
        assert!(
            !out.contains("fn hidden"),
            "private rust item leaked:\n{out}"
        );
    }

    /// A restricted `pub` is still a deliberate export.
    #[test]
    fn rust_pub_crate_counts_as_surface() {
        let decls = vec![with_visibility("shown", 1, "pub(crate)")];
        let out = public_only(decls, "rust", &[(1, "aa")]);
        assert!(out.contains("fn shown"), "pub(crate) was dropped:\n{out}");
    }

    /// Everywhere but Rust, blank means the adapter had nothing to say — which
    /// is not evidence of privacy. Dropping those would hide most of the file.
    #[test]
    fn blank_visibility_is_public_outside_rust() {
        for lang in ["python", "go", "typescript", "java", "ruby"] {
            let decls = vec![with_visibility("shown", 1, "")];
            let out = public_only(decls, lang, &[(1, "aa")]);
            assert!(
                out.contains("fn shown"),
                "{lang}: blank visibility was treated as private:\n{out}"
            );
        }
    }

    /// `_go_visibility` and `_visibility_for_name` already normalise to these
    /// words, so they must be honoured regardless of language.
    #[test]
    fn explicit_markers_are_honoured_everywhere() {
        for lang in ["rust", "python", "go", "csharp"] {
            let decls = vec![
                with_visibility("shown", 1, "public"),
                with_visibility("hidden", 2, "private"),
                with_visibility("prot", 3, "protected"),
            ];
            let out = public_only(decls, lang, &[(1, "aa"), (2, "bb"), (3, "cc")]);
            assert!(out.contains("fn shown"), "{lang}: public dropped");
            assert!(!out.contains("fn hidden"), "{lang}: private leaked");
            assert!(!out.contains("fn prot"), "{lang}: protected leaked");
        }
    }

    // --- shared start lines ----------------------------------------------

    /// `struct T { pub a: u32 }` puts two declarations on one physical line.
    /// They share an anchor legitimately, but repeating it down the gutter
    /// reads as a rendering bug.
    #[test]
    fn declarations_sharing_a_line_print_the_anchor_once() {
        let decls = vec![decl("outer", 1, 1, vec![decl("field", 1, 1, vec![])])];
        let out = render(
            &decls,
            &anchors_for(&[(1, "like")]),
            &OutlineOptions::default(),
            "rust",
        );
        assert_eq!(out.matches("like:").count(), 1, "anchor repeated:\n{out}");
        assert!(out.contains("·"), "continuation marker missing:\n{out}");
        assert!(out.contains("fn field"), "second declaration lost:\n{out}");
    }

    /// The budget must bound the output, not merely stop descent. It seeded
    /// `rows` with every top-level declaration before the unfold loop, so a
    /// flat file emitted everything however small the budget was.
    #[test]
    fn the_budget_bounds_a_flat_file_too() {
        let decls: Vec<Declaration> = (0..50)
            .map(|i| decl(&format!("f{i}"), i + 1, i + 1, vec![]))
            .collect();
        let anchors: HashMap<usize, &'static str> = (0..50).map(|i| (i + 1, "aa")).collect();
        let opts = OutlineOptions {
            budget: 10,
            ..Default::default()
        };
        let (_, coverage) = render_with_coverage(&decls, &anchors, &opts, "rust");
        assert_eq!(coverage.shown, 10, "budget did not bound a flat file");
        assert_eq!(coverage.total, 50);
        assert!(!coverage.is_complete());
    }

    #[test]
    fn coverage_reports_complete_when_everything_fits() {
        let decls = vec![decl("only", 1, 1, vec![])];
        let (_, coverage) = render_with_coverage(
            &decls,
            &anchors_for(&[(1, "aa")]),
            &OutlineOptions::default(),
            "rust",
        );
        assert!(coverage.is_complete());
        assert_eq!((coverage.shown, coverage.total), (1, 1));
    }

    /// Nested declarations count toward the total, or a file of impl blocks
    /// would under-report how much shape is missing.
    #[test]
    fn nested_declarations_count_toward_the_total() {
        let decls = vec![decl("outer", 1, 9, vec![decl("inner", 2, 3, vec![])])];
        assert_eq!(count_declarations(&decls), 2);
    }
}
