//! The shape of a project, in about three hundred tokens.
//!
//! This is the thing people hand-write into `AGENTS.md` and then never update.
//! Generated instead — and deliberately *not* written to a file, because
//! materialising it is what creates the staleness problem it exists to solve.
//! Nothing on disk means nothing to rot, no marked regions to preserve, no
//! merge conflicts, and no need for the output to be diff-stable.
//!
//! # Why not a ranked list of definitions
//!
//! Aider's repo map ranks every definition by PageRank over a graph of files
//! and identifiers, then fills a token budget. That design answers a question
//! artist does not have: it exists because aider cannot look anything up, so
//! everything it might need must be guessed in advance and shipped up front.
//! Artist has `code_map`, `code_surface`, `code_impact` and a first-touch hook
//! that fires when a file is actually opened.
//!
//! What tools cannot supply is the part nobody thinks to ask for: which units
//! exist, which sits beneath which, what is load-bearing, and what is tangled.
//! Centrality does not measure that — the most-referenced identifier in a Rust
//! project is `Error` — so this measures it structurally instead, from the
//! import graph.
//!
//! # Roles come from the code
//!
//! The one-line description of each unit is its existing crate-level doc
//! comment. Ten of the twelve crates in this repository already have a good
//! one; they had simply never been collected into a single view. A unit without
//! one is reported as lacking it, naming the file to add it to. Never invent a
//! role: a plausible wrong description is worse than an admitted gap, because
//! nothing later contradicts it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// One architectural unit: a crate, a package, or a top-level source directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub name: String,
    pub root: PathBuf,
    /// What this unit is for.
    pub role: Option<String>,
    /// True when `role` was read off the public API rather than written by the
    /// author. Distinguished because the two are different kinds of claim.
    pub role_inferred: bool,
    /// File to add a doc comment to, when `role` is `None`.
    pub role_hint: Option<PathBuf>,
    /// Depth in the dependency order; 0 depends on nothing else here.
    pub layer: usize,
    /// How many units break if this one changes — transitive dependents, not
    /// direct importers.
    pub blast_radius: usize,
    /// Other units this one is mutually entangled with.
    pub cycle_with: Vec<String>,
}

/// Two units that keep changing in the same commit without importing each
/// other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HiddenCoupling {
    pub a: String,
    pub b: String,
    pub commits: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Skeleton {
    pub units: Vec<Unit>,
    /// Coupling the import graph cannot see.
    pub hidden: Vec<HiddenCoupling>,
    /// Units omitted because the budget ran out.
    pub omitted: usize,
    /// Units with uncommitted changes or very recent commits, most recent
    /// first.
    pub active: Vec<String>,
}

/// How much room a rendered skeleton may take, in characters.
///
/// A size budget rather than a unit count, because that is the thing actually
/// being spent: forty short lines and forty long ones cost very differently,
/// and a count has to be set pessimistically for the worst case. Twelve crates
/// come to well under this; a two-hundred-module monorepo fills it and reports
/// what it dropped.
pub const DEFAULT_BUDGET: usize = 2_400;

/// Build the skeleton for a project, or `None` when there is no graph to read.
/// Words in the opening request, for choosing what survives the budget.
///
/// The map is built on the turn that carries the user's first message, so what
/// the session is *for* is already known when it is assembled. Nothing was
/// using that. A structural map is a guess about what will be needed; a request
/// is a statement of it.
///
/// Only ever affects which units survive a budget squeeze — never the order,
/// which stays dependency order because that is what makes the thing readable,
/// and never the content. On a project that fits, this changes nothing at all.
fn relevance(focus: &str, unit: &Unit) -> usize {
    let terms: Vec<String> = focus
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| word.len() > 3)
        .map(str::to_ascii_lowercase)
        .collect();
    if terms.is_empty() {
        return 0;
    }
    let haystack = format!(
        "{} {}",
        unit.name.to_ascii_lowercase(),
        unit.role
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
    );
    terms
        .iter()
        .filter(|term| {
            // Match on the stem, so "wayland" finds "wayland" and "canvases"
            // finds "canvas".
            let stem = &term[..term.len().min(6)];
            haystack.contains(stem)
        })
        .count()
}

pub fn build(root: &Path, budget: usize) -> Option<Skeleton> {
    build_for(root, budget, "")
}

/// [`build`], with the opening request available to break budget ties.
pub fn build_for(root: &Path, budget: usize, focus: &str) -> Option<Skeleton> {
    let graph = artist_ast::graph_cache::shared::get_or_init(root).ok()?;
    let units = discover_units(root);
    if units.len() < 2 {
        // A single-unit project has no architecture to describe; its shape is
        // whatever `code_map` says about its files.
        return None;
    }
    let owner = ownership(&units);
    let edges = aggregate_edges(&graph.deps, &owner, &units);
    let mut skeleton = assemble(units.clone(), &edges, budget, focus);
    let history = commits(root, &units);
    skeleton.active = recently_touched(&history);
    skeleton.hidden = hidden_coupling(&history, &edges);
    // Paths are shown to a model that addresses files project-relatively; an
    // absolute path is both longer and in the wrong coordinate system.
    for unit in &mut skeleton.units {
        if let Some(hint) = &unit.role_hint
            && let Ok(relative) = hint.strip_prefix(root)
        {
            unit.role_hint = Some(relative.to_path_buf());
        }
    }
    Some(skeleton)
}

/// Files that declare a unit.
const MANIFESTS: &[&str] = &["Cargo.toml", "package.json", "pyproject.toml", "go.mod"];

/// Directories that own source: package manifests where they exist, top-level
/// source directories otherwise.
///
/// Manifests first because they are what the project itself considers a unit.
/// The fallback matters for the many real projects that are one package with
/// meaningful internal structure.
fn discover_units(root: &Path) -> Vec<(String, PathBuf)> {
    let mut found: BTreeMap<String, PathBuf> = BTreeMap::new();

    let walker = ignore::WalkBuilder::new(root)
        .max_depth(Some(3))
        .hidden(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .require_git(false)
        .sort_by_file_name(std::ffi::OsStr::cmp)
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let is_manifest = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| MANIFESTS.contains(&n));
        if !is_manifest {
            continue;
        }
        let Some(dir) = path.parent() else { continue };
        // The workspace root's own manifest describes the whole project, not a
        // unit within it.
        if dir == root {
            continue;
        }
        if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
            found.insert(name.to_owned(), dir.to_path_buf());
        }
    }

    if found.is_empty() {
        for base in [root.join("src"), root.to_path_buf()] {
            let Ok(entries) = std::fs::read_dir(&base) else {
                continue;
            };
            let mut names: Vec<_> = entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|k| k.is_dir()))
                .filter_map(|e| {
                    let name = e.file_name().to_str()?.to_owned();
                    (!name.starts_with('.')
                        && !is_supporting(Path::new(&name))
                        && holds_source(&e.path()))
                    .then_some((name, e.path()))
                })
                .collect();
            names.sort();
            if !names.is_empty() {
                return names;
            }
        }
    }
    found.into_iter().collect()
}

/// Whether a directory contains code, as opposed to notes, data or artefacts.
///
/// The manifest-free fallback picks top-level directories, and on a real Python
/// project that produced `notes`, `data` and `models` as architectural units —
/// one of them described, via its markdown headings, as "exports Autoflow
/// prior-work audit". A directory with no source in it is not part of the
/// architecture whatever it is named.
fn holds_source(dir: &Path) -> bool {
    const SOURCE: &[&str] = &[
        "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "kt", "rb", "php", "c", "h", "cc",
        "cpp", "hpp", "cs", "swift", "scala",
    ];
    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(3))
        .hidden(false)
        .require_git(false)
        .build();
    walker.flatten().any(|entry| {
        entry.file_type().is_some_and(|kind| kind.is_file())
            && entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| SOURCE.contains(&ext))
    })
}

/// Recent commits, as the sets of units each one touched.
///
/// One `git log` for both the recency hint and the co-change measure below;
/// they want the same data and it is not worth walking the history twice.
fn commits(root: &Path, units: &[(String, PathBuf)]) -> Vec<BTreeSet<String>> {
    let owner = ownership(units);
    let mut out: Vec<BTreeSet<String>> = Vec::new();

    // Uncommitted work first, as a commit of its own: it is the strongest
    // statement about what is in hand, and it has not been committed yet.
    if let Some(status) = git(root, &["status", "--porcelain"]) {
        let mut current = BTreeSet::new();
        for line in status.lines() {
            let path = line.get(3..).unwrap_or_default();
            let path = path.rsplit(" -> ").next().unwrap_or(path);
            if let Some(name) = owner_of(&owner, &root.join(path.trim())) {
                current.insert(name.to_owned());
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
    }

    let Some(log) = git(
        root,
        &["log", "-n", "300", "--name-only", "--pretty=format:%x00"],
    ) else {
        return out;
    };
    for chunk in log.split('\0') {
        let mut current = BTreeSet::new();
        for line in chunk.lines().filter(|line| !line.trim().is_empty()) {
            if let Some(name) = owner_of(&owner, &root.join(line.trim())) {
                current.insert(name.to_owned());
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    out
}

/// Units that change together but are not connected.
///
/// The import graph describes the architecture as *declared*. This describes it
/// as *practised*, and the two disagreeing is the interesting part: a pair that
/// must be edited in lockstep while nothing structural joins them is an
/// invariant maintained by hand — a duplicated constant, a wire format written
/// at both ends, a protocol two units implement independently. Nothing static
/// can see it, and it is exactly what a person would warn you about.
///
/// Only that direction is reported. The converse — imports but never co-changes
/// — reads like dead structure and is almost always the opposite: a stable
/// interface, which is the thing good design produces. Flagging it would
/// punish the healthiest dependencies in the project.
fn hidden_coupling(
    history: &[BTreeSet<String>],
    edges: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<HiddenCoupling> {
    let mut together: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut apart: BTreeMap<String, usize> = BTreeMap::new();

    for touched in history {
        // A sweep — a rename, a lint fix, a version bump — touches everything
        // and means nothing. Counting it would couple every pair in the
        // project to every other.
        if touched.len() > SWEEP_UNITS {
            continue;
        }
        for name in touched {
            *apart.entry(name.clone()).or_default() += 1;
        }
        for a in touched {
            for b in touched {
                if a < b {
                    *together.entry((a.clone(), b.clone())).or_default() += 1;
                }
            }
        }
    }

    let connected = |a: &str, b: &str| {
        edges.get(a).is_some_and(|to| to.contains(b))
            || edges.get(b).is_some_and(|to| to.contains(a))
    };
    let mut found: Vec<HiddenCoupling> = together
        .into_iter()
        .filter(|((a, b), count)| {
            if *count < MIN_CO_CHANGES || connected(a, b) {
                return false;
            }
            // Coincidence is not coupling. Jaccard rather than a ratio against
            // the quieter unit: the asymmetric form called a pair uncoupled
            // whenever one of them was simply busier, which describes every
            // real project.
            let seen_a = apart.get(a).copied().unwrap_or(0);
            let seen_b = apart.get(b).copied().unwrap_or(0);
            let union = seen_a + seen_b - count;
            union > 0 && (count * 100) / union >= MIN_OVERLAP_PERCENT
        })
        .map(|((a, b), commits)| HiddenCoupling { a, b, commits })
        .collect();
    found.sort_by(|x, y| y.commits.cmp(&x.commits).then(x.a.cmp(&y.a)));
    found.truncate(HIDDEN_SHOWN);
    found
}

/// Commits touching more units than this are sweeps, not coupling.
const SWEEP_UNITS: usize = 5;
/// Below this, a pair moving together is a coincidence.
const MIN_CO_CHANGES: usize = 3;
/// Share of the commits touching either unit that must touch both.
///
/// A quarter is deliberately lenient. Hand-maintained invariants are edited
/// together when they are edited at all, but each unit has a life of its own
/// besides — demanding a majority found nothing on any real repository, which
/// is the signature of a threshold that has stopped measuring anything.
const MIN_OVERLAP_PERCENT: usize = 25;
/// Enough to notice a pattern, not enough to bury the map.
const HIDDEN_SHOWN: usize = 3;

/// Units the working tree says this session is probably about.
///
/// A skeleton is otherwise the same on every session in a repository, which
/// makes it orientation but not a prior. Before anything has been touched
/// there is still a strong signal available and nothing else uses it: what is
/// uncommitted, and what was committed most recently. A branch with changes in
/// two crates is a session about those two crates far more often than not.
///
/// Failures are silent by construction — no repository, no `git`, a detached
/// worktree — because this only ever adds a hint, and a map without it is the
/// map we would have had anyway.
fn recently_touched(history: &[BTreeSet<String>]) -> Vec<String> {
    let mut ordered: Vec<String> = Vec::new();
    for touched in history {
        for name in touched {
            if !ordered.iter().any(|seen| seen == name) {
                ordered.push(name.clone());
            }
        }
        if ordered.len() >= ACTIVE_SHOWN {
            break;
        }
    }
    ordered.truncate(ACTIVE_SHOWN);
    ordered
}

/// Enough to point somewhere, not so many that it stops pointing.
const ACTIVE_SHOWN: usize = 4;

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Longest-prefix map from a file to the unit that owns it.
fn ownership(units: &[(String, PathBuf)]) -> Vec<(PathBuf, String)> {
    let mut owner: Vec<(PathBuf, String)> = units
        .iter()
        .map(|(name, path)| (path.clone(), name.clone()))
        .collect();
    // Longest path first, so a nested unit wins over its parent.
    owner.sort_by(|a, b| b.0.as_os_str().len().cmp(&a.0.as_os_str().len()));
    owner
}

fn owner_of<'a>(owner: &'a [(PathBuf, String)], file: &Path) -> Option<&'a str> {
    owner
        .iter()
        .find(|(path, _)| file.starts_with(path))
        .map(|(_, name)| name.as_str())
}

/// Dependencies a unit declares only for its own tests and benchmarks.
///
/// Path-based exclusion is not enough on its own: a dev-dependency is most
/// often used from a `#[cfg(test)]` block inside `src/`, which no filename
/// filter can see. The manifest is the one place the distinction is stated, so
/// it is read directly. `rten-testing` — internal test utilities — otherwise
/// ranks among the foundations of a real project on the strength of twelve
/// transitive dependents that only exist while testing.
///
/// Deliberately a scan and not a parse. Section headers and `name = …` /
/// `name.workspace = …` keys are all this needs, and a malformed manifest
/// should cost an over-reported edge rather than a missing map.
fn dev_only(units: &[(String, PathBuf)]) -> BTreeSet<(String, String)> {
    let mut excluded = BTreeSet::new();
    for (name, root) in units {
        let mut normal: BTreeSet<String> = BTreeSet::new();
        let mut dev: BTreeSet<String> = BTreeSet::new();

        if let Ok(text) = std::fs::read_to_string(root.join("Cargo.toml")) {
            let mut section = String::new();
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    section = line.trim_matches(['[', ']']).to_owned();
                    continue;
                }
                let Some((key, _)) = line.split_once('=') else {
                    continue;
                };
                let key = key.trim().trim_matches('"').split('.').next().unwrap_or("");
                if key.is_empty() {
                    continue;
                }
                if section.ends_with("dev-dependencies") {
                    dev.insert(import_key(key));
                } else if section.ends_with("dependencies")
                    || section.ends_with("build-dependencies")
                {
                    normal.insert(import_key(key));
                }
            }
        }
        if let Ok(text) = std::fs::read_to_string(root.join("package.json"))
            && let Ok(wire) = serde_json::from_str::<serde_json::Value>(&text)
        {
            for (field, into) in [("dependencies", &mut normal), ("devDependencies", &mut dev)] {
                if let Some(map) = wire.get(field).and_then(serde_json::Value::as_object) {
                    into.extend(map.keys().map(|key| import_key(key)));
                }
            }
        }

        for (other, _) in units {
            let key = import_key(other);
            if dev.contains(&key) && !normal.contains(&key) {
                excluded.insert((name.clone(), other.clone()));
            }
        }
    }
    excluded
}

/// Files that exist to exercise the code rather than to be it.
///
/// Their imports are real but say nothing about the architecture: a test
/// harness imported by every crate's test suite is not a foundation the system
/// rests on. Leaving them in put `rten-testing` — internal test utilities —
/// near the base of a real project map on the strength of eight importers.
fn is_supporting(file: &Path) -> bool {
    file.components().any(|part| {
        matches!(
            part.as_os_str().to_str(),
            Some("tests" | "benches" | "examples" | "test" | "__tests__")
        )
    })
}

/// Unit-level import edges.
///
/// Two sources, because neither alone is enough. `forward` resolves imports to
/// files, which within a Rust crate means `mod` declarations — it carries no
/// cross-crate edge at all, since `use artist_session::Recorder` names a
/// package rather than a path. Those land in `external` instead, as the raw
/// import string. Reading only `forward` produced a skeleton where twelve of
/// thirteen crates had a fan-in of zero, which is how this was noticed.
///
/// So: file edges for units that are directories inside one package, and
/// external import roots matched against unit names for units that are
/// packages. A project of either shape gets real edges.
fn aggregate_edges(
    graph: &artist_ast::deps::graph::DepGraph,
    owner: &[(PathBuf, String)],
    units: &[(String, PathBuf)],
) -> BTreeMap<String, BTreeSet<String>> {
    let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let by_key: BTreeMap<String, String> = units
        .iter()
        .map(|(name, _)| (import_key(name), name.clone()))
        .collect();

    let dev = dev_only(units);
    let mut link = |from: &str, to: &str, edges: &mut BTreeMap<String, BTreeSet<String>>| {
        if from != to && !dev.contains(&(from.to_owned(), to.to_owned())) {
            edges
                .entry(from.to_owned())
                .or_default()
                .insert(to.to_owned());
        }
    };

    for (file, outgoing) in &graph.forward {
        let Some(from) = owner_of(owner, file) else {
            continue;
        };
        if is_supporting(file) {
            continue;
        }
        for edge in outgoing {
            if let Some(to) = owner_of(owner, &edge.target) {
                link(from, to, &mut edges);
            }
        }
    }
    for (file, imports) in &graph.external {
        let Some(from) = owner_of(owner, file) else {
            continue;
        };
        if is_supporting(file) {
            continue;
        }
        for import in imports {
            if let Some(to) = by_key.get(&import_key(root_segment(import))) {
                let to = to.clone();
                link(from, &to, &mut edges);
            }
        }
    }
    edges
}

/// The first component of an import path, however the language spells it.
fn root_segment(import: &str) -> &str {
    let head = import.trim_start_matches(['@', '.']);
    let head = head.split("::").next().unwrap_or(head);
    let head = head.split('/').next().unwrap_or(head);
    head.split('.').next().unwrap_or(head)
}

/// `artist_session` and `artist-session` are the same unit under two spellings.
fn import_key(name: &str) -> String {
    name.replace('-', "_").to_ascii_lowercase()
}

fn assemble(
    units: Vec<(String, PathBuf)>,
    edges: &BTreeMap<String, BTreeSet<String>>,
    budget: usize,
    focus: &str,
) -> Skeleton {
    let cycles = mutual_groups(&units, edges);
    let layers = layer_of(&units, edges, &cycles);

    // Transitive, not direct. The question a map is asked is "what breaks if I
    // change this", and direct in-degree answers a shallower one: a unit with
    // two importers that the whole system sits on top of outranks one with six
    // leaves hanging off it.
    let reach = transitive(&units, edges);
    let mut blast: HashMap<String, usize> = HashMap::new();
    for (name, downstream) in &reach {
        for target in downstream {
            if target != name {
                *blast.entry(target.clone()).or_default() += 1;
            }
        }
    }

    let mut assembled: Vec<Unit> = units
        .into_iter()
        .map(|(name, root)| {
            let (role, role_inferred, role_hint) = role_of(&root);
            Unit {
                layer: layers.get(&name).copied().unwrap_or(0),
                blast_radius: blast.get(&name).copied().unwrap_or(0),
                cycle_with: cycles
                    .get(&name)
                    .map(|group| {
                        group
                            .iter()
                            .filter(|other| **other != name)
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default(),
                name,
                root,
                role,
                role_inferred,
                role_hint,
            }
        })
        .collect();

    // Dependency order, then most-depended-upon, then by name so the output is
    // the same for the same tree.
    assembled.sort_by(|a, b| {
        a.layer
            .cmp(&b.layer)
            .then(b.blast_radius.cmp(&a.blast_radius))
            .then(a.name.cmp(&b.name))
    });

    // Admit units in blast-radius order until the budget is spent, then put
    // the survivors back into dependency order. Selecting by importance and
    // presenting by structure are different jobs.
    let mut by_importance = assembled.clone();
    by_importance.sort_by(|a, b| {
        // What was asked about first, then what most depends on it. A unit the
        // request names is worth more than a foundation it will never open.
        relevance(focus, b)
            .cmp(&relevance(focus, a))
            .then(b.blast_radius.cmp(&a.blast_radius))
            .then(a.name.cmp(&b.name))
    });
    let mut spent = 0usize;
    let mut kept: Vec<&Unit> = Vec::new();
    for unit in &by_importance {
        let cost = row_cost(unit);
        // Always admit one, so a pathological first row cannot produce a map
        // with nothing in it.
        if !kept.is_empty() && spent + cost > budget {
            continue;
        }
        spent += cost;
        kept.push(unit);
    }
    let omitted = assembled.len() - kept.len();
    let keep: std::collections::HashSet<&str> =
        kept.iter().map(|unit| unit.name.as_str()).collect();
    assembled.retain(|unit| keep.contains(unit.name.as_str()));

    Skeleton {
        units: assembled,
        omitted,
        // Filled by `build`, which has the repository; `assemble` sees only the
        // graph.
        active: Vec::new(),
        hidden: Vec::new(),
    }
}

/// What one rendered row costs, near enough. Name, radius, role, and the
/// separators between them.
fn row_cost(unit: &Unit) -> usize {
    let role = unit.role.as_ref().map(String::len).unwrap_or(24);
    unit.name.len() + role + 12 + unit.cycle_with.iter().map(|n| n.len() + 2).sum::<usize>()
}

/// Units that reach each other, grouped. Small and iterative rather than
/// Tarjan: the unit count is in the tens, and the file-level SCC pass already
/// exists for the case that is not.
fn mutual_groups(
    units: &[(String, PathBuf)],
    edges: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeMap<String, Vec<String>> {
    let reach = transitive(units, edges);
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, _) in units {
        let mut group: Vec<String> = units
            .iter()
            .map(|(other, _)| other)
            .filter(|other| {
                *other != name
                    && reach.get(name).is_some_and(|set| set.contains(*other))
                    && reach.get(*other).is_some_and(|set| set.contains(name))
            })
            .cloned()
            .collect();
        if !group.is_empty() {
            group.push(name.clone());
            group.sort();
            groups.insert(name.clone(), group);
        }
    }
    groups
}

fn transitive(
    units: &[(String, PathBuf)],
    edges: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut reach: BTreeMap<String, BTreeSet<String>> = units
        .iter()
        .map(|(name, _)| (name.clone(), edges.get(name).cloned().unwrap_or_default()))
        .collect();
    // Fixed point. Bounded by unit count, which is small.
    for _ in 0..units.len() {
        let mut changed = false;
        for (name, _) in units {
            let current = reach.get(name).cloned().unwrap_or_default();
            let mut grown = current.clone();
            for target in &current {
                if let Some(onward) = reach.get(target) {
                    grown.extend(onward.iter().cloned());
                }
            }
            if grown != current {
                reach.insert(name.clone(), grown);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    reach
}

/// Depth in the dependency order: one more than the deepest thing it imports.
///
/// Units in a cycle share a layer, because there is no order between them —
/// pretending otherwise would put one above the other arbitrarily.
fn layer_of(
    units: &[(String, PathBuf)],
    edges: &BTreeMap<String, BTreeSet<String>>,
    cycles: &BTreeMap<String, Vec<String>>,
) -> HashMap<String, usize> {
    let mut layers: HashMap<String, usize> = units.iter().map(|(n, _)| (n.clone(), 0)).collect();
    for _ in 0..units.len() {
        let mut changed = false;
        for (name, _) in units {
            let peers = cycles.get(name);
            let deepest = edges
                .get(name)
                .map(|targets| {
                    targets
                        .iter()
                        .filter(|target| !peers.is_some_and(|group| group.contains(target)))
                        .filter_map(|target| layers.get(target).map(|depth| depth + 1))
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            if layers.get(name).is_some_and(|current| *current < deepest) {
                layers.insert(name.clone(), deepest);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    layers
}

/// A unit's one-line description, and where to put one if it has none.
///
/// Prefers what the author wrote. Falls back to what the unit exports, which is
/// the difference between a usable map and a directory listing on the many
/// projects that carry no module docs at all — a real run against cozo found
/// ten of ten crates undocumented, and the whole skeleton degraded to names.
///
/// The two are never conflated: an inferred role says "exports …", because a
/// list of public names is evidence about a unit, not a statement of intent,
/// and presenting it as the latter would be the same fabrication as inventing
/// prose.
fn role_of(root: &Path) -> (Option<String>, bool, Option<PathBuf>) {
    const ENTRIES: &[&str] = &[
        "src/lib.rs",
        "src/main.rs",
        "lib.rs",
        "mod.rs",
        "__init__.py",
        "index.ts",
        "index.js",
    ];
    for candidate in ENTRIES {
        let path = root.join(candidate);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(role) = first_doc_line(&text) {
            return (Some(role), false, None);
        }
        // Found the entry point but it says nothing about itself. Ask the code.
        return match exported_role(root) {
            Some(role) => (Some(role), true, Some(path)),
            None => (None, false, Some(path)),
        };
    }
    (exported_role(root).map(|role| (role, true)))
        .map(|(role, inferred)| (Some(role), inferred, None))
        .unwrap_or((None, false, None))
}

/// A description assembled from what the unit actually exports.
///
/// Names only, and few of them: the point is to say what kind of thing this is,
/// which the first handful of public items conveys and a full API listing
/// buries.
fn exported_role(root: &Path) -> Option<String> {
    let entries = artist_ast::surface::resolve_surface(root, &Default::default()).ok()?;
    if entries.is_empty() {
        return None;
    }
    // Types before functions. A crate is characterised by the things it
    // defines, not by the verbs attached to them: `DbInstance, RocksDb` says
    // what cozo-core is, where `new, new_with_str` — the first four entries in
    // declaration order — say nothing at all.
    let mut names: Vec<&str> = Vec::new();
    for wanted_type in [true, false] {
        for entry in &entries {
            // A markdown document resolves to its headings. They are prose, not
            // an interface, and reporting them as exports produced
            // "exports Autoflow prior-work audit (agent report, 2026-07-10)".
            if matches!(
                entry.kind,
                artist_ast::core::DeclarationKind::Heading
                    | artist_ast::core::DeclarationKind::CodeBlock
            ) {
                continue;
            }
            if is_type(&entry.kind) != wanted_type {
                continue;
            }
            let name = entry
                .qualified_path
                .rsplit("::")
                .next()
                .unwrap_or(&entry.qualified_path);
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    let shown: Vec<&str> = names.iter().copied().take(EXPORTS_SHOWN).collect();
    if shown.is_empty() {
        return None;
    }
    let more = names.len().saturating_sub(shown.len());
    let listed = trim_to_width(&shown.join(", "));
    Some(if more > 0 {
        format!("exports {listed}, +{more} more")
    } else {
        format!("exports {listed}")
    })
}

/// Declarations that name a thing rather than an action.
fn is_type(kind: &artist_ast::core::DeclarationKind) -> bool {
    use artist_ast::core::DeclarationKind as K;
    matches!(
        kind,
        K::Class | K::Struct | K::Interface | K::Record | K::Enum | K::Namespace
    )
}

/// Enough to characterise, not enough to enumerate.
const EXPORTS_SHOWN: usize = 4;

/// The unit's own top-level modules, with whatever each says about itself.
///
/// This is the altitude the one-line skeleton cannot reach. `artist-computer —
/// Generalized computer use.` is true and useless once you are inside it: the
/// subsystem has a stage, surfaces, a ladder and an OCR path, and none of that
/// is derivable from the crate's own sentence. Naming them costs a line and is
/// spent only where the session actually went.
fn modules_of(root: &Path) -> Vec<(String, Option<String>)> {
    let mut found: BTreeMap<String, Option<String>> = BTreeMap::new();
    for base in ["src", "."] {
        let Ok(entries) = std::fs::read_dir(root.join(base)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_stem().and_then(|n| n.to_str()) else {
                continue;
            };
            // The crate root describes the crate, not a module within it.
            if matches!(name, "lib" | "main" | "mod" | "__init__" | "index") {
                continue;
            }
            if name.starts_with('.') {
                continue;
            }
            let doc = if path.is_dir() {
                ["mod.rs", "__init__.py", "index.ts"]
                    .iter()
                    .find_map(|entry| std::fs::read_to_string(path.join(entry)).ok())
                    .and_then(|text| first_doc_line(&text))
            } else if path
                .extension()
                .is_some_and(|ext| ext != "toml" && ext != "md")
            {
                std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| first_doc_line(&text))
            } else {
                continue;
            };
            found.entry(name.to_owned()).or_insert(doc);
        }
        if !found.is_empty() {
            break;
        }
    }
    found.into_iter().collect()
}

/// Modules named when entering a unit.
const MODULES_SHOWN: usize = 8;

/// Immediate dependencies named when entering a unit.
const REGION_USES_SHOWN: usize = 5;

/// The first prose sentence of a leading module doc comment.
///
/// Reads a paragraph rather than a line, for two reasons the real output made
/// obvious. A sentence wrapped across two `//!` lines was being cut at the wrap
/// and presented as if complete — `"…that stores, prints and"` with no
/// indication anything followed. And a crate whose doc opens with a markdown
/// heading yielded its own name as its description: `hashline-tools` was
/// documented as "hashline-tools".
fn first_doc_line(text: &str) -> Option<String> {
    let mut paragraph = String::new();
    for line in text.lines().take(60) {
        let trimmed = line.trim();
        if trimmed.starts_with("#!") {
            continue;
        }
        let Some(body) = trimmed
            .strip_prefix("//!")
            .or_else(|| trimmed.strip_prefix("///"))
            .or_else(|| trimmed.strip_prefix("\"\"\""))
            .or_else(|| trimmed.strip_prefix("/**"))
            .or_else(|| trimmed.strip_prefix("*"))
        else {
            // Left the doc comment. Anything gathered is all there is.
            if paragraph.is_empty() && trimmed.is_empty() {
                continue;
            }
            break;
        };
        let body = body.trim().trim_end_matches("\"\"\"").trim();
        // A heading names the unit; it does not describe it. Rules and blank
        // lines separate paragraphs.
        let decoration =
            body.starts_with('#') || body.chars().all(|c| c == '=' || c == '-' || c == '#');
        if body.is_empty() || decoration {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(body);
        if paragraph.contains(". ") || paragraph.ends_with('.') {
            break;
        }
    }
    (!paragraph.is_empty()).then(|| trim_to_sentence(&paragraph))
}

/// Markdown decoration, removed. Link syntax and emphasis markers are written
/// for a renderer that is not present here, and a link target is pure cost.
///
/// Underscores are *not* stripped, though they mark emphasis in markdown.
/// Doc comments are full of snake_case identifiers, and removing the
/// underscore corrupts the name: `rten_tensor` became "rtentensor" in a real
/// run. A stray `_emphasis_` reads fine; a mangled identifier does not.
fn strip_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '`' => {}
            '[' => {
                // Keep the label; drop whatever addresses it. Covers inline
                // `[label](target)`, reference `[label][id]` — which produced
                // "ONNXonnx" before this existed — and bare rustdoc `[Item]`.
                for inner in chars.by_ref() {
                    if inner == ']' {
                        break;
                    }
                    out.push(inner);
                }
                let closer = match chars.peek() {
                    Some('(') => Some(')'),
                    Some('[') => Some(']'),
                    _ => None,
                };
                if let Some(closer) = closer {
                    for inner in chars.by_ref() {
                        if inner == closer {
                            break;
                        }
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One line, one sentence. The value is in being glanceable.
fn trim_to_sentence(text: &str) -> String {
    let stripped = strip_markdown(text);
    let text = stripped.trim_start_matches('#').trim();
    let sentence = match text.find(". ") {
        Some(end) => &text[..=end],
        None => text,
    };
    trim_to_width(sentence)
}

/// Cut to one line's worth, on a boundary, marked.
///
/// Applies to inferred roles as well as written ones: four JNI exports run to
/// two hundred characters and wrecked the column layout on a real repository.
fn trim_to_width(text: &str) -> String {
    trim_to(text, ROLE_WIDTH)
}

fn trim_to(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let cut: String = text.chars().take(width).collect();
    let cut = cut
        .rsplit_once(|c: char| c == ' ' || c == ',')
        .map(|(head, _)| head)
        .unwrap_or(&cut);
    format!("{}…", cut.trim_end_matches(','))
}

const ROLE_WIDTH: usize = 78;

/// Module lists get more room than a role: they are the payload of an
/// introduction rather than a column in a table.
const MODULE_WIDTH: usize = 220;

impl Skeleton {
    /// Render for the model.
    ///
    /// Dependency order top to bottom, so reading it in sequence is reading the
    /// architecture: everything a line depends on has already appeared above
    /// it.
    pub fn render(&self) -> String {
        if self.units.is_empty() {
            return String::new();
        }
        let width = self
            .units
            .iter()
            .map(|unit| unit.name.chars().count())
            .max()
            .unwrap_or(0)
            .min(28);

        let mut out = String::from(
            "[project shape — a unit uses only what is above it; ←n = units that break if it changes]\n",
        );
        let mut last_layer = usize::MAX;
        for unit in &self.units {
            if unit.layer != last_layer {
                out.push_str(&format!("\n  ── layer {} ──\n", unit.layer));
                last_layer = unit.layer;
            }
            let role = match (&unit.role, unit.role_inferred, &unit.role_hint) {
                (Some(role), false, _) => role.clone(),
                (Some(role), true, _) => role.clone(),
                (None, _, Some(path)) => {
                    format!("(undocumented — add a doc comment to {})", path.display())
                }
                (None, _, None) => "(undocumented)".to_owned(),
            };
            out.push_str(&format!(
                "  {:width$}  ←{:<3} {role}\n",
                unit.name,
                unit.blast_radius,
                width = width
            ));
            if !unit.cycle_with.is_empty() {
                out.push_str(&format!(
                    "  {:width$}       ⟲ mutually depends on {}\n",
                    "",
                    unit.cycle_with.join(", "),
                    width = width
                ));
            }
        }
        if self.omitted > 0 {
            out.push_str(&format!(
                "\n  +{} more unit(s), least depended-upon, not shown.\n",
                self.omitted
            ));
        }
        if !self.hidden.is_empty() {
            out.push_str("\n  changed together but not connected — an invariant kept by hand:\n");
            for pair in &self.hidden {
                out.push_str(&format!(
                    "  {} ↔ {} ({} commits)\n",
                    pair.a, pair.b, pair.commits
                ));
            }
        }
        if !self.active.is_empty() {
            out.push_str(&format!(
                "\n  recent work is in: {}\n",
                self.active.join(", ")
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rust_doc_comment_becomes_the_role() {
        assert_eq!(
            first_doc_line("//! Does the thing.\n//! More detail.\npub fn x() {}").as_deref(),
            Some("Does the thing.")
        );
    }

    /// A licence header or a `#!` line must not be mistaken for the role.
    #[test]
    fn leading_noise_is_skipped() {
        assert_eq!(
            first_doc_line("#!/usr/bin/env python\n\n\"\"\"Runs the importer.\"\"\"").as_deref(),
            Some("Runs the importer.")
        );
    }

    #[test]
    fn a_file_without_a_doc_comment_has_no_role() {
        assert!(first_doc_line("pub fn x() {}\n").is_none());
    }

    /// A heading names the unit, it does not describe it. Real case: the
    /// generated skeleton documented `hashline-tools` as "hashline-tools".
    #[test]
    fn a_heading_is_skipped_in_favour_of_the_prose_below_it() {
        assert_eq!(
            first_doc_line("//! # hashline-tools\n//!\n//! Anchors for lines.").as_deref(),
            Some("Anchors for lines.")
        );
    }

    /// A sentence wrapped across two comment lines must be joined, not cut at
    /// the wrap and presented as though it were whole.
    #[test]
    fn a_wrapped_sentence_is_rejoined() {
        assert_eq!(
            first_doc_line("//! One representation that stores,\n//! prints and parses.")
                .as_deref(),
            Some("One representation that stores, prints and parses.")
        );
    }

    /// Markdown is written for a renderer that is not here; the link target is
    /// pure cost. Real case: a crate described as
    /// "Excised from [RealArtist](https://github.com/): stable **mnemonic…".
    #[test]
    fn markdown_decoration_is_stripped_from_a_role() {
        let role = first_doc_line("//! Excised from [RealArtist](https://x.com/): **anchors**.")
            .expect("a role");
        assert_eq!(role, "Excised from RealArtist: anchors.");
    }

    /// Both from real crates. `[ONNX][onnx]` yielded "ONNXonnx"; stripping the
    /// underscore in `rten_tensor` yielded "rtentensor". A corrupted identifier
    /// is worse than a stray emphasis marker, so underscores stay.
    #[test]
    fn identifiers_and_reference_links_survive_stripping() {
        assert_eq!(
            first_doc_line("//! This crate provides a parser for [ONNX][onnx] ML model files.")
                .as_deref(),
            Some("This crate provides a parser for ONNX ML model files.")
        );
        assert_eq!(
            first_doc_line("//! rten_tensor provides arrays, called _tensors_ here.").as_deref(),
            Some("rten_tensor provides arrays, called _tensors_ here.")
        );
    }

    #[test]
    fn a_long_role_is_cut_at_a_word_boundary() {
        let long = format!("//! {}", "word ".repeat(40));
        let role = first_doc_line(&long).expect("a role");
        assert!(role.chars().count() <= ROLE_WIDTH + 1, "{role}");
        assert!(role.ends_with('…'));
        assert!(!role.contains("wor…"), "cut mid-word: {role}");
    }

    fn units(names: &[&str]) -> Vec<(String, PathBuf)> {
        names
            .iter()
            .map(|n| ((*n).to_owned(), PathBuf::from(format!("/p/{n}"))))
            .collect()
    }

    fn assemble_test(
        units: Vec<(String, PathBuf)>,
        edges: &BTreeMap<String, BTreeSet<String>>,
        budget: usize,
    ) -> Skeleton {
        assemble(units, edges, budget, "")
    }

    fn edges(pairs: &[(&str, &str)]) -> BTreeMap<String, BTreeSet<String>> {
        let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (from, to) in pairs {
            map.entry((*from).to_owned())
                .or_default()
                .insert((*to).to_owned());
        }
        map
    }

    /// The ordering claim the render makes: everything a unit needs appears
    /// above it. If this breaks, the document is actively misleading.
    #[test]
    fn a_dependency_is_always_listed_before_its_dependent() {
        let skeleton = assemble_test(
            units(&["app", "core", "util"]),
            &edges(&[("app", "core"), ("core", "util")]),
            DEFAULT_BUDGET,
        );
        let order: Vec<&str> = skeleton.units.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(order, ["util", "core", "app"]);
    }

    #[test]
    fn blast_radius_counts_everything_downstream() {
        let skeleton = assemble_test(
            units(&["a", "b", "shared"]),
            &edges(&[("a", "shared"), ("b", "shared")]),
            DEFAULT_BUDGET,
        );
        let shared = skeleton
            .units
            .iter()
            .find(|u| u.name == "shared")
            .expect("shared present");
        assert_eq!(shared.blast_radius, 2);
    }

    /// Two units that need each other have no order between them, and inventing
    /// one would put a false claim in the output.
    #[test]
    fn a_cycle_is_reported_and_shares_a_layer() {
        let skeleton = assemble_test(
            units(&["left", "right"]),
            &edges(&[("left", "right"), ("right", "left")]),
            DEFAULT_BUDGET,
        );
        let left = skeleton.units.iter().find(|u| u.name == "left").unwrap();
        let right = skeleton.units.iter().find(|u| u.name == "right").unwrap();
        assert_eq!(left.cycle_with, ["right"]);
        assert_eq!(right.cycle_with, ["left"]);
        assert_eq!(left.layer, right.layer, "a cycle has no internal order");
        assert!(skeleton.render().contains("mutually depends on"));
    }

    /// A cycle must not make the layering loop forever.
    #[test]
    fn a_three_way_cycle_terminates() {
        let skeleton = assemble_test(
            units(&["a", "b", "c"]),
            &edges(&[("a", "b"), ("b", "c"), ("c", "a")]),
            DEFAULT_BUDGET,
        );
        assert_eq!(skeleton.units.len(), 3);
        for unit in &skeleton.units {
            assert_eq!(unit.cycle_with.len(), 2, "{unit:?}");
        }
    }

    /// The budget bounds the rendered size, keeps what is load-bearing, and
    /// says how much it dropped — silently showing a subset would read as
    /// showing everything.
    #[test]
    fn an_oversized_project_fills_the_budget_and_says_what_it_dropped() {
        let names: Vec<String> = (0..40).map(|i| format!("unit{i:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let pairs: Vec<(&str, &str)> = refs[1..].iter().map(|n| (*n, "unit00")).collect();

        let skeleton = assemble_test(units(&refs), &edges(&pairs), 300);
        assert!(skeleton.omitted > 0, "40 units must not fit in 300 chars");
        assert!(
            skeleton.units.iter().any(|u| u.name == "unit00"),
            "the unit everything depends on must survive"
        );
        let rendered = skeleton.render();
        assert!(rendered.contains("more unit(s)"), "{rendered}");
        assert!(rendered.len() < 900, "budget overrun: {}", rendered.len());
    }

    /// Depth beats breadth. A unit the whole system rests on outranks one with
    /// more direct importers but nothing behind them.
    #[test]
    fn transitive_dependents_outrank_direct_importers() {
        let skeleton = assemble_test(
            units(&["base", "mid", "top", "popular", "l1", "l2", "l3"]),
            &edges(&[
                ("mid", "base"),
                ("top", "mid"),
                ("l1", "popular"),
                ("l2", "popular"),
                ("l3", "popular"),
            ]),
            DEFAULT_BUDGET,
        );
        let radius = |name: &str| {
            skeleton
                .units
                .iter()
                .find(|u| u.name == name)
                .unwrap()
                .blast_radius
        };
        // `base` has one direct importer, `popular` has three — but changing
        // `base` reaches `mid` and `top`.
        assert_eq!(radius("base"), 2);
        assert_eq!(radius("popular"), 3);
    }

    /// Same tree, same output — the render is read by a model whose context
    /// should not churn because a directory listing came back differently.
    #[test]
    fn the_render_is_deterministic() {
        let build = || {
            assemble_test(
                units(&["a", "b", "c"]),
                &edges(&[("a", "b"), ("b", "c")]),
                DEFAULT_BUDGET,
            )
            .render()
        };
        assert_eq!(build(), build());
    }

    /// Never invent a description. An admitted gap is recoverable; a plausible
    /// wrong one is not, because nothing later contradicts it.
    #[test]
    fn a_missing_role_is_admitted_not_invented() {
        let skeleton = assemble_test(units(&["mystery"]), &edges(&[]), DEFAULT_BUDGET);
        let rendered = skeleton.render();
        assert!(rendered.contains("undocumented"), "{rendered}");
    }
}

/// What the architecture looked like when the session opened.
///
/// The skeleton is issued once and never reissued, which leaves a gap: a
/// session long enough to change the architecture would be running on a map
/// that has quietly stopped being true. This is the other half — the baseline
/// a change is noticed against.
///
/// Deliberately not a whole skeleton rebuild per edit. Unit discovery walks the
/// tree and the render costs tokens; neither is worth paying on every commit.
/// What actually changes is one file's imports, and the graph already records
/// those per file, so a delta is a set lookup against what was recorded here.
#[derive(Clone, Debug)]
pub struct Baseline {
    root: PathBuf,
    units: Vec<(String, PathBuf)>,
    owner: Vec<(PathBuf, String)>,
    edges: BTreeMap<String, BTreeSet<String>>,
}

/// Capture the architecture as it currently stands.
pub fn baseline(root: &Path) -> Option<Baseline> {
    let graph = artist_ast::graph_cache::shared::get_or_init(root).ok()?;
    let units = discover_units(root);
    if units.len() < 2 {
        return None;
    }
    let owner = ownership(&units);
    let edges = aggregate_edges(&graph.deps, &owner, &units);
    Some(Baseline {
        root: root.to_path_buf(),
        units,
        owner,
        edges,
    })
}

impl Baseline {
    /// An introduction to the unit `file` belongs to.
    ///
    /// The project shape names every unit in one line each, which is enough to
    /// know a place exists and not enough to work in it. This is the second
    /// altitude, and it is spent only where it is earned: the first time a
    /// session opens a file in a unit it has not been in before.
    ///
    /// That trigger is the whole reason it can afford to say more than the
    /// skeleton does. A map has to guess what will be needed; this already
    /// knows, because the model just went there.
    /// Which unit owns `file`, if any.
    pub fn unit_of(&self, file: &Path) -> Option<String> {
        owner_of(&self.owner, file).map(str::to_owned)
    }

    pub fn region_note(&self, file: &Path) -> Option<String> {
        let unit = owner_of(&self.owner, file)?;
        let (_, root) = self.units.iter().find(|(name, _)| name == unit)?;
        let (role, _, _) = role_of(root);

        let dependents = transitive(&self.units, &self.edges)
            .iter()
            .filter(|(name, reach)| *name != unit && reach.contains(unit))
            .count();
        // How entangled, not just whether. Importing forty items from a crate
        // is a different relationship from importing one, and the count is
        // already in the graph.
        let weights = artist_ast::graph_cache::shared::get_or_init(&self.root)
            .ok()
            .map(|graph| self.outgoing(&graph.deps, unit).1)
            .unwrap_or_default();
        let uses: Vec<String> = self
            .edges
            .get(unit)
            .map(|targets| {
                targets
                    .iter()
                    .map(|to| match weights.get(to) {
                        Some(n) if *n > 1 => format!("{to} ({n} items)"),
                        _ => to.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut note = format!("entering {unit}");
        if let Some(role) = role {
            note.push_str(&format!(" — {role}"));
        }
        // The number that changes behaviour: how careful to be here.
        if dependents > 0 {
            note.push_str(&format!("\n {dependents} unit(s) break if it changes"));
        }
        if !uses.is_empty() {
            let shown: Vec<&str> = uses
                .iter()
                .map(String::as_str)
                .take(REGION_USES_SHOWN)
                .collect();
            let more = uses.len().saturating_sub(shown.len());
            note.push_str(&format!(
                "{} it uses {}{}",
                if dependents > 0 { ";" } else { "\n" },
                shown.join(", "),
                if more > 0 {
                    format!(", +{more} more")
                } else {
                    String::new()
                }
            ));
        }
        let modules = modules_of(root);
        if !modules.is_empty() {
            let shown: Vec<String> = modules
                .iter()
                .take(MODULES_SHOWN)
                .map(|(name, doc)| match doc {
                    Some(doc) => format!("{name} — {doc}"),
                    None => name.clone(),
                })
                .collect();
            let more = modules.len().saturating_sub(shown.len());
            let listed = trim_to(&shown.join("; "), MODULE_WIDTH);
            // Count what the width cut as well as what the cap did, or the
            // tally understates how much is not shown.
            let hidden = more + shown.len().saturating_sub(listed.matches("; ").count() + 1);
            note.push_str(&format!("\n modules: {listed}"));
            if hidden > 0 {
                note.push_str(&format!(" +{hidden} more"));
            }
        }
        Some(note)
    }

    /// Architectural changes caused by editing `file`, if any.
    ///
    /// Absorbs what it reports, so a change is announced once rather than on
    /// every subsequent commit. Silence is the overwhelmingly common case and
    /// has to stay free — most edits do not move the architecture at all, and a
    /// note that fires constantly stops being read.
    /// A fingerprint of the whole derived architecture.
    ///
    /// This is what makes the delta complete rather than merely thorough.
    /// Enumerating what can change — an edge, a unit, a cycle — is a list I
    /// wrote, and a list I wrote is a list I can be wrong about; a category I
    /// did not think of goes unreported and the frozen map rots exactly as a
    /// hand-maintained one does.
    ///
    /// Hashing every derived fact inverts that. The description of a change is
    /// still enumerated and still incomplete, but the *detection* of one is
    /// not: anything that moves the architecture moves this, and a mismatch we
    /// cannot describe is reported as a mismatch we cannot describe. Being
    /// vague is recoverable. Being silent is not.
    fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for (name, _) in &self.units {
            name.hash(&mut hasher);
        }
        for (from, targets) in &self.edges {
            from.hash(&mut hasher);
            for to in targets {
                to.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// Re-derive everything cheaply enough to do on every commit.
    ///
    /// Deliberately does not re-walk the tree: the graph is already cached and
    /// mtime-checked, so aggregating edges is hashmap iteration. Unit
    /// membership is handled by the two cheap checks in `observe` — a root that
    /// stopped existing, and a manifest that was touched — which between them
    /// cover every way a unit can appear or vanish.
    fn rederive(&mut self, graph: &artist_ast::deps::graph::DepGraph) {
        self.edges = aggregate_edges(graph, &self.owner, &self.units);
    }

    pub fn observe(&mut self, root: &Path, file: &Path) -> Vec<String> {
        let Ok(graph) = artist_ast::graph_cache::shared::get_or_init(root) else {
            return Vec::new();
        };
        let mut notes = Vec::new();

        // A unit that has gone. Cheapest check here and the most consequential
        // to miss: everything the map said about it, and everything that
        // depended on it, is now wrong.
        let vanished: Vec<String> = self
            .units
            .iter()
            .filter(|(_, path)| !path.exists())
            .map(|(name, _)| name.clone())
            .collect();
        for name in &vanished {
            notes.push(format!("{name} is gone from the project"));
            self.forget_unit(name);
        }

        // A unit that has appeared. Re-discovery walks the tree, so it is not
        // run on every edit — only when the edit could have created one, which
        // means a manifest was touched or the file belongs to nothing known.
        let touched_manifest = file
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| MANIFESTS.contains(&n));
        let orphan = owner_of(&self.owner, file).is_none();
        if touched_manifest || orphan {
            for (name, path) in discover_units(root) {
                if !self.units.iter().any(|(known, _)| *known == name) {
                    notes.push(format!("{name} is new to the project"));
                    self.units.push((name, path));
                }
            }
            self.owner = ownership(&self.units);
        }

        // Everything this unit imports, recomputed from the graph rather than
        // from the one file that changed.
        //
        // Per-file was wrong in a way that could not be patched: a unit-level
        // edge survives while *any* file in the unit still carries the import,
        // so a removal is only visible from the whole unit. Recomputing the set
        // yields additions and removals together and cannot disagree with
        // itself.
        let before = self.fingerprint();
        let previous = self.edges.clone();
        self.rederive(&graph.deps);
        if self.fingerprint() == before {
            // Nothing derived moved. The overwhelmingly common case, and it has
            // to cost nothing.
            return notes;
        }

        // Something changed. Describe as much of it as we know how to.
        let described = notes.len();
        let mut added: Vec<(String, String)> = Vec::new();
        let mut removed: Vec<(String, String)> = Vec::new();
        let empty = BTreeSet::new();
        for (from, current) in &self.edges {
            let known = previous.get(from).unwrap_or(&empty);
            added.extend(
                current
                    .difference(known)
                    .map(|to| (from.clone(), to.clone())),
            );
            removed.extend(
                known
                    .difference(current)
                    .map(|to| (from.clone(), to.clone())),
            );
        }
        for (from, known) in &previous {
            if !self.edges.contains_key(from) {
                removed.extend(known.iter().map(|to| (from.clone(), to.clone())));
            }
        }

        // Recomputed after the update, so a newly closed — or newly broken —
        // loop is visible.
        let mutual = transitive(&self.units, &self.edges);
        for (from, to) in added {
            // A cycle is the more important fact and subsumes the plain edge:
            // a unit inside a ring cannot be lifted out of it alone.
            if mutual.get(&to).is_some_and(|onward| onward.contains(&from)) {
                notes.push(format!(
                    "{from} and {to} now depend on each other — neither can be \
                     extracted without the other"
                ));
            } else {
                notes.push(format!("new dependency: {from} → {to}"));
            }
        }
        for (from, to) in removed {
            notes.push(format!("{from} no longer depends on {to}"));
        }
        // The fingerprint moved and nothing above accounted for it.
        //
        // Unreachable today: the summariser covers every field the fingerprint
        // includes. It stays because that coincidence is not a guarantee — the
        // moment a field is added here without a matching description, this is
        // what keeps the change from passing in silence. Vague is recoverable;
        // silent is what rots a frozen map.
        if notes.len() == described {
            notes.push(
                "the project structure changed in a way not summarised here — \
                 re-check with code_deps or code_cycles"
                    .to_owned(),
            );
        }
        notes
    }

    /// Every unit `from` imports, according to the current graph.
    ///
    /// Also returns how many distinct symbols back each edge, which is what
    /// separates a unit that borrows one helper from one built on top of
    /// another.
    fn outgoing(
        &self,
        graph: &artist_ast::deps::graph::DepGraph,
        from: &str,
    ) -> (BTreeSet<String>, BTreeMap<String, usize>) {
        let by_key: BTreeMap<String, String> = self
            .units
            .iter()
            .map(|(name, _)| (import_key(name), name.clone()))
            .collect();
        let dev = dev_only(&self.units);
        let mut reached = BTreeSet::new();
        let mut symbols: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

        let mine = |file: &Path| owner_of(&self.owner, file) == Some(from) && !is_supporting(file);

        for (file, outgoing) in &graph.forward {
            if !mine(file) {
                continue;
            }
            for edge in outgoing {
                if let Some(to) = owner_of(&self.owner, &edge.target)
                    && to != from
                    && !dev.contains(&(from.to_owned(), to.to_owned()))
                {
                    reached.insert(to.to_owned());
                }
            }
        }
        for (file, imports) in &graph.external {
            if !mine(file) {
                continue;
            }
            for import in imports {
                if let Some(to) = by_key.get(&import_key(root_segment(import)))
                    && to != from
                    && !dev.contains(&(from.to_owned(), to.clone()))
                {
                    reached.insert(to.clone());
                    symbols
                        .entry(to.clone())
                        .or_default()
                        .insert(import.to_owned());
                }
            }
        }
        let weights = symbols
            .into_iter()
            .map(|(to, names)| (to, names.len()))
            .collect();
        (reached, weights)
    }

    /// Move the recorded architecture out from under the summariser, so a test
    /// can produce a change none of the described categories accounts for.
    #[doc(hidden)]
    pub fn corrupt_for_test(&mut self) {
        self.edges
            .entry("app".to_owned())
            .or_default()
            .insert("a-unit-that-was-never-here".to_owned());
    }

    /// Drop a unit and every edge that mentioned it.
    fn forget_unit(&mut self, name: &str) {
        self.units.retain(|(known, _)| known != name);
        self.owner = ownership(&self.units);
        self.edges.remove(name);
        for targets in self.edges.values_mut() {
            targets.remove(name);
        }
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;

    fn many_units(count: usize) -> Vec<(String, PathBuf)> {
        (0..count)
            .map(|i| {
                (
                    format!("unit{i:02}"),
                    PathBuf::from(format!("/p/unit{i:02}")),
                )
            })
            .collect()
    }

    /// A project too large to show whole should show the part the request is
    /// about, not an arbitrary top slice by dependency count.
    #[test]
    fn the_request_decides_what_survives_a_squeeze() {
        let units = many_units(30);
        let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        // Everything depends on unit00, so on structure alone it wins and the
        // rest are interchangeable.
        for (name, _) in units.iter().skip(1) {
            edges
                .entry(name.clone())
                .or_default()
                .insert("unit00".to_owned());
        }

        let blind = assemble(units.clone(), &edges, 200, "");
        let asked = assemble(units, &edges, 200, "please look at unit17 for me");

        assert!(blind.omitted > 0 && asked.omitted > 0, "both must squeeze");
        assert!(
            asked.units.iter().any(|u| u.name == "unit17"),
            "the unit the request names must survive: {:?}",
            asked.units.iter().map(|u| &u.name).collect::<Vec<_>>()
        );
        assert!(
            !blind.units.iter().any(|u| u.name == "unit17"),
            "and without the request it would not have"
        );
    }

    /// Relevance is a tie-breaker for selection, never a reordering: the map
    /// reads top to bottom as dependency order, and shuffling it by topicality
    /// would destroy the one property that makes it scannable.
    #[test]
    fn the_request_never_reorders_the_map() {
        let units = many_units(4);
        let edges = BTreeMap::from([("unit03".to_owned(), BTreeSet::from(["unit00".to_owned()]))]);

        let blind = assemble(units.clone(), &edges, DEFAULT_BUDGET, "");
        let asked = assemble(units, &edges, DEFAULT_BUDGET, "unit03 unit03 unit03");

        let names = |s: &Skeleton| s.units.iter().map(|u| u.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&blind), names(&asked));
    }

    /// A request that names nothing in the project must not perturb anything.
    #[test]
    fn an_unrelated_request_changes_nothing() {
        let units = many_units(6);
        let edges = BTreeMap::new();
        let blind = assemble(units.clone(), &edges, DEFAULT_BUDGET, "");
        let asked = assemble(
            units,
            &edges,
            DEFAULT_BUDGET,
            "fix the flaky deployment pipeline",
        );
        assert_eq!(blind.render(), asked.render());
    }

    /// Short words are noise. Matching on "the" or "for" would make every unit
    /// equally relevant, which is the same as none being relevant.
    #[test]
    fn short_words_do_not_count_as_relevance() {
        let unit = Unit {
            name: "the".to_owned(),
            root: PathBuf::new(),
            role: Some("a for and".to_owned()),
            role_inferred: false,
            role_hint: None,
            layer: 0,
            blast_radius: 0,
            cycle_with: Vec::new(),
        };
        assert_eq!(relevance("the a for and", &unit), 0);
    }
}

#[cfg(test)]
mod coupling_tests {
    use super::*;

    fn history(commits: &[&[&str]]) -> Vec<BTreeSet<String>> {
        commits
            .iter()
            .map(|units| units.iter().map(|u| (*u).to_owned()).collect())
            .collect()
    }

    fn edge(from: &str, to: &str) -> BTreeMap<String, BTreeSet<String>> {
        BTreeMap::from([(from.to_owned(), BTreeSet::from([to.to_owned()]))])
    }

    /// The signal this exists for: two units edited in lockstep with nothing
    /// structural joining them — a wire format written at both ends, a constant
    /// duplicated, a protocol implemented twice.
    #[test]
    fn units_that_always_move_together_without_importing_are_reported() {
        let found = hidden_coupling(
            &history(&[
                &["client", "server"],
                &["client", "server"],
                &["client", "server"],
            ]),
            &BTreeMap::new(),
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].a, "client");
        assert_eq!(found[0].b, "server");
        assert_eq!(found[0].commits, 3);
    }

    /// If they import each other the coupling is declared, visible, and already
    /// on the map as an edge. Repeating it as a discovery would be noise.
    #[test]
    fn a_connected_pair_is_not_hidden_coupling() {
        let found = hidden_coupling(
            &history(&[
                &["client", "server"],
                &["client", "server"],
                &["client", "server"],
            ]),
            &edge("client", "server"),
        );
        assert!(found.is_empty(), "{found:?}");
    }

    /// Twice is a coincidence.
    #[test]
    fn an_occasional_overlap_is_not_reported() {
        let found = hidden_coupling(
            &history(&[&["a", "b"], &["a", "b"], &["a"], &["a"], &["b"], &["b"]]),
            &BTreeMap::new(),
        );
        assert!(found.is_empty(), "{found:?}");
    }

    /// Two busy units will collide by chance. Overlap has to be a real share of
    /// their combined activity, not a raw count.
    #[test]
    fn busy_units_that_rarely_coincide_are_not_reported() {
        let mut commits: Vec<&[&str]> = vec![&["a", "b"], &["a", "b"], &["a", "b"]];
        for _ in 0..20 {
            commits.push(&["a"]);
            commits.push(&["b"]);
        }
        let found = hidden_coupling(&history(&commits), &BTreeMap::new());
        assert!(found.is_empty(), "3 of 43 is coincidence: {found:?}");
    }

    /// A sweep — a rename, a lint pass, a version bump — touches everything and
    /// means nothing. Counting one would couple every pair to every other.
    #[test]
    fn a_repository_wide_sweep_couples_nothing() {
        let sweep: &[&str] = &["a", "b", "c", "d", "e", "f", "g"];
        let found = hidden_coupling(&history(&[sweep, sweep, sweep, sweep]), &BTreeMap::new());
        assert!(found.is_empty(), "{found:?}");
    }
}
