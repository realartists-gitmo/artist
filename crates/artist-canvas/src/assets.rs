//! Everything the browser needs that the model did not write.
//!
//! The dependency set is compiled into the binary rather than fetched, so a
//! canvas opens with no network and no `npm install`. That is the whole reason
//! the pure-Rust toolchain is worth owning: the first canvas on a fresh machine
//! works on a plane.

use std::collections::BTreeMap;

use include_dir::{Dir, include_dir};

use crate::manifest::Manifest;

static VENDOR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/vendor");

/// The canvas runtime: hot reload, error reporting, and the `artist` global.
pub const CLIENT: &str = include_str!("../assets/client.js");

/// The component kit. Vendored so a canvas is assembled from parts rather than
/// re-deriving a button, a table, and a dark mode every single time.
///
/// Written in JSX like any canvas and compiled through the same transformer,
/// so the source stays readable instead of being hand-rolled `createElement`.
pub const UI: &str = include_str!("../assets/ui.jsx");

/// React bindings over the runtime — the idiomatic way to reach the bridge.
pub const HOOKS: &str = include_str!("../assets/hooks.js");

/// Fast Refresh bootstrap, loaded ahead of every canvas module.
pub const REFRESH: &str = include_str!("../assets/refresh.js");

/// Wrap a compiled module so its Fast Refresh registrations are scoped to it.
///
/// The transform emits bare `$RefreshReg$` / `$RefreshSig$` calls; without this
/// namespacing, two modules that both export a `App` would collide in the
/// runtime's family registry and swap each other's components.
pub fn scope_refresh(module_id: &str, code: &str) -> String {
    let id = json_string(module_id);
    format!(
        "import \"/@artist/refresh.js\";\n\
         const __artistPrevReg = window.$RefreshReg$;\n\
         const __artistPrevSig = window.$RefreshSig$;\n\
         window.$RefreshReg$ = (type, id) => window.__ARTIST_REFRESH__.register(type, {id} + \" \" + id);\n\
         window.$RefreshSig$ = window.__ARTIST_REFRESH__.createSignatureFunctionForTransform;\n\
         {code}\n\
         window.$RefreshReg$ = __artistPrevReg;\n\
         window.$RefreshSig$ = __artistPrevSig;\n"
    )
}

/// Compile a first-party module once and reuse it. These never change at
/// runtime, so paying the transform per request would be pure waste.
pub fn compiled(name: &str, source: &str) -> &'static str {
    use std::{
        collections::HashMap,
        sync::{Mutex, OnceLock},
    };

    static CACHE: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().expect("asset cache poisoned");
    if let Some(existing) = cache.get(name) {
        return existing;
    }
    let code = crate::transform::transform(
        std::path::Path::new(name),
        source,
        crate::transform::Options::default(),
    )
    .map(|output| output.code)
    // A first-party asset that will not compile is a build error we want to
    // see in the browser rather than a silent blank page.
    .unwrap_or_else(|error| format!("throw new Error({});\n", json_string(&error.to_string())));
    let leaked: &'static str = Box::leak(code.into_boxed_str());
    cache.insert(name.to_owned(), leaked);
    leaked
}

/// Bare specifiers the vendored set satisfies, mapped to their served paths.
///
/// `react-dom/client`, `react/jsx-runtime` and friends are subpaths rather than
/// package roots, so they need explicit entries — an import map does not do
/// prefix resolution for a package that is not also served as a directory.
const BARE: &[(&str, &str)] = &[
    ("react", "react.js"),
    ("react/jsx-runtime", "react-jsx-runtime.js"),
    ("react/jsx-dev-runtime", "react-jsx-dev-runtime.js"),
    ("react-dom/client", "react-dom-client.js"),
    ("react-refresh/runtime", "react-refresh-runtime.js"),
    ("uplot", "uplot.js"),
    ("@tanstack/react-table", "tanstack-react-table.js"),
];

/// Our own modules, served from `/@artist/` rather than `/@vendor/`.
const OURS: &[(&str, &str)] = &[
    ("@artist/canvas", "/@artist/client.js"),
    ("@artist/ui", "/@artist/ui.js"),
    ("@artist/react", "/@artist/react.js"),
    ("@artist/refresh", "/@artist/refresh.js"),
];

/// Specifiers a canvas declared that replace something shipped in the binary.
///
/// Allowed, because pinning a newer React is a legitimate thing to want — but
/// loud, because it silently defeats the offline guarantee this module's own
/// header asserts, and a mismatched React is the hardest failure to diagnose.
pub fn shadowed_specifiers(manifest: &Manifest) -> Vec<String> {
    manifest
        .deps
        .keys()
        .filter(|specifier| {
            BARE.iter().any(|(bare, _)| bare == *specifier)
                || OURS.iter().any(|(ours, _)| ours == *specifier)
        })
        .cloned()
        .collect()
}

/// Fetch a vendored asset by file name.
pub fn vendored(name: &str) -> Option<&'static [u8]> {
    VENDOR.get_file(name).map(|file| file.contents())
}

pub fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, ext)| ext) {
        Some("js" | "mjs" | "jsx" | "ts" | "tsx") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn import_map(manifest: &Manifest, slug: &str, key: &str) -> String {
    let mut imports: BTreeMap<&str, String> = BARE
        .iter()
        .map(|(specifier, file)| (*specifier, format!("/@vendor/{file}")))
        .chain(
            OURS.iter()
                .map(|(specifier, path)| (*specifier, (*path).to_owned())),
        )
        .collect();
    // Declared packages are proxied rather than linked directly: the canvas
    // then works offline after the first load, and the browser never talks to
    // a third-party host.
    // Keyed and slugged: a module import cannot carry a header, so the proxy's
    // only gate is the path — and naming the canvas is what stops one canvas
    // resolving a specifier out of another's manifest.
    for specifier in manifest.deps.keys() {
        imports.insert(
            specifier.as_str(),
            format!("/@dep/{key}/{slug}/{}", urlencode(specifier)),
        );
    }
    let entries = imports
        .iter()
        .map(|(specifier, target)| {
            format!("    {}: {}", json_string(specifier), json_string(target))
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!("{{\n  \"imports\": {{\n{entries}\n  }}\n}}")
}

/// The page the browser actually loads.
///
/// Order matters: the import map must precede any module, the runtime must
/// precede the entry so it catches errors thrown during the first render, and
/// Tailwind must be able to see the DOM the entry produces.
pub fn shell(slug: &str, manifest: &Manifest, key: &str) -> String {
    let title = if manifest.title.trim().is_empty() {
        slug
    } else {
        manifest.title.trim()
    };
    let tailwind = if manifest.tailwind {
        "\n    <script src=\"/@vendor/tailwind-browser.js\"></script>"
    } else {
        ""
    };
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <!-- This page's own address contains the session key, and the referrer is
         how an address reaches a third party: follow a link or load a remote
         image and the browser says where you came from. Modern browsers already
         withhold the path cross-origin, but that is their default, and the page
         is model-written — a copied snippet setting a laxer policy would give
         the key away. A canvas is a local tool with nothing to gain from
         referrers, so it sends none. -->
    <meta name="referrer" content="no-referrer" />
    <title>{title}</title>
    <link rel="stylesheet" href="/@vendor/uplot.css" />
    <style>
{tokens}{base}    </style>
    <!-- Tailwind reads its palette from here, so a utility the model writes
         without thinking resolves to artist's colours rather than stock ones. -->
    <style type="text/tailwindcss">
{theme}    </style>
    <script type="importmap">
{map}
    </script>
    <script>
      window.__ARTIST__ = {{ slug: {slug_json}, key: {key_json} }};
    </script>{tailwind}
    <script type="module" src="/@artist/refresh.js"></script>
    <script type="module" src="/@artist/client.js"></script>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="{entry}"></script>
  </body>
</html>
"#,
        title = escape_html(title),
        tokens = crate::palette::tokens_css(),
        base = crate::palette::BASE_CSS,
        theme = crate::palette::theme_css(),
        map = import_map(manifest, slug, key),
        slug_json = json_string(slug),
        key_json = json_string(key),
        tailwind = tailwind,
        entry = escape_html(&format!("./{}", manifest.entry.trim_start_matches("./"))),
    )
}

/// Percent-encode a specifier for use as one path segment.
fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("a string is always serializable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mapped_specifier_actually_ships() {
        for (specifier, file) in BARE {
            assert!(
                vendored(file).is_some_and(|bytes| !bytes.is_empty()),
                "{specifier} maps to {file}, which is missing or empty"
            );
        }
        for extra in ["tailwind-browser.js", "uplot.css"] {
            assert!(vendored(extra).is_some(), "{extra} is missing");
        }
    }

    /// The vendored bundles import `react` by bare specifier, so a missing or
    /// misspelled map entry breaks every canvas at once.
    /// A JSX typo in the kit breaks every canvas at once, and the failure would
    /// otherwise only surface in a browser.
    #[test]
    fn the_first_party_modules_compile() {
        for (name, source) in [("ui.jsx", UI), ("hooks.js", HOOKS)] {
            let compiled = compiled(name, source);
            assert!(
                !compiled.starts_with("throw new Error"),
                "{name} failed to compile: {compiled}"
            );
            assert!(compiled.len() > 500, "{name} compiled suspiciously small");
        }
        // client.js is plain JS served verbatim, so it only has to parse.
        assert!(CLIENT.contains("globalThis.artist"));
    }

    /// Every specifier the kit imports has to be in the map, or the browser
    /// fails to resolve it at load time with no useful message.
    #[test]
    fn the_kit_only_imports_what_the_map_resolves() {
        let map = import_map(&Manifest::default(), "demo", "k");
        for source in [UI, HOOKS] {
            for line in source.lines().filter(|line| line.starts_with("import ")) {
                let Some(start) = line.rfind(" from \"") else {
                    continue;
                };
                let specifier = line[start + 7..].trim_end_matches("\";");
                assert!(
                    map.contains(&format!("\"{specifier}\"")),
                    "{specifier} is imported but not in the import map"
                );
            }
        }
    }

    #[test]
    fn react_resolves_for_the_bundles_that_import_it() {
        let map = import_map(&Manifest::default(), "demo", "k");
        assert!(map.contains("\"react\": \"/@vendor/react.js\""), "{map}");
        assert!(map.contains("\"react/jsx-runtime\""), "{map}");
    }

    /// Declared deps resolve through our proxy, not straight at the CDN: that
    /// is what makes them cache locally and keeps the page off third-party
    /// hosts. Pointing the import map at the raw URL would quietly undo both.
    #[test]
    fn declared_deps_are_proxied_not_linked_directly() {
        let manifest = Manifest {
            deps: BTreeMap::from([("three".to_owned(), "https://esm.sh/three".to_owned())]),
            ..Manifest::default()
        };
        let map = import_map(&manifest, "demo", "k");
        assert!(map.contains("\"three\": \"/@dep/k/demo/three\""), "{map}");
        assert!(
            !map.contains("esm.sh"),
            "the CDN URL leaked into the page: {map}"
        );
        assert!(map.contains("\"react\""), "{map}");
    }

    /// A specifier becomes one path segment, so a scoped package must not
    /// split into two.
    #[test]
    fn scoped_specifiers_stay_a_single_path_segment() {
        let manifest = Manifest {
            deps: BTreeMap::from([("@scope/pkg".to_owned(), "https://esm.sh/x".to_owned())]),
            ..Manifest::default()
        };
        assert!(import_map(&manifest, "demo", "k").contains("/@dep/k/demo/%40scope%2Fpkg"));
        assert_eq!(urlencode("@scope/pkg"), "%40scope%2Fpkg");
    }

    #[test]
    fn the_shell_boots_the_declared_entry_and_carries_the_key() {
        let manifest = Manifest {
            entry: "app.tsx".into(),
            ..Manifest::default()
        };
        let html = shell("perf", &manifest, "s3cret");

        assert!(html.contains(r#"src="./app.tsx""#), "{html}");
        assert!(html.contains(r#"key: "s3cret""#), "{html}");
        assert!(html.contains("tailwind-browser.js"), "{html}");
        // The runtime has to be in place before the entry can throw.
        let runtime = html.find("/@artist/client.js").expect("runtime");
        let entry = html.find("./app.tsx").expect("entry");
        assert!(runtime < entry, "runtime must precede the entry");
    }

    /// The page's own address carries the session key, and a referrer is how an
    /// address reaches a third party. Browsers withhold the path cross-origin by
    /// default now, but the page is model-written and can change that default,
    /// so the policy is stated rather than assumed.
    #[test]
    fn the_shell_sends_no_referrer() {
        let html = shell("perf", &Manifest::default(), "s3cret");
        assert!(
            html.contains(r#"<meta name="referrer" content="no-referrer" />"#),
            "{html}"
        );
        // Before anything that could load a remote resource, or the first such
        // load happens under whatever the browser defaulted to.
        let policy = html.find("no-referrer").expect("policy");
        let first_load = html.find("<link").or(html.find("<script")).expect("load");
        assert!(policy < first_load, "the policy must precede any load");
    }

    #[test]
    fn tailwind_can_be_declined() {
        let manifest = Manifest {
            tailwind: false,
            ..Manifest::default()
        };
        assert!(!shell("x", &manifest, "k").contains("tailwind-browser.js"));
    }

    /// A canvas title is model-written text landing in markup.
    #[test]
    fn a_title_cannot_break_out_of_the_document() {
        let manifest = Manifest {
            title: "</title><script>alert(1)</script>".into(),
            ..Manifest::default()
        };
        let html = shell("x", &manifest, "k");
        assert!(!html.contains("<script>alert(1)"), "{html}");
        assert!(html.contains("&lt;/title&gt;"), "{html}");
    }
}
