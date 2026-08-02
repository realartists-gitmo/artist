//! Differential probes: does the output move when the input moves?
//!
//! The claims lint reads what output *says about itself* — "the whole file",
//! "3 of 12" — and checks the claim against what the code did. It cannot catch
//! a tool that quietly does part of the job, because a partial result makes no
//! false claim. It shows you a correct diff of an incomplete change.
//!
//! That is exactly how `ast_rewrite` shipped rewriting the first match in each
//! file: the diff was accurate, the write was real, and 199 of 200 call sites
//! were silently untouched. The bug was found by noticing that scaling the
//! input twentyfold did not move the output at all.
//!
//! So these probes ask the other question. Hold the request fixed, scale the
//! part of the input the tool is supposed to report on, and require the
//! measurement to track it. A tool whose output is invariant under an input it
//! claims to cover is broken no matter how honest its prose is.
//!
//! Each probe names the *relation* it is defending, not the implementation, so
//! a rewrite of the internals cannot quietly satisfy it.

use artist_tools::{ToolBundle, Workspace};
use rig_core::tool::{IntoToolOutput, PortableTool};
use serde_json::json;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn bundle_with(files: &[(&str, String)]) -> (tempfile::TempDir, tempfile::TempDir, ToolBundle) {
    let root = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let target = root.path().join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, content).unwrap();
    }
    let state = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(root.path(), state.path(), "test").unwrap();
    (root, state, ToolBundle::new(workspace))
}

async fn call<T: PortableTool>(tool: &T, value: serde_json::Value) -> String
where
    T::Error: std::fmt::Debug,
{
    let args = serde_json::from_value(value).unwrap();
    tool.call(args)
        .await
        .unwrap()
        .into_tool_output()
        .unwrap()
        .render()
}

/// Require a measurement to climb with the input it is supposed to track.
///
/// The failure message has to explain the *class* of bug, because the symptom
/// — two equal numbers — reads like a bad fixture, and that misreading is what
/// kept the `ast_rewrite` bug alive through a green suite.
#[track_caller]
fn tracks(what: &str, scaled: &str, samples: &[(usize, usize)]) {
    for pair in samples.windows(2) {
        let (from, before) = pair[0];
        let (to, after) = pair[1];
        assert!(
            after > before,
            "{what}\n  {scaled} went {from} -> {to}, but the measurement stayed \
             {before} -> {after}.\n  Output that does not move under an input it \
             reports on is a tool doing part of the job and calling it done.\n  \
             full ladder: {samples:?}"
        );
    }
}

/// Require two runs that must differ to actually differ.
#[track_caller]
fn differs(what: &str, a: &str, b: &str) {
    assert_ne!(
        a, b,
        "{what}\n  the output is byte-identical across a change it must reflect."
    );
}

/// A ladder wide enough that an off-by-one cannot pass it, and short enough to
/// stay far inside every output cap — a cap flattening the curve would be a
/// legitimate reason for growth to stop, and would make the probe a liar.
const LADDER: [usize; 3] = [1, 4, 16];

/// How many times a token appears across the whole output.
fn occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

fn rows_containing(out: &str, needle: &str) -> usize {
    out.lines().filter(|l| l.contains(needle)).count()
}

/// A grep hit line is `path:line:col: content`; context is `path-line- content`.
fn grep_hits(out: &str) -> usize {
    out.lines()
        .filter(|line| {
            let mut parts = line.rsplitn(4, ':');
            // content, col, line, path — from the right, so a Windows drive
            // letter or a colon in the path cannot shift the fields.
            parts.next().is_some()
                && parts
                    .next()
                    .is_some_and(|col| col.trim().parse::<u32>().is_ok())
                && parts
                    .next()
                    .is_some_and(|num| num.trim().parse::<u32>().is_ok())
        })
        .count()
}

/// `n` Rust functions, each containing one call to `target`.
fn calls(n: usize) -> String {
    (0..n)
        .map(|i| format!("fn f{i}() -> u32 {{ target(1) }}\n"))
        .collect()
}

// ---------------------------------------------------------------------------
// ast_rewrite — the probe the whole file is named after
// ---------------------------------------------------------------------------

/// The regression guard for the first-match-only bug, stated as a relation:
/// twice the call sites must produce twice the rewritten lines in the preview.
///
/// Under the old `Root::replace` this measured 2 at every rung.
#[tokio::test]
async fn a_rewrite_preview_grows_with_the_number_of_call_sites() {
    let mut samples = Vec::new();
    for n in LADDER {
        let body = calls(n);
        let (_r, _s, tools) = bundle_with(&[
            ("src/a.rs", body.clone()),
            ("src/b.rs", body.clone()),
        ]);
        let out = call(
            &tools.ast_rewrite,
            json!({"pattern": "target($N)", "replacement": "renamed($N)"}),
        )
        .await;
        samples.push((n, occurrences(&out, "renamed(")));
    }
    tracks(
        "ast_rewrite previewed the same number of rewrites regardless of how many sites matched",
        "call sites per file",
        &samples,
    );
}

/// The disk-level version of the same relation, which is the one that matters:
/// after an apply, nothing that matched the pattern may still be there.
///
/// The preview probe above checks the report; this checks the world. A tool can
/// render a complete diff and still write less than it drew.
#[tokio::test]
async fn applying_a_rewrite_leaves_no_matching_site_behind() {
    let body = calls(16);
    let (root, _s, tools) = bundle_with(&[
        ("src/a.rs", body.clone()),
        ("src/b.rs", body.clone()),
    ]);
    call(
        &tools.ast_rewrite,
        json!({
            "pattern": "target($N)",
            "replacement": "renamed($N)",
            "apply": true
        }),
    )
    .await;

    for file in ["src/a.rs", "src/b.rs"] {
        let after = std::fs::read_to_string(root.path().join(file)).unwrap();
        assert_eq!(
            occurrences(&after, "target("),
            0,
            "{file} still holds sites the rewrite said it applied:\n{after}"
        );
        assert_eq!(occurrences(&after, "renamed("), 16, "{file}:\n{after}");
    }
}

// ---------------------------------------------------------------------------
// ast_query
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_structural_query_grows_with_the_number_of_matches() {
    let mut samples = Vec::new();
    for n in LADDER {
        let body = calls(n);
        let (_r, _s, tools) = bundle_with(&[("src/a.rs", body)]);
        let out = call(&tools.ast_query, json!({"pattern": "target($N)"})).await;
        samples.push((n, rows_containing(&out, "target(1)")));
    }
    tracks(
        "ast_query reported the same number of matches regardless of how many existed",
        "matching sites",
        &samples,
    );
}

/// A limit that does not bind is decoration; a limit that binds at a constant
/// is a cap pretending to be a parameter. Both fail this.
#[tokio::test]
async fn a_query_limit_binds_and_responds() {
    let (_r, _s, tools) = bundle_with(&[("src/a.rs", calls(40))]);
    let mut samples = Vec::new();
    for limit in [2usize, 7, 23] {
        let out = call(
            &tools.ast_query,
            json!({"pattern": "target($N)", "limit": limit}),
        )
        .await;
        let rows = rows_containing(&out, "target(1)");
        assert!(
            rows <= limit,
            "ast_query returned {rows} rows for limit={limit}"
        );
        samples.push((limit, rows));
    }
    tracks(
        "ast_query returned the same number of rows regardless of the limit asked for",
        "limit",
        &samples,
    );
}

// ---------------------------------------------------------------------------
// grep
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grep_grows_with_the_number_of_matching_lines() {
    let mut samples = Vec::new();
    for n in LADDER {
        let body: String = (0..n).map(|i| format!("let x{i} = needleword;\n")).collect();
        let (_r, _s, tools) = bundle_with(&[("src/a.rs", body)]);
        let out = call(&tools.grep, json!({"query": "needleword"})).await;
        samples.push((n, grep_hits(&out)));
    }
    tracks(
        "grep reported the same number of hits regardless of how many lines matched",
        "matching lines",
        &samples,
    );
}

#[tokio::test]
async fn grep_context_widens_what_comes_back() {
    let body: String = (0..20)
        .map(|i| {
            if i == 10 {
                "let x = needleword;\n".to_owned()
            } else {
                format!("let y{i} = 0;\n")
            }
        })
        .collect();
    let mut samples = Vec::new();
    for context in [0usize, 2, 5] {
        let (_r, _s, tools) = bundle_with(&[("src/a.rs", body.clone())]);
        let out = call(
            &tools.grep,
            json!({"query": "needleword", "context": context}),
        )
        .await;
        samples.push((context, out.lines().count()));
    }
    tracks(
        "grep returned the same output regardless of the context requested",
        "context radius",
        &samples,
    );
}

// ---------------------------------------------------------------------------
// find
// ---------------------------------------------------------------------------

#[tokio::test]
async fn find_grows_with_the_number_of_matching_paths() {
    let mut samples = Vec::new();
    for n in LADDER {
        let files: Vec<(String, String)> = (0..n)
            .map(|i| (format!("src/zeppelin{i}.rs", ), "fn a() {}\n".to_owned()))
            .collect();
        let borrowed: Vec<(&str, String)> = files
            .iter()
            .map(|(p, c)| (p.as_str(), c.clone()))
            .collect();
        let (_r, _s, tools) = bundle_with(&borrowed);
        let out = call(&tools.find, json!({"query": "zeppelin"})).await;
        samples.push((n, rows_containing(&out, "zeppelin")));
    }
    tracks(
        "find returned the same number of paths regardless of how many matched",
        "matching files",
        &samples,
    );
}

// ---------------------------------------------------------------------------
// read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_read_limit_bounds_the_window_and_responds_to_it() {
    let body: String = (0..60).map(|i| format!("let line{i} = 0;\n")).collect();
    let mut samples = Vec::new();
    for limit in [3usize, 11, 29] {
        let (_r, _s, tools) = bundle_with(&[("src/a.rs", body.clone())]);
        let out = call(
            &tools.read,
            json!({"path": "src/a.rs", "limit": limit}),
        )
        .await;
        let shown = rows_containing(&out, "let line");
        assert!(shown <= limit, "read returned {shown} lines for limit={limit}");
        samples.push((limit, shown));
    }
    tracks(
        "read returned the same number of lines regardless of the limit asked for",
        "limit",
        &samples,
    );
}

#[tokio::test]
async fn a_read_offset_moves_the_window() {
    let body: String = (0..60).map(|i| format!("let line{i} = 0;\n")).collect();
    let (_r, _s, tools) = bundle_with(&[("src/a.rs", body)]);
    let head = call(&tools.read, json!({"path": "src/a.rs", "limit": 5})).await;
    let deep = call(
        &tools.read,
        json!({"path": "src/a.rs", "offset": 30, "limit": 5}),
    )
    .await;
    differs("read ignored offset", &head, &deep);
    assert!(
        deep.contains("let line30") && !deep.contains("let line0 "),
        "offset=30 did not land on line 30:\n{deep}"
    );
}

// ---------------------------------------------------------------------------
// code_map / code_surface
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_outline_grows_with_the_number_of_declarations() {
    let mut samples = Vec::new();
    for n in LADDER {
        let body: String = (0..n).map(|i| format!("pub fn decl{i}() {{}}\n")).collect();
        let (_r, _s, tools) = bundle_with(&[("src/a.rs", body)]);
        let out = call(&tools.code_map, json!({"path": "src/a.rs"})).await;
        samples.push((n, rows_containing(&out, "fn decl")));
    }
    tracks(
        "code_map showed the same number of declarations regardless of how many the file had",
        "declarations",
        &samples,
    );
}

#[tokio::test]
async fn the_public_surface_grows_with_the_number_of_exports() {
    let mut samples = Vec::new();
    for n in LADDER {
        let body: String = (0..n).map(|i| format!("pub fn export{i}() {{}}\n")).collect();
        let (_r, _s, tools) = bundle_with(&[("src/lib.rs", body)]);
        let out = call(&tools.code_surface, json!({"path": "src"})).await;
        samples.push((n, rows_containing(&out, "export")));
    }
    tracks(
        "code_surface reported the same API regardless of how much was exported",
        "exported items",
        &samples,
    );
}

// ---------------------------------------------------------------------------
// code_impact
// ---------------------------------------------------------------------------

/// Blast radius is the one answer where being quietly partial is worst: a model
/// that is told two things call this, when twelve do, changes it confidently.
#[tokio::test]
async fn blast_radius_grows_with_the_number_of_callers() {
    let mut samples = Vec::new();
    for n in LADDER {
        let mut files = vec![(
            "src/lib.rs".to_owned(),
            "pub fn target() -> u32 { 1 }\n".to_owned(),
        )];
        for i in 0..n {
            files.push((
                format!("src/caller{i}.rs"),
                format!("use crate::target;\npub fn c{i}() -> u32 {{ target() }}\n"),
            ));
        }
        let borrowed: Vec<(&str, String)> = files
            .iter()
            .map(|(p, c)| (p.as_str(), c.clone()))
            .collect();
        let (_r, _s, tools) = bundle_with(&borrowed);
        let out = call(&tools.code_impact, json!({"symbol": "target"})).await;
        samples.push((n, rows_containing(&out, "caller")));
    }
    tracks(
        "code_impact reported the same blast radius regardless of how many callers existed",
        "callers",
        &samples,
    );
}

// ===========================================================================
// The systematic pass: every advertised parameter must bind to something
// ===========================================================================
//
// The probes above are hand-picked, and hand-picked coverage has the same
// blind spot as the claims lint — it finds what someone thought to look for.
// `code_surface` advertises `tree` and `includeChain` with descriptions of
// what they do, deserializes both, and reads neither; no ladder above would
// ever have touched them.
//
// So this pass is driven off each tool's own `parameters()` schema. Every
// advertised property must appear in the table below with either a live
// differential probe or a written reason. Adding a knob to a schema without
// accounting for it fails the suite, which is the only version of this that
// stays true after I stop looking at it.

use serde_json::Value;

fn name_of<T: PortableTool>(_: &T) -> &'static str {
    T::NAME
}

fn props_of<T: PortableTool>(tool: &T) -> Vec<String> {
    tool.parameters()["properties"]
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

/// Every `(tool, parameter)` the agent is told it may pass.
fn advertised(tools: &ToolBundle) -> Vec<(&'static str, String)> {
    macro_rules! surface {
        ($($field:ident),* $(,)?) => {
            vec![$((name_of(&tools.$field), props_of(&tools.$field))),*]
        };
    }
    surface![
        bash,
        read,
        find,
        grep,
        edit,
        write,
        code_map,
        code_show,
        code_surface,
        code_implements,
        code_deps,
        code_cycles,
        code_trace,
        code_impact,
        ast_query,
        ast_rewrite,
    ]
    .into_iter()
    .flat_map(|(tool, params)| params.into_iter().map(move |p| (tool, p)))
    .collect()
}

/// Invoke by name, rendering errors as output.
///
/// An error is a legitimate way for a parameter to bind — refusing an invalid
/// value *is* responding to it — so the driver compares whatever comes back
/// rather than insisting on success.
async fn invoke(tools: &ToolBundle, tool: &str, args: Value) -> String {
    async fn go<T: PortableTool>(tool: &T, args: Value) -> String
    where
        T::Error: std::fmt::Display,
    {
        match serde_json::from_value(args) {
            Err(e) => format!("<rejected: {e}>"),
            Ok(parsed) => match tool.call(parsed).await {
                Err(e) => format!("<error: {e}>"),
                Ok(out) => out
                    .into_tool_output()
                    .map(|o| o.render())
                    .unwrap_or_else(|_| "<unrenderable>".into()),
            },
        }
    }
    match tool {
        "bash" => go(&tools.bash, args).await,
        "read" => go(&tools.read, args).await,
        "find" => go(&tools.find, args).await,
        "grep" => go(&tools.grep, args).await,
        "edit" => go(&tools.edit, args).await,
        "write" => go(&tools.write, args).await,
        "code_map" => go(&tools.code_map, args).await,
        "code_show" => go(&tools.code_show, args).await,
        "code_surface" => go(&tools.code_surface, args).await,
        "code_implements" => go(&tools.code_implements, args).await,
        "code_deps" => go(&tools.code_deps, args).await,
        "code_cycles" => go(&tools.code_cycles, args).await,
        "code_trace" => go(&tools.code_trace, args).await,
        "code_impact" => go(&tools.code_impact, args).await,
        "ast_query" => go(&tools.ast_query, args).await,
        "ast_rewrite" => go(&tools.ast_rewrite, args).await,
        other => panic!("no dispatch for {other} — add it when the bundle grows"),
    }
}

/// How we know a parameter reaches the implementation.
enum Binds {
    /// Two calls that must not produce the same output.
    Moves(Fixture, Value, Value),
    /// Bound by a named test elsewhere that this driver cannot express —
    /// typically because it needs multi-call setup.
    Covered(&'static str),
}

struct Knob {
    tool: &'static str,
    param: &'static str,
    binds: Binds,
}

fn moves(tool: &'static str, param: &'static str, f: Fixture, a: Value, b: Value) -> Knob {
    Knob { tool, param, binds: Binds::Moves(f, a, b) }
}

fn covered(tool: &'static str, param: &'static str, by: &'static str) -> Knob {
    Knob { tool, param, binds: Binds::Covered(by) }
}

// --- fixtures --------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Fixture {
    /// One small crate carrying every shape the graph tools need: a trait and
    /// an impl, a three-deep call chain, an import cycle, a test target, and
    /// both a private and a public declaration.
    Crate,
    /// Many similarly-named files across two directories, for the scoping and
    /// limiting knobs.
    Wide,
    /// A subclass chain. Rust cannot express one — `impl Trait for Type` gives
    /// a single level and nothing inherits from a struct — so `direct` has
    /// nothing to be the opposite of in a Rust-only fixture. It binds here.
    Inherited,
}

fn files_for(fixture: Fixture) -> Vec<(&'static str, String)> {
    match fixture {
        Fixture::Crate => vec![
            ("Cargo.toml", manifest("probe")),
            ("src/lib.rs", CRATE_LIB.to_owned()),
            ("src/other.rs", "use crate::ring_a::a_fn;\n\npub fn consumer() -> u32 {\n    a_fn()\n}\n".to_owned()),
            ("src/deep.rs", "use crate::other::consumer;\n\npub fn deep() -> u32 {\n    consumer()\n}\n".to_owned()),
            ("src/ring_a.rs", "use crate::ring_b::b_fn;\npub fn a_fn() -> u32 { b_fn() }\n".to_owned()),
            ("src/ring_b.rs", "use crate::ring_a::a_fn;\npub fn b_fn() -> u32 { 1 }\n".to_owned()),
            // `_test` suffix, so `is_test_file` classifies it while the import
            // still produces an ordinary file edge for `excludeTests` to drop.
            ("src/ring_test.rs", "use crate::ring_a::a_fn;\npub fn checks() -> u32 { a_fn() }\n".to_owned()),
            // A second crate root. Several tools take `path` only to decide
            // *which project* to analyse, not to narrow the search within one —
            // a distinction no single-root fixture can see.
            ("sub/Cargo.toml", manifest("probe_sub")),
            ("sub/src/lib.rs", "pub fn only_here() -> u32 { 1 }\n".to_owned()),
        ],
        Fixture::Inherited => vec![
            ("pyproject.toml", "[project]\nname = \"probe\"\nversion = \"0.1.0\"\n".to_owned()),
            (
                "shapes.py",
                "class Base:\n    pass\n\n\nclass Middle(Base):\n    pass\n\n\nclass Leaf(Middle):\n    pass\n".to_owned(),
            ),
        ],
        Fixture::Wide => {
            let mut files: Vec<(&'static str, String)> = Vec::new();
            for (i, path) in WIDE_RS.iter().enumerate() {
                files.push((path, format!("pub fn z{i}() -> u32 {{ target(1) }}\n")));
            }
            for path in WIDE_MD {
                files.push((path, "# zeppelin\n\nprose about target(1)\n".to_owned()));
            }
            files
        }
    }
}

fn manifest(name: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
}

const CRATE_LIB: &str = r#"pub mod deep;
pub mod other;
pub mod ring_a;
pub mod ring_b;
pub mod ring_test;

/// A re-export, so at least one surface entry has a chain for `includeChain`
/// to show. Without one the knob has nothing to differ on and the probe would
/// pass an unwired parameter.
pub use ring_a::a_fn;

pub trait Greeter {
    fn greet(&self) -> u32;
}

/// `Derived` extends `Greeter` and `Thing` implements both, so a transitive
/// search finds strictly more than a direct one — which is the only way
/// `direct` can be shown to bind.
pub trait Derived: Greeter {}

pub struct Thing {
    pub a: u32,
}

impl Greeter for Thing {
    fn greet(&self) -> u32 {
        self.a
    }
}

impl Derived for Thing {}

pub fn top() -> u32 {
    middle()
}

fn middle() -> u32 {
    bottom()
}

fn bottom() -> u32 {
    7
}

pub fn target(n: u32) -> u32 {
    n
}

/// Holds both `abc` and the literal `a.c` so a regex match and a plain-text
/// match of the same query return different things.
pub fn needleword() -> u32 {
    let abc = 1;
    // a.c
    abc
}
"#;

const WIDE_RS: [&str; 6] = [
    "src/zeppelin0.rs",
    "src/zeppelin1.rs",
    "src/zeppelin2.rs",
    "src/zeppelin3.rs",
    "src/zeppelin4.rs",
    "src/zeppelin5.rs",
];
const WIDE_MD: [&str; 3] = ["docs/zeppelin0.md", "docs/zeppelin1.md", "docs/zeppelin2.md"];

// --- the table -------------------------------------------------------------

fn knobs() -> Vec<Knob> {
    use Fixture::{Crate, Inherited, Wide};
    vec![
        // bash
        moves("bash", "mode", Crate, json!({"mode": "list"}), json!({"mode": "exec", "command": "echo alpha"})),
        moves("bash", "command", Crate, json!({"command": "echo alpha"}), json!({"command": "echo beta"})),
        moves("bash", "background", Crate, json!({"command": "echo alpha"}), json!({"command": "echo alpha", "background": true})),
        moves("bash", "sessionId", Crate, json!({"mode": "start", "sessionId": "probe-one", "command": "cat"}), json!({"mode": "start", "sessionId": "probe-two", "command": "cat"})),
        moves("bash", "timeout", Crate, json!({"command": "sleep 2", "timeout": 1}), json!({"command": "sleep 2", "timeout": 30})),
        moves("bash", "maxBytes", Crate, json!({"command": "seq 1 4000", "maxBytes": 64}), json!({"command": "seq 1 4000", "maxBytes": 20000})),
        moves("bash", "cwd", Crate, json!({"command": "pwd"}), json!({"command": "pwd", "cwd": "src"})),
        moves("bash", "env", Crate, json!({"command": "echo $PROBE", "env": {"PROBE": "alpha"}}), json!({"command": "echo $PROBE", "env": {"PROBE": "beta"}})),
        covered("bash", "input", "tools.rs::bash_exec_and_persistent_session_work_from_root — needs a live session to send into"),
        covered("bash", "waitMs", "tools.rs::bash_exec_and_persistent_session_work_from_root — only meaningful against a live session"),
        covered("bash", "signal", "tools.rs::bash_exec_and_persistent_session_work_from_root — only meaningful against a running session"),
        // read
        moves("read", "path", Crate, json!({"path": "src/lib.rs"}), json!({"path": "src/other.rs"})),
        moves("read", "offset", Crate, json!({"path": "src/lib.rs", "limit": 5}), json!({"path": "src/lib.rs", "offset": 20, "limit": 5})),
        moves("read", "limit", Crate, json!({"path": "src/lib.rs", "limit": 3}), json!({"path": "src/lib.rs", "limit": 25})),
        // find
        moves("find", "query", Wide, json!({"query": "zeppelin"}), json!({"query": "nothingmatchesthis"})),
        moves("find", "path", Wide, json!({"query": "zeppelin", "path": "src"}), json!({"query": "zeppelin", "path": "docs"})),
        moves("find", "glob", Wide, json!({"query": "zeppelin", "glob": "**/*.rs"}), json!({"query": "zeppelin", "glob": "**/*.md"})),
        moves("find", "limit", Wide, json!({"query": "zeppelin", "limit": 1}), json!({"query": "zeppelin", "limit": 50})),
        // grep
        moves("grep", "query", Crate, json!({"query": "needleword"}), json!({"query": "Greeter"})),
        moves("grep", "path", Crate, json!({"query": "target", "path": "src/lib.rs"}), json!({"query": "target", "path": "src/other.rs"})),
        moves("grep", "glob", Wide, json!({"query": "target", "glob": "**/*.rs"}), json!({"query": "target", "glob": "**/*.md"})),
        moves("grep", "match", Crate, json!({"query": "a.c", "match": "regex"}), json!({"query": "a.c", "match": "literal"})),
        moves("grep", "case", Crate, json!({"query": "NEEDLEWORD", "case": "sensitive"}), json!({"query": "NEEDLEWORD", "case": "insensitive"})),
        moves("grep", "context", Crate, json!({"query": "needleword", "context": 0}), json!({"query": "needleword", "context": 5})),
        moves("grep", "limit", Wide, json!({"query": "target", "limit": 1}), json!({"query": "target", "limit": 50})),
        // edit — both parameters take mnemonic anchors, which only exist after
        // a read of the same file, so a two-call driver cannot construct them.
        covered("edit", "path", "tools.rs::all_file_tools_accept_external_absolute_paths"),
        covered("edit", "replacements", "tools.rs::reads_then_edits_with_mnemonic_anchor"),
        // write
        moves("write", "path", Crate, json!({"path": "src/new_a.rs", "content": "fn a() {}\n"}), json!({"path": "src/new_b.rs", "content": "fn a() {}\n"})),
        moves("write", "content", Crate, json!({"path": "src/new.rs", "content": "fn alpha() {}\n"}), json!({"path": "src/new.rs", "content": "fn beta() {}\n"})),
        // code_map
        moves("code_map", "path", Crate, json!({"path": "src/lib.rs"}), json!({"path": "src/other.rs"})),
        moves("code_map", "budget", Crate, json!({"path": "src/lib.rs", "budget": 2}), json!({"path": "src/lib.rs", "budget": 200})),
        moves("code_map", "includePrivate", Crate, json!({"path": "src/lib.rs", "includePrivate": false}), json!({"path": "src/lib.rs", "includePrivate": true})),
        // code_show
        moves("code_show", "path", Crate, json!({"path": "src/lib.rs", "symbol": "top"}), json!({"path": "src/other.rs", "symbol": "top"})),
        moves("code_show", "symbol", Crate, json!({"path": "src/lib.rs", "symbol": "top"}), json!({"path": "src/lib.rs", "symbol": "middle"})),
        // code_surface
        moves("code_surface", "path", Crate, json!({"path": "src"}), json!({"path": "src/other.rs"})),
        moves("code_surface", "tree", Crate, json!({"path": ".", "tree": false}), json!({"path": ".", "tree": true})),
        // Scoped to the crate root, not `src`: a re-export chain only exists
        // when the Rust resolver runs, and it is selected by finding a manifest
        // at the scope. Pointed at `src` this silently drops to the fallback
        // resolver, which sets every chain empty — so the probe would have been
        // measuring the fixture, not the knob.
        moves("code_surface", "includeChain", Crate, json!({"path": ".", "includeChain": false}), json!({"path": ".", "includeChain": true})),
        // code_implements
        moves("code_implements", "target", Crate, json!({"target": "Greeter"}), json!({"target": "NoSuchTrait"})),
        moves("code_implements", "path", Crate, json!({"target": "Greeter", "path": "src"}), json!({"target": "Greeter", "path": "sub/src"})),
        moves("code_implements", "direct", Inherited, json!({"target": "Base", "direct": true}), json!({"target": "Base", "direct": false})),
        // code_deps
        moves("code_deps", "file", Crate, json!({"file": "src/other.rs"}), json!({"file": "src/deep.rs"})),
        moves("code_deps", "direction", Crate, json!({"file": "src/other.rs", "direction": "forward"}), json!({"file": "src/other.rs", "direction": "reverse"})),
        moves("code_deps", "depth", Crate, json!({"file": "src/deep.rs", "direction": "forward", "depth": 1}), json!({"file": "src/deep.rs", "direction": "forward", "depth": 5})),
        moves("code_deps", "limit", Crate, json!({"file": "src/deep.rs", "direction": "forward", "depth": 5, "limit": 1}), json!({"file": "src/deep.rs", "direction": "forward", "depth": 5, "limit": 50})),
        moves("code_deps", "excludeTests", Crate, json!({"file": "src/ring_a.rs", "direction": "reverse", "excludeTests": false}), json!({"file": "src/ring_a.rs", "direction": "reverse", "excludeTests": true})),
        // code_cycles
        moves("code_cycles", "path", Crate, json!({"path": "src"}), json!({"path": "sub/src"})),
        moves("code_cycles", "minSize", Crate, json!({"path": "src", "minSize": 2}), json!({"path": "src", "minSize": 40})),
        // code_trace
        moves("code_trace", "from", Crate, json!({"from": "top", "to": "bottom", "depth": 5}), json!({"from": "middle", "to": "bottom", "depth": 5})),
        moves("code_trace", "to", Crate, json!({"from": "top", "to": "bottom", "depth": 5}), json!({"from": "top", "to": "middle", "depth": 5})),
        moves("code_trace", "path", Crate, json!({"from": "top", "to": "bottom", "depth": 5, "path": "src"}), json!({"from": "top", "to": "bottom", "depth": 5, "path": "sub/src"})),
        moves("code_trace", "depth", Crate, json!({"from": "top", "to": "bottom", "depth": 1}), json!({"from": "top", "to": "bottom", "depth": 5})),
        // code_impact
        moves("code_impact", "symbol", Crate, json!({"symbol": "top"}), json!({"symbol": "target"})),
        moves("code_impact", "path", Crate, json!({"symbol": "top", "path": "src"}), json!({"symbol": "top", "path": "sub/src"})),
        moves("code_impact", "depth", Crate, json!({"symbol": "bottom", "depth": 1}), json!({"symbol": "bottom", "depth": 5})),
        moves("code_impact", "mode", Crate, json!({"symbol": "top", "mode": "all"}), json!({"symbol": "top", "mode": "tests"})),
        // ast_query
        moves("ast_query", "pattern", Crate, json!({"pattern": "target($N)"}), json!({"pattern": "fn $NAME() -> u32 { $$$BODY }"})),
        moves("ast_query", "path", Wide, json!({"pattern": "target($N)", "path": "src"}), json!({"pattern": "target($N)", "path": "docs"})),
        moves("ast_query", "glob", Wide, json!({"pattern": "target($N)", "glob": "**/zeppelin0.rs"}), json!({"pattern": "target($N)", "glob": "**/*.rs"})),
        // `target($N)` would be the obvious probe and a useless one: it parses
        // in both languages, so the override binds and the output still
        // matches. `let` exists only in Rust.
        moves("ast_query", "lang", Crate, json!({"pattern": "let $V = $E;", "lang": "rust"}), json!({"pattern": "let $V = $E;", "lang": "python"})),
        moves("ast_query", "limit", Wide, json!({"pattern": "target($N)", "limit": 1}), json!({"pattern": "target($N)", "limit": 50})),
        // ast_rewrite
        moves("ast_rewrite", "pattern", Wide, json!({"pattern": "target($N)", "replacement": "renamed($N)"}), json!({"pattern": "pub fn $N() -> u32 { $$$B }", "replacement": "renamed($N)"})),
        moves("ast_rewrite", "replacement", Wide, json!({"pattern": "target($N)", "replacement": "alpha($N)"}), json!({"pattern": "target($N)", "replacement": "beta($N)"})),
        moves("ast_rewrite", "path", Wide, json!({"pattern": "target($N)", "replacement": "renamed($N)", "path": "src"}), json!({"pattern": "target($N)", "replacement": "renamed($N)", "path": "docs"})),
        moves("ast_rewrite", "glob", Wide, json!({"pattern": "target($N)", "replacement": "renamed($N)", "glob": "**/zeppelin0.rs"}), json!({"pattern": "target($N)", "replacement": "renamed($N)", "glob": "**/*.rs"})),
        moves("ast_rewrite", "lang", Crate, json!({"pattern": "let $V = $E;", "replacement": "let $V = ($E);", "lang": "rust"}), json!({"pattern": "let $V = $E;", "replacement": "let $V = ($E);", "lang": "python"})),
        moves("ast_rewrite", "apply", Wide, json!({"pattern": "target($N)", "replacement": "renamed($N)", "apply": false}), json!({"pattern": "target($N)", "replacement": "renamed($N)", "apply": true})),
    ]
}

// --- the two tests ---------------------------------------------------------

/// The by-construction half. A knob added to a schema with no entry here fails,
/// so the coverage cannot rot quietly the way hand-picked coverage does.
#[tokio::test]
async fn every_advertised_parameter_is_accounted_for() {
    let (_r, _s, tools) = bundle_with(&files_for(Fixture::Crate));
    let table = knobs();

    let missing: Vec<String> = advertised(&tools)
        .iter()
        .filter(|(tool, param)| {
            !table.iter().any(|k| k.tool == *tool && k.param == param)
        })
        .map(|(tool, param)| format!("{tool}.{param}"))
        .collect();
    assert!(
        missing.is_empty(),
        "advertised to the model but not accounted for by any probe: {missing:?}\n\
         Add a row to knobs() — a live differential, or a written reason."
    );

    let stale: Vec<String> = table
        .iter()
        .filter(|k| {
            !advertised(&tools)
                .iter()
                .any(|(tool, param)| *tool == k.tool && param == k.param)
        })
        .map(|k| format!("{}.{}", k.tool, k.param))
        .collect();
    assert!(
        stale.is_empty(),
        "probed a parameter the schema no longer advertises: {stale:?}"
    );

    // An exemption has to name the test that carries the weight instead. A
    // bare "can't probe this" is how a dead knob gets waved through, so the
    // reason is checked rather than just written.
    for knob in &table {
        if let Binds::Covered(reason) = knob.binds {
            assert!(
                reason.contains("::"),
                "{}.{} is exempt from the live probe but its reason does not name \
                 a covering test: {reason:?}",
                knob.tool,
                knob.param
            );
        }
    }
}

/// The live half. Every knob claimed to be wired must actually move something.
#[tokio::test]
async fn every_advertised_parameter_changes_the_answer() {
    let mut dead = Vec::new();
    for knob in knobs() {
        let Binds::Moves(fixture, ref a, ref b) = knob.binds else {
            continue;
        };
        let (_ra, _sa, one) = bundle_with(&files_for(fixture));
        let first = invoke(&one, knob.tool, a.clone()).await;
        let (_rb, _sb, two) = bundle_with(&files_for(fixture));
        let second = invoke(&two, knob.tool, b.clone()).await;
        if first == second {
            dead.push(format!(
                "\n  {}.{} — {a} and {b} both returned:\n    {}",
                knob.tool,
                knob.param,
                first.lines().take(4).collect::<Vec<_>>().join("\n    ")
            ));
        }
    }
    assert!(
        dead.is_empty(),
        "parameters advertised to the model that change nothing:{}\n\n\
         A knob wired to nothing is worse than a missing one: the schema \
         describes what it does, so the model spends a call believing it.",
        dead.join("")
    );
}
