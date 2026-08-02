//! AST-aware search and rewrite via ast-grep-core pattern matching.
//!
//! `ast-bro run -p 'pattern'` finds AST nodes matching the pattern.
//! `ast-bro run -p 'pattern' -r 'replacement'` rewrites matched nodes.
//!
//! Uses ast-grep-core's `Root::find_all()` for search and `Root::replace()` +
//! `Root::generate()` for rewrite. Meta-variables ($A, $$$ARGS, $_) work
//! exactly like ast-grep.

pub mod cli;

use ast_grep_core::Language;
use ast_grep_language::LanguageExt;
// Re-exported: `detect_lang` and `parse_lang` hand these back, so a caller
// outside the crate needs to be able to name the type.
pub use ast_grep_language::SupportLang;
use std::path::Path;

/// A single match result.
#[derive(serde::Serialize)]
pub struct RunMatch {
    pub file: String,
    pub start_line: usize,
    pub end_line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub matched_text: String,
}

/// Detect language from file extension.
pub fn detect_lang(path: &Path) -> Option<SupportLang> {
    SupportLang::from_path(path)
}

/// Search for pattern matches in source.
#[allow(dead_code)] // public API; prefer search_with_pattern in loops
/// The compiled form of a pattern, re-exported so callers need not depend on
/// `ast-grep-core` themselves.
pub use ast_grep_core::Pattern;

/// Compile a pattern once, for callers that will apply it to many files.
///
/// The gap this closes: `search` takes a `&str` and compiles per call, while
/// `search_with_pattern` takes an already-compiled `&Pattern` and exists
/// precisely to be used in a loop — but nothing here produced one. Every
/// caller wanting the loop had to reach past this crate to `ast-grep-core` and
/// take on the dependency, which puts the pattern-engine version in two places
/// that must agree.
pub fn compile(pattern: &str, lang: SupportLang) -> Result<Pattern, String> {
    Pattern::try_new(pattern, lang).map_err(|e| format!("invalid pattern: {e}"))
}

/// Whether a compiled pattern is built from a syntax error.
///
/// `Pattern::try_new` is lenient: `fn (` compiles without complaint into a
/// pattern rooted at an ERROR node, which then matches nothing. A caller that
/// treats compilation as validation therefore cannot tell a malformed pattern
/// from a well-formed one with no hits — the two produce identical silence.
pub fn pattern_is_malformed(pattern: &Pattern) -> bool {
    pattern.has_error()
}

pub fn search(
    source: &str,
    lang: SupportLang,
    pattern: &str,
) -> Result<Vec<RunMatch>, String> {
    search_with_pattern(source, lang, &compile(pattern, lang)?)
}

/// Search for pattern matches using a pre-compiled pattern.
///
/// Use this variant in loops where the same pattern is applied to many files
/// with the same language — compile once, clone per file.
pub fn search_with_pattern(
    source: &str,
    lang: SupportLang,
    pattern: &Pattern,
) -> Result<Vec<RunMatch>, String> {
    let ast = lang.ast_grep(source);
    let matches: Vec<RunMatch> = ast
        .root()
        .find_all(pattern.clone())
        .map(|m| {
            let start = m.start_pos();
            let end = m.end_pos();
            RunMatch {
                file: String::new(),
                start_line: start.line() + 1,
                end_line: end.line() + 1,
                start_col: start.column(&m) + 1,
                end_col: end.column(&m) + 1,
                matched_text: m.text().to_string(),
            }
        })
        .collect();
    Ok(matches)
}

/// Rewrite matches in source using a pre-compiled pattern.
///
/// Use this variant in loops where the same pattern is applied to many files
/// with the same language — compile once, clone per file.
pub fn rewrite_with_pattern(
    source: &str,
    lang: SupportLang,
    pattern: &Pattern,
    replacement: &str,
) -> Result<Option<String>, String> {
    // `Root::replace` rewrites the *first* match and returns. Used alone it
    // makes a codemod tool that changes one call site per file and reports
    // success — a file with two hundred matches came back with one line
    // altered, and nothing in the output said so.
    //
    // `replace_all` collects an edit per match, already sorted by position.
    // Applying them back to front keeps every earlier offset valid, and
    // matching on the original tree means a replacement that itself matches
    // the pattern cannot be rewritten again.
    let ast = lang.ast_grep(source);
    let edits = ast.root().replace_all(pattern.clone(), replacement);
    if edits.is_empty() {
        return Ok(None);
    }
    let mut out = source.as_bytes().to_vec();
    for edit in edits.iter().rev() {
        let end = edit.position + edit.deleted_length;
        if end > out.len() {
            return Err("rewrite produced an edit outside the source".to_owned());
        }
        out.splice(edit.position..end, edit.inserted_text.iter().copied());
    }
    String::from_utf8(out)
        .map(Some)
        .map_err(|_| "rewrite produced invalid UTF-8".to_owned())
}

/// How many syntax errors `source` contains under `lang`.
///
/// A replacement template is arbitrary text — nothing in the pattern language
/// requires it to parse — so a structural rewrite will happily splice `fn (`
/// into every match and produce a file no compiler will read. Counting before
/// and after is the check: a rewrite may leave errors it found, but it may not
/// add any.
///
/// Counting rather than testing for *any* error, because files that already
/// fail to parse are ordinary — a half-finished edit, a templating syntax the
/// grammar does not know — and refusing to rewrite them would be worse than
/// the problem.
pub fn syntax_errors(source: &str, lang: SupportLang) -> usize {
    let ast = lang.ast_grep(source);
    let mut count = 0;
    let mut stack = vec![ast.root()];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            count += 1;
        }
        stack.extend(node.children());
    }
    count
}

/// The metavariables a pattern or replacement names, e.g. `$N`, `$$$ARGS`.
///
/// A replacement naming a capture the pattern never binds expands to nothing —
/// `target($N)` -> `renamed($Z)` silently produces `renamed()` and drops the
/// argument. That is a typo with no symptom, so it has to be caught by name.
pub fn metavariables(text: &str) -> std::collections::BTreeSet<String> {
    let mut found = std::collections::BTreeSet::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '$' {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && chars[j] == '$' {
            j += 1;
        }
        let start = j;
        // The *first* character decides: the pattern language requires an
        // uppercase name, so `$foo` is a literal, not a capture. Digits and
        // lowercase are allowed only after that first character, which is why
        // this cannot be one loop.
        if j < chars.len() && (chars[j].is_ascii_uppercase() || chars[j] == '_') {
            j += 1;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
        }
        if j > start {
            found.insert(chars[start..j].iter().collect());
        }
        i = j.max(i + 1);
    }
    found
}

/// Per-file byte cap for `ast-bro run` (CLI and MCP). The walker filters
/// by extension only, so a minified bundle or generated data file under a
/// source extension would otherwise be read whole into memory. 5 MiB is
/// generous for real source files and defensive against pathological ones.
pub const RUN_MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// Crash-safe in-place file replacement: writes to a sibling temp file,
/// fsyncs it, renames over the target, then fsyncs the parent directory.
/// On POSIX the rename is atomic; on Windows std::fs::rename uses
/// `MOVEFILE_REPLACE_EXISTING`. Either way, an interrupted write can no
/// longer truncate or corrupt the original. The parent-dir fsync (Unix
/// only) ensures the rename's directory entry survives a crash.
///
/// If `path` is a symlink, the symlink's target is rewritten rather than
/// the link being replaced with a regular file.
///
/// Permissions are best-effort copied from the original before the rename,
/// since the rename swaps the inode.
pub fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    // Resolve symlinks so we rewrite the real file rather than destroying
    // the link with a regular-file rename. If the target doesn't exist yet
    // (new-file case), canonicalize fails — fall back to the original path.
    let canonical = std::fs::canonicalize(path).ok();
    let path: &Path = canonical.as_deref().unwrap_or(path);

    let dir = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "atomic_write: path has no parent directory",
        )
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "atomic_write: path has no file name",
        )
    })?;

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!(
        ".{}.ast-bro-tmp-{}-{}",
        file_name.to_string_lossy(),
        std::process::id(),
        n
    );
    let tmp_path = dir.join(tmp_name);

    let orig_perms = std::fs::metadata(path).map(|m| m.permissions()).ok();

    // Open the temp file restrictively on Unix so a permissive umask can't
    // briefly expose contents that the original kept private (e.g.,
    // rewriting a 0o600 file under umask 0o022). The final mode is set
    // below — before the rename — to match the original.
    // `create_new` also guards against clobbering an unrelated file with
    // our temp name and against simple symlink-target races.
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let write_result = (|| -> std::io::Result<()> {
        let mut f = opts.open(&tmp_path)?;
        f.write_all(contents)?;
        f.sync_all()
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    if let Some(perms) = orig_perms {
        let _ = std::fs::set_permissions(&tmp_path, perms);
    }

    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    // Persist the directory entry so the rename survives a crash. fsync on
    // a directory FD is the POSIX recipe; on Windows the std API doesn't
    // expose a portable equivalent and NTFS journals provide implicit
    // durability for atomic renames. Errors here are non-fatal — the
    // rename already succeeded.
    #[cfg(unix)]
    {
        if let Ok(dir_file) = std::fs::OpenOptions::new().read(true).open(dir) {
            let _ = dir_file.sync_all();
        }
    }

    Ok(())
}

/// Rewrite matches in source. Returns the new source if any replacements were made.
#[allow(dead_code)] // public API; prefer rewrite_with_pattern in loops
pub fn rewrite(
    source: &str,
    lang: SupportLang,
    pattern: &str,
    replacement: &str,
) -> Result<Option<String>, String> {
    rewrite_with_pattern(source, lang, &compile(pattern, lang)?, replacement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn atomic_write_replaces_contents_and_leaves_no_tempfiles() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("src.rs");
        std::fs::write(&p, "old\n").unwrap();
        atomic_write(&p, b"new\n").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new\n");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("ast-bro-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "stray temp files: {:?}", leftovers);
    }

    #[test]
    fn atomic_write_creates_new_file_when_target_missing() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("fresh.rs");
        atomic_write(&p, b"hello\n").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello\n");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_updates_symlink_target_not_the_link() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("target.rs");
        let link = dir.path().join("link.rs");
        std::fs::write(&target, "old\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        atomic_write(&link, b"new\n").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new\n");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "symlink should be preserved, not replaced with a regular file",
        );
    }
}

#[cfg(test)]
mod rewrite_all_tests {
    use super::*;
    use ast_grep_language::SupportLang;

    /// Every match, not the first.
    ///
    /// `Root::replace` stops after one edit. Built on that alone, a codemod
    /// changed one call site per file and reported success — which is worse
    /// than failing, because the run looks complete.
    #[test]
    fn every_occurrence_is_rewritten() {
        let source = "fn a() { b(1) }\nfn c() { b(2) }\nfn d() { b(3) }\n";
        let out = rewrite(source, SupportLang::Rust, "b($N)", "e($N)")
            .unwrap()
            .expect("a rewrite");
        assert_eq!(out.matches("e(").count(), 3, "{out}");
        assert!(!out.contains("b("), "{out}");
    }

    /// Captures still bind per match rather than leaking across them.
    #[test]
    fn each_match_keeps_its_own_capture() {
        let source = "fn a() { b(1) }\nfn c() { b(2) }\n";
        let out = rewrite(source, SupportLang::Rust, "b($N)", "e($N)")
            .unwrap()
            .expect("a rewrite");
        assert!(out.contains("e(1)") && out.contains("e(2)"), "{out}");
    }

    /// A replacement that itself matches the pattern must not be rewritten
    /// again — the edits come from one pass over the original tree.
    #[test]
    fn a_self_matching_replacement_terminates() {
        let source = "fn a() { b(1) }\n";
        let out = rewrite(source, SupportLang::Rust, "b($N)", "b(b($N))")
            .unwrap()
            .expect("a rewrite");
        assert_eq!(out.matches("b(").count(), 2, "{out}");
    }

    #[test]
    fn no_match_still_reports_nothing() {
        let source = "fn a() { z(1) }\n";
        assert!(
            rewrite(source, SupportLang::Rust, "b($N)", "e($N)")
                .unwrap()
                .is_none()
        );
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    #[test]
    fn a_malformed_pattern_compiles_but_is_reported_as_malformed() {
        // The whole reason `pattern_is_malformed` exists: this does not fail
        // to compile, it compiles into something that can never match.
        let compiled = compile("fn (", SupportLang::Rust).expect("try_new is lenient");
        assert!(pattern_is_malformed(&compiled));

        let good = compile("target($N)", SupportLang::Rust).unwrap();
        assert!(!pattern_is_malformed(&good));
    }

    #[test]
    fn syntax_errors_counts_rather_than_flags() {
        assert_eq!(syntax_errors("fn a() {}\n", SupportLang::Rust), 0);
        assert!(syntax_errors("fn a( {\n", SupportLang::Rust) > 0);
        // The property the rewrite guard relies on: adding breakage to an
        // already-broken file is still detectable as an increase.
        let broken = syntax_errors("fn a( {\n", SupportLang::Rust);
        let worse = syntax_errors("fn a( {\nfn b( {\n", SupportLang::Rust);
        assert!(worse > broken, "{worse} !> {broken}");
    }

    #[test]
    fn metavariables_are_found_in_both_forms() {
        let found = metavariables("target($N, $$$REST, $_)");
        assert!(found.contains("N"), "{found:?}");
        assert!(found.contains("REST"), "{found:?}");
    }

    #[test]
    fn a_lowercase_name_is_not_a_metavariable() {
        // `$foo` is a literal in the pattern language, so treating it as a
        // capture would make the unbound-capture check reject valid rewrites.
        let found = metavariables("target($foo)");
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn an_unbound_capture_is_visible_as_a_set_difference() {
        let bound = metavariables("target($N)");
        let used = metavariables("renamed($Z)");
        let unbound: Vec<_> = used.difference(&bound).collect();
        assert_eq!(unbound, vec!["Z"]);
    }
}
