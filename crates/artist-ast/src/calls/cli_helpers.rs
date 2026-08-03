//! Shared helpers used by both `cli.rs` (one-shot CLI) and `mcp.rs`
//! (long-lived MCP server). Pulled out so the two callers can stay thin
//! while the symbol-resolution logic has one home.

use crate::calls::graph::{CallGraph, Qn};
use std::collections::BTreeSet;

/// What a resolved target qn refers to. Lets the caller dispatch to either
/// the call-edge path (`Callable`) or the type-aware path (`Type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// qn is a function/method/constructor — present in `forward`/`reverse`.
    Callable,
    /// qn is a class/struct/trait/interface/enum/record — present in
    /// `types` / `type_by_name` but not in the call edges.
    Type,
}

#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub qn: Qn,
    pub kind: SymbolKind,
}

/// Suffix-match `target` against every qn in the graph, mirroring the
/// `show`/`implements` mental model. Accepts:
/// - bare name: `TakeDamage` → matches `*::TakeDamage`.
/// - dotted path: `Player.TakeDamage` → matches `*::Player::TakeDamage`.
/// - explicit qn: `src/Player.cs::Player::TakeDamage` → exact.
/// - file-scoped: `src/Player.cs:TakeDamage` (single `:`) → restricts the
///   match to qns whose file equals or ends with that path. Useful when the
///   same name lives in multiple files. The symbol part still supports the
///   dotted form (`src/Player.cs:Player.TakeDamage`).
///
/// Returns *only* callable qns. For the kind-aware variant that also
/// surfaces types, use [`resolve_target_full`].
pub fn resolve_target_qns(calls: &CallGraph, target: &str) -> Vec<Qn> {
    resolve_target_full(calls, target)
        .into_iter()
        .filter(|r| r.kind == SymbolKind::Callable)
        .map(|r| r.qn)
        .collect()
}

/// Kind-aware resolution: returns every matching qn paired with whether
/// it's a callable or a type. Both halves are searched and deduped together.
pub fn resolve_target_full(calls: &CallGraph, target: &str) -> Vec<ResolvedTarget> {
    let (file_filter, symbol) = split_file_filter(target);
    // Accept `Type::method` as well as `Type.method`.
    //
    // Every renderer prints qns with `::` — it is what `Qn::to_string` emits
    // and what `callees`, `impact` and `map` put on screen — but this only
    // ever split on `.`, so the exact string the tools *print* was rejected
    // when fed back in. For an agent that copies a symbol out of one command's
    // output and into another's argument, that is the difference between the
    // surface composing and not.
    //
    // `symbol` itself is still passed to `qn_matches` untouched, so the
    // explicit whole-qn form keeps matching on the fast path above the
    // segment comparison.
    let normalised = symbol.replace("::", ".");
    let parts: Vec<&str> = normalised.split('.').collect();

    let mut out: Vec<ResolvedTarget> = Vec::new();
    for qn in collect_callable_qns(calls) {
        if let Some(filter) = file_filter {
            if !file_matches(&qn, filter) {
                continue;
            }
        }
        if qn_matches(&qn, symbol, &parts) {
            out.push(ResolvedTarget {
                qn,
                kind: SymbolKind::Callable,
            });
        }
    }
    for qn in calls.types.keys() {
        if let Some(filter) = file_filter {
            if !file_matches(qn, filter) {
                continue;
            }
        }
        if qn_matches(qn, symbol, &parts) {
            out.push(ResolvedTarget {
                qn: qn.clone(),
                kind: SymbolKind::Type,
            });
        }
    }
    out.sort_by(|a, b| a.qn.0.cmp(&b.qn.0));
    out.dedup_by(|a, b| a.qn == b.qn && a.kind == b.kind);
    out
}

/// `src/foo.rs:bar` → `(Some("src/foo.rs"), "bar")`. Plain symbol → `(None, target)`.
/// `::`-only inputs (already-qualified qns) stay intact.
fn split_file_filter(target: &str) -> (Option<&str>, &str) {
    let mut i = 0;
    let bytes = target.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b':' {
            // Skip over `::` (already-qualified-name segment separator).
            if bytes.get(i + 1) == Some(&b':') {
                i += 2;
                continue;
            }
            let (file, rest) = (&target[..i], &target[i + 1..]);
            if file.is_empty() || rest.is_empty() {
                return (None, target);
            }
            return (Some(file), rest);
        }
        i += 1;
    }
    (None, target)
}

fn file_matches(qn: &Qn, filter: &str) -> bool {
    let f = qn.file();
    if f == filter {
        return true;
    }
    // Trailing-path match — `Player.cs` matches `src/game/Player.cs`.
    f.ends_with(filter)
        && f.as_bytes()
            .get(f.len().saturating_sub(filter.len()).saturating_sub(1))
            == Some(&b'/')
}

fn collect_callable_qns(calls: &CallGraph) -> Vec<Qn> {
    let mut s: BTreeSet<Qn> = BTreeSet::new();
    for k in calls.forward.keys() {
        s.insert(k.clone());
    }
    for k in calls.reverse.keys() {
        s.insert(k.clone());
    }
    s.into_iter().collect()
}

fn qn_matches(qn: &Qn, raw: &str, parts: &[&str]) -> bool {
    if qn.0 == raw {
        return true;
    }
    let segments: Vec<&str> = qn.0.split("::").collect();
    if parts.len() > segments.len() {
        return false;
    }
    let start = segments.len() - parts.len();
    parts
        .iter()
        .enumerate()
        .all(|(i, p)| segments[start + i] == *p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_file_filter_plain_symbol() {
        assert_eq!(split_file_filter("greet"), (None, "greet"));
    }

    #[test]
    fn split_file_filter_with_file() {
        assert_eq!(
            split_file_filter("src/main.rs:parse_file"),
            (Some("src/main.rs"), "parse_file")
        );
    }

    #[test]
    fn split_file_filter_double_colon_stays_intact() {
        assert_eq!(
            split_file_filter("src/main.rs::parse_file"),
            (None, "src/main.rs::parse_file")
        );
    }

    #[test]
    fn split_file_filter_with_dotted_symbol() {
        assert_eq!(
            split_file_filter("src/foo.rs:Foo.bar"),
            (Some("src/foo.rs"), "Foo.bar")
        );
    }

    /// Every renderer prints qns with `::`, so the printed form must be a valid
    /// input form. It was not: `callees` would emit `PathAnchors::cursor` and
    /// `callers`/`trace`/`impact` would then reject that exact string, which
    /// breaks copying a symbol from one command into another.
    #[test]
    fn the_printed_separator_is_an_accepted_input_separator() {
        let dotted = split_file_filter("Type.method").1.replace("::", ".");
        let colons = split_file_filter("Type::method").1.replace("::", ".");
        assert_eq!(dotted, colons);
        assert_eq!(
            colons.split('.').collect::<Vec<_>>(),
            vec!["Type", "method"]
        );
    }

    #[test]
    fn a_bare_name_is_unaffected_by_separator_normalisation() {
        assert_eq!(
            split_file_filter("method")
                .1
                .replace("::", ".")
                .split('.')
                .collect::<Vec<_>>(),
            vec!["method"]
        );
    }
}
