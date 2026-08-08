//! Structural code-navigation tools.
//!
//! These wrap `artist-ast`'s analysis engine. Two conventions differ from
//! upstream and are worth stating once here rather than in every tool:
//!
//! - **Every location is anchored.** `artist-ast` reports `file:line` the way a
//!   compiler does; nothing here passes that on. Same-file output carries the
//!   anchor alone, cross-file output carries `path@anchor` — see [`crate::locate`],
//!   which also explains why naming a line in an unread file issues handles for
//!   it. A line number is invalidated by the next insert above it; an anchor is
//!   not, so a location the model is told about stays one it can act on.
//! - **Nothing writes without being asked.** `ast_rewrite` previews by default,
//!   and its apply path goes through the coordinator so the anchor ledger sees
//!   the change. Its preview anchors removals only: the additions have not been
//!   written, so no handle exists for them.

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

/// Root an explicit analysis at the requested subtree instead of silently
/// widening it to the enclosing repository. A file scopes to its directory.
fn analysis_root(target: &Path) -> PathBuf {
    if target.is_file() {
        target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| target.to_path_buf())
    } else {
        target.to_path_buf()
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
    budget: Option<usize>,
    include_private: Option<bool>,
}

impl PortableTool for CodeMapTool {
    const NAME: &'static str = "code_map";
    type Error = ToolError;
    type Args = CodeMapArgs;
    type Output = String;

    fn description(&self) -> String {
        "Structural outline of a source file: declarations with their semantic anchors, no bodies. \
         Each row is `ANCHOR: signature`, or `START ⟶ END: signature` when the declaration spans \
         lines. Those anchors are live — pass them straight to edit without reading the file first. \
         Prefer this over read when you need a file's shape rather than its contents."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Project-relative or absolute path to one source file."},
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

        let opts = outline::OutlineOptions {
            budget: args.budget.unwrap_or(120),
            ceiling: args.budget.map_or(240, |b| b * 2),
            include_private: args.include_private.unwrap_or(true),
        };
        let body = outline::render(&parsed.declarations, &anchors, &opts, parsed.language);
        // `code_map` issues anchors just as `read` does, so it is equally a
        // first observation — hooking only `read` would let an outline-then-edit
        // path skip the note entirely.
        let note = if self.0.first_observation(&target) {
            crate::annotate::first_touch(&self.0, &target)
                .await
                .unwrap_or_default()
        } else {
            String::new()
        };
        Ok(output::head(
            format!("{} ({})\n\n{body}{note}", args.path, parsed.language),
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
        "Extract one symbol's source from a file, with each line's semantic anchor. Code symbols \
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
        let tree = args.tree.unwrap_or(false);
        let include_chain = args.include_chain.unwrap_or(false);
        let opts = artist_ast::surface::SurfaceOptions {
            include_chain,
            ..Default::default()
        };
        let mut entries = artist_ast::surface::resolve_surface(&target, &opts)
            .map_err(|e| ToolError::Message(format!("surface: {e}")))?;
        // Grouping only reads as grouping if entries from one module arrive
        // together; the resolver orders by discovery, not by path.
        if tree {
            entries.sort_by(|a, b| a.qualified_path.cmp(&b.qualified_path));
        }
        // Rendered here rather than by the vendored renderer so each export
        // reports an anchor. This is the command where that matters most: the
        // whole point of resolving `pub use` chains is that it lands on the
        // real definition, which is by construction somewhere the model has not
        // read.
        // Both knobs are applied here rather than left to `SurfaceOptions`,
        // whose `output` and `include_chain` steer the vendored renderers this
        // command deliberately does not use. They were deserialized and then
        // read by nothing: the schema described what they did, the model could
        // set them, and the output never moved.
        let mut locator = crate::locate::Locator::new(&self.0);
        let mut out = String::new();
        let mut module = String::new();
        for entry in &entries {
            let at = locator
                .locate(&entry.source_path, entry.source_line as u32)
                .await;
            // Under `tree`, each entry is printed under its module heading and
            // shortened to its own name — the module path is the heading, so
            // repeating it on every row is the thing grouping was asked to fix.
            let name = if tree {
                let (parent, leaf) = entry
                    .qualified_path
                    .rsplit_once("::")
                    .or_else(|| entry.qualified_path.rsplit_once('.'))
                    .unwrap_or(("", entry.qualified_path.as_str()));
                if parent != module {
                    module = parent.to_owned();
                    out.push_str(&format!(
                        "\n{}\n",
                        if module.is_empty() { "(root)" } else { &module }
                    ));
                }
                format!("  {leaf}")
            } else {
                entry.qualified_path.clone()
            };
            let chain = if include_chain && !entry.re_export_chain.is_empty() {
                let hops: Vec<&str> = entry
                    .re_export_chain
                    .iter()
                    .map(|hop| hop.module_path.as_str())
                    .collect();
                format!("  via {}", hops.join(" -> "))
            } else {
                String::new()
            };
            out.push_str(&format!(
                "{name}  {} ({}){chain}\n",
                entry.kind,
                at.render()
            ));
        }
        if out.trim().is_empty() {
            out.push_str("(no public surface found)\n");
        }
        Ok(output::head(out, output::OUTPUT_CAP))
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
        let mut locator = crate::locate::Locator::new(&self.0);
        let mut out = format!("implementations of {} ({})\n", args.target, hits.len());
        for h in &hits {
            let at = locator
                .locate(Path::new(&h.path), h.start_line as u32)
                .await;
            out.push_str(&format!("  {} {} ({})\n", h.kind, h.name, at.render()));
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
        let scoped_graph;
        let deps = if graph.deps.forward.contains_key(&file) {
            &graph.deps
        } else {
            scoped_graph = artist_ast::deps::build_graph(&analysis_root(&file))
                .map_err(|e| ToolError::Message(format!("scoped dep graph: {e}")))?;
            &scoped_graph
        };
        let depth = args.depth.unwrap_or(3).min(10);

        let body = if args.direction.as_deref() == Some("reverse") {
            // In the reverse walk each edge's `target` is the *importer*, so
            // that is what the test-path filter applies to.
            let exclude_tests = args.exclude_tests.unwrap_or(false);
            let hits = artist_ast::deps::traverse::reverse(
                deps,
                &file,
                depth,
                args.limit.unwrap_or(200),
                |edge| {
                    !exclude_tests || !artist_ast::file_filter::is_test_file(&edge.target, &root)
                },
            );
            artist_ast::deps::render::render_reverse_deps_text(deps, &file, &hits)
        } else {
            let hits = artist_ast::deps::traverse::forward_limited(
                deps,
                &file,
                depth,
                args.limit.unwrap_or(200),
            );
            artist_ast::deps::render::render_deps_text(deps, &file, &hits, true)
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
        let scoped_graph;
        let project_graph;
        let deps = if args.path.is_some() {
            scoped_graph = artist_ast::deps::build_graph(&analysis_root(&target))
                .map_err(|e| ToolError::Message(format!("scoped dep graph: {e}")))?;
            &scoped_graph
        } else {
            let root = artist_ast::project_root::find_root_for(&target)
                .map_err(|e| ToolError::Message(format!("project root: {e}")))?;
            project_graph = artist_ast::graph_cache::shared::get_or_init(&root)
                .map_err(|e| ToolError::Message(format!("dep graph: {e}")))?;
            &project_graph.deps
        };
        let cycles = artist_ast::deps::scc::detect(deps, args.min_size.unwrap_or(2));
        if cycles.is_empty() {
            return Ok("no import cycles found".into());
        }
        Ok(output::head(
            artist_ast::deps::render::render_cycles_text(deps, &cycles),
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
        let depth = args.depth.unwrap_or(1).min(5);
        let limit = args.limit.unwrap_or(200);
        let scoped_calls;
        let project_graph;
        let calls = if args.path.is_some() {
            let root = analysis_root(&target);
            let deps = artist_ast::deps::build_graph(&root)
                .map_err(|e| ToolError::Message(format!("scoped dep graph: {e}")))?;
            scoped_calls = artist_ast::calls::build::build_call_graph(&root, &deps);
            &scoped_calls
        } else {
            let root = artist_ast::project_root::find_root_for(&target)
                .map_err(|e| ToolError::Message(format!("project root: {e}")))?;
            project_graph = artist_ast::graph_cache::ensure_with_calls(&root, false)
                .map_err(|e| ToolError::Message(format!("call graph: {e}")))?;
            project_graph
                .calls
                .as_ref()
                .ok_or_else(|| ToolError::Message("call graph is empty".into()))?
        };
        let targets = artist_ast::calls::cli_helpers::resolve_target_qns(calls, &args.symbol);
        let Some(target) = targets.first() else {
            return Ok(format!(
                "no symbol matches '{}' (try a more specific suffix like 'Type.method')",
                args.symbol
            ));
        };

        // Rendered here so each edge reports an anchor rather than a line.
        let mut locator = crate::locate::Locator::new(&self.0);
        let mut out = String::new();
        if args.direction.as_deref() == Some("callees") {
            let hits = artist_ast::calls::traverse::callees(calls, target, depth);
            out.push_str(&format!("{} callee(s) of {}\n", hits.len(), target.0));
            for hit in hits.iter().take(limit) {
                let at = locator
                    .locate(Path::new(&hit.edge.file), hit.edge.line)
                    .await;
                out.push_str(&format!(
                    "  {} ({}) {}\n",
                    hit.edge.target.display(),
                    at.render(),
                    hit.edge.confidence.as_str()
                ));
            }
        } else {
            let hits = artist_ast::calls::traverse::callers(calls, target, depth, limit, |_| true);
            out.push_str(&format!("{} caller(s) of {}\n", hits.len(), target.0));
            for hit in hits.iter().take(limit) {
                let at = locator
                    .locate(Path::new(&hit.edge.file), hit.edge.line)
                    .await;
                out.push_str(&format!(
                    "  {} ({}) {}\n",
                    hit.edge.source.0,
                    at.render(),
                    hit.edge.confidence.as_str()
                ));
            }
        }
        Ok(output::head(out, output::OUTPUT_CAP))
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
        let scoped_calls;
        let project_graph;
        let calls = if args.path.is_some() {
            let root = analysis_root(&target);
            let deps = artist_ast::deps::build_graph(&root)
                .map_err(|e| ToolError::Message(format!("scoped dep graph: {e}")))?;
            scoped_calls = artist_ast::calls::build::build_call_graph(&root, &deps);
            &scoped_calls
        } else {
            let root = artist_ast::project_root::find_root_for(&target)
                .map_err(|e| ToolError::Message(format!("project root: {e}")))?;
            project_graph = artist_ast::graph_cache::ensure_with_calls(&root, false)
                .map_err(|e| ToolError::Message(format!("call graph: {e}")))?;
            project_graph
                .calls
                .as_ref()
                .ok_or_else(|| ToolError::Message("call graph is empty".into()))?
        };
        let froms = artist_ast::calls::cli_helpers::resolve_target_qns(calls, &args.from);
        let tos = artist_ast::calls::cli_helpers::resolve_target_qns(calls, &args.to);
        if froms.is_empty() || tos.is_empty() {
            let missing = if froms.is_empty() {
                &args.from
            } else {
                &args.to
            };
            return Ok(format!("no callable symbol matches '{missing}'"));
        }

        let depth = args.depth.unwrap_or(12).min(20);
        let Some(found) = artist_ast::calls::trace::find_path(calls, &froms, &tos, depth) else {
            return Ok(format!(
                "no static call path from '{}' to '{}' within {depth} hops.\n\
                 The chain may break at dynamic dispatch — a trait object, a callback, \
                 a handler registered at runtime — which this cannot follow.",
                args.from, args.to
            ));
        };

        // Each hop is a place the model may want to act, so it gets an anchor
        // like every other cross-file answer.
        let mut locator = crate::locate::Locator::new(&self.0);
        let mut out = format!(
            "{} → {} ({} hop(s))\n",
            args.from,
            args.to,
            found.hops.len()
        );
        out.push_str(&format!("  {}\n", found.start.0));
        for hop in &found.hops {
            let at = locator.locate(Path::new(&hop.via.file), hop.via.line).await;
            out.push_str(&format!(
                "  → {} ({}) {}\n",
                hop.qn.0,
                at.render(),
                hop.via.confidence.as_str()
            ));
        }
        Ok(output::head(out, output::OUTPUT_CAP))
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
    glob: Option<String>,
    lang: Option<String>,
    limit: Option<usize>,
}

/// Compile a pattern, treating one built from a syntax error as no pattern.
///
/// `Pattern::try_new` accepts `fn (` and returns a pattern rooted at an ERROR
/// node, which matches nothing. Taking that as a successful compile is what
/// made a typo indistinguishable from an honest miss.
fn compile_usable(
    pattern: &str,
    lang: artist_ast::run::SupportLang,
) -> Option<artist_ast::run::Pattern> {
    let compiled = artist_ast::run::compile(pattern, lang).ok()?;
    (!artist_ast::run::pattern_is_malformed(&compiled)).then_some(compiled)
}

/// Say which of the three empty outcomes this was.
///
/// "No matches" answers a question the model did not ask. It wants to know
/// whether to fix the pattern, widen the scope, or believe the result — and
/// those are three different states that rendered as one sentence.
fn empty_search(
    pattern: &str,
    lang: Option<&str>,
    searched: usize,
    seen: &std::collections::BTreeSet<String>,
    parsed: &std::collections::BTreeSet<String>,
    skipped: usize,
) -> String {
    let langs = |set: &std::collections::BTreeSet<String>| {
        set.iter().cloned().collect::<Vec<_>>().join(", ")
    };
    if seen.is_empty() {
        return match lang {
            Some(name) => format!(
                "nothing to search: no {name} file is in scope. Widen path or glob, or omit lang."
            ),
            None => {
                "nothing to search: no file in scope has a language with an adapter.".to_owned()
            }
        };
    }
    if parsed.is_empty() {
        return format!(
            "`{pattern}` does not parse as a single node in {}, so nothing was searched.\n\n\
             {PATTERN_HELP}",
            langs(seen)
        );
    }
    format!(
        "no structural matches for `{pattern}` in {searched} {} of {}{}.",
        if searched == 1 { "file" } else { "files" },
        langs(parsed),
        skipped_note(skipped)
    )
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
                "glob": {"type": "string", "description": "Filter the scope by path pattern, as in find and grep."},
                "lang": {"type": "string", "description": "Restrict the search to files of this language. Detected per file when omitted."},
                "limit": {"type": "integer", "minimum": 1, "maximum": 200}
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: AstQueryArgs) -> Result<String, ToolError> {
        let root = scope(&self.0, args.path.as_deref())?;
        let want = requested_lang(args.lang.as_deref())?;
        let limit = args.limit.unwrap_or(50).min(200);
        // Scoped the same way `find` and `grep` are. A structural search that
        // could not be narrowed to `**/*.test.ts` while its two neighbours
        // could was a hole in a surface the model learns as one thing.
        let files = artist_ast::walk_paths(&[root], args.glob.as_deref());
        // A structural search is usually the prelude to a change, so every hit
        // is a place the model is about to act on. Line numbers here were the
        // last raw `path:line` left in the surface.
        let mut locator = crate::locate::Locator::new(&self.0);
        let mut out = String::new();
        let mut found = 0usize;
        let mut skipped = 0usize;

        // Compile once per language, not once per file. `run::search` compiles
        // on every call, and the vendored `search_with_pattern` exists to avoid
        // exactly that: "use this variant in loops where the same pattern is
        // applied to many files with the same language". A workspace walk is
        // ~400 files here, so this was ~400 redundant compilations.
        let mut compiled: std::collections::HashMap<String, Option<artist_ast::run::Pattern>> =
            std::collections::HashMap::new();

        // Three different reasons a structural search comes back empty, and
        // three different things to do about them. They used to render the
        // same sentence, so a malformed pattern was indistinguishable from a
        // correct one over a tree with no hits.
        let mut searched = 0usize;
        let mut langs_seen: std::collections::BTreeSet<String> = Default::default();
        let mut langs_parsed: std::collections::BTreeSet<String> = Default::default();

        for file in &files {
            if found >= limit {
                break;
            }
            let Some(lang) = lang_for(want, file) else {
                continue;
            };
            langs_seen.insert(format!("{lang:?}").to_lowercase());
            let pattern = compiled
                .entry(format!("{lang:?}"))
                .or_insert_with(|| compile_usable(&args.pattern, lang));
            // A pattern that does not parse in *this* language is normal in a
            // mixed tree — only report it if it parsed nowhere.
            let Some(pattern) = pattern.as_ref() else {
                continue;
            };
            langs_parsed.insert(format!("{lang:?}").to_lowercase());
            let Some(source) = readable_source(file) else {
                skipped += 1;
                continue;
            };
            searched += 1;
            let matches = match artist_ast::run::search_with_pattern(&source, lang, pattern) {
                Ok(m) => m,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };
            for m in matches {
                if found >= limit {
                    break;
                }
                let at = locator.locate(file, m.start_line as u32).await;
                out.push_str(&format!(
                    "{} {}\n",
                    at.render(),
                    m.matched_text.lines().next().unwrap_or("").trim()
                ));
                found += 1;
            }
        }

        if found == 0 {
            return Ok(empty_search(
                &args.pattern,
                args.lang.as_deref(),
                searched,
                &langs_seen,
                &langs_parsed,
                skipped,
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
    glob: Option<String>,
    lang: Option<String>,
    /// Write the previewed changes. Defaults to false: the model sees the diff
    /// first and opts in, rather than discovering the write after the fact.
    apply: Option<bool>,
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
             into the replacement. Previews by default and writes nothing: read the diff, then \
             re-run with apply=true to commit exactly those changes, or apply them selectively with \
             edit. An apply re-checks every file and refuses if anything moved since the preview. \
             {PATTERN_HELP}"
        )
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "replacement": {"type": "string", "description": "Empty string deletes the matched node."},
                "path": {"type": "string"},
                "glob": {"type": "string", "description": "Filter the scope by path pattern, as in find and grep."},
                "lang": {"type": "string", "description": "Restrict the rewrite to files of this language. Detected per file when omitted."},
                "apply": {
                    "type": "boolean",
                    "description": "Write the previewed changes. Defaults to false. Anchors for written files change, so re-read or re-outline before editing them."
                }
            },
            "required": ["pattern", "replacement"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: AstRewriteArgs) -> Result<String, ToolError> {
        let (plan, skipped) = self.plan(&args)?;

        if plan.is_empty() {
            return Ok(format!(
                "no structural matches for `{}`{} — nothing to rewrite.",
                args.pattern,
                skipped_note(skipped)
            ));
        }

        if !args.apply.unwrap_or(false) {
            let (diffs, shown) = render_plan(&self.0, &plan).await;
            return Ok(output::head(
                format!(
                    "PREVIEW — {} file(s) would change. Nothing has been written.\n\
                     Re-run with apply=true to write these exact changes, or apply them \
                     selectively with edit.\n{}\n{}",
                    plan.len(),
                    unshown_note(shown, plan.len(), false),
                    diffs
                ),
                output::OUTPUT_CAP,
            ));
        }

        self.apply(&args, plan).await
    }
}

/// One file's proposed rewrite.
struct PlannedRewrite {
    path: PathBuf,
    before: String,
    after: String,
}

/// Render a preview with an anchor gutter.
///
/// `DiffStyle::Explicit` and an empty `after` set, both deliberately. The
/// removal rows carry the *pre-edit* anchors — the handles the model is holding
/// right now — which is what lets it apply a subset of the preview through
/// `edit` instead of taking the whole rewrite. The additions deliberately get
/// no anchor: this text has not been written, so no handle has been issued for
/// it, and showing one would invite an edit against a handle that does not
/// exist.
/// Room the diffs may take, leaving the surrounding prose its own space.
///
/// Bounded here rather than by truncating the finished string: a diff cut at an
/// arbitrary byte tells the model output was lost but not *what*, and the thing
/// it needs to know is how many files it is approving unseen.
const PLAN_DIFF_BUDGET: usize = output::OUTPUT_CAP - 4096;

/// Render as many file diffs as fit, and say how many that was.
async fn render_plan(ws: &Workspace, plan: &[PlannedRewrite]) -> (String, usize) {
    let mut out = String::new();
    let mut shown = 0usize;
    for change in plan {
        let diff = similar::TextDiff::from_lines(&change.before, &change.after)
            .unified_diff()
            .context_radius(2)
            .to_string();
        let display = ws.display(&change.path);
        let before = ws
            .files
            .read_file(
                &ws.actor,
                hashline_tools::ReadFileRequest {
                    path: display.clone(),
                    start_line: 1,
                    max_lines: None,
                },
            )
            .await
            .map(|r| r.result.lines)
            .unwrap_or_default();
        let rendered =
            output::anchored_diff_styled(&diff, &before, &[], output::DiffStyle::Explicit);
        let section = format!("--- {display}\n{rendered}\n");
        // Always render one, so a single pathological file cannot produce a
        // preview with nothing in it.
        if shown > 0 && out.len() + section.len() > PLAN_DIFF_BUDGET {
            break;
        }
        out.push_str(&section);
        shown += 1;
    }
    (out, shown)
}

/// Say plainly when the diff shown is not the whole change.
///
/// The count of affected files already led the preview, so scope was never
/// hidden — but nothing said the unshown files are written too. A model that
/// approves what it can see, on output that stops without explaining itself,
/// is consenting to less than it is authorising.
fn unshown_note(shown: usize, total: usize, applied: bool) -> String {
    if shown >= total {
        return String::new();
    }
    let hidden = total - shown;
    if applied {
        format!("\nDiffs shown for {shown} of {total}; the other {hidden} were written too.\n")
    } else {
        format!(
            "\nDiffs shown for {shown} of {total}. apply=true writes all {total}, \
             including the {hidden} not shown below.\n"
        )
    }
}

impl AstRewriteTool {
    /// Compute the rewrite for every matching file without touching disk.
    fn plan(&self, args: &AstRewriteArgs) -> Result<(Vec<PlannedRewrite>, usize), ToolError> {
        let root = scope(&self.0, args.path.as_deref())?;
        let want = requested_lang(args.lang.as_deref())?;

        // Refuse before touching the tree when the replacement names a capture
        // the pattern never binds. Such a metavariable expands to nothing, so
        // `target($N)` -> `renamed($Z)` writes `renamed()` and drops the
        // argument at every site — a typo whose only symptom is a diff that
        // looks deliberate.
        let bound = artist_ast::run::metavariables(&args.pattern);
        let used = artist_ast::run::metavariables(&args.replacement);
        let unbound: Vec<&String> = used.difference(&bound).collect();
        if !unbound.is_empty() {
            let names = unbound
                .iter()
                .map(|n| format!("${n}"))
                .collect::<Vec<_>>()
                .join(", ");
            let known = if bound.is_empty() {
                "the pattern binds none".to_owned()
            } else {
                format!(
                    "the pattern binds {}",
                    bound
                        .iter()
                        .map(|n| format!("${n}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            return Err(ToolError::Message(format!(
                "the replacement uses {names}, which {known}. An unbound metavariable \
                 expands to nothing, so this would silently delete code at every match. \
                 Nothing was written."
            )));
        }
        // Scoped the same way `find` and `grep` are. A structural search that
        // could not be narrowed to `**/*.test.ts` while its two neighbours
        // could was a hole in a surface the model learns as one thing.
        let files = artist_ast::walk_paths(&[root], args.glob.as_deref());
        let mut plan = Vec::new();
        let mut skipped = 0usize;
        // Compile once per language rather than once per file — see the same
        // note in `ast_query`.
        let mut compiled: std::collections::HashMap<String, Option<artist_ast::run::Pattern>> =
            std::collections::HashMap::new();

        for file in &files {
            let Some(lang) = lang_for(want, file) else {
                continue;
            };
            let pattern = compiled
                .entry(format!("{lang:?}"))
                .or_insert_with(|| compile_usable(&args.pattern, lang));
            let Some(pattern) = pattern.as_ref() else {
                continue;
            };
            let Some(source) = readable_source(file) else {
                skipped += 1;
                continue;
            };
            let rewritten =
                artist_ast::run::rewrite_with_pattern(&source, lang, pattern, &args.replacement)
                    .map_err(ToolError::Message)?;
            let Some(rewritten) = rewritten else { continue };
            if rewritten == source {
                continue;
            }
            // A replacement template is arbitrary text, so nothing so far has
            // required the result to be code. Refuse the whole run rather than
            // the file: a codemod that lands on some paths and breaks others
            // leaves the tree in a state nobody asked for, and the model has
            // already been shown a preview that looked fine.
            let before = artist_ast::run::syntax_errors(&source, lang);
            let after = artist_ast::run::syntax_errors(&rewritten, lang);
            if after > before {
                return Err(ToolError::Message(format!(
                    "the replacement does not parse as {lang:?}: rewriting {} would introduce \
                     {} syntax error(s). Nothing was written — check the replacement is valid \
                     code in the target language.",
                    self.0.display(file),
                    after - before
                )));
            }
            plan.push(PlannedRewrite {
                path: file.clone(),
                before: source,
                after: rewritten,
            });
        }
        Ok((plan, skipped))
    }

    /// Re-plan, verify nothing moved, then write through the hashline ledger.
    async fn apply(
        &self,
        args: &AstRewriteArgs,
        preview: Vec<PlannedRewrite>,
    ) -> Result<String, ToolError> {
        // Stale-preview check. The preview the model reasoned about was computed
        // moments ago against a tree that may since have changed — by a
        // concurrent agent, a formatter, or a rebase. Re-plan and refuse if the
        // shape differs at all, rather than writing a change nobody approved.
        let (fresh, _) = self.plan(args)?;
        if fresh.len() != preview.len() {
            return Err(ToolError::Message(format!(
                "the working tree changed since the preview: {} file(s) matched then, {} now. \
                 Nothing was written — re-run the preview.",
                preview.len(),
                fresh.len()
            )));
        }
        for (a, b) in preview.iter().zip(fresh.iter()) {
            if a.path != b.path || a.before != b.before || a.after != b.after {
                return Err(ToolError::Message(format!(
                    "{} changed since the preview. Nothing was written — re-run the preview.",
                    self.0.display(&a.path)
                )));
            }
        }

        // Write through the coordinator so the anchor ledger is updated with the
        // new content. Writing the file directly would leave every anchor issued
        // for it pointing at text that no longer exists, with nothing to tell the
        // model its handles went stale.
        //
        // `ContentHash` makes each write a compare-and-swap against the exact
        // bytes the preview was computed from, so a file that moved between the
        // re-plan above and this loop fails at the coordinator rather than being
        // silently overwritten.
        let mut written = Vec::new();
        for change in &fresh {
            let display = self.0.display(&change.path);
            self.0
                .files
                .write_file(
                    &self.0.actor,
                    display.clone(),
                    change.after.clone(),
                    hashline_tools::WriteCondition::ContentHash {
                        hash: hashline_tools::content_hash(change.before.as_bytes()),
                    },
                )
                .await?;
            self.0.refresh_index(&change.path);
            written.push(display);
        }

        // A codemod is the operation most likely to move the architecture —
        // it is the only one that edits many files at once — and it was the
        // one path that reported nothing. The delta absorbs what it reports,
        // so touching twenty files in one crate still yields one note.
        let mut aftermath = String::new();
        for change in &fresh {
            if let Some(note) = crate::annotate::after_commit(&self.0, &change.path, &[]).await {
                aftermath.push_str(&note);
            }
        }

        let (diffs, shown) = render_plan(&self.0, &fresh).await;
        Ok(output::head(
            format!(
                "Applied to {} file(s). Anchors for these files have changed — re-read or \
                 re-outline before editing them.\n\n{}\n{}\n{}{aftermath}",
                written.len(),
                written
                    .iter()
                    .map(|p| format!("  {p}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                unshown_note(shown, fresh.len(), true),
                diffs
            ),
            output::OUTPUT_CAP,
        ))
    }
}

/// Qualify an empty result by what was never looked at.
///
/// "No matches" is a claim about the project; what these tools can honestly
/// report is a claim about the files they read. Oversized blobs, files that
/// fail to parse, and languages with no grammar are all passed over silently,
/// and a model told there are no matches will stop looking. Naming the gap
/// turns a false conclusion into a next step.
fn skipped_note(skipped: usize) -> String {
    match skipped {
        0 => String::new(),
        1 => " (1 file was skipped: too large, or it did not parse)".to_owned(),
        n => format!(" ({n} files were skipped: too large, or they did not parse)"),
    }
}

/// Read a file for pattern matching, or skip it if it is too big to be source.
///
/// The walker filters by extension alone, so a minified bundle, a generated
/// parser table or a checked-in data blob under a `.js` or `.py` name is a
/// candidate as far as it is concerned. `artist-ast` exports the cap for
/// exactly this and applies it in its own CLI; both tools here were reading
/// whatever they were handed, in a loop, across an entire scope.
fn readable_source(file: &Path) -> Option<String> {
    let size = std::fs::metadata(file).ok()?.len();
    if size > artist_ast::run::RUN_MAX_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(file).ok()
}

/// Resolve the language for a file, honouring an explicit override.
/// Resolve `lang` once, refusing a name no grammar answers to.
///
/// Previously an unrecognised name silently matched nothing, which reads
/// exactly like a correct query over a tree with no hits.
fn requested_lang(lang: Option<&str>) -> Result<Option<artist_ast::run::SupportLang>, ToolError> {
    let Some(name) = lang else { return Ok(None) };
    artist_ast::run::cli::parse_lang(name)
        .map(Some)
        .ok_or_else(|| {
            ToolError::Message(format!(
                "unknown language `{name}`. Omit lang to detect it from each file's extension."
            ))
        })
}

/// The language to parse `file` as, or `None` to skip it.
///
/// An explicit `lang` *narrows* to files of that language. It used to force
/// the grammar onto every file in scope instead, so `lang=rust` over a mixed
/// tree parsed the Markdown as Rust and reported structural matches inside
/// prose — a request to narrow the search returned more results than no
/// request at all.
fn lang_for(
    explicit: Option<artist_ast::run::SupportLang>,
    file: &Path,
) -> Option<artist_ast::run::SupportLang> {
    let detected = artist_ast::run::detect_lang(file);
    match explicit {
        Some(want) => detected.filter(|found| *found == want),
        None => detected,
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
        let root = if args.path.is_some() {
            analysis_root(&target)
        } else {
            artist_ast::project_root::find_root_for(&target)
                .map_err(|e| ToolError::Message(format!("project root: {e}")))?
        };
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
        // Rendered here rather than by `report_text`, so every location comes
        // back as an anchor the model can edit through instead of a line number
        // that the next insert invalidates.
        let reports =
            artist_ast::impact::report(&args.symbol, &root, &opts).map_err(ToolError::Message)?;
        let mut locator = crate::locate::Locator::new(&self.0);
        let mut out = String::new();
        for report in &reports {
            let target = locator
                .locate(Path::new(&report.target_file), report.target_line)
                .await;
            out.push_str(&format!(
                "{} {} ({})\n",
                report.target_kind,
                report
                    .target_qn
                    .split("::")
                    .last()
                    .unwrap_or(&report.target_qn),
                target.render()
            ));
            let Some(sections) = &report.sections else {
                continue;
            };
            for section in sections {
                if section.entries.is_empty() {
                    continue;
                }
                out.push_str(&format!("\n  {}\n", section.title));
                for entry in &section.entries {
                    let at = locator.locate(Path::new(&entry.file), entry.line).await;
                    let confidence = entry
                        .confidence
                        .as_deref()
                        .map(|c| format!(" {c}"))
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "    {} {} ({}){}\n",
                        entry.kind,
                        entry.qn.split("::").last().unwrap_or(&entry.qn),
                        at.render(),
                        confidence
                    ));
                }
            }
        }
        Ok(output::head(out, output::OUTPUT_CAP))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hidden_workspace() -> (tempfile::TempDir, tempfile::TempDir, Workspace) {
        let project = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let src = project.path().join(".hidden/src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "pub mod a;\npub mod b;\npub fn double(value: i32) -> i32 { value * 2 }\npub fn quadruple(value: i32) -> i32 { double(double(value)) }\n",
        )
        .unwrap();
        std::fs::write(
            src.join("a.rs"),
            "use crate::b::from_b;\npub fn from_a(value: i32) -> i32 { from_b(value) + 1 }\n",
        )
        .unwrap();
        std::fs::write(
            src.join("b.rs"),
            "use crate::a::from_a;\npub fn from_b(value: i32) -> i32 { if value == 0 { 0 } else { from_a(value - 1) } }\n",
        )
        .unwrap();
        let workspace = Workspace::open(project.path(), state.path(), "code-test").unwrap();
        (project, state, workspace)
    }

    #[tokio::test]
    async fn explicit_hidden_paths_are_analyzed_without_widening_or_disappearing() {
        let (_project, _state, workspace) = hidden_workspace();

        let deps = CodeDepsTool(workspace.clone())
            .call(CodeDepsArgs {
                file: ".hidden/src/a.rs".into(),
                direction: None,
                depth: Some(2),
                limit: Some(20),
                exclude_tests: None,
            })
            .await
            .unwrap();
        assert!(deps.contains("b.rs"), "{deps}");

        let cycles = CodeCyclesTool(workspace.clone())
            .call(CodeCyclesArgs {
                path: Some(".hidden/src".into()),
                min_size: Some(2),
            })
            .await
            .unwrap();
        assert!(cycles.contains("cycle of 2 files"), "{cycles}");
        assert!(
            cycles.contains("a.rs") && cycles.contains("b.rs"),
            "{cycles}"
        );

        let trace = CodeTraceTool(workspace)
            .call(CodeTraceArgs {
                from: "quadruple".into(),
                to: "double".into(),
                path: Some(".hidden/src".into()),
                depth: Some(4),
            })
            .await
            .unwrap();
        assert!(
            trace.contains("quadruple") && trace.contains("double"),
            "{trace}"
        );
        assert!(!trace.contains("no callable symbol matches"), "{trace}");
    }
}
