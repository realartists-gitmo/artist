//! Structural code-navigation tools.
//!
//! These wrap `artist-ast`'s analysis engine. Two conventions differ from
//! upstream and are worth stating once here rather than in every tool:
//!
//! - **Shape output is anchored.** `code_map` addresses declarations by
//!   mnemonic anchor, not line number, so the model can go straight from an
//!   outline to `edit` without an intervening `read`. Everything else reports
//!   file positions, because a cross-file graph answer is a place to *look*,
//!   not a place to edit.
//! - **Nothing writes.** `ast_rewrite` previews only; applying goes through the
//!   existing edit path, which is where atomicity and the anchor ledger live.

use crate::{ToolError, Workspace, outline, output};
use hashline_tools::ReadFileRequest;
use rig_core::tool::PortableTool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Resolve a tool path argument against the workspace, defaulting to the root.
fn scope(ws: &Workspace, path: Option<&str>) -> Result<PathBuf, ToolError> {
    match path {
        Some(p) => Ok(ws.resolve_existing(p)?),
        None => Ok(ws.root().to_path_buf()),
    }
}

// ---------------------------------------------------------------------------
// code_map — structural outline, anchored
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct CodeMapTool(pub Workspace);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeMapArgs {
    path: String,
    /// `full` keeps every declaration the budget allows; `digest` shows only
    /// top-level shape.
    mode: Option<String>,
    budget: Option<usize>,
    include_private: Option<bool>,
}

impl PortableTool for CodeMapTool {
    const NAME: &'static str = "code_map";
    type Error = ToolError;
    type Args = CodeMapArgs;
    type Output = String;

    fn description(&self) -> String {
        "Structural outline of a source file: declarations with their mnemonic anchors, no bodies. \
         Each row is `ANCHOR: signature`, or `START..END: signature` when the declaration spans \
         lines. Those anchors are live — pass them straight to edit without reading the file first. \
         Prefer this over read when you need a file's shape rather than its contents."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Project-relative or absolute path to one source file."},
                "mode": {"enum": ["full", "digest"], "description": "digest shows top-level shape only."},
                "budget": {"type": "integer", "minimum": 1, "maximum": 2000, "description": "Target row count. Unfolding stops once reached."},
                "includePrivate": {"type": "boolean"}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: CodeMapArgs) -> Result<String, ToolError> {
        let target = self.0.resolve_existing(&args.path)?;
        let parsed = artist_ast::parse_file(&target).ok_or_else(|| {
            ToolError::Message(format!(
                "no language adapter for {} — use read instead",
                args.path
            ))
        })?;

        // Read the whole file through the ledger so anchors are issued for
        // every line. Rendering a subset is fine; *reconciling* a subset would
        // free the handles for every line the outline does not show.
        let read = self
            .0
            .files
            .read_file(
                &self.0.actor,
                ReadFileRequest {
                    path: args.path.clone(),
                    start_line: 1,
                    max_lines: None,
                },
            )
            .await?;
        let anchors = outline::anchor_map(&read.result.lines);

        let digest = args.mode.as_deref() == Some("digest");
        let opts = outline::OutlineOptions {
            // A digest is the top level and nothing more, so a budget equal to
            // the root declaration count keeps the unfold from descending.
            budget: if digest {
                parsed.declarations.len().max(1)
            } else {
                args.budget.unwrap_or(120)
            },
            ceiling: args.budget.map_or(240, |b| b * 2),
            include_private: args.include_private.unwrap_or(true),
        };
        let body = outline::render(&parsed.declarations, &anchors, &opts);
        Ok(output::head(
            format!("{} ({})\n\n{body}", args.path, parsed.language),
            output::OUTPUT_CAP,
        ))
    }
}

// ---------------------------------------------------------------------------
// code_show — one symbol's source, anchored
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct CodeShowTool(pub Workspace);

#[derive(Deserialize)]
pub struct CodeShowArgs {
    path: String,
    symbol: String,
}

impl PortableTool for CodeShowTool {
    const NAME: &'static str = "code_show";
    type Error = ToolError;
    type Args = CodeShowArgs;
    type Output = String;

    fn description(&self) -> String {
        "Extract one symbol's source from a file, with each line's mnemonic anchor. Code symbols \
         match by exact suffix; markdown headings match by case-insensitive substring."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "symbol": {"type": "string", "description": "Symbol name, or a markdown heading."}
            },
            "required": ["path", "symbol"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: CodeShowArgs) -> Result<String, ToolError> {
        let target = self.0.resolve_existing(&args.path)?;
        let parsed = artist_ast::parse_file(&target)
            .ok_or_else(|| ToolError::Message(format!("no language adapter for {}", args.path)))?;
        let matches = artist_ast::core::find_symbols(&parsed, &args.symbol);
        if matches.is_empty() {
            return Err(ToolError::Message(format!(
                "no symbol matching '{}' in {}",
                args.symbol, args.path
            )));
        }
        let read = self
            .0
            .files
            .read_file(
                &self.0.actor,
                ReadFileRequest {
                    path: args.path.clone(),
                    start_line: 1,
                    max_lines: None,
                },
            )
            .await?;

        let mut out = String::new();
        for m in &matches {
            out.push_str(&format!("{} — {}\n", args.symbol, args.path));
            for line in read
                .result
                .lines
                .iter()
                .filter(|l| l.line_number >= m.start_line && l.line_number <= m.end_line)
            {
                out.push_str(&format!("{}: {}\n", line.anchor, line.text));
            }
            out.push('\n');
        }
        Ok(output::head(out, output::OUTPUT_CAP))
    }
}

// ---------------------------------------------------------------------------
// code_surface — true public API
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct CodeSurfaceTool(pub Workspace);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeSurfaceArgs {
    path: Option<String>,
    tree: Option<bool>,
    include_chain: Option<bool>,
}

impl PortableTool for CodeSurfaceTool {
    const NAME: &'static str = "code_surface";
    type Error = ToolError;
    type Args = CodeSurfaceArgs;
    type Output = String;

    fn description(&self) -> String {
        "The true public API of a crate, package or directory — follows `pub use` and `__all__` \
         re-export chains to what is actually exported, rather than what one file happens to declare."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Crate root, package init, or directory. Defaults to the project root."},
                "tree": {"type": "boolean", "description": "Group hierarchically by module."},
                "includeChain": {"type": "boolean", "description": "Show the re-export chain for each entry."}
            },
            "additionalProperties": false
        })
    }

    async fn call(&self, args: CodeSurfaceArgs) -> Result<String, ToolError> {
        let target = scope(&self.0, args.path.as_deref())?;
        let opts = artist_ast::surface::SurfaceOptions::default();
        let entries = artist_ast::surface::resolve_surface(&target, &opts)
            .map_err(|e| ToolError::Message(format!("surface: {e}")))?;
        let mode = if args.tree.unwrap_or(false) {
            artist_ast::surface::OutputMode::Tree
        } else {
            artist_ast::surface::OutputMode::Flat
        };
        let body = artist_ast::surface::render::render(
            &entries,
            mode,
            args.include_chain.unwrap_or(false),
        );
        Ok(output::head(body, output::OUTPUT_CAP))
    }
}

// ---------------------------------------------------------------------------
// code_implements — subclasses / trait impls
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct CodeImplementsTool(pub Workspace);

#[derive(Deserialize)]
pub struct CodeImplementsArgs {
    target: String,
    path: Option<String>,
    direct: Option<bool>,
}

impl PortableTool for CodeImplementsTool {
    const NAME: &'static str = "code_implements";
    type Error = ToolError;
    type Args = CodeImplementsArgs;
    type Output = String;

    fn description(&self) -> String {
        "Find implementations or subclasses of a type or trait. Structural, so it does not \
         false-positive on the name appearing in comments or unrelated code the way grep does."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": {"type": "string", "description": "Type or trait name."},
                "path": {"type": "string", "description": "Search scope. Defaults to the project root."},
                "direct": {"type": "boolean", "description": "Direct implementors only, no transitive chain."}
            },
            "required": ["target"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: CodeImplementsArgs) -> Result<String, ToolError> {
        let root = scope(&self.0, args.path.as_deref())?;
        let parsed = artist_ast::walk_and_parse(&[root], None);
        let transitive = !args.direct.unwrap_or(false);
        let hits = artist_ast::core::find_implementations(&parsed, &args.target, transitive);
        if hits.is_empty() {
            return Ok(format!("no implementations of '{}' found", args.target));
        }
        let mut out = format!("implementations of {} ({})\n", args.target, hits.len());
        for h in &hits {
            out.push_str(&format!(
                "  {}:{} {} {}\n",
                self.0.display(Path::new(&h.path)),
                h.start_line,
                h.kind,
                h.name
            ));
        }
        Ok(output::head(out, output::OUTPUT_CAP))
    }
}

// ---------------------------------------------------------------------------
// code_deps — import graph, either direction
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct CodeDepsTool(pub Workspace);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeDepsArgs {
    file: String,
    direction: Option<String>,
    depth: Option<usize>,
    limit: Option<usize>,
    exclude_tests: Option<bool>,
}

impl PortableTool for CodeDepsTool {
    const NAME: &'static str = "code_deps";
    type Error = ToolError;
    type Args = CodeDepsArgs;
    type Output = String;

    fn description(&self) -> String {
        "Import graph around a file. `forward` is what it imports (transitively); `reverse` is who \
         imports it — check reverse before changing anything with callers you cannot see."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file": {"type": "string"},
                "direction": {"enum": ["forward", "reverse"], "description": "Defaults to forward."},
                "depth": {"type": "integer", "minimum": 1, "maximum": 10},
                "limit": {"type": "integer", "minimum": 1, "maximum": 500},
                "excludeTests": {"type": "boolean", "description": "reverse only: drop importers under test paths."}
            },
            "required": ["file"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: CodeDepsArgs) -> Result<String, ToolError> {
        let file = self.0.resolve_existing(&args.file)?;
        let root = artist_ast::project_root::find_root_for(&file)
            .map_err(|e| ToolError::Message(format!("project root: {e}")))?;
        let graph = artist_ast::graph_cache::shared::get_or_init(&root)
            .map_err(|e| ToolError::Message(format!("dep graph: {e}")))?;
        let depth = args.depth.unwrap_or(3).min(10);

        let body = if args.direction.as_deref() == Some("reverse") {
            // In the reverse walk each edge's `target` is the *importer*, so
            // that is what the test-path filter applies to.
            let exclude_tests = args.exclude_tests.unwrap_or(false);
            let hits = artist_ast::deps::traverse::reverse(
                &graph.deps,
                &file,
                depth,
                args.limit.unwrap_or(200),
                |edge| {
                    !exclude_tests || !artist_ast::file_filter::is_test_file(&edge.target, &root)
                },
            );
            artist_ast::deps::render::render_reverse_deps_text(&graph.deps, &file, &hits)
        } else {
            let hits = artist_ast::deps::traverse::forward(&graph.deps, &file, depth);
            artist_ast::deps::render::render_deps_text(&graph.deps, &file, &hits, true)
        };
        Ok(output::head(body, output::OUTPUT_CAP))
    }
}

// ---------------------------------------------------------------------------
// code_cycles — import cycles
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct CodeCyclesTool(pub Workspace);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeCyclesArgs {
    path: Option<String>,
    min_size: Option<usize>,
}

impl PortableTool for CodeCyclesTool {
    const NAME: &'static str = "code_cycles";
    type Error = ToolError;
    type Args = CodeCyclesArgs;
    type Output = String;

    fn description(&self) -> String {
        "Import cycles in the project, via Tarjan SCC. Useful when a refactor keeps pulling in more \
         than it should, or a module will not extract cleanly."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "minSize": {"type": "integer", "minimum": 2, "maximum": 50}
            },
            "additionalProperties": false
        })
    }

    async fn call(&self, args: CodeCyclesArgs) -> Result<String, ToolError> {
        let target = scope(&self.0, args.path.as_deref())?;
        let root = artist_ast::project_root::find_root_for(&target)
            .map_err(|e| ToolError::Message(format!("project root: {e}")))?;
        let graph = artist_ast::graph_cache::shared::get_or_init(&root)
            .map_err(|e| ToolError::Message(format!("dep graph: {e}")))?;
        let cycles = artist_ast::deps::scc::detect(&graph.deps, args.min_size.unwrap_or(2));
        if cycles.is_empty() {
            return Ok("no import cycles found".into());
        }
        Ok(output::head(
            artist_ast::deps::render::render_cycles_text(&graph.deps, &cycles),
            output::OUTPUT_CAP,
        ))
    }
}

// ---------------------------------------------------------------------------
// code_calls / code_trace — call graph
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct CodeCallsTool(pub Workspace);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeCallsArgs {
    symbol: String,
    direction: Option<String>,
    path: Option<String>,
    depth: Option<usize>,
    limit: Option<usize>,
}

impl PortableTool for CodeCallsTool {
    const NAME: &'static str = "code_calls";
    type Error = ToolError;
    type Args = CodeCallsArgs;
    type Output = String;

    fn description(&self) -> String {
        "Call graph around a symbol. `callers` is who calls it — the first thing to check before \
         changing a signature; `callees` is what it calls. AST-accurate, so unlike grep it does not \
         match the name in comments, strings, or an unrelated type's method."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "symbol": {"type": "string", "description": "`name`, `Type.method`, or `path/file.rs:name` to scope to one file."},
                "direction": {"enum": ["callers", "callees"], "description": "Defaults to callers."},
                "path": {"type": "string"},
                "depth": {"type": "integer", "minimum": 1, "maximum": 5},
                "limit": {"type": "integer", "minimum": 1, "maximum": 500}
            },
            "required": ["symbol"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: CodeCallsArgs) -> Result<String, ToolError> {
        let target = scope(&self.0, args.path.as_deref())?;
        let root = artist_ast::project_root::find_root_for(&target)
            .map_err(|e| ToolError::Message(format!("project root: {e}")))?;
        let depth = args.depth.unwrap_or(1).min(5);
        let body = if args.direction.as_deref() == Some("callees") {
            artist_ast::calls::mcp::run_callees_text(&args.symbol, &root, depth, true, false)
        } else {
            artist_ast::calls::mcp::run_callers_text(
                &args.symbol,
                &root,
                depth,
                args.limit.unwrap_or(200),
                true,
                false,
            )
        };
        Ok(output::head(body, output::OUTPUT_CAP))
    }
}

#[derive(Clone)]
pub struct CodeTraceTool(pub Workspace);

#[derive(Deserialize)]
pub struct CodeTraceArgs {
    from: String,
    to: String,
    path: Option<String>,
    depth: Option<usize>,
}

impl PortableTool for CodeTraceTool {
    const NAME: &'static str = "code_trace";
    type Error = ToolError;
    type Args = CodeTraceArgs;
    type Output = String;

    fn description(&self) -> String {
        "How does one symbol reach another? Shortest static call path between two symbols, with \
         each hop's source inlined. Answers a flow question in one call instead of chaining callees."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "from": {"type": "string"},
                "to": {"type": "string"},
                "path": {"type": "string"},
                "depth": {"type": "integer", "minimum": 1, "maximum": 20, "description": "Max path length in hops."}
            },
            "required": ["from", "to"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: CodeTraceArgs) -> Result<String, ToolError> {
        let target = scope(&self.0, args.path.as_deref())?;
        let root = artist_ast::project_root::find_root_for(&target)
            .map_err(|e| ToolError::Message(format!("project root: {e}")))?;
        let body = artist_ast::calls::mcp::run_trace_text(
            &args.from,
            &args.to,
            &root,
            args.depth.unwrap_or(12).min(20),
            false,
        );
        Ok(output::head(body, output::OUTPUT_CAP))
    }
}

// ---------------------------------------------------------------------------
// ast_query / ast_rewrite — structural pattern search and rewrite
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AstQueryTool(pub Workspace);

#[derive(Deserialize)]
pub struct AstQueryArgs {
    pattern: String,
    path: Option<String>,
    lang: Option<String>,
    limit: Option<usize>,
}

const PATTERN_HELP: &str = "`$NAME` captures one node, `$_` matches one without binding, `$$$NAME` \
     captures zero or more, `$$$` matches zero or more unbound. Metavariable names must be \
     UPPERCASE and stand for whole AST nodes — `prefix$VAR` will not match. The same metavariable \
     twice requires identical code at both sites. The pattern must parse as one valid node in the \
     target language.";

impl PortableTool for AstQueryTool {
    const NAME: &'static str = "ast_query";
    type Error = ToolError;
    type Args = AstQueryArgs;
    type Output = String;

    fn description(&self) -> String {
        format!(
            "Structural pattern search — matches AST shape, not text, so formatting and whitespace \
             are irrelevant. Finds things grep cannot express: `$X.unwrap()` inside a specific \
             construct, a call with a particular argument shape. {PATTERN_HELP}"
        )
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "e.g. `$FUNC($$$ARGS)` or `if ($COND) { $$$BODY }`"},
                "path": {"type": "string"},
                "lang": {"type": "string", "description": "Auto-detected per file when omitted."},
                "limit": {"type": "integer", "minimum": 1, "maximum": 200}
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: AstQueryArgs) -> Result<String, ToolError> {
        let root = scope(&self.0, args.path.as_deref())?;
        let limit = args.limit.unwrap_or(50).min(200);
        let files = artist_ast::walk_paths(&[root], None);
        let mut out = String::new();
        let mut found = 0usize;

        for file in &files {
            if found >= limit {
                break;
            }
            let Some(lang) = lang_for(&args.lang, file) else {
                continue;
            };
            let Ok(source) = std::fs::read_to_string(file) else {
                continue;
            };
            let matches = match artist_ast::run::search(&source, lang, &args.pattern) {
                Ok(m) => m,
                // A pattern that does not parse in *this* language is normal in
                // a mixed tree — only report it if nothing matches anywhere.
                Err(_) => continue,
            };
            for m in matches {
                if found >= limit {
                    break;
                }
                out.push_str(&format!(
                    "{}:{}: {}\n",
                    self.0.display(file),
                    m.start_line,
                    m.matched_text.lines().next().unwrap_or("").trim()
                ));
                found += 1;
            }
        }

        if found == 0 {
            return Ok(format!(
                "no structural matches for `{}`.\n\nIf that is unexpected, check the pattern parses \
                 as one node in the target language.",
                args.pattern
            ));
        }
        Ok(output::head(
            format!("{found} match(es)\n\n{out}"),
            output::OUTPUT_CAP,
        ))
    }
}

#[derive(Clone)]
pub struct AstRewriteTool(pub Workspace);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AstRewriteArgs {
    pattern: String,
    replacement: String,
    path: Option<String>,
    lang: Option<String>,
}

impl PortableTool for AstRewriteTool {
    const NAME: &'static str = "ast_rewrite";
    type Error = ToolError;
    type Args = AstRewriteArgs;
    type Output = String;

    fn description(&self) -> String {
        format!(
            "Preview a structural rewrite across many files — the multi-file codemod case edit \
             cannot do, since edit works on one path at a time. Captures from the pattern substitute \
             into the replacement. This previews only and never writes: review the diff, then apply \
             the changes with edit. {PATTERN_HELP}"
        )
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "replacement": {"type": "string", "description": "Empty string deletes the matched node."},
                "path": {"type": "string"},
                "lang": {"type": "string"}
            },
            "required": ["pattern", "replacement"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: AstRewriteArgs) -> Result<String, ToolError> {
        let root = scope(&self.0, args.path.as_deref())?;
        let files = artist_ast::walk_paths(&[root], None);
        let mut out = String::new();
        let mut changed_files = 0usize;

        for file in &files {
            let Some(lang) = lang_for(&args.lang, file) else {
                continue;
            };
            let Ok(source) = std::fs::read_to_string(file) else {
                continue;
            };
            let compiled = match ast_grep_pattern(&args.pattern, lang) {
                Ok(p) => p,
                Err(e) => return Err(ToolError::Message(e)),
            };
            let rewritten =
                artist_ast::run::rewrite_with_pattern(&source, lang, &compiled, &args.replacement)
                    .map_err(ToolError::Message)?;
            let Some(rewritten) = rewritten else { continue };
            if rewritten == source {
                continue;
            }
            changed_files += 1;
            let diff = similar::TextDiff::from_lines(&source, &rewritten)
                .unified_diff()
                .context_radius(2)
                .to_string();
            out.push_str(&format!("--- {}\n{diff}\n", self.0.display(file)));
        }

        if changed_files == 0 {
            return Ok(format!(
                "no structural matches for `{}` — nothing to rewrite.",
                args.pattern
            ));
        }
        Ok(output::head(
            format!(
                "PREVIEW ONLY — {changed_files} file(s) would change. Nothing has been written. \
                 Apply the changes you want with edit.\n\n{out}"
            ),
            output::OUTPUT_CAP,
        ))
    }
}

fn ast_grep_pattern(
    pattern: &str,
    lang: artist_ast::run::SupportLang,
) -> Result<ast_grep_core::Pattern, String> {
    ast_grep_core::Pattern::try_new(pattern, lang).map_err(|e| format!("invalid pattern: {e}"))
}

/// Resolve the language for a file, honouring an explicit override.
fn lang_for(explicit: &Option<String>, file: &Path) -> Option<artist_ast::run::SupportLang> {
    match explicit {
        Some(l) => artist_ast::run::cli::parse_lang(l),
        None => artist_ast::run::detect_lang(file),
    }
}

// ---------------------------------------------------------------------------
// code_impact — composite blast radius
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct CodeImpactTool(pub Workspace);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeImpactArgs {
    symbol: String,
    path: Option<String>,
    depth: Option<usize>,
    mode: Option<String>,
}

impl PortableTool for CodeImpactTool {
    const NAME: &'static str = "code_impact";
    type Error = ToolError;
    type Args = CodeImpactArgs;
    type Output = String;

    fn description(&self) -> String {
        "Blast radius for changing a symbol: callers, callees, file-level reverse dependencies and \
         affected tests, in one call. Use before a change whose reach is not obvious — it replaces \
         four separate lookups."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "symbol": {"type": "string"},
                "path": {"type": "string"},
                "depth": {"type": "integer", "minimum": 1, "maximum": 5},
                "mode": {"enum": ["all", "deps", "dependents", "tests"]}
            },
            "required": ["symbol"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: CodeImpactArgs) -> Result<String, ToolError> {
        let target = scope(&self.0, args.path.as_deref())?;
        let root = artist_ast::project_root::find_root_for(&target)
            .map_err(|e| ToolError::Message(format!("project root: {e}")))?;
        let mode = artist_ast::impact::ImpactMode::parse(args.mode.as_deref().unwrap_or("all"))
            .ok_or_else(|| ToolError::Message("invalid mode".into()))?;
        let opts = artist_ast::impact::ImpactOptions {
            depth: args.depth.unwrap_or(2).min(5),
            limit: 200,
            mode,
            // Ambiguous edges carry real construction sites, so hiding them
            // would silently shrink the blast radius the model is asking about.
            include_ambiguous: true,
            tests: false,
            exclude_tests: false,
            json: false,
            pretty: false,
        };
        let body = artist_ast::impact::report_text(&args.symbol, &root, &opts)
            .map_err(ToolError::Message)?;
        Ok(output::head(body, output::OUTPUT_CAP))
    }
}
