//! One canvas, one file.
//!
//! A live canvas is a server plus a file tree, which makes it excellent while
//! you are making it and gone the moment artist exits. An export is the other
//! thing a canvas should be able to be: a copy that travels. It opens from a
//! phone's Files app, from a Dropbox folder, from an email attachment, months
//! later, on a machine that has never heard of artist — because it is one HTML
//! file with the modules, the kit, React and the styling inlined, and it asks
//! the network for nothing.
//!
//! # Why every module becomes a `data:` URL
//!
//! Inlining JavaScript into a page is easy until the JavaScript is ESM: modules
//! import each other by URL, and there are no URLs in a single file. So each
//! module is inlined as a `data:` URL and named in an import map.
//!
//! That introduces the one real constraint here. A `data:` URL has no base URL,
//! so a relative specifier inside one — `./App.jsx` — cannot resolve and fails
//! outright. Every relative specifier is therefore rewritten to a synthetic
//! bare one (`canvas:App.jsx`) that the import map covers. This was verified in
//! a browser before it was built on: a data-module importing a bare specifier
//! resolves through the page's map, three levels deep, and the same module
//! importing `./leaf.js` fails exactly as predicted.
//!
//! # What does not survive
//!
//! [`crate::assets::EXPORT`] replaces the client, and the rule it implements is
//! *drop affordances, keep content*. Nothing here decides that; it just stops
//! shipping the parts that would need a harness.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use crate::{assets, registry::Registry, transform};

/// The scheme relative specifiers are rewritten to.
///
/// Not a real scheme and never fetched — it exists only as an import map key,
/// and the colon is what keeps it from colliding with a package name.
const SCHEME: &str = "canvas:";

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("no canvas named `{0}`")]
    Unknown(String),
    #[error("{0}")]
    Compile(#[from] transform::TransformError),
    #[error("could not read `{path}`: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{specifier}` in {module} points outside the canvas")]
    Escapes { module: String, specifier: String },
    #[error("could not fetch a declared dependency: {0}")]
    Fetch(String),
}

/// A canvas, flattened.
///
/// The document is omitted from `Debug` on purpose: it is a megabyte of inlined
/// React, and a failing `expect` that dumped it would bury the assertion.
pub struct Exported {
    pub html: String,
    /// Canvas-relative paths of the modules that went in, entry first.
    pub modules: Vec<String>,
    /// Declared `[deps]` that were fetched and inlined, with how many modules
    /// each pulled in. Reported because it is the one part of an export that
    /// needed the network to *build*, even though it needs none to open.
    pub dependencies: Vec<String>,
    /// Places the canvas reaches the harness directly rather than through the
    /// kit, which is the one thing an export cannot make degrade gracefully.
    pub wont_travel: Vec<String>,
}

impl std::fmt::Debug for Exported {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Exported")
            .field("bytes", &self.html.len())
            .field("modules", &self.modules)
            .field("dependencies", &self.dependencies)
            .field("wont_travel", &self.wont_travel)
            .finish()
    }
}

/// One canvas, reduced to import-map entries.
struct Flattened {
    imports: BTreeMap<String, String>,
    /// The specifier that starts it.
    entry: String,
    modules: Vec<String>,
    dependencies: Vec<String>,
    wont_travel: Vec<String>,
}

/// Flatten `slug` into a single self-contained page.
pub async fn export(project: &Path, slug: &str) -> Result<Exported, ExportError> {
    let registry = Registry::discover(project);
    let canvas = registry
        .get(slug)
        .ok_or_else(|| ExportError::Unknown(slug.to_owned()))?;

    // A canvas alone in a document keeps its boot data in a global, which is
    // where a page's own runtime naturally lives.
    let mut flat = flatten(canvas, SCHEME, "globalThis.__ARTIST__").await?;
    // Encoded, not raw. An import map's values are URLs, and a value that is
    // not one becomes a null entry the browser refuses to resolve — which took
    // the page down with a message no test was looking for.
    flat.imports.extend(
        shared_vendor()
            .into_iter()
            .map(|(specifier, code)| (specifier, data_url(&code))),
    );

    let state = crate::StateStore::open(&canvas.root).snapshot();
    let html = page(
        slug,
        &canvas.manifest.title,
        canvas.manifest.tailwind,
        &flat.imports,
        &flat.entry,
        &serde_json::to_string(&state.plain()).unwrap_or_else(|_| "{}".into()),
        state.rev,
    );

    Ok(Exported {
        html,
        modules: flat.modules,
        dependencies: flat.dependencies,
        wont_travel: flat.wont_travel,
    })
}

/// Walk one canvas's modules and inline them under `prefix`.
///
/// The prefix is what lets several canvases live in one document: every
/// specifier a canvas produces is namespaced by it, so two canvases that both
/// have an `App.jsx` — or both import `@artist/ui` — never collide.
async fn flatten(
    canvas: &crate::Canvas,
    prefix: &str,
    boot: &str,
) -> Result<Flattened, ExportError> {
    let manifest = &canvas.manifest;
    let entry = normalise(manifest.entry.trim_start_matches("./"));
    let mut compiled: BTreeMap<String, String> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut queue: Vec<String> = vec![entry.clone()];
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut wont_travel: Vec<String> = Vec::new();

    while let Some(module) = queue.pop() {
        if !seen.insert(module.clone()) {
            continue;
        }
        let absolute = canvas.root.join(&module);
        let source = std::fs::read_to_string(&absolute).map_err(|source| ExportError::Read {
            path: module.clone(),
            source,
        })?;

        wont_travel.extend(reaches_past_the_kit(&module, &source));

        // Rewrite on the source, not the output: spans are only meaningful
        // against the text they were parsed from.
        let found = transform::specifiers(Path::new(&module), &source);
        let mut rewritten = source.clone();
        // Back to front, so an earlier edit cannot shift a later span.
        for specifier in found.iter().rev() {
            // A canvas's own `@artist/*` imports point at its own copies of the
            // kit, which is what lets several canvases share one page without
            // sharing one `artist`.
            if PER_CANVAS.contains(&specifier.value.as_str()) {
                let quoted = assets::json_string(&format!("{prefix}{}", specifier.value));
                rewritten.replace_range(specifier.start as usize..specifier.end as usize, &quoted);
                continue;
            }
            if !is_relative(&specifier.value) {
                continue;
            }
            let target =
                resolve(&module, &specifier.value).ok_or_else(|| ExportError::Escapes {
                    module: module.clone(),
                    specifier: specifier.value.clone(),
                })?;
            queue.push(target.clone());
            let quoted = assets::json_string(&format!("{prefix}{target}"));
            rewritten.replace_range(specifier.start as usize..specifier.end as usize, &quoted);
        }

        // No refresh runtime and no dev runtime: an exported file has no
        // watcher to swap modules and nobody to read a source location.
        let output = transform::transform(
            Path::new(&module),
            &rewritten,
            transform::Options::default(),
        )?;
        compiled.insert(module.clone(), output.code);
        order.push(module);
    }

    // Entry first, so the module list reads the way the graph does.
    order.sort_by_key(|module| (module != &entry, module.clone()));

    let mut imports: BTreeMap<String, String> = BTreeMap::new();
    for (module, code) in &compiled {
        imports.insert(format!("{prefix}{module}"), data_url(code));
    }
    for (specifier, code) in kit_for(prefix, boot) {
        imports.insert(specifier, data_url(&code));
    }

    // Declared dependencies are fetched and inlined like everything else, so
    // "opens with no network" is true rather than nearly true. Each fetched
    // module is keyed in the map by its own absolute URL, which is what the
    // modules above it were rewritten to import — an import map may key on a
    // URL, and that is what turns a CDN's internal references into references
    // to the copies sitting in this file.
    let mut dependencies = Vec::new();
    for (specifier, url) in &manifest.deps {
        if crate::deps::check(url).is_err() {
            continue;
        }
        let mut fetched = BTreeMap::new();
        inline_dependency(url, &mut fetched).await?;

        let Some(entry) = fetched.get(url) else {
            return Err(ExportError::Fetch(format!("{url} returned nothing")));
        };
        imports.insert(specifier.clone(), data_url(entry));
        for (fetched_url, code) in &fetched {
            if fetched_url != url {
                imports.insert(fetched_url.clone(), data_url(code));
            }
        }
        dependencies.push(format!("{specifier} ({} modules)", fetched.len()));
    }

    Ok(Flattened {
        imports,
        entry: format!("{prefix}{entry}"),
        modules: order,
        dependencies,
        wont_travel,
    })
}

/// Every canvas in a project, in one file, with a lobby.
///
/// A canvas that links to another exports, on its own, to a document with a
/// dead link in it. This is the answer: the whole project travels together, and
/// `CanvasLink` becomes in-document navigation.
///
/// Each canvas goes in as a complete single-canvas export inside an `srcdoc`
/// frame. That is deliberate and it is the cheap way to be correct. Canvases
/// are independent programs that happen to share specifier names — two of them
/// both import `@artist/canvas`, both expect their own state under
/// `window.__ARTIST__`, and both mount `#root`. Flattening them into one realm
/// would mean namespacing every specifier per canvas and duplicating the kit
/// anyway; separate realms give that for free and cannot leak into each other.
/// The cost is React once per canvas, which is text in a file nobody is paying
/// bandwidth for.
pub async fn export_project(project: &Path) -> Result<ExportedSet, ExportError> {
    let registry = Registry::discover(project);
    let mut imports: BTreeMap<String, String> = shared_vendor()
        .into_iter()
        .map(|(specifier, code)| (specifier, data_url(&code)))
        .collect();
    let mut dependencies = Vec::new();
    let mut wont_travel = Vec::new();
    let mut pages = Vec::new();

    for canvas in &registry.canvases {
        let slug = &canvas.slug;
        let prefix = format!("{SCHEME}{slug}/");
        let state = crate::StateStore::open(&canvas.root).snapshot();

        // Each canvas's runtime is handed its own slug, state and mount point
        // as a literal. A global would be the first canvas's data seen by all
        // of them, which is the thing this whole arrangement exists to avoid.
        let boot = format!(
            "{{ slug: {}, title: {}, state: {}, mount: {} }}",
            assets::json_string(slug),
            assets::json_string(canvas.manifest.title.trim()),
            serde_json::to_string(&state.plain()).unwrap_or_else(|_| "{}".into()),
            assets::json_string(&format!("root-{slug}")),
        );

        let flat = flatten(canvas, &prefix, &boot).await?;
        dependencies.extend(flat.dependencies.iter().cloned());
        wont_travel.extend(flat.wont_travel.iter().cloned());
        imports.extend(flat.imports);

        let title = if canvas.manifest.title.trim().is_empty() {
            slug.clone()
        } else {
            canvas.manifest.title.trim().to_owned()
        };
        pages.push((slug.clone(), title, flat.entry));
    }

    dependencies.sort();
    dependencies.dedup();
    let html = lobby_page(&pages, &imports);
    Ok(ExportedSet {
        html,
        canvases: pages.into_iter().map(|(slug, _, _)| slug).collect(),
        dependencies,
        wont_travel,
    })
}

/// A whole project, flattened.
pub struct ExportedSet {
    pub html: String,
    pub canvases: Vec<String>,
    pub dependencies: Vec<String>,
    pub wont_travel: Vec<String>,
}

/// The document holding every canvas.
fn lobby_page(pages: &[(String, String, String)], imports: &BTreeMap<String, String>) -> String {
    let mut nav = String::new();
    let mut frames = String::new();
    let mut entries = String::new();
    for (index, (slug, title, entry)) in pages.iter().enumerate() {
        nav.push_str(&format!(
            "      <button data-for=\"{slug}\"{selected}>{title}</button>\n",
            slug = assets::escape_html(slug),
            title = assets::escape_html(title),
            selected = if index == 0 {
                " aria-current=\"true\""
            } else {
                ""
            },
        ));
        // A div, not an iframe. Each canvas mounts into its own element in one
        // shared realm — which is only possible because an entry asks the
        // runtime where it goes rather than hunting for `#root`.
        frames.push_str(&format!(
            "    <div class=\"canvas\" data-canvas=\"{slug}\"{hidden}><div id=\"root-{slug}\"></div></div>\n",
            slug = assets::escape_html(slug),
            hidden = if index == 0 { "" } else { " hidden" },
        ));
        entries.push_str(&format!("      import {};\n", assets::json_string(entry)));
    }
    if pages.is_empty() {
        frames.push_str("    <p class=\"empty\">This project has no canvases.</p>\n");
    }

    let map = imports
        .iter()
        .map(|(specifier, target)| {
            format!(
                "    {}: {}",
                assets::json_string(specifier),
                assets::json_string(target)
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");

    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Canvases</title>
    <style>
{tokens}{base}
      body {{ margin: 0; display: flex; flex-direction: column; height: 100vh; }}
      nav {{
        display: flex; gap: 4px; padding: 8px 12px; overflow-x: auto;
        border-bottom: 1px solid var(--a-border); background: var(--a-surface);
      }}
      nav button {{
        font: 13px var(--a-sans); padding: 6px 12px; border-radius: 8px;
        border: 1px solid transparent; background: none; color: var(--a-muted);
        cursor: pointer; white-space: nowrap;
      }}
      nav button[aria-current="true"] {{
        color: var(--a-text); border-color: var(--a-border); background: var(--a-bg);
      }}
      .canvas {{ flex: 1; overflow: auto; }}
      .empty {{ padding: 32px; color: var(--a-muted); }}
    </style>
    <script type="importmap">
{{
  "imports": {{
{map}
  }}
}}
    </script>
  </head>
  <body>
    <nav>
{nav}    </nav>
{frames}    <script type="module">
{entries}    </script>
    <script>
      // Navigation only. Every canvas is already mounted; this decides which
      // one is on screen, so switching keeps whatever the others were showing.
      const show = (slug) => {{
        for (const frame of document.querySelectorAll(".canvas[data-canvas]")) {{
          frame.hidden = frame.dataset.canvas !== slug;
        }}
        for (const button of document.querySelectorAll("nav button")) {{
          button.setAttribute("aria-current", String(button.dataset.for === slug));
        }}
        if (location.hash.slice(1) !== slug) location.hash = slug;
      }};
      document.querySelector("nav").addEventListener("click", (event) => {{
        const button = event.target.closest("button[data-for]");
        if (button) show(button.dataset.for);
      }});
      addEventListener("hashchange", () => show(location.hash.slice(1)));
      if (location.hash.length > 1) show(location.hash.slice(1));
    </script>
  </body>
</html>
"#,
        tokens = crate::palette::tokens_css(),
        base = crate::palette::BASE_CSS,
        map = map,
        nav = nav,
        frames = frames,
        entries = entries,
    )
}

/// Escape a whole document so it can live inside an HTML attribute.
///
/// `srcdoc` holds markup as an attribute value, so anything that could close
/// the attribute or the tag has to go — and `&` first, or the escapes escape
/// each other.
fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A sortable, readable UTC stamp: `20260801-014233`.
///
/// Exports accumulate in a directory the user is expected to browse — it is the
/// canvas's version history — so the names have to sort chronologically *and*
/// be readable at a glance. Epoch seconds manage the first and not the second.
/// The workspace has no date library and this does not justify adding one; the
/// civil-from-days arithmetic below is Howard Hinnant's, and it is exact for
/// every date this will ever see.
pub fn stamp(now: std::time::SystemTime) -> String {
    let seconds = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;

    // Shift the epoch to 0000-03-01 so leap days land at the end of the cycle.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    )
}

/// Our own modules, which need one copy per canvas in a project export.
///
/// They bind `artist` at module scope — `import { artist } from
/// "@artist/canvas"` — so a single shared copy would bind to whichever canvas
/// loaded first and hand every other canvas that one's state and mount point.
/// The vendored libraries have no such binding and stay shared, which is where
/// the weight is anyway: React and ReactDOM are over a megabyte together, the
/// kit is sixty kilobytes.
const PER_CANVAS: &[&str] = &[
    "@artist/canvas",
    "@artist/ui",
    "@artist/ui-full",
    "@artist/react",
];

/// The kit, rewritten to import its own per-canvas copies.
///
/// `boot` is the canvas's own slug, state and mount element, substituted into
/// the runtime rather than read from a global — a global is exactly the thing
/// that cannot work once two canvases share a page.
fn kit_for(prefix: &str, boot: &str) -> Vec<(String, String)> {
    assets::inlinable()
        .into_iter()
        .filter(|(specifier, _)| PER_CANVAS.contains(specifier))
        .map(|(specifier, code)| {
            let code = if specifier == "@artist/canvas" {
                code.replace("globalThis.__ARTIST__ ?? {}", &format!("{boot} ?? {{}}"))
            } else {
                code
            };
            (format!("{prefix}{specifier}"), rewrite_kit(&code, prefix))
        })
        .collect()
}

/// Point a module's `@artist/*` imports at this canvas's copies.
fn rewrite_kit(code: &str, prefix: &str) -> String {
    let found = transform::specifiers(Path::new("kit.js"), code);
    let mut out = code.to_owned();
    for specifier in found.iter().rev() {
        if !PER_CANVAS.contains(&specifier.value.as_str()) {
            continue;
        }
        let quoted = assets::json_string(&format!("{prefix}{}", specifier.value));
        out.replace_range(specifier.start as usize..specifier.end as usize, &quoted);
    }
    out
}

/// The vendored libraries, shared by every canvas in a document.
fn shared_vendor() -> Vec<(String, String)> {
    assets::inlinable()
        .into_iter()
        .filter(|(specifier, _)| !PER_CANVAS.contains(specifier))
        .map(|(specifier, code)| (specifier.to_owned(), code))
        .collect()
}

/// How deep a CDN package's own imports will be followed.
///
/// esm.sh answers a package with a stub that re-exports from one or two more
/// modules, so a couple of levels covers the real shape. A ceiling exists at
/// all because the graph is on somebody else's server and its size is their
/// decision, not ours.
const DEP_DEPTH: usize = 6;

/// How many remote modules one export will pull in.
const DEP_MODULES: usize = 64;

/// Fetch a declared dependency and everything it imports.
///
/// Export used to leave `[deps]` pointing at the CDN, which made "opens with no
/// network" false for any canvas declaring one — the failure landing on the
/// person you sent it to rather than on you. Fetching here is not the proxying
/// the live path refuses: that was a permanent server-side fetcher on the
/// request path for a model-written URL. This is one explicit operation, run
/// when a person asks for a file to take away, and [`crate::deps::check`] is
/// applied at *every* hop rather than only the first — a CDN redirecting into
/// somewhere else does not get to smuggle a module in.
async fn inline_dependency(
    entry: &str,
    into: &mut BTreeMap<String, String>,
) -> Result<(), ExportError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|error| ExportError::Fetch(error.to_string()))?;

    let mut queue = vec![(entry.to_owned(), 0usize)];
    let mut seen: BTreeSet<String> = BTreeSet::new();

    while let Some((url, depth)) = queue.pop() {
        if depth > DEP_DEPTH || into.len() >= DEP_MODULES || !seen.insert(url.clone()) {
            continue;
        }
        crate::deps::check(&url).map_err(|error| ExportError::Fetch(error.to_string()))?;

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|error| ExportError::Fetch(format!("{url}: {error}")))?;
        if !response.status().is_success() {
            return Err(ExportError::Fetch(format!(
                "{url}: the CDN answered {}",
                response.status()
            )));
        }
        let source = response
            .text()
            .await
            .map_err(|error| ExportError::Fetch(format!("{url}: {error}")))?;

        // The module's own imports, resolved against *its* URL — which is the
        // whole reason the live path could not proxy these. A CDN entry point
        // re-exports from a root-relative path, and once it is inlined here
        // there is no origin left to resolve that against, so each one is
        // turned into an absolute URL now and rewritten to point at the copy.
        let found = transform::specifiers(Path::new("dep.js"), &source);
        let mut rewritten = source.clone();
        for specifier in found.iter().rev() {
            let Some(absolute) = absolutise(&url, &specifier.value) else {
                continue;
            };
            queue.push((absolute.clone(), depth + 1));
            let quoted = assets::json_string(&absolute);
            rewritten.replace_range(specifier.start as usize..specifier.end as usize, &quoted);
        }
        into.insert(url, rewritten);
    }
    Ok(())
}

/// Resolve a specifier found inside a module fetched from `base`.
///
/// Deliberately small: absolute URLs pass through, root-relative and relative
/// paths resolve against the origin and directory of the module that named
/// them. A bare specifier inside a CDN module is left alone, because it is the
/// CDN's own import map's business and not something to guess at.
fn absolutise(base: &str, specifier: &str) -> Option<String> {
    if specifier.starts_with("https://") {
        return Some(specifier.to_owned());
    }
    let rest = base.strip_prefix("https://")?;
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, format!("/{path}")),
        None => (rest, "/".to_owned()),
    };
    if let Some(rooted) = specifier.strip_prefix('/') {
        return Some(format!("https://{host}/{rooted}"));
    }
    if !is_relative(specifier) {
        return None;
    }
    let directory = path.rsplit_once('/').map(|(head, _)| head).unwrap_or("");
    Some(format!(
        "https://{host}{}",
        prefixed(&normalise(&format!("{directory}/{specifier}")))
    ))
}

/// `normalise` drops the leading slash; a URL path needs it back.
fn prefixed(path: &str) -> String {
    format!("/{path}")
}

/// Reaching past the kit to the harness, which an export cannot substitute.
///
/// `<Action>` and the kit's other agent-facing components are swapped for
/// content-only versions when a canvas is exported, so a canvas built from them
/// degrades correctly with nothing in it checking which world it is in. A raw
/// `artist.call` in the model's own handler cannot be swapped for anything — it
/// becomes a button that rejects on somebody else's machine, with no way for
/// them to tell you.
///
/// ESM has no access control, so this cannot be prevented: every specifier in
/// the map is reachable and hiding the capability behind another module just
/// moves the import. What it can be is *caught*, here, at the one moment when
/// the question "will this survive the trip?" is both askable and answerable.
///
/// Read from the syntax, not from the text. An earlier version searched for the
/// string `artist.call`, which found it only when the module happened to have
/// named the import `artist` — true of every template and every doc, and not
/// something a canvas is obliged to do. `import { artist as a }` made the check
/// silently stop working, which is the worst property a warning can have.
fn reaches_past_the_kit(module: &str, source: &str) -> Vec<String> {
    transform::harness_reaches(Path::new(module), source)
        .into_iter()
        .map(|(what, at)| {
            // A byte offset means nothing to the reader; the line it falls on
            // is what they will go and look at.
            let line = source[..at as usize].matches('\n').count() + 1;
            format!("{module}:{line} uses {what}")
        })
        .collect()
}

/// Is this a specifier the exporter has to resolve on disk?
fn is_relative(specifier: &str) -> bool {
    specifier.starts_with("./") || specifier.starts_with("../")
}

/// Collapse `.` and `..` without touching the filesystem.
fn normalise(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Where a relative specifier lands, canvas-relative.
///
/// `None` when it climbs out of the canvas — which is a refusal rather than a
/// clamp, because a canvas reaching into the wider filesystem is a thing the
/// author should be told about, not have quietly rewritten.
fn resolve(from: &str, specifier: &str) -> Option<String> {
    let mut base = PathBuf::from(from);
    base.pop();
    let joined = base.join(specifier);
    let joined = joined.to_string_lossy().replace('\\', "/");

    // Count how far the specifier climbs against how deep the module sits.
    let depth = from.matches('/').count() as isize;
    let climbs = specifier.matches("../").count() as isize;
    if climbs > depth {
        return None;
    }
    Some(normalise(&joined))
}

/// A module, inlined.
///
/// Base64 rather than percent-encoding because JavaScript is mostly punctuation
/// and percent-encoding would roughly triple it, where base64 costs a third.
fn data_url(code: &str) -> String {
    format!("data:text/javascript;base64,{}", base64(code.as_bytes()))
}

/// Standard base64, written out rather than pulled in.
///
/// The workspace has an old base64 in the lock via another crate's tree, and
/// adding a second version of a dependency for one encoder — in a workspace
/// several people are building concurrently — costs more than sixteen lines.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let bits = chunk.iter().enumerate().fold(0u32, |acc, (index, byte)| {
            acc | (*byte as u32) << (16 - 8 * index)
        });
        for slot in 0..4 {
            if slot <= chunk.len() {
                let index = (bits >> (18 - 6 * slot)) & 0b11_1111;
                out.push(ALPHABET[index as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The document itself.
#[allow(clippy::too_many_arguments)]
fn page(
    slug: &str,
    title: &str,
    tailwind: bool,
    imports: &BTreeMap<String, String>,
    entry: &str,
    state: &str,
    rev: u64,
) -> String {
    let shown = if title.trim().is_empty() { slug } else { title };
    let map = imports
        .iter()
        .map(|(specifier, target)| {
            format!(
                "    {}: {}",
                assets::json_string(specifier),
                assets::json_string(target)
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");

    // Inlined, not linked: Tailwind's browser build generates from the live DOM
    // at runtime, so it keeps working with no network at all.
    let tailwind = if tailwind {
        match assets::vendored("tailwind-browser.js") {
            Some(bytes) => format!("    <script>{}</script>\n", String::from_utf8_lossy(bytes)),
            None => String::new(),
        }
    } else {
        String::new()
    };
    let uplot = assets::vendored("uplot.css")
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_default();

    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>{title}</title>
    <style>
{uplot}
{tokens}{base}    </style>
    <style type="text/tailwindcss">
{theme}    </style>
    <script type="importmap">
{{
  "imports": {{
{map}
  }}
}}
    </script>
    <script>
      window.__ARTIST__ = {{ slug: {slug_json}, title: {title_json}, state: {state}, rev: {rev} }};
    </script>
{tailwind}  </head>
  <body>
    <div id="root"></div>
    <script type="module">import {entry_json};</script>
  </body>
</html>
"#,
        title = assets::escape_html(shown),
        uplot = uplot,
        tokens = crate::palette::tokens_css(),
        base = crate::palette::BASE_CSS,
        theme = crate::palette::theme_css(),
        map = map,
        slug_json = assets::json_string(slug),
        title_json = assets::json_string(shown),
        state = state,
        rev = rev,
        tailwind = tailwind,
        // Already namespaced by `flatten`, which is the only thing that knows
        // the prefix — prefixing again here produced `canvas:canvas:main.jsx`,
        // an unresolved specifier that took the whole page down silently.
        entry_json = assets::json_string(entry),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        // Bytes above 0x7f have to survive: the kit's source is not ASCII.
        assert_eq!(base64("é".as_bytes()), "w6k=");
    }

    /// Exports are the canvas's version history, so the names have to sort in
    /// the order they were taken and still be readable.
    #[test]
    fn stamps_are_readable_and_sort_chronologically() {
        use std::time::{Duration, UNIX_EPOCH};

        assert_eq!(stamp(UNIX_EPOCH), "19700101-000000");
        // A leap day, which is where naive day arithmetic goes wrong.
        assert_eq!(
            stamp(UNIX_EPOCH + Duration::from_secs(1_709_164_800)),
            "20240229-000000"
        );
        assert_eq!(
            stamp(UNIX_EPOCH + Duration::from_secs(1_767_225_599)),
            "20251231-235959"
        );

        let earlier = stamp(UNIX_EPOCH + Duration::from_secs(1_767_225_598));
        let later = stamp(UNIX_EPOCH + Duration::from_secs(1_767_225_599));
        assert!(earlier < later, "{earlier} should sort before {later}");
    }

    #[test]
    fn relative_specifiers_resolve_canvas_relative() {
        assert_eq!(resolve("main.jsx", "./App.jsx").as_deref(), Some("App.jsx"));
        assert_eq!(
            resolve("main.jsx", "./components/Chart.jsx").as_deref(),
            Some("components/Chart.jsx")
        );
        assert_eq!(
            resolve("components/Chart.jsx", "../shared.js").as_deref(),
            Some("shared.js")
        );
        assert_eq!(
            resolve("components/deep/Thing.jsx", "../../main.jsx").as_deref(),
            Some("main.jsx")
        );
    }

    /// A canvas reaching out of its own directory is refused rather than
    /// clamped: exporting it would inline a file from somewhere else on the
    /// machine into a document meant to be handed to other people.
    #[test]
    fn a_specifier_that_climbs_out_is_refused() {
        assert_eq!(resolve("main.jsx", "../secrets.js"), None);
        assert_eq!(resolve("components/Chart.jsx", "../../../etc/passwd"), None);
    }

    #[test]
    fn only_relative_specifiers_are_rewritten() {
        assert!(is_relative("./App.jsx"));
        assert!(is_relative("../shared.js"));
        // These are import map keys and must be left alone.
        assert!(!is_relative("react"));
        assert!(!is_relative("@artist/ui"));
        assert!(!is_relative("https://esm.sh/nanoid"));
        assert!(!is_relative("/absolute.js"));
    }
}
