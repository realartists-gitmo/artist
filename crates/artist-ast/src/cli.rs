//! Command-line shim retained as a test fixture.
//!
//! Artist drives this crate as a library and never builds this module.
//! It exists because the 155 vendored integration tests in `tests/` are
//! end-to-end CLI tests that exec `CARGO_BIN_EXE_ast-bro` — 3.7k lines of
//! adapter and analysis coverage over the riskiest code in the fork. Kept
//! behind the default-on `cli` feature so consumers can opt out of clap.
#![allow(clippy::all)]

use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::core::{DigestOptions, MapOptions};
use crate::{LineRange, parse_file, parse_file_line, parse_line_range, walk_and_parse};

#[derive(Parser)]
#[command(name = "ast-bro")]
#[command(version)]
#[command(about = "Fast, AST-based structural outline for source files", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Map files or directories — signatures with line ranges, no method bodies.
    Map {
        /// Files or directories to map.
        #[arg(num_args = 1..)]
        paths: Vec<PathBuf>,

        #[arg(long)]
        no_private: bool,
        #[arg(long)]
        no_fields: bool,
        #[arg(long)]
        no_docs: bool,
        #[arg(long)]
        no_attrs: bool,
        #[arg(long)]
        no_lines: bool,
        #[arg(long)]
        glob: Option<String>,
        /// Emit output as JSON instead of text
        #[arg(long)]
        json: bool,
        /// With --json: emit compact (single-line) JSON instead of pretty-printed
        #[arg(long)]
        compact: bool,
    },
    /// Extract source of a symbol
    Show {
        path: PathBuf,
        symbol: String,
        #[arg(num_args = 0..)]
        others: Vec<String>,
        /// Emit output as JSON instead of text
        #[arg(long)]
        json: bool,
        /// With --json: emit compact (single-line) JSON
        #[arg(long)]
        compact: bool,
    },
    /// Compress repetitive log/text into a smaller, reversible form with a legend.
    ///
    /// For **logs and text**, not code — for code use `map` / `digest` / `show`.
    /// Shrinks repeated lines, tags, timestamps and token sequences; prints a legend
    /// so the original is recoverable. Falls back to the raw input when squeezing
    /// would make it larger.
    Squeeze {
        /// Path to the log/text file to read.
        path: PathBuf,
        /// Optional 1-indexed inclusive line range: `N`, `A:B`, `A:`, or `:B`.
        #[arg(value_parser = parse_line_range)]
        range: Option<LineRange>,
        /// Skip compression — emit the raw text with a header (for diffing/inspecting).
        #[arg(long)]
        raw: bool,
        /// Emit output as JSON instead of text
        #[arg(long)]
        json: bool,
        /// With --json: emit compact (single-line) JSON
        #[arg(long)]
        compact: bool,
    },
    /// One-page module map
    Digest {
        #[arg(num_args = 1..)]
        paths: Vec<PathBuf>,

        #[arg(long)]
        include_private: bool,
        #[arg(long)]
        include_fields: bool,
        #[arg(long, default_value_t = 50)]
        max_members: usize,
        /// Emit output as JSON instead of text
        #[arg(long)]
        json: bool,
        /// With --json: emit compact (single-line) JSON
        #[arg(long)]
        compact: bool,
    },
    /// Find subclasses / implementations
    Implements {
        target: String,
        #[arg(num_args = 1..)]
        paths: Vec<PathBuf>,

        #[arg(short, long)]
        direct: bool,
        /// Emit output as JSON instead of text
        #[arg(long)]
        json: bool,
        /// With --json: emit compact (single-line) JSON
        #[arg(long)]
        compact: bool,
    },
    // `prompt`, `install`, `uninstall`, `status`, `hook` and `mcp` are dropped:
    // artist owns installation, read-interception and prompting.
    /// Hybrid BM25 + dense semantic search over the repo
    Search {
        /// Search query (free-form text or symbol name)
        query: String,
        /// Repository root to search in (default: ".")
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Number of results to return
        #[arg(short = 'k', long = "top-k", default_value_t = 10)]
        top_k: usize,
        /// Override auto alpha (semantic vs. BM25 weight, 0.0–1.0)
        #[arg(long)]
        alpha: Option<f32>,
        /// Filter by language (repeatable, e.g. `--lang rust --lang python`)
        #[arg(long = "lang")]
        languages: Vec<String>,
        /// Force a full rebuild of the index before searching
        #[arg(long)]
        rebuild: bool,
        /// Emit output as JSON instead of text
        #[arg(long)]
        json: bool,
        /// With --json: emit compact (single-line) JSON
        #[arg(long)]
        compact: bool,
    },
    /// Find chunks semantically similar to a given file:line
    ///
    /// Pass the source location either as a positional `<FILE>:<LINE>`
    /// (matches grep / search-result output you can paste back) or via
    /// `--file <FILE> --line <LINE>` for scripting use.
    FindRelated {
        /// Source location as `<FILE>:<LINE>`. Optional when `--file` and
        /// `--line` are passed together.
        #[arg(required_unless_present_all = ["file", "line"], conflicts_with_all = ["file", "line"])]
        target: Option<String>,
        /// Repository root containing the index (default: ".")
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Alternative to the positional `<FILE>:<LINE>` form
        #[arg(long, requires = "line")]
        file: Option<String>,
        /// 1-indexed line number when using `--file`
        #[arg(long, requires = "file")]
        line: Option<u32>,
        #[arg(short = 'k', long = "top-k", default_value_t = 10)]
        top_k: usize,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        compact: bool,
    },
    /// True public API surface — resolves `pub use` / `__all__` re-exports.
    Surface {
        /// Crate root file, package init, or directory to auto-detect.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Render as a hierarchical tree grouped by module.
        #[arg(long)]
        tree: bool,
        /// Append the via-chain on each entry (text mode only).
        #[arg(long)]
        include_chain: bool,
        /// Recursion guard for re-export chains.
        #[arg(long, default_value_t = 16)]
        max_depth: usize,
        /// Include private items (only meaningful for fallback languages).
        #[arg(long)]
        include_private: bool,
        /// Force a specific resolver: `rust`, `python`, or `fallback`.
        #[arg(long)]
        lang: Option<String>,
        /// Emit output as JSON instead of text.
        #[arg(long)]
        json: bool,
        /// With --json: emit compact (single-line) JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Forward import-graph traversal: what does this file import (transitively)?
    Deps {
        file: PathBuf,
        #[arg(long, default_value_t = 3)]
        depth: usize,
        /// Hide unresolved external imports from the footer.
        /// Shown by default (tagged `[external]`); set this flag to drop them.
        #[arg(long)]
        hide_external: bool,
        /// Force a fresh dep-graph build.
        #[arg(long)]
        rebuild: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        compact: bool,
    },
    /// Reverse import-graph: who imports this file (transitively)?
    ReverseDeps {
        file: PathBuf,
        #[arg(long, default_value_t = 3)]
        depth: usize,
        #[arg(long, default_value_t = 200)]
        limit: usize,
        /// Show only importers from test files (path heuristic: tests/, __tests__/, *_test.*, *.spec.*, …).
        #[arg(long)]
        tests: bool,
        /// Exclude importers from test files (same path heuristic as --tests).
        #[arg(long, conflicts_with = "tests")]
        exclude_tests: bool,
        #[arg(long)]
        rebuild: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        compact: bool,
    },
    /// Find import cycles via Tarjan SCC.
    Cycles {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value_t = 2)]
        min_size: usize,
        #[arg(long)]
        rebuild: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        compact: bool,
    },
    /// Emit the dep graph (text or JSON).
    Graph {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        json: bool,
        /// Hide unresolved external imports from the graph.
        /// Shown by default (tagged `[external]`); set this flag to drop them.
        #[arg(long)]
        hide_external: bool,
        /// (Deprecated — unresolved externals are shown by default now.)
        #[arg(long, hide = true)]
        include_external: bool,
        #[arg(long)]
        rebuild: bool,
        #[arg(long)]
        compact: bool,
    },
    /// Token-budgeted context for a symbol — target body + relevant deps/callers in one call.
    Context {
        /// Symbol to build context for (same form as callers/callees).
        #[arg(required_unless_present_all = ["file", "symbol"], conflicts_with_all = ["file", "symbol"])]
        target: Option<String>,
        /// Repository root (default: ".").
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Alternative to the positional target.
        #[arg(long, requires = "symbol")]
        file: Option<String>,
        /// Symbol name when using `--file`.
        #[arg(long, requires = "file")]
        symbol: Option<String>,
        /// Token budget (default 8000). Rough estimate: 1 token ≈ 4 bytes.
        #[arg(long, default_value_t = 8000)]
        budget: usize,
        #[arg(long)]
        rebuild: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        compact: bool,
    },
    /// Find callers of a symbol — AST-accurate, no grep noise.
    ///
    /// Pass the symbol either as a positional `<TARGET>` (suffix-matched
    /// like `show`/`implements`: `TakeDamage`, `Player.TakeDamage`, or
    /// `src/Player.cs:TakeDamage` to scope to one file), or via
    /// `--file <FILE> --symbol <NAME>` for scripting use.
    Callers {
        /// Symbol to look up. Optional when `--file` and `--symbol` are passed.
        #[arg(required_unless_present_all = ["file", "symbol"], conflicts_with_all = ["file", "symbol"])]
        target: Option<String>,
        /// Repository root (default: ".").
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Alternative to the `<FILE>:<NAME>` positional form.
        #[arg(long, requires = "symbol")]
        file: Option<String>,
        /// Symbol name when using `--file`.
        #[arg(long, requires = "file")]
        symbol: Option<String>,
        /// Max BFS depth (1 = direct callers only).
        #[arg(long, default_value_t = 1)]
        depth: usize,
        /// Cap result count (mirrors reverse-deps).
        #[arg(long, default_value_t = 200)]
        limit: usize,
        /// Hide callers whose target is `Ambiguous` (multiple candidates).
        /// Shown by default (tagged red); set this flag to drop them.
        #[arg(long)]
        hide_ambiguous: bool,
        /// (Deprecated — ambiguous matches are shown by default now.)
        #[arg(long, hide = true)]
        include_ambiguous: bool,
        /// Show only callers in test files (path heuristic: tests/, __tests__/, *_test.*, *.spec.*, …).
        #[arg(long)]
        tests: bool,
        /// Exclude callers from test files (same path heuristic as --tests).
        #[arg(long, conflicts_with = "tests")]
        exclude_tests: bool,
        /// Force a fresh call-graph build.
        #[arg(long)]
        rebuild: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        compact: bool,
    },
    /// What does this symbol call? — AST-accurate forward call traversal.
    ///
    /// Same target-spec rules as `callers`: positional `<TARGET>` (with
    /// optional `<FILE>:<NAME>` scoping) or `--file --symbol`.
    Callees {
        #[arg(required_unless_present_all = ["file", "symbol"], conflicts_with_all = ["file", "symbol"])]
        target: Option<String>,
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long, requires = "symbol")]
        file: Option<String>,
        #[arg(long, requires = "file")]
        symbol: Option<String>,
        #[arg(long, default_value_t = 1)]
        depth: usize,
        /// Hide unresolved callees (the `Bare`/`External` bucket).
        /// Shown by default (tagged cyan/red); set this flag to drop them.
        #[arg(long)]
        hide_external: bool,
        /// (Deprecated — unresolved/external callees are shown by default now.)
        #[arg(long, hide = true)]
        external: bool,
        #[arg(long)]
        rebuild: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        compact: bool,
    },
    /// Trace the static call path between two symbols — "how does <FROM> reach <TO>?"
    ///
    /// Shortest-path BFS over the call graph, with each hop's source body
    /// inlined so a flow question is answered in one call instead of chaining
    /// `callees`. When no static path exists (the chain broke at dynamic
    /// dispatch), both endpoints + the target file's siblings are shown.
    /// Targets are suffix-matched like `callers` (`run`, `Type.method`, or
    /// `src/f.rs:name`).
    Trace {
        /// Source symbol — where the call path starts.
        from: String,
        /// Destination symbol — where the call path should reach.
        to: String,
        /// Repository root (default: ".").
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Max path length in hops.
        #[arg(long, default_value_t = 12)]
        depth: usize,
        /// Force a fresh call-graph build.
        #[arg(long)]
        rebuild: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        compact: bool,
    },
    /// Cross-file impact analysis: callers + callees + file reverse-deps + test detection, one command.
    Impact {
        /// Symbol to analyse (same form as callers/callees: `TakeDamage`, `Player.TakeDamage`, `src/Player.cs:TakeDamage`).
        #[arg(required_unless_present_all = ["file", "symbol"], conflicts_with_all = ["file", "symbol"])]
        target: Option<String>,
        /// Repository root (default: ".").
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Alternative to the positional target.
        #[arg(long, requires = "symbol")]
        file: Option<String>,
        /// Symbol name when using `--file`.
        #[arg(long, requires = "file")]
        symbol: Option<String>,
        /// Transitive depth (default 2).
        #[arg(long, default_value_t = 2)]
        depth: usize,
        /// Cap number of results per section.
        #[arg(long, default_value_t = 200)]
        limit: usize,
        /// Section to show: `deps`, `dependents`, `tests`, or `all` (default).
        #[arg(long, default_value = "all")]
        mode: String,
        /// Hide ambiguous call-edge matches from impact output.
        /// Shown by default (tagged red); set this flag to drop them.
        #[arg(long)]
        hide_ambiguous: bool,
        /// Show only test files (path heuristic: tests/, __tests__/, *_test.*, *.spec.*, …).
        #[arg(long)]
        tests: bool,
        /// Exclude test files from the output (same path heuristic as --tests).
        #[arg(long, conflicts_with = "tests")]
        exclude_tests: bool,
        #[arg(long)]
        rebuild: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        compact: bool,
    },
    /// Build, refresh, or inspect the per-repo search index
    Index {
        /// Repository root (default: ".")
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Drop any existing cache and rebuild from scratch
        #[arg(long)]
        rebuild: bool,
        /// Print index stats and exit
        #[arg(long)]
        stats: bool,
        /// With --stats: emit output as JSON
        #[arg(long)]
        json: bool,
        /// With --json: emit compact (single-line) JSON
        #[arg(long)]
        compact: bool,
    },
    /// AST-aware search and rewrite using pattern matching with metavariables
    Run {
        /// Pattern to match (e.g. '$FUNC($$$)', 'if ($COND) { $$$BODY }')
        #[arg(short, long)]
        pattern: String,

        /// Replacement template (e.g. 'bar($A)'). Omit for search-only mode.
        #[arg(short, long)]
        rewrite: Option<String>,

        /// Language (auto-detected from file extension if omitted)
        #[arg(short, long)]
        lang: Option<String>,

        /// Paths to search (files or directories). Defaults to current directory.
        paths: Vec<PathBuf>,

        /// Filter files by glob pattern
        #[arg(long)]
        glob: Option<String>,

        /// Actually write changes. Without this flag, only shows matches/dry-run.
        #[arg(long)]
        write: bool,

        /// Emit output as JSON
        #[arg(long)]
        json: bool,

        /// With --json: compact single-line JSON
        #[arg(long)]
        compact: bool,
    },
}
pub fn run() {
    use clap::CommandFactory;
    use clap::error::ErrorKind;

    // Agent-friendly arg handling: instead of dying on a typo or unknown
    // flag, print the help text so the calling agent can self-correct
    // without a separate `--help` round-trip. `--help` / `--version` keep
    // their normal exit-0 behaviour; everything else prints help to stdout
    // and exits 0 too (agents see "output" rather than "error").
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => match e.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                e.exit();
            }
            _ => {
                let mut cmd = Cli::command();
                let _ = cmd.print_help();
                println!();
                println!(
                    "# note: could not parse args ({}). Showing help instead.",
                    e.kind()
                );
                std::process::exit(0);
            }
        },
    };

    match &cli.command {
        Commands::Map {
            paths,
            no_private,
            no_fields,
            no_docs,
            no_attrs,
            no_lines,
            glob,
            json,
            compact,
        } => {
            let results = walk_and_parse(paths, glob.as_deref());
            let opts = MapOptions {
                include_private: !(*no_private),
                include_fields: !(*no_fields),
                include_docs: !(*no_docs),
                include_attributes: !(*no_attrs),
                include_line_numbers: !(*no_lines),
                max_doc_lines: 6,
                max_members: None,
            };
            let json_on = *json;
            let pretty = !(*compact);
            if json_on {
                println!("{}", crate::core::render_json_map(&results, &opts, pretty));
            } else {
                for res in results {
                    println!("{}", crate::core::render_map(&res, &opts));
                    println!();
                }
            }
        }
        Commands::Show {
            path,
            symbol,
            others,
            json,
            compact,
        } => {
            if !path.exists() {
                println!("# note: path not found: {}", path.display());
            } else if let Some(res) = parse_file(path) {
                let mut symbols = vec![symbol.as_str()];
                symbols.extend(others.iter().map(|s| s.as_str()));
                if *json {
                    let mut seen = std::collections::HashSet::new();
                    let mut all_matches = Vec::new();
                    for sym in &symbols {
                        for m in crate::core::find_symbols(&res, sym) {
                            let key = (m.start_line, m.end_line, m.qualified_name.clone());
                            if seen.insert(key) {
                                all_matches.push(m);
                            }
                        }
                    }
                    println!(
                        "{}",
                        crate::core::render_json_show(&res, &all_matches, !(*compact))
                    );
                    if all_matches.is_empty() {
                        // JSON consumers see [] in the payload; humans/agents
                        // glancing at stderr-free output get a hint too.
                        println!(
                            "# note: no symbol matching {:?} in {}",
                            symbol,
                            path.display()
                        );
                    }
                } else {
                    let mut any_match = false;
                    for sym in &symbols {
                        let matches = crate::core::find_symbols(&res, sym);
                        for m in matches {
                            any_match = true;
                            println!(
                                "# {}:{}-{} {} ({})",
                                res.path.display(),
                                m.start_line,
                                m.end_line,
                                m.qualified_name,
                                m.kind
                            );
                            if !m.ancestor_signatures.is_empty() {
                                println!("# in: {}", m.ancestor_signatures.join(" → "));
                            }
                            println!("{}", m.source);
                        }
                    }
                    if !any_match {
                        let joined = symbols.join(", ");
                        println!(
                            "# note: no symbol matching '{}' in {}",
                            joined,
                            path.display()
                        );
                    }
                }
            } else {
                println!(
                    "# note: unsupported file type for `show`: {}",
                    path.display()
                );
            }
        }
        Commands::Squeeze {
            path,
            range,
            raw,
            json,
            compact,
        } => {
            if !path.exists() {
                println!("# note: path not found: {}", path.display());
                return;
            }
            let text = match std::fs::read_to_string(path) {
                Ok(t) => t,
                Err(e) => {
                    if path.is_dir() {
                        println!("# note: path is a directory: {}", path.display());
                    } else {
                        println!("# note: could not read {}: {}", path.display(), e);
                    }
                    return;
                }
            };
            let line_count = text.lines().count();
            let resolved: Option<(usize, usize)> = range.as_ref().map(|r| r.resolve(line_count));
            let sliced = crate::squeeze::render::slice_lines(&text, resolved);
            let path_str = path.display().to_string();
            let report = crate::squeeze::render::SqueezeReport {
                path: &path_str,
                range: resolved,
                raw: &sliced,
                raw_requested: *raw,
            };
            if *json {
                println!(
                    "{}",
                    crate::squeeze::render::render_json(&report, !(*compact))
                );
            } else {
                println!("{}", crate::squeeze::render::render_text(&report));
            }
        }
        Commands::Digest {
            paths,
            include_private,
            include_fields,
            max_members,
            json,
            compact,
        } => {
            let results = walk_and_parse(paths, None);
            if *json {
                let opts = MapOptions {
                    include_private: *include_private,
                    include_fields: *include_fields,
                    include_docs: true,
                    include_attributes: true,
                    include_line_numbers: true,
                    max_doc_lines: 6,
                    max_members: Some(*max_members),
                };
                println!(
                    "{}",
                    crate::core::render_json_map(&results, &opts, !(*compact))
                );
            } else {
                let opts = DigestOptions {
                    include_private: *include_private,
                    include_fields: *include_fields,
                    max_members_per_type: *max_members,
                    max_heading_depth: 3,
                };
                let root = if paths.len() == 1 && paths[0].is_dir() {
                    Some(paths[0].as_path())
                } else {
                    None
                };
                println!("{}", crate::core::render_digest(&results, &opts, root));
            }
        }
        Commands::Implements {
            target,
            paths,
            direct,
            json,
            compact,
        } => {
            let results = walk_and_parse(paths, None);
            let transitive = !direct;
            let matches = crate::core::find_implementations(&results, target, transitive);
            if *json {
                println!(
                    "{}",
                    crate::core::render_json_implements(target, &matches, transitive, !(*compact),)
                );
            } else {
                println!(
                    "# {} match(es) for '{}' (incl. transitive):",
                    matches.len(),
                    target
                );
                for m in matches {
                    let via = if m.via.is_empty() {
                        String::new()
                    } else {
                        format!(" [via {}]", m.via.last().unwrap())
                    };
                    println!("{}:{}  {} {}{}", m.path, m.start_line, m.kind, m.name, via);
                }
            }
        }
        Commands::Search {
            query,
            path,
            top_k,
            alpha,
            languages,
            rebuild,
            json,
            compact,
        } => {
            if *rebuild {
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                if let Err(e) = crate::search::index::Index::build(path, &cwd) {
                    eprintln!("ast-bro: rebuild failed: {e}");
                    std::process::exit(1);
                }
            }
            let exit = crate::search::cli::run_search(
                query,
                path,
                *top_k,
                *alpha,
                languages.clone(),
                *json,
                !(*compact),
            );
            std::process::exit(exit);
        }
        Commands::FindRelated {
            target,
            path,
            file,
            line,
            top_k,
            json,
            compact,
        } => {
            // Clap guarantees one of: (target alone) or (file + line).
            let (file_path, line_num) = match (target, file, line) {
                (Some(t), _, _) => match parse_file_line(t) {
                    Some(parsed) => parsed,
                    None => {
                        println!(
                            "# note: expected <FILE>:<LINE>, got {t:?} \
                                 (or use --file FILE --line N instead)"
                        );
                        return;
                    }
                },
                (None, Some(f), Some(l)) => (f.clone(), *l),
                _ => unreachable!("clap should have rejected this argument combination"),
            };
            let exit = crate::search::cli::run_find_related(
                &file_path,
                line_num,
                path,
                *top_k,
                *json,
                !(*compact),
            );
            std::process::exit(exit);
        }
        Commands::Surface {
            path,
            tree,
            include_chain,
            max_depth,
            include_private,
            lang,
            json,
            compact,
        } => {
            let lang_override = match lang {
                Some(s) => match crate::surface::LangOverride::parse(s) {
                    Some(l) => Some(l),
                    None => {
                        println!(
                            "# note: unknown --lang value '{}'. Expected rust|python|fallback.",
                            s
                        );
                        return;
                    }
                },
                None => None,
            };
            let json_on = *json;
            let pretty = !(*compact);
            let output = if json_on {
                crate::surface::OutputMode::Json { compact: !pretty }
            } else if *tree {
                crate::surface::OutputMode::Tree
            } else {
                crate::surface::OutputMode::Flat
            };
            let opts = crate::surface::SurfaceOptions {
                output,
                include_private: *include_private,
                max_depth: *max_depth,
                include_chain: *include_chain,
                lang_override,
            };
            match crate::surface::resolve_surface(path, &opts) {
                Ok(entries) => {
                    let rendered =
                        crate::surface::render::render(&entries, opts.output, opts.include_chain);
                    print!("{}", rendered);
                }
                Err(e) => {
                    println!("# note: {e}");
                }
            }
        }
        Commands::Deps {
            file,
            depth,
            hide_external,
            rebuild,
            json,
            compact,
        } => {
            let exit = crate::deps::cli::run_deps(
                file,
                *depth,
                !(*hide_external),
                *json,
                !(*compact),
                *rebuild,
            );
            std::process::exit(exit);
        }
        Commands::ReverseDeps {
            file,
            depth,
            limit,
            tests,
            exclude_tests,
            rebuild,
            json,
            compact,
        } => {
            let exit = crate::deps::cli::run_reverse_deps(
                file,
                *depth,
                *limit,
                *tests,
                *exclude_tests,
                *json,
                !(*compact),
                *rebuild,
            );
            std::process::exit(exit);
        }
        Commands::Cycles {
            path,
            min_size,
            rebuild,
            json,
            compact,
        } => {
            let exit = crate::deps::cli::run_cycles(path, *min_size, *json, !(*compact), *rebuild);
            std::process::exit(exit);
        }
        Commands::Graph {
            path,
            json,
            hide_external,
            include_external,
            rebuild,
            compact,
        } => {
            if *include_external {
                eprintln!(
                    "# note: --include-external is deprecated; unresolved imports are shown by default now (use --hide-external to drop them)"
                );
            }
            let exit =
                crate::deps::cli::run_graph(path, *json, !(*hide_external), !(*compact), *rebuild);
            std::process::exit(exit);
        }
        Commands::Index {
            path,
            rebuild,
            stats,
            json,
            compact,
        } => {
            let exit = crate::search::cli::run_index(path, *rebuild, *stats, *json, !(*compact));
            std::process::exit(exit);
        }
        Commands::Callers {
            target,
            path,
            file,
            symbol,
            depth,
            limit,
            hide_ambiguous,
            include_ambiguous,
            tests,
            exclude_tests,
            rebuild,
            json,
            compact,
        } => {
            if *include_ambiguous {
                eprintln!(
                    "# note: --include-ambiguous is deprecated; ambiguous callers are shown by default now (use --hide-ambiguous to drop them)"
                );
            }
            let resolved = compose_target(target.as_deref(), file.as_deref(), symbol.as_deref());
            let exit = crate::calls::cli::run_callers(
                &resolved,
                path,
                *depth,
                *limit,
                !(*hide_ambiguous),
                *tests,
                *exclude_tests,
                *rebuild,
                *json,
                !(*compact),
            );
            std::process::exit(exit);
        }
        Commands::Context {
            target,
            path,
            file,
            symbol,
            budget,
            rebuild,
            json,
            compact,
        } => {
            let resolved = compose_target(target.as_deref(), file.as_deref(), symbol.as_deref());
            let exit = crate::context::run_context(
                &resolved,
                path,
                &crate::context::ContextOptions {
                    budget: *budget,
                    json: *json,
                    pretty: !(*compact),
                },
                *rebuild,
            );
            std::process::exit(exit);
        }
        Commands::Callees {
            target,
            path,
            file,
            symbol,
            depth,
            hide_external,
            external,
            rebuild,
            json,
            compact,
        } => {
            if *external {
                eprintln!(
                    "# note: --external is deprecated; unresolved/external callees are shown by default now (use --hide-external to drop them)"
                );
            }
            let resolved = compose_target(target.as_deref(), file.as_deref(), symbol.as_deref());
            let exit = crate::calls::cli::run_callees(
                &resolved,
                path,
                *depth,
                !(*hide_external),
                *rebuild,
                *json,
                !(*compact),
            );
            std::process::exit(exit);
        }
        Commands::Trace {
            from,
            to,
            path,
            depth,
            rebuild,
            json,
            compact,
        } => {
            let exit =
                crate::calls::cli::run_trace(from, to, path, *depth, *rebuild, *json, !(*compact));
            std::process::exit(exit);
        }
        Commands::Impact {
            target,
            path,
            file,
            symbol,
            depth,
            limit,
            mode,
            hide_ambiguous,
            tests,
            exclude_tests,
            rebuild,
            json,
            compact,
        } => {
            let impact_mode = match crate::impact::ImpactMode::parse(mode) {
                Some(m) => m,
                None => {
                    eprintln!(
                        "# note: unknown --mode '{}'. Expected: deps, dependents, tests, all",
                        mode
                    );
                    std::process::exit(2);
                }
            };
            let resolved = compose_target(target.as_deref(), file.as_deref(), symbol.as_deref());
            let opts = crate::impact::ImpactOptions {
                depth: *depth,
                limit: *limit,
                mode: impact_mode,
                include_ambiguous: !(*hide_ambiguous),
                tests: *tests,
                exclude_tests: *exclude_tests,
                json: *json,
                pretty: !(*compact),
            };
            let exit = crate::impact::run_impact(&resolved, path, &opts, *rebuild);
            std::process::exit(exit);
        }
        Commands::Run {
            pattern,
            rewrite,
            lang,
            paths,
            glob,
            write,
            json,
            compact,
        } => {
            let exit = crate::run::cli::run(
                pattern,
                rewrite.as_deref(),
                lang.as_deref(),
                paths,
                glob.as_deref(),
                *write,
                *json,
                !(*compact),
            );
            std::process::exit(exit);
        }
    }
}

/// Fold `--file <F> --symbol <S>` into the same `<file>:<symbol>` canonical
/// form the positional `<TARGET>` arg uses. Clap's `required_unless_present_all`
/// guarantees exactly one of the two arms is populated.
fn compose_target(target: Option<&str>, file: Option<&str>, symbol: Option<&str>) -> String {
    if let Some(t) = target {
        return t.to_string();
    }
    match (file, symbol) {
        (Some(f), Some(s)) => format!("{}:{}", f, s),
        _ => unreachable!("clap guarantees target XOR (file && symbol)"),
    }
}
