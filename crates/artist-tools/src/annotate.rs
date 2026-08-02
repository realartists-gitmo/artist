//! What the harness tells the model about a file, unasked.
//!
//! Some things are worth knowing about a file at the moment you first touch it,
//! and are things a model reliably will not think to ask: that thirty files
//! import it, that it sits in an import cycle, that its `pub use` lines re-export
//! from somewhere else entirely. Offered as tools these go uncalled, because
//! calling them requires already suspecting the answer.
//!
//! So they ride the first observation of a path instead. That trigger is chosen
//! deliberately: hooking `read` would be bypassable, because `code_map` also
//! issues anchors and the model can go straight from an outline to an edit
//! without ever reading — there is a test proving exactly that. Anchor issuance
//! is the one thing every path into a file shares.
//!
//! Fires once per path per session. A repeated note is noise, and the second
//! read of a file is not the moment anything is learned.

use crate::Workspace;
use std::path::Path;

/// Names that mean "this file is a module's front door", where a plain read
/// shows re-export statements rather than what is actually exported.
const MODULE_ROOTS: &[&str] = &["lib.rs", "mod.rs", "main.rs", "__init__.py", "index.ts"];

/// Build the note for a file being observed for the first time, or `None` when
/// there is nothing worth saying.
///
/// Every section is silent when empty. A file that nothing imports, sits in no
/// cycle and is not a module root produces no output at all — which is most
/// files, and is the point: the note means something precisely because it is
/// rare.
pub async fn first_touch(workspace: &Workspace, file: &Path) -> Option<String> {
    let mut sections: Vec<String> = Vec::new();
    // Crossing into a part of the project this session has not been in before.
    // The skeleton named it in one line; this is the line expanded, spent only
    // where the model actually went.
    if let Some(note) = workspace.region_note(file) {
        sections.push(note);
    }

    let root = artist_ast::project_root::find_root_for(file).ok()?;
    let graph = artist_ast::graph_cache::shared::get_or_init(&root).ok()?;

    // Who depends on this. Checked before what it depends on, because the
    // question that changes behaviour is "what breaks if I touch this", and a
    // file with many importers is one to be careful with.
    let importers =
        artist_ast::deps::traverse::reverse(&graph.deps, file, 1, IMPORTER_LIMIT, |_| true);
    if !importers.is_empty() {
        sections.push(format!(
            "imported by {} file(s): {}",
            importers.len(),
            summarise(
                importers
                    .iter()
                    .map(|h| workspace.display(&h.file))
                    .collect()
            )
        ));
    }

    let imports = artist_ast::deps::traverse::forward(&graph.deps, file, 1);
    if !imports.is_empty() {
        sections.push(format!(
            "imports {} file(s): {}",
            imports.len(),
            summarise(imports.iter().map(|h| workspace.display(&h.file)).collect())
        ));
    }

    // Cycle membership. Rare, and when it is true it changes what a refactor
    // can do: a file in a ring cannot be lifted out of it alone.
    let cycles = artist_ast::deps::scc::detect(&graph.deps, 2);
    if let Some(cycle) = cycles.iter().find(|c| c.members.iter().any(|f| f == file)) {
        let others: Vec<String> = cycle
            .members
            .iter()
            .filter(|f| *f != file)
            .map(|f| workspace.display(f))
            .collect();
        sections.push(format!("in an import cycle with {}", summarise(others)));
    }

    if is_module_root(file) {
        if let Some(surface) = module_surface(workspace, file) {
            sections.push(surface);
        }
    }

    if sections.is_empty() {
        return None;
    }
    Some(format!("\n[{}]\n", sections.join("\n ")))
}

/// What changed underneath an edit, reported after the fact.
///
/// The model just altered a declaration; who calls it is the thing most likely
/// to matter next and least likely to be asked about, because asking requires
/// already suspecting there are callers.
///
/// Called from every tool that writes — `edit`, `write`, and
/// `ast_rewrite --apply` — because a hook on one of them would miss the other
/// two. It was on `edit` alone for a while despite this comment claiming
/// otherwise, which meant a codemod could introduce a cross-crate dependency,
/// or close a cycle, in total silence.
///
/// Three call sites is a convention, not a guarantee. The structural version
/// routes every write through one wrapper so the coordinator cannot be reached
/// without it; until then a fourth writing tool can be added without this, and
/// nothing will complain.
///
/// Also reports implementors when the touched declaration is a trait. That is
/// the precise signal — not "this file contains a trait", which is true of half
/// of Rust, but "the line you just changed is inside one".
pub async fn after_commit(
    workspace: &Workspace,
    file: &Path,
    changed_lines: &[usize],
) -> Option<String> {
    let mut sections: Vec<String> = Vec::new();
    // Checked before the per-symbol work, and independently of it: a change
    // that adds an inter-unit dependency matters whether or not it landed
    // inside a declaration, and it is the one fact the opening project shape
    // can no longer be trusted on.
    sections.extend(workspace.architecture_delta(file));

    let Some(parsed) = artist_ast::parse_file(file) else {
        return finish(sections);
    };
    let touched = enclosing_declarations(&parsed.declarations, changed_lines);
    if touched.is_empty() {
        return finish(sections);
    }
    let Ok(root) = artist_ast::project_root::find_root_for(file) else {
        return finish(sections);
    };

    for decl in touched.iter().take(TOUCHED_LIMIT) {
        if is_trait_like(decl) {
            if let Some(note) = implementors(workspace, &decl.name, &root) {
                sections.push(note);
            }
            continue;
        }
        let opts = artist_ast::impact::ImpactOptions {
            depth: 1,
            limit: IMPORTER_LIMIT,
            mode: artist_ast::impact::ImpactMode::parse("dependents")?,
            include_ambiguous: false,
            tests: false,
            exclude_tests: false,
            json: false,
            pretty: false,
        };
        let Ok(reports) = artist_ast::impact::report(&decl.name, &root, &opts) else {
            continue;
        };
        let callers: Vec<String> = reports
            .iter()
            .filter_map(|r| r.sections.as_ref())
            .flatten()
            .filter(|s| s.title.contains("called by"))
            .flat_map(|s| s.entries.iter())
            .map(|e| e.qn.split("::").last().unwrap_or(&e.qn).to_string())
            .collect();
        if !callers.is_empty() {
            sections.push(format!(
                "{} has {} caller(s): {}",
                decl.name,
                callers.len(),
                summarise(callers)
            ));
        }
    }

    finish(sections)
}

/// One note, or nothing when there was nothing worth saying.
fn finish(sections: Vec<String>) -> Option<String> {
    if sections.is_empty() {
        return None;
    }
    Some(format!("\n[{}]\n", sections.join("\n ")))
}

fn implementors(workspace: &Workspace, name: &str, root: &Path) -> Option<String> {
    let parsed = artist_ast::walk_and_parse(&[root.to_path_buf()], None);
    let hits = artist_ast::core::find_implementations(&parsed, name, true);
    if hits.is_empty() {
        return None;
    }
    Some(format!(
        "trait {} has {} implementor(s): {}",
        name,
        hits.len(),
        summarise(
            hits.iter()
                .map(|h| format!("{} ({})", h.name, workspace.display(Path::new(&h.path))))
                .collect()
        )
    ))
}

/// Rust `trait`, and the equivalents other adapters report as `interface`.
fn is_trait_like(decl: &artist_ast::core::Declaration) -> bool {
    matches!(decl.kind, artist_ast::core::DeclarationKind::Interface)
        || decl
            .native_kind
            .as_deref()
            .is_some_and(|k| k == "trait" || k == "interface" || k == "protocol")
}

/// Declarations whose span covers any changed line, innermost first.
fn enclosing_declarations<'a>(
    decls: &'a [artist_ast::core::Declaration],
    lines: &[usize],
) -> Vec<&'a artist_ast::core::Declaration> {
    let mut out = Vec::new();
    for decl in decls {
        if lines
            .iter()
            .any(|l| *l >= decl.start_line && *l <= decl.end_line)
        {
            let inner = enclosing_declarations(&decl.children, lines);
            if inner.is_empty() {
                out.push(decl);
            } else {
                out.extend(inner);
            }
        }
    }
    out
}

const TOUCHED_LIMIT: usize = 3;

/// The resolved exports of a module root.
///
/// This is the file where a plain read is least informative: `pub use` tells you
/// a name is re-exported but not what it is or where it came from.
fn module_surface(workspace: &Workspace, file: &Path) -> Option<String> {
    let opts = artist_ast::surface::SurfaceOptions::default();
    let entries = artist_ast::surface::resolve_surface(file, &opts).ok()?;
    if entries.is_empty() {
        return None;
    }
    let shown: Vec<String> = entries
        .iter()
        .take(SURFACE_LIMIT)
        .map(|e| {
            format!(
                "{} ({})",
                e.qualified_path,
                workspace.display(&e.source_path)
            )
        })
        .collect();
    let more = entries.len().saturating_sub(shown.len());
    Some(format!(
        "exports {} item(s): {}{}",
        entries.len(),
        shown.join(", "),
        if more > 0 {
            format!(", +{more} more — use code_surface for all")
        } else {
            String::new()
        }
    ))
}

fn is_module_root(file: &Path) -> bool {
    file.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| MODULE_ROOTS.contains(&n))
}

/// Cap on importers walked. A file imported by hundreds is interesting for the
/// *count*, not the list.
const IMPORTER_LIMIT: usize = 50;
const SURFACE_LIMIT: usize = 12;
const NAME_LIMIT: usize = 6;

/// List a few names and count the rest. The note has to stay one glance long or
/// it stops being something read in passing.
fn summarise(mut names: Vec<String>) -> String {
    names.sort();
    names.dedup();
    if names.len() <= NAME_LIMIT {
        return names.join(", ");
    }
    let shown = names[..NAME_LIMIT].join(", ");
    format!("{shown}, +{} more", names.len() - NAME_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_roots_are_recognised_across_languages() {
        for name in ["lib.rs", "mod.rs", "__init__.py", "index.ts"] {
            assert!(is_module_root(Path::new(name)), "{name} not recognised");
        }
        assert!(!is_module_root(Path::new("helper.rs")));
    }

    #[test]
    fn a_short_list_is_shown_in_full() {
        assert_eq!(summarise(vec!["b.rs".into(), "a.rs".into()]), "a.rs, b.rs");
    }

    /// The note is meant to be glanced at, so a long list becomes a count.
    #[test]
    fn a_long_list_is_capped_with_a_count() {
        let names: Vec<String> = (0..20).map(|i| format!("f{i:02}.rs")).collect();
        let out = summarise(names);
        assert!(out.contains("+14 more"), "{out}");
        assert_eq!(out.matches(", ").count(), NAME_LIMIT);
    }

    #[test]
    fn duplicates_collapse() {
        assert_eq!(summarise(vec!["a.rs".into(), "a.rs".into()]), "a.rs");
    }
}
