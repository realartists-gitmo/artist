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
}

/// A canvas, flattened.
///
/// The document is omitted from `Debug` on purpose: it is a megabyte of inlined
/// React, and a failing `expect` that dumped it would bury the assertion.
pub struct Exported {
    pub html: String,
    /// Canvas-relative paths of the modules that went in, entry first.
    pub modules: Vec<String>,
    /// Specifiers left pointing at the network, which is the one thing an
    /// export cannot make offline: a declared `[deps]` package lives on a CDN
    /// and there is nothing to inline. Reported so the caller can say so.
    pub still_online: Vec<String>,
}

impl std::fmt::Debug for Exported {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Exported")
            .field("bytes", &self.html.len())
            .field("modules", &self.modules)
            .field("still_online", &self.still_online)
            .finish()
    }
}

/// Flatten `slug` into a single self-contained page.
pub fn export(project: &Path, slug: &str) -> Result<Exported, ExportError> {
    let registry = Registry::discover(project);
    let canvas = registry
        .get(slug)
        .ok_or_else(|| ExportError::Unknown(slug.to_owned()))?;
    let manifest = &canvas.manifest;

    let entry = normalise(manifest.entry.trim_start_matches("./"));
    let mut compiled: BTreeMap<String, String> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut queue: Vec<String> = vec![entry.clone()];
    let mut seen: BTreeSet<String> = BTreeSet::new();

    while let Some(module) = queue.pop() {
        if !seen.insert(module.clone()) {
            continue;
        }
        let absolute = canvas.root.join(&module);
        let source = std::fs::read_to_string(&absolute).map_err(|source| ExportError::Read {
            path: module.clone(),
            source,
        })?;

        // Rewrite on the source, not the output: spans are only meaningful
        // against the text they were parsed from.
        let found = transform::specifiers(Path::new(&module), &source);
        let mut rewritten = source.clone();
        // Back to front, so an earlier edit cannot shift a later span.
        for specifier in found.iter().rev() {
            if !is_relative(&specifier.value) {
                continue;
            }
            let target =
                resolve(&module, &specifier.value).ok_or_else(|| ExportError::Escapes {
                    module: module.clone(),
                    specifier: specifier.value.clone(),
                })?;
            queue.push(target.clone());
            let quoted = assets::json_string(&format!("{SCHEME}{target}"));
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
        imports.insert(format!("{SCHEME}{module}"), data_url(code));
    }
    for (specifier, code) in assets::inlinable() {
        imports.insert(specifier.to_owned(), data_url(&code));
    }

    // A declared dependency is the one thing that cannot be inlined: it lives
    // on a CDN and nothing here fetches. Left pointing at the network, and
    // reported, rather than dropped into a broken import.
    let mut still_online = Vec::new();
    for (specifier, url) in &manifest.deps {
        if crate::deps::check(url).is_ok() {
            imports.insert(specifier.clone(), url.clone());
            still_online.push(specifier.clone());
        }
    }

    let state = crate::StateStore::open(&canvas.root).snapshot();
    let html = page(
        slug,
        &manifest.title,
        manifest.tailwind,
        &imports,
        &entry,
        &serde_json::to_string(&state.plain()).unwrap_or_else(|_| "{}".into()),
        state.rev,
    );

    Ok(Exported {
        html,
        modules: order,
        still_online,
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
pub fn export_project(project: &Path) -> Result<ExportedSet, ExportError> {
    let registry = Registry::discover(project);
    let mut pages = Vec::new();
    let mut still_online = Vec::new();

    for canvas in &registry.canvases {
        let flattened = export(project, &canvas.slug)?;
        still_online.extend(flattened.still_online.iter().cloned());
        let title = if canvas.manifest.title.trim().is_empty() {
            canvas.slug.clone()
        } else {
            canvas.manifest.title.trim().to_owned()
        };
        pages.push((canvas.slug.clone(), title, flattened.html));
    }

    still_online.sort();
    still_online.dedup();
    let html = lobby_page(&pages);
    Ok(ExportedSet {
        html,
        canvases: pages.into_iter().map(|(slug, _, _)| slug).collect(),
        still_online,
    })
}

/// A whole project, flattened.
pub struct ExportedSet {
    pub html: String,
    pub canvases: Vec<String>,
    pub still_online: Vec<String>,
}

/// The document holding every canvas.
fn lobby_page(pages: &[(String, String, String)]) -> String {
    let mut nav = String::new();
    let mut frames = String::new();
    for (index, (slug, title, html)) in pages.iter().enumerate() {
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
        // `srcdoc` rather than a blob: a blob URL is minted at runtime and dies
        // with the tab, which would make the file depend on having been opened
        // by something that could mint one. srcdoc is just text in the document.
        frames.push_str(&format!(
            "    <iframe data-canvas=\"{slug}\"{hidden} srcdoc=\"{doc}\"></iframe>\n",
            slug = assets::escape_html(slug),
            hidden = if index == 0 { "" } else { " hidden" },
            doc = escape_attribute(html),
        ));
    }
    if pages.is_empty() {
        frames.push_str("    <p class=\"empty\">This project has no canvases.</p>\n");
    }

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
      iframe {{ flex: 1; width: 100%; border: 0; }}
      .empty {{ padding: 32px; color: var(--a-muted); }}
    </style>
  </head>
  <body>
    <nav>
{nav}    </nav>
{frames}    <script>
      // Navigation only. The frames are separate realms on purpose and nothing
      // reaches across them.
      const show = (slug) => {{
        for (const frame of document.querySelectorAll("iframe[data-canvas]")) {{
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
        nav = nav,
        frames = frames,
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
        entry_json = assets::json_string(&format!("{SCHEME}{entry}")),
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
