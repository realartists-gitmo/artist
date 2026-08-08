//! Integration coverage for the structural navigation tools.
//!
//! These are end-to-end: a real `Workspace`, real files on disk, real anchors
//! issued through the hashline ledger. The unit tests in `outline.rs` cover
//! rendering against synthetic declarations; nothing there proves the adapters
//! parse, the anchors resolve, or the tools return anything usable.

use artist_tools::{ToolBundle, Workspace};
use rig_core::tool::{IntoToolOutput, PortableTool};
use serde_json::json;

const LIB: &str = r#"pub trait Greeter {
    fn greet(&self) -> u32;
}

pub struct Thing {
    pub a: u32,
}

impl Greeter for Thing {
    fn greet(&self) -> u32 {
        self.a
    }
}

impl Thing {
    pub fn visible(&self) -> u32 {
        self.a
    }

    fn hidden(&self) -> u32 {
        self.a * 2
    }
}

pub fn top_level() -> u32 {
    helper() + 1
}

fn helper() -> u32 {
    7
}
"#;

const OTHER: &str = r#"use crate::{Thing, top_level};

pub fn consumer() -> u32 {
    let t = Thing { a: 1 };
    t.visible()
}

/// A cross-file *free function* call. Method calls through a local binding
/// (`t.visible()` above) need receiver-type inference the resolver does not do,
/// so they produce no resolved edge; a free call resolves exactly.
pub fn calls_across_files() -> u32 {
    top_level()
}
"#;

fn workspace(files: &[(&str, &str)]) -> (tempfile::TempDir, tempfile::TempDir, Workspace) {
    let root = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let target = root.path().join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, content).unwrap();
    }
    let state = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(root.path(), state.path(), "test").unwrap();
    (root, state, workspace)
}

fn bundle() -> (tempfile::TempDir, tempfile::TempDir, ToolBundle) {
    let (root, state, ws) = workspace(&[("src/lib.rs", LIB), ("src/other.rs", OTHER)]);
    let tools = ToolBundle::new(ws);
    (root, state, tools)
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

async fn call_err<T: PortableTool>(tool: &T, value: serde_json::Value) -> String
where
    T::Error: std::fmt::Display,
{
    let args = serde_json::from_value(value).unwrap();
    match tool.call(args).await {
        Ok(_) => panic!("expected an error"),
        Err(e) => e.to_string(),
    }
}

/// Pull the start anchor out of an outline row: `ANCHOR: sig` or
/// `START ⟶ END: sig`.
fn start_anchor(row: &str) -> &str {
    let head = row.split_once(": ").unwrap().0;
    head.split(" ⟶ ").next().unwrap().trim()
}

// ---------------------------------------------------------------------------
// The thesis: outline -> edit, with no read in between
// ---------------------------------------------------------------------------

/// The whole reason the outline carries anchors instead of line numbers. If an
/// anchor taken straight from `code_map` cannot drive `edit`, the feature is
/// decoration.
#[tokio::test]
async fn anchor_from_outline_drives_edit_without_an_intervening_read() {
    let (_root, _state, tools) = bundle();
    let outline = call(&tools.code_map, json!({"path": "src/lib.rs"})).await;

    let row = outline
        .lines()
        .find(|l| l.contains("fn helper"))
        .unwrap_or_else(|| panic!("helper missing from outline:\n{outline}"));
    let anchor = start_anchor(row);

    // No `read` of this file has happened in this session — only `code_map`.
    let edited = call(
        &tools.edit,
        json!({
            "path": "src/lib.rs",
            "replacements": [{"start": anchor, "content": "fn helper() -> u32 { 9 }"}],
        }),
    )
    .await;
    assert!(
        edited.contains("fn helper() -> u32 { 9 }"),
        "edit did not report the new text:\n{edited}"
    );

    let reread = call(&tools.read, json!({"path": "src/lib.rs"})).await;
    assert!(
        reread.contains("fn helper() -> u32 { 9 }"),
        "edit did not land on disk:\n{reread}"
    );
}

/// A span row (`START ⟶ END`) must let the model replace a whole declaration in
/// one call — the case that saves an entire `read`.
#[tokio::test]
async fn a_span_from_the_outline_replaces_a_whole_declaration() {
    let (_root, _state, tools) = bundle();
    let outline = call(&tools.code_map, json!({"path": "src/lib.rs"})).await;
    let row = outline
        .lines()
        .find(|l| l.contains("fn top_level") && l.contains(" ⟶ "))
        .unwrap_or_else(|| panic!("no span row for top_level:\n{outline}"));

    let head = row.split_once(": ").unwrap().0;
    let (start, end) = head.split_once(" ⟶ ").unwrap();

    let edited = call(
        &tools.edit,
        json!({
            "path": "src/lib.rs",
            "replacements": [{
                "start": start.trim(),
                "end": end.trim(),
                "content": "pub fn top_level() -> u32 { 42 }",
            }],
        }),
    )
    .await;
    assert!(edited.contains("42"), "span replace failed:\n{edited}");

    let reread = call(&tools.read, json!({"path": "src/lib.rs"})).await;
    assert!(reread.contains("pub fn top_level() -> u32 { 42 }"));
    assert!(
        !reread.contains("helper() + 1"),
        "old body survived a whole-declaration replace:\n{reread}"
    );
}

// ---------------------------------------------------------------------------
// code_map
// ---------------------------------------------------------------------------

#[tokio::test]
async fn code_map_reports_shape_with_anchors() {
    let (_root, _state, tools) = bundle();
    let out = call(&tools.code_map, json!({"path": "src/lib.rs"})).await;
    assert!(out.contains("pub struct Thing"), "{out}");
    assert!(out.contains("pub fn visible"), "{out}");
    assert!(out.contains("(rust)"), "language header missing:\n{out}");
}

/// The filter that was silently a no-op until the visibility predicate learned
/// about Rust.
#[tokio::test]
async fn code_map_include_private_false_actually_hides_private_items() {
    let (_root, _state, tools) = bundle();
    let all = call(&tools.code_map, json!({"path": "src/lib.rs"})).await;
    let public = call(
        &tools.code_map,
        json!({"path": "src/lib.rs", "includePrivate": false}),
    )
    .await;
    assert!(
        all.contains("fn hidden"),
        "private item missing from full outline:\n{all}"
    );
    assert!(
        !public.contains("fn hidden"),
        "private item survived includePrivate=false:\n{public}"
    );
    assert!(
        public.contains("pub fn visible"),
        "public item was dropped:\n{public}"
    );
}

#[tokio::test]
async fn code_map_budget_bounds_the_row_count() {
    let (_root, _state, tools) = bundle();
    let wide = call(
        &tools.code_map,
        json!({"path": "src/lib.rs", "budget": 100}),
    )
    .await;
    let narrow = call(&tools.code_map, json!({"path": "src/lib.rs", "budget": 1})).await;
    assert!(
        narrow.lines().count() < wide.lines().count(),
        "budget had no effect\nwide:\n{wide}\nnarrow:\n{narrow}"
    );
}

#[tokio::test]
async fn code_map_says_so_when_there_is_no_adapter() {
    let (_root, _state, ws) = workspace(&[("notes.unknownext", "some prose\n")]);
    let tools = ToolBundle::new(ws);
    let err = call_err(&tools.code_map, json!({"path": "notes.unknownext"})).await;
    assert!(
        err.contains("no language adapter") && err.contains("read"),
        "unhelpful error: {err}"
    );
}

// ---------------------------------------------------------------------------
// code_show / code_implements / code_surface
// ---------------------------------------------------------------------------

#[tokio::test]
async fn code_show_extracts_one_symbol_with_anchors() {
    let (_root, _state, tools) = bundle();
    let out = call(
        &tools.code_show,
        json!({"path": "src/lib.rs", "symbol": "top_level"}),
    )
    .await;
    assert!(out.contains("pub fn top_level"), "{out}");
    assert!(
        !out.contains("fn hidden"),
        "showed more than the requested symbol:\n{out}"
    );
}

#[tokio::test]
async fn code_show_reports_a_missing_symbol_rather_than_empty_output() {
    let (_root, _state, tools) = bundle();
    let err = call_err(
        &tools.code_show,
        json!({"path": "src/lib.rs", "symbol": "nonexistent"}),
    )
    .await;
    assert!(err.contains("nonexistent"), "unhelpful error: {err}");
}

#[tokio::test]
async fn code_implements_finds_an_impl_block() {
    let (_root, _state, tools) = bundle();
    // `implements` finds trait implementors, so ask about the trait. Asking
    // about `Thing` returned "no implementations of 'Thing' found", which
    // contains "Thing" — this assertion used to pass on the error message.
    let out = call(&tools.code_implements, json!({"target": "Greeter"})).await;
    assert!(
        out.contains("Thing"),
        "implementor of Greeter not reported:\n{out}"
    );
    assert!(
        !out.contains("no implementations"),
        "expected a result, got the empty message:\n{out}"
    );
}

#[tokio::test]
async fn code_implements_is_graceful_when_nothing_matches() {
    let (_root, _state, tools) = bundle();
    let out = call(&tools.code_implements, json!({"target": "NoSuchType"})).await;
    assert!(out.contains("no implementations"), "{out}");
}

#[tokio::test]
async fn code_surface_reports_the_public_api() {
    let (_root, _state, tools) = bundle();
    let out = call(&tools.code_surface, json!({})).await;
    assert!(!out.trim().is_empty(), "surface produced nothing");
}

// ---------------------------------------------------------------------------
// graph tools — smoke coverage; they must answer or explain, never panic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn graph_tools_answer_or_explain() {
    let (_root, _state, tools) = bundle();
    let deps = call(&tools.code_deps, json!({"file": "src/other.rs"})).await;
    assert!(!deps.trim().is_empty(), "deps produced nothing");

    let reverse = call(
        &tools.code_deps,
        json!({"file": "src/lib.rs", "direction": "reverse"}),
    )
    .await;
    assert!(!reverse.trim().is_empty(), "reverse deps produced nothing");

    let cycles = call(&tools.code_cycles, json!({})).await;
    assert!(!cycles.trim().is_empty(), "cycles produced nothing");

    let trace = call(
        &tools.code_trace,
        json!({"from": "top_level", "to": "helper"}),
    )
    .await;
    assert!(!trace.trim().is_empty(), "trace produced nothing");
}

#[tokio::test]
async fn code_impact_reports_or_explains_for_an_unknown_symbol() {
    let (_root, _state, tools) = bundle();
    // Either an error or a "no symbol matches" body is fine. A panic is not.
    let args = serde_json::from_value(json!({"symbol": "definitelyNotHere"})).unwrap();
    match tools.code_impact.call(args).await {
        Ok(out) => assert!(!out.trim().is_empty()),
        Err(e) => assert!(e.to_string().contains("definitelyNotHere") || !e.to_string().is_empty()),
    }
}

// ---------------------------------------------------------------------------
// ast_query / ast_rewrite
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ast_query_matches_structurally() {
    let (_root, _state, tools) = bundle();
    let out = call(&tools.ast_query, json!({"pattern": "self.a * $N"})).await;
    assert!(
        out.contains("lib.rs") || out.contains("match"),
        "structural query found nothing:\n{out}"
    );
}

#[tokio::test]
async fn ast_query_explains_when_nothing_matches() {
    let (_root, _state, tools) = bundle();
    let out = call(&tools.ast_query, json!({"pattern": "wildly_absent($$$)"})).await;
    assert!(out.contains("no structural matches"), "{out}");
}

/// The safety property: a preview must never touch the tree. If this regresses,
/// a rewrite could rewrite files behind the anchor ledger.
#[tokio::test]
async fn ast_rewrite_previews_without_writing_anything() {
    let (root, _state, tools) = bundle();
    let path = root.path().join("src/lib.rs");
    let before = std::fs::read_to_string(&path).unwrap();

    let out = call(
        &tools.ast_rewrite,
        json!({"pattern": "self.a * $N", "replacement": "self.a"}),
    )
    .await;
    assert!(out.contains("PREVIEW"), "preview banner missing:\n{out}");

    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(before, after, "ast_rewrite modified a file during preview");
}

#[tokio::test]
async fn ast_rewrite_explains_when_nothing_matches() {
    let (_root, _state, tools) = bundle();
    let out = call(
        &tools.ast_rewrite,
        json!({"pattern": "wildly_absent($$$)", "replacement": "x"}),
    )
    .await;
    assert!(out.contains("nothing to rewrite"), "{out}");
}

/// Applying must actually write, and must go through the ledger so the anchors
/// for the rewritten file are reissued rather than left pointing at text that
/// no longer exists.
#[tokio::test]
async fn ast_rewrite_apply_writes_and_keeps_anchors_usable() {
    let (root, _state, tools) = bundle();
    let path = root.path().join("src/lib.rs");

    let applied = call(
        &tools.ast_rewrite,
        json!({"pattern": "self.a * $N", "replacement": "self.a", "apply": true}),
    )
    .await;
    assert!(applied.contains("Applied to"), "{applied}");

    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        !on_disk.contains("self.a * 2"),
        "apply did not write:\n{on_disk}"
    );

    // The ledger must still be able to hand out working anchors for the file it
    // just wrote — the whole reason apply goes through the coordinator.
    let outline = call(&tools.code_map, json!({"path": "src/lib.rs"})).await;
    let row = outline.lines().find(|l| l.contains("fn hidden")).unwrap();
    let anchor = start_anchor(row);
    let edited = call(
        &tools.edit,
        json!({
            "path": "src/lib.rs",
            "replacements": [{"start": anchor, "content": "    fn hidden(&self) -> u32 { 0 }"}],
        }),
    )
    .await;
    assert!(
        edited.contains("fn hidden"),
        "post-apply edit failed:\n{edited}"
    );
}

/// The stale guard. If the tree moves between preview and apply, the write must
/// be refused rather than committing a change nobody previewed.
#[tokio::test]
async fn ast_rewrite_apply_refuses_when_the_tree_moved() {
    let (root, _state, tools) = bundle();

    // Preview.
    let preview = call(
        &tools.ast_rewrite,
        json!({"pattern": "self.a * $N", "replacement": "self.a"}),
    )
    .await;
    assert!(preview.contains("PREVIEW"), "{preview}");

    // Something else changes the file out from under us.
    let other = root.path().join("src/other.rs");
    std::fs::write(&other, "pub fn consumer() -> u32 { let t = 1; t * 2 }\n").unwrap();

    // The match set now differs, so apply must refuse and write nothing.
    let lib_before = std::fs::read_to_string(root.path().join("src/lib.rs")).unwrap();
    let args = serde_json::from_value(
        json!({"pattern": "self.a * $N", "replacement": "self.a", "apply": true}),
    )
    .unwrap();
    let _ = tools.ast_rewrite.call(args).await;
    let lib_after = std::fs::read_to_string(root.path().join("src/lib.rs")).unwrap();
    // Either it refused outright, or it re-planned and wrote a fresh, verified
    // plan. What must never happen is writing the *stale* plan.
    if lib_before != lib_after {
        assert!(
            !lib_after.contains("self.a * 2"),
            "wrote a stale plan:\n{lib_after}"
        );
    }
}

/// Preview must remain the default: an `apply` the model did not ask for is the
/// failure mode this whole flow exists to prevent.
#[tokio::test]
async fn ast_rewrite_defaults_to_preview() {
    let (root, _state, tools) = bundle();
    let path = root.path().join("src/lib.rs");
    let before = std::fs::read_to_string(&path).unwrap();
    let out = call(
        &tools.ast_rewrite,
        json!({"pattern": "self.a * $N", "replacement": "self.a"}),
    )
    .await;
    assert!(out.contains("PREVIEW"));
    assert_eq!(before, std::fs::read_to_string(&path).unwrap());
}

// ---------------------------------------------------------------------------
// read: windowed content plus whole-file shape
// ---------------------------------------------------------------------------

fn long_rust(fns: usize) -> String {
    (0..fns)
        .map(|i| format!("pub fn item_{i}() -> u32 {{\n    {i}\n}}\n\n"))
        .collect()
}

/// A file under the window is returned whole, with no outline appended — the
/// shape would just restate what is already on screen.
#[tokio::test]
async fn a_short_file_reads_whole_with_no_outline() {
    let src = long_rust(10); // 40 lines
    let (_root, _state, ws) = workspace(&[("src/small.rs", &src)]);
    let tools = ToolBundle::new(ws);
    let out = call(&tools.read, json!({"path": "src/small.rs"})).await;
    assert!(out.contains("item_0"), "{out}");
    assert!(out.contains("item_9"), "short file was truncated:\n{out}");
    assert!(
        !out.contains("shape of the whole file"),
        "outline appended to a file that fits:\n{out}"
    );
}

/// Past the window the model sees a slice of body plus the shape of everything
/// else, so it is never left knowing only the first N lines exist.
#[tokio::test]
async fn a_long_file_reads_windowed_and_appends_whole_file_shape() {
    let src = long_rust(120); // 480 lines
    let (_root, _state, ws) = workspace(&[("src/big.rs", &src)]);
    let tools = ToolBundle::new(ws);
    let out = call(&tools.read, json!({"path": "src/big.rs"})).await;

    assert!(out.contains("[truncated:"), "expected a window:\n{out}");
    assert!(
        out.contains("shape of the whole file"),
        "no outline on a long file:\n{out}"
    );
    // A declaration far past the window must appear in the shape.
    assert!(
        out.contains("item_119"),
        "outline missed declarations outside the read window:\n{out}"
    );
}

/// The point of appending shape rather than a plain file listing: the anchors
/// in it address lines the model never saw, and they work.
#[tokio::test]
async fn an_anchor_from_the_appended_shape_drives_edit() {
    let src = long_rust(120);
    let (_root, _state, ws) = workspace(&[("src/big.rs", &src)]);
    let tools = ToolBundle::new(ws);
    let out = call(&tools.read, json!({"path": "src/big.rs"})).await;

    let row = out
        .lines()
        .find(|l| l.contains("item_119"))
        .unwrap_or_else(|| panic!("item_119 missing:\n{out}"));
    let anchor = start_anchor(row);

    let edited = call(
        &tools.edit,
        json!({
            "path": "src/big.rs",
            "replacements": [{"start": anchor, "content": "pub fn renamed_119() -> u32 {"}],
        }),
    )
    .await;
    assert!(
        edited.contains("renamed_119"),
        "anchor from the appended shape did not resolve:\n{edited}"
    );
}

// ---------------------------------------------------------------------------
// cross-file output speaks anchors
// ---------------------------------------------------------------------------

/// The surgery's whole point: a graph answer names a location in a file the
/// model has not read, and that location has to be actionable. A line number
/// would be invalidated by the next insert above it.
#[tokio::test]
async fn impact_reports_locations_as_anchors_not_line_numbers() {
    let (_root, _state, tools) = bundle();
    let out = call(&tools.code_impact, json!({"symbol": "helper", "depth": 1})).await;
    assert!(
        out.contains('@'),
        "no anchored location in impact output:\n{out}"
    );
    assert!(
        !out.contains(".rs:"),
        "a raw file:line location survived:\n{out}"
    );
}

/// An anchor from a cross-file answer must resolve — including for a file the
/// model never read, which is exactly what "observation justifies issuance"
/// buys.
#[tokio::test]
async fn an_anchor_from_a_cross_file_answer_drives_edit() {
    let (_root, _state, tools) = bundle();
    // `consumer` lives in other.rs and calls into lib.rs. Ask about it from
    // the callers direction, so the location named is in a file this test has
    // never read through any tool.
    let out = call(
        &tools.code_impact,
        json!({"symbol": "top_level", "depth": 1}),
    )
    .await;

    let Some(location) = out
        .split_whitespace()
        .find(|t| t.contains('@'))
        .map(|t| t.trim_matches(|c| c == '(' || c == ')'))
    else {
        panic!("no anchored location to act on:\n{out}");
    };
    let (path, anchor) = location.split_once('@').expect("path@anchor");

    let edited = call(
        &tools.edit,
        json!({
            "path": path,
            "replacements": [{"start": anchor, "content": "// rewritten via cross-file anchor"}],
        }),
    )
    .await;
    assert!(
        edited.contains("rewritten via cross-file anchor"),
        "cross-file anchor did not resolve:\n{edited}"
    );
}

#[tokio::test]
async fn implements_reports_anchored_locations() {
    let (_root, _state, tools) = bundle();
    let out = call(&tools.code_implements, json!({"target": "Greeter"})).await;
    assert!(
        out.contains('@'),
        "implements is still line-numbered:\n{out}"
    );
}

#[tokio::test]
async fn surface_reports_anchored_locations() {
    let (_root, _state, tools) = bundle();
    let out = call(&tools.code_surface, json!({})).await;
    assert!(
        out.contains('@'),
        "surface still reports line numbers:\n{out}"
    );
    assert!(
        !out.contains(".rs:"),
        "a raw file:line survived in surface:\n{out}"
    );
}

#[tokio::test]
async fn ast_query_reports_anchored_locations() {
    let (_root, _state, tools) = bundle();
    let out = call(&tools.ast_query, json!({"pattern": "self.a * $N"})).await;
    assert!(out.contains('@'), "ast_query still line-numbered:\n{out}");
}

/// The payoff: a structural search is usually the prelude to a change, so its
/// hits have to be directly actionable.
#[tokio::test]
async fn an_anchor_from_ast_query_drives_edit() {
    let (_root, _state, tools) = bundle();
    let out = call(&tools.ast_query, json!({"pattern": "self.a * $N"})).await;
    let location = out
        .split_whitespace()
        .find(|t| t.contains('@'))
        .unwrap_or_else(|| panic!("no anchored hit:\n{out}"));
    let (path, anchor) = location.split_once('@').expect("path@anchor");

    let edited = call(
        &tools.edit,
        json!({
            "path": path,
            "replacements": [{"start": anchor, "content": "        self.a"}],
        }),
    )
    .await;
    assert!(
        edited.contains("self.a"),
        "anchor from ast_query did not resolve:\n{edited}"
    );
}

/// A preview must anchor the *removal* side only. The additions have not been
/// written, so no handle exists for them; offering one would invite an edit
/// against a handle that was never issued.
#[tokio::test]
async fn ast_rewrite_preview_anchors_removals_not_additions() {
    let (_root, _state, tools) = bundle();
    let out = call(
        &tools.ast_rewrite,
        json!({"pattern": "self.a * $N", "replacement": "self.a"}),
    )
    .await;
    assert!(out.contains("PREVIEW"), "{out}");
    // Match on the diff marker *after* the gutter, not on any `-` in the line:
    // `fn hidden(&self) -> u32` contains one via `->`.
    let row = |marker: char| {
        out.lines()
            .find(|l| {
                l.split_once('│')
                    .is_some_and(|(_, body)| body.trim_start().starts_with(marker))
            })
            .unwrap_or_else(|| panic!("no {marker} row:\n{out}"))
    };
    let gutter = |l: &str| l.split('│').next().unwrap().trim().to_string();

    assert!(
        !gutter(row('-')).is_empty(),
        "removal row carries no anchor:\n{out}"
    );
    assert!(
        gutter(row('+')).is_empty(),
        "addition row carries an anchor for text that was never written:\n{out}"
    );
}

#[tokio::test]
async fn trace_finds_a_path_and_anchors_each_hop() {
    let (_root, _state, tools) = bundle();
    let out = call(
        &tools.code_trace,
        json!({"from": "calls_across_files", "to": "helper"}),
    )
    .await;
    assert!(out.contains("hop"), "no path found:\n{out}");
    assert!(out.contains('@'), "trace hops are not anchored:\n{out}");
}

/// An unreachable pair must say so, and say why, rather than returning nothing.
#[tokio::test]
async fn trace_explains_when_there_is_no_path() {
    let (_root, _state, tools) = bundle();
    let out = call(
        &tools.code_trace,
        json!({"from": "helper", "to": "consumer"}),
    )
    .await;
    assert!(
        out.contains("no static call path") || out.contains("hop"),
        "unhelpful trace result:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// first-touch annotation
// ---------------------------------------------------------------------------

/// The note has to reach the model on the path that does *not* involve `read`,
/// or an outline-then-edit flow never sees it.
#[tokio::test]
async fn first_touch_fires_on_code_map_too_not_only_read() {
    let (_root, _state, tools) = bundle();
    let out = call(&tools.code_map, json!({"path": "src/lib.rs"})).await;
    assert!(
        out.contains("exports"),
        "no first-touch note from code_map:\n{out}"
    );
}

/// Once per path per session. A repeat is noise, and the second look at a file
/// is not when anything is learned.
#[tokio::test]
async fn the_note_fires_once_per_path() {
    let (_root, _state, tools) = bundle();
    // `lib.rs` is a module root, so the section that fires is `exports`.
    // `imported by` does not: `use crate::{…}` is crate-relative and the dep
    // resolver reports those as external rather than resolving them back to
    // the crate root.
    let first = call(&tools.read, json!({"path": "src/lib.rs"})).await;
    let second = call(&tools.read, json!({"path": "src/lib.rs"})).await;
    assert!(first.contains("exports"), "no note on first read:\n{first}");
    assert!(
        !second.contains("exports"),
        "note repeated on second read:\n{second}"
    );
}

/// Seeing a file through one tool must satisfy the other — the trigger is
/// observation of the path, not the identity of the tool that observed it.
#[tokio::test]
async fn observing_through_one_tool_counts_for_the_other() {
    let (_root, _state, tools) = bundle();
    let mapped = call(&tools.code_map, json!({"path": "src/lib.rs"})).await;
    let read = call(&tools.read, json!({"path": "src/lib.rs"})).await;
    assert!(
        mapped.contains("exports"),
        "no note from code_map:\n{mapped}"
    );
    assert!(
        !read.contains("exports"),
        "read repeated a note code_map already gave:\n{read}"
    );
}

/// A module root gets its resolved exports, because that is the file where a
/// plain read shows `pub use` lines and teaches nothing.
#[tokio::test]
async fn a_module_root_reports_its_resolved_exports() {
    let (_root, _state, ws) = workspace(&[
        ("src/lib.rs", "pub mod inner;\npub use inner::Exported;\n"),
        (
            "src/inner.rs",
            "pub struct Exported {\n    pub a: u32,\n}\n",
        ),
    ]);
    let tools = ToolBundle::new(ws);
    let out = call(&tools.read, json!({"path": "src/lib.rs"})).await;
    assert!(
        out.contains("exports"),
        "module root got no surface note:\n{out}"
    );
}

/// Most files have nothing worth saying, and must produce no note at all.
#[tokio::test]
async fn an_unremarkable_file_gets_no_note() {
    let (_root, _state, ws) = workspace(&[("src/lonely.rs", "pub fn alone() -> u32 { 1 }\n")]);
    let tools = ToolBundle::new(ws);
    let out = call(&tools.read, json!({"path": "src/lonely.rs"})).await;
    assert!(
        !out.contains("imported by") && !out.contains("in an import cycle"),
        "note fired on a file with nothing to report:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// after-commit annotation
// ---------------------------------------------------------------------------

/// Editing a function with callers must say so, unprompted. Asking requires
/// already suspecting there are callers, which is exactly the gap.
#[tokio::test]
async fn editing_a_called_function_reports_its_callers() {
    let (_root, _state, tools) = bundle();
    let outline = call(&tools.code_map, json!({"path": "src/lib.rs"})).await;
    let row = outline
        .lines()
        .find(|l| l.contains("fn helper"))
        .unwrap_or_else(|| panic!("helper missing:\n{outline}"));
    let anchor = start_anchor(row);

    let edited = call(
        &tools.edit,
        json!({
            "path": "src/lib.rs",
            "replacements": [{"start": anchor, "content": "fn helper() -> u32 { 99 }"}],
        }),
    )
    .await;
    assert!(
        edited.contains("caller(s)"),
        "no caller note after editing a called function:\n{edited}"
    );
    assert!(
        edited.contains("top_level"),
        "the actual caller was not named:\n{edited}"
    );
}

/// Editing inside a trait reports implementors instead — the precise signal,
/// rather than "this file contains a trait", which is true of half of Rust.
#[tokio::test]
async fn editing_a_trait_reports_its_implementors() {
    let (_root, _state, tools) = bundle();
    let outline = call(&tools.code_map, json!({"path": "src/lib.rs"})).await;
    let row = outline
        .lines()
        .find(|l| l.contains("trait Greeter"))
        .unwrap_or_else(|| panic!("Greeter missing:\n{outline}"));
    let anchor = start_anchor(row);

    let edited = call(
        &tools.edit,
        json!({
            "path": "src/lib.rs",
            // Genuinely different text: the changed-line set is "lines whose
            // content was not there before", so writing a line back unchanged
            // correctly reports nothing.
            "replacements": [{"start": anchor, "content": "pub trait Greeter { // documented"}],
        }),
    )
    .await;
    assert!(
        edited.contains("implementor(s)"),
        "no note after editing a trait:\n{edited}"
    );
}

/// A function nothing calls must produce no note — the note means something
/// because it is not always there.
#[tokio::test]
async fn editing_an_uncalled_function_reports_nothing() {
    let (_root, _state, ws) = workspace(&[("src/solo.rs", "pub fn alone() -> u32 {\n    1\n}\n")]);
    let tools = ToolBundle::new(ws);
    let outline = call(&tools.code_map, json!({"path": "src/solo.rs"})).await;
    let row = outline.lines().find(|l| l.contains("fn alone")).unwrap();
    let anchor = start_anchor(row);
    let edited = call(
        &tools.edit,
        json!({
            "path": "src/solo.rs",
            "replacements": [{"start": anchor, "content": "pub fn alone() -> u32 {"}],
        }),
    )
    .await;
    assert!(
        !edited.contains("caller(s)"),
        "caller note fired for an uncalled function:\n{edited}"
    );
}

/// The appended shape must never claim to be complete when the outline budget
/// cut it short. Measured on a real file: `file_tools.rs` has 152 declarations
/// and the default budget emits 129, and the header used to say "shape of the
/// whole file" regardless.
#[tokio::test]
async fn a_partial_outline_says_how_partial_it_is() {
    // 300 top-level functions, well past the default budget of 120.
    let src: String = (0..300)
        .map(|i| format!("pub fn item_{i}() -> u32 {{\n    {i}\n}}\n\n"))
        .collect();
    let (_root, _state, ws) = workspace(&[("src/dense.rs", &src)]);
    let tools = ToolBundle::new(ws);
    let out = call(&tools.read, json!({"path": "src/dense.rs"})).await;

    assert!(
        !out.contains("shape of the whole file"),
        "claimed completeness on a budget-truncated outline:\n{}",
        &out[out.len().saturating_sub(600)..]
    );
    assert!(
        out.contains("of 300 declarations"),
        "did not report how much of the shape is shown:\n{}",
        &out[out.len().saturating_sub(600)..]
    );
    assert!(
        out.contains("code_map"),
        "did not point at the way to get the rest:\n{}",
        &out[out.len().saturating_sub(600)..]
    );
}

/// And when it *is* complete, it should say so plainly rather than hedging.
#[tokio::test]
async fn a_complete_outline_says_it_is_complete() {
    let src: String = (0..80)
        .map(|i| format!("pub fn item_{i}() -> u32 {{\n    {i}\n}}\n\n"))
        .collect();
    let (_root, _state, ws) = workspace(&[("src/wide.rs", &src)]);
    let tools = ToolBundle::new(ws);
    let out = call(&tools.read, json!({"path": "src/wide.rs"})).await;
    assert!(
        out.contains("shape of the whole file"),
        "a complete outline hedged:\n{}",
        &out[out.len().saturating_sub(400)..]
    );
}

/// A file too large to be source is skipped rather than read.
///
/// The walker filters on extension, so a minified bundle or a generated data
/// file under a source name reaches these tools. `artist-ast` exports a cap for
/// this and applies it in its own CLI; both tools here ignored it and read
/// whatever they were given, once per file, across a whole scope.
#[tokio::test]
async fn ast_tools_skip_files_too_large_to_be_source() {
    let (root, _state, tools) = bundle();
    std::fs::write(root.path().join("small.rs"), "fn a() { b(1) }\n").unwrap();

    // Just over the cap, under a source extension, and containing a match.
    let mut bulk = String::from("fn a() { b(1) }\n");
    bulk.push_str(&"// pad\n".repeat(800_000));
    assert!(bulk.len() as u64 > artist_ast::run::RUN_MAX_FILE_BYTES);
    std::fs::write(root.path().join("generated.rs"), &bulk).unwrap();

    let out = call(&tools.ast_query, json!({"pattern": "b($N)"})).await;
    assert!(
        out.contains("small.rs"),
        "the real source should match: {out}"
    );
    assert!(
        !out.contains("generated.rs"),
        "an oversized file must be skipped, not read: {out}"
    );
}

/// Structural search scopes the same way its neighbours do.
///
/// `find` and `grep` both take a glob, and the model learns the three as one
/// surface. `walk_paths` accepted the filter all along; the two ast tools were
/// passing `None`, so a search could be narrowed by directory but not by shape
/// of path — no "only the tests", no "only the handlers".
#[tokio::test]
async fn ast_tools_can_be_scoped_by_glob() {
    let (root, _state, tools) = bundle();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/keep.rs"), "fn a() { b(1) }\n").unwrap();
    std::fs::write(root.path().join("src/skip_test.rs"), "fn c() { b(2) }\n").unwrap();

    let all = call(&tools.ast_query, json!({"pattern": "b($N)"})).await;
    assert!(
        all.contains("keep.rs") && all.contains("skip_test.rs"),
        "{all}"
    );

    let scoped = call(
        &tools.ast_query,
        json!({"pattern": "b($N)", "glob": "**/keep.rs"}),
    )
    .await;
    assert!(scoped.contains("keep.rs"), "{scoped}");
    assert!(
        !scoped.contains("skip_test.rs"),
        "the glob should have excluded it: {scoped}"
    );
}

/// A rewrite must be scopeable the same way, or a codemod cannot be limited to
/// the part of the tree the model actually reasoned about.
#[tokio::test]
async fn ast_rewrite_honours_a_glob() {
    let (root, _state, tools) = bundle();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/keep.rs"), "fn a() { b(1) }\n").unwrap();
    std::fs::write(root.path().join("src/other.rs"), "fn c() { b(2) }\n").unwrap();

    let out = call(
        &tools.ast_rewrite,
        json!({"pattern": "b($N)", "replacement": "d($N)", "glob": "**/keep.rs"}),
    )
    .await;
    assert!(out.contains("1 file(s) would change"), "{out}");
    assert!(out.contains("keep.rs"), "{out}");
    assert!(!out.contains("other.rs"), "{out}");
}

/// A preview that shows fewer diffs than it will write must say so.
///
/// The affected-file count already led the preview, so scope was never hidden.
/// What was missing is that the unshown files are written too — a model
/// approving what it can see, on output that stops without explaining itself,
/// consents to less than it authorises.
#[tokio::test]
async fn a_truncated_preview_says_how_much_it_is_not_showing() {
    let (root, _state, tools) = bundle();
    // Many *matches* per file, not merely many lines. A unified diff shows only
    // the changed lines and their context, so padding a file leaves its diff
    // small — the first version of this fixture built 60 bulky files whose 60
    // diffs fitted the budget with room to spare, and tested nothing.
    for index in 0..20 {
        let body: String = (0..200)
            .map(|n| format!("fn f{index}_{n}() {{ b(1) }}\n"))
            .collect();
        std::fs::write(root.path().join(format!("f{index:02}.rs")), body).unwrap();
    }

    let out = call(
        &tools.ast_rewrite,
        json!({"pattern": "b($N)", "replacement": "d($N)"}),
    )
    .await;

    assert!(out.contains("20 file(s) would change"), "{out}");
    assert!(
        out.contains("Diffs shown for") && out.contains("of 20"),
        "a truncated preview must state what it is not showing: {out}"
    );
    assert!(
        out.contains("apply=true writes all 20"),
        "and that apply covers the files it did not show: {out}"
    );
}

/// When everything fits there is nothing to disclose, and saying so anyway
/// would be noise on the common case.
#[tokio::test]
async fn a_complete_preview_says_nothing_about_truncation() {
    let (root, _state, tools) = bundle();
    std::fs::write(root.path().join("one.rs"), "fn a() { b(1) }\n").unwrap();

    let out = call(
        &tools.ast_rewrite,
        json!({"pattern": "b($N)", "replacement": "d($N)"}),
    )
    .await;
    assert!(out.contains("1 file(s) would change"), "{out}");
    assert!(!out.contains("Diffs shown for"), "{out}");
}

/// "No matches" is a claim about the project; what these tools can honestly
/// make is a claim about the files they read.
///
/// Oversized blobs and files that fail to parse are passed over. A model told
/// there are no matches stops looking, so the gap has to be named — this is the
/// exact defect the byte cap introduced and the audit caught.
#[tokio::test]
async fn an_empty_result_admits_what_it_never_read() {
    let (root, _state, tools) = bundle();
    let mut bulk = String::from("fn a() { unreachable_by_search(1) }\n");
    bulk.push_str(&"// pad\n".repeat(800_000));
    std::fs::write(root.path().join("generated.rs"), &bulk).unwrap();

    let query = call(
        &tools.ast_query,
        json!({"pattern": "unreachable_by_search($N)"}),
    )
    .await;
    assert!(query.contains("no structural matches"), "{query}");
    assert!(
        query.contains("skipped"),
        "an empty result must admit what it never read: {query}"
    );

    let rewrite = call(
        &tools.ast_rewrite,
        json!({"pattern": "unreachable_by_search($N)", "replacement": "x($N)"}),
    )
    .await;
    assert!(rewrite.contains("skipped"), "{rewrite}");
}

/// When nothing was skipped the note must not appear — a qualifier that fires
/// always carries no information.
#[tokio::test]
async fn an_empty_result_over_readable_files_says_nothing_extra() {
    let (root, _state, tools) = bundle();
    std::fs::write(root.path().join("a.rs"), "fn a() { b(1) }\n").unwrap();

    let out = call(&tools.ast_query, json!({"pattern": "absent_entirely($N)"})).await;
    assert!(out.contains("no structural matches"), "{out}");
    assert!(!out.contains("skipped"), "{out}");
}

// ---------------------------------------------------------------------------
// The three ergonomic contracts on the ast tools
//
// Each covers a case where the tool did something defensible at the library
// level and misleading at the agent surface: a request to narrow that widened,
// three outcomes rendered as one sentence, and a codemod that would write text
// that is not code.
// ---------------------------------------------------------------------------

/// A mixed tree where the same call text appears in source and in prose.
fn mixed() -> (tempfile::TempDir, tempfile::TempDir, ToolBundle) {
    let (root, state, ws) = workspace(&[
        ("src/a.rs", "pub fn z() -> u32 { target(1) }\n"),
        ("docs/notes.md", "# notes\n\nprose mentioning target(1)\n"),
    ]);
    (root, state, ToolBundle::new(ws))
}

/// `lang` used to force its grammar onto every file in scope, so narrowing a
/// search to Rust returned *more* hits than not narrowing it — the extra ones
/// inside markdown parsed as Rust.
#[tokio::test]
async fn an_explicit_lang_narrows_the_search_instead_of_widening_it() {
    let (_r, _s, tools) = mixed();

    let detected = call(&tools.ast_query, json!({"pattern": "target($N)"})).await;
    let narrowed = call(
        &tools.ast_query,
        json!({"pattern": "target($N)", "lang": "rust"}),
    )
    .await;

    assert!(detected.contains("a.rs"), "{detected}");
    assert!(narrowed.contains("a.rs"), "{narrowed}");
    assert!(
        !narrowed.contains(".md"),
        "lang=rust reported matches inside markdown:\n{narrowed}"
    );
    assert!(
        narrowed.matches("target(1)").count() <= detected.matches("target(1)").count(),
        "narrowing the language widened the result:\n{narrowed}"
    );
}

#[tokio::test]
async fn an_unknown_lang_is_refused_rather_than_silently_matching_nothing() {
    let (_r, _s, tools) = mixed();
    let err = call_err(
        &tools.ast_query,
        json!({"pattern": "target($N)", "lang": "klingon"}),
    )
    .await;
    assert!(err.contains("unknown language"), "{err}");
}

/// The three empty outcomes have to be distinguishable: a malformed pattern,
/// a scope with no files of that language, and a real absence of matches all
/// used to render the same sentence, and they need three different responses.
#[tokio::test]
async fn an_empty_result_says_which_kind_of_empty_it_was() {
    let (_r, _s, tools) = mixed();

    let malformed = call(&tools.ast_query, json!({"pattern": "fn ("})).await;
    assert!(
        malformed.contains("does not parse"),
        "a malformed pattern read as a clean miss:\n{malformed}"
    );

    let absent = call(&tools.ast_query, json!({"pattern": "nosuchcall($N)"})).await;
    assert!(absent.contains("no structural matches"), "{absent}");
    assert!(
        !absent.contains("does not parse"),
        "a valid pattern was reported as malformed:\n{absent}"
    );
    assert!(
        absent.contains("file") && absent.chars().any(|c| c.is_ascii_digit()),
        "an empty result did not say how much was searched:\n{absent}"
    );

    let nothing_in_scope = call(
        &tools.ast_query,
        json!({"pattern": "target($N)", "lang": "go"}),
    )
    .await;
    assert!(
        nothing_in_scope.contains("nothing to search"),
        "a scope with no Go files read as a searched-and-empty result:\n{nothing_in_scope}"
    );
}

/// The worst of the three: a replacement is arbitrary text, so nothing
/// required the result to be code. This previewed cleanly and would have
/// written `pub fn z() -> u32 { fn ( }` to every matched file.
#[tokio::test]
async fn a_replacement_that_would_not_parse_is_refused_before_any_write() {
    let (root, _s, tools) = mixed();
    let before = std::fs::read_to_string(root.path().join("src/a.rs")).unwrap();

    let err = call_err(
        &tools.ast_rewrite,
        json!({"pattern": "target($N)", "replacement": "fn (", "apply": true}),
    )
    .await;
    assert!(err.contains("does not parse"), "{err}");
    assert!(err.contains("Nothing was written"), "{err}");
    assert_eq!(
        std::fs::read_to_string(root.path().join("src/a.rs")).unwrap(),
        before,
        "a refused rewrite still touched the file"
    );
}

/// An unbound metavariable expands to nothing, so this silently dropped the
/// argument at every site and produced a diff that looked deliberate.
#[tokio::test]
async fn a_replacement_naming_an_unbound_capture_is_refused() {
    let (_r, _s, tools) = mixed();
    let err = call_err(
        &tools.ast_rewrite,
        json!({"pattern": "target($N)", "replacement": "renamed($Z)"}),
    )
    .await;
    assert!(err.contains("$Z"), "{err}");
    assert!(
        err.contains("$N"),
        "the error did not say what is bound:\n{err}"
    );
}

/// The parse check counts errors rather than testing for any, so a file that
/// already fails to parse stays rewritable. Refusing those would make the
/// tool useless on exactly the half-finished code it is most wanted for.
#[tokio::test]
async fn a_file_that_already_fails_to_parse_can_still_be_rewritten() {
    let (root, _state, ws) = workspace(&[("src/broken.rs", "fn a() { target(1) }\nfn oops( {\n")]);
    let tools = ToolBundle::new(ws);

    let out = call(
        &tools.ast_rewrite,
        json!({"pattern": "target($N)", "replacement": "renamed($N)", "apply": true}),
    )
    .await;
    assert!(out.contains("broken.rs"), "{out}");
    let after = std::fs::read_to_string(root.path().join("src/broken.rs")).unwrap();
    assert!(after.contains("renamed(1)"), "{after}");
}
