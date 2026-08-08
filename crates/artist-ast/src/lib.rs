//! AST navigation engine: structural shape, public API surface, import and
//! call graphs, structural pattern search and rewrite.
//!
//! Forked from [ast-bro](https://github.com/aeroxy/ast-bro) (MIT) at upstream
//! revision `d9bccef`. See `LICENSE-ast-bro`.
//!
//! The fork keeps the analysis engine essentially whole and drops only the
//! agent plumbing artist already owns — the CLI, installers, the read-intercept
//! hook, and the MCP server. What remains is a library: callers get
//! [`ParseResult`] and [`core::Declaration`] and decide for themselves how to
//! render them.
//!
//! That last part matters here specifically. Upstream renders every result
//! against line numbers; artist addresses lines by semantic occurrence
//! anchor, so the renderers are deliberately not part of this crate's contract.
//! Analysis in, structured data out.

pub mod adapters;
pub mod anchors;
pub mod core;
pub mod file_filter;
pub mod main_helpers;
pub mod path_glob;
pub mod project_root;
pub mod search;

#[cfg(feature = "graphs")]
pub mod calls;
#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "context")]
pub mod context;
#[cfg(feature = "graphs")]
pub mod deps;
#[cfg(feature = "graphs")]
pub mod graph_cache;
#[cfg(feature = "impact")]
pub mod impact;
#[cfg(feature = "run")]
pub mod run;
#[cfg(feature = "squeeze")]
pub mod squeeze;
#[cfg(feature = "surface")]
pub mod surface;

use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

pub use crate::core::ParseResult;

/// Parse one file into its declarations, or `None` when the path has no
/// adapter or cannot be read.
pub fn parse_file(path: &Path) -> Option<ParseResult> {
    crate::main_helpers::parse_file_for_hook(path)
}

/// A 1-indexed, inclusive line range for `squeeze`. Either bound may be open:
/// `start` defaults to line 1, `end` defaults to EOF.
#[derive(Clone, Debug)]
pub struct LineRange {
    pub start: Option<usize>,
    pub end: Option<usize>,
}

impl LineRange {
    /// Collapse to the `(start, end)` pair the report uses, filling open bounds
    /// with line 1 / EOF and clamping `end` to the real line count so consumers
    /// never see an out-of-range sentinel like `usize::MAX`.
    pub fn resolve(&self, line_count: usize) -> (usize, usize) {
        clamp_line_range(self.start.unwrap_or(1), self.end, line_count)
    }
}

/// Clamp a 1-indexed inclusive line range to a file's real line count.
pub fn clamp_line_range(start: usize, end: Option<usize>, line_count: usize) -> (usize, usize) {
    let end = end.unwrap_or(line_count).min(line_count).max(start);
    (start, end)
}

/// Parse a line-range spec: `N` (single line), `A:B` (inclusive), `A:` (A to
/// EOF), `:B` (start to B). All bounds are 1-indexed; `0` and `A > B` are
/// rejected. Clamping to the file's real line count happens in [`LineRange::resolve`].
pub fn parse_line_range(s: &str) -> Result<LineRange, String> {
    let parse_bound = |part: &str| -> Result<Option<usize>, String> {
        if part.is_empty() {
            return Ok(None);
        }
        match part.parse::<usize>() {
            Ok(0) => Err("line numbers are 1-indexed (got 0)".to_string()),
            Ok(n) => Ok(Some(n)),
            Err(_) => Err(format!("invalid line number: {part:?}")),
        }
    };

    let range = match s.split_once(':') {
        // `A:B`, `A:`, `:B`, or `:`
        Some((a, b)) => LineRange {
            start: parse_bound(a)?,
            end: parse_bound(b)?,
        },
        // `N` — single line, both bounds equal
        None => {
            let n = parse_bound(s)?;
            LineRange { start: n, end: n }
        }
    };

    if let (Some(start), Some(end)) = (range.start, range.end)
        && start > end
    {
        return Err(format!("range start {start} is after end {end}"));
    }

    Ok(range)
}

/// Parse `<FILE>:<LINE>` into the two parts. Returns `None` if there's no
/// colon or the suffix doesn't parse as a `u32`. Used by `find-related`.
pub fn parse_file_line(s: &str) -> Option<(String, u32)> {
    let (file, line) = s.rsplit_once(':')?;
    if file.is_empty() {
        return None;
    }
    Some((file.to_string(), line.parse().ok()?))
}

/// Expand the requested paths, drop the ones that do not exist, and build a
/// gitignore-aware walker over what survives.
///
/// Returns `None` when nothing exists. Upstream printed a note to stdout for
/// each missing path; a library must not write to stdout, so missing paths are
/// simply absent from the returned set.
fn build_filtered_walker(
    paths: &[PathBuf],
    glob_str: Option<&str>,
) -> Option<(WalkBuilder, Vec<PathBuf>)> {
    if paths.is_empty() {
        return None;
    }

    let existing: Vec<PathBuf> = paths
        .iter()
        .flat_map(|p| path_glob::expand_existing(p))
        .collect();
    if existing.is_empty() {
        return None;
    }

    let mut builder = WalkBuilder::new(&existing[0]);
    for p in existing.iter().skip(1) {
        builder.add(p);
    }

    builder.hidden(false);
    file_filter::add_filters(&mut builder, &existing[0]);

    if let Some(g) = glob_str
        && let Ok(override_builder) = ignore::overrides::OverrideBuilder::new("").add(g)
        && let Ok(over) = override_builder.build()
    {
        builder.overrides(over);
    }

    Some((builder, existing))
}

/// Every file under `paths` that survives the ignore and skip filters.
pub fn walk_paths(paths: &[PathBuf], glob_str: Option<&str>) -> Vec<PathBuf> {
    let (tx, rx) = std::sync::mpsc::channel();
    let Some((builder, existing)) = build_filtered_walker(paths, glob_str) else {
        return Vec::new();
    };
    let walker = builder.build_parallel();
    let root = existing[0].clone();

    walker.run(|| {
        let tx = tx.clone();
        let root = root.clone();
        Box::new(move |result| {
            if let Ok(entry) = result
                && entry.file_type().is_some_and(|ft| ft.is_file())
                && !file_filter::should_skip_path(entry.path(), &root)
            {
                let _ = tx.send(entry.path().to_path_buf());
            }
            ignore::WalkState::Continue
        })
    });

    drop(tx);
    let mut results: Vec<_> = rx.into_iter().collect();
    results.sort();
    results
}

/// Walk and parse in one pass, concurrently. Files without an adapter are
/// skipped rather than reported.
pub fn walk_and_parse(paths: &[PathBuf], glob_str: Option<&str>) -> Vec<ParseResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    let Some((builder, existing)) = build_filtered_walker(paths, glob_str) else {
        return Vec::new();
    };
    let walker = builder.build_parallel();
    let root = existing[0].clone();

    walker.run(|| {
        let tx = tx.clone();
        let root = root.clone();
        Box::new(move |result| {
            if let Ok(entry) = result
                && entry.file_type().is_some_and(|ft| ft.is_file())
                && !file_filter::should_skip_path(entry.path(), &root)
                && let Some(parsed) = parse_file(entry.path())
            {
                let _ = tx.send(parsed);
            }
            ignore::WalkState::Continue
        })
    });

    drop(tx);
    let mut results: Vec<_> = rx.into_iter().collect();
    results.sort_by(|a, b| a.path.cmp(&b.path));
    results
}
