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

/// `client.js`'s stand-in for a canvas that has been exported to a file.
pub const EXPORT: &str = include_str!("../assets/export.js");

/// The kit as it exists in an exported file: the same components, minus the
/// ones that were only ever ways to reach a harness.
pub const UI_STATIC: &str = include_str!("../assets/ui-static.jsx");

/// Every module an exported canvas needs inlined, as (specifier, JavaScript).
///
/// The knowledge of what is vendored lives here rather than in the exporter,
/// so adding a package to the kit cannot silently produce exports that fail on
/// an unresolved import.
///
/// Three deliberate differences from what the server hands a live page, all of
/// them substitutions at the module boundary rather than flags inside a module.
///
/// `@artist/canvas` resolves to [`EXPORT`] rather than the client, because
/// there is no server to talk to. `@artist/ui` resolves to [`UI_STATIC`], which
/// is the kit with its affordances removed and its content kept — the real kit
/// stays reachable at `@artist/ui-full`, which is what the static one builds
/// on. And `@artist/refresh` is absent: Fast Refresh swaps modules the file
/// watcher noticed changing, and an exported file has neither, so shipping the
/// runtime would be dead weight in something whose whole job is to travel.
pub fn inlinable() -> Vec<(&'static str, String)> {
    let mut modules: Vec<(&'static str, String)> = BARE
        .iter()
        .filter_map(|(specifier, file)| {
            let bytes = vendored(file)?;
            Some((*specifier, String::from_utf8_lossy(bytes).into_owned()))
        })
        .collect();

    modules.push(("@artist/canvas", compiled("export.js", EXPORT).to_owned()));
    // Order matters to neither the map nor the browser, but the pairing does:
    // `ui-static` imports `ui-full`, so shipping one without the other is an
    // unresolved import in a file nobody can debug once it has travelled.
    modules.push((
        "@artist/ui",
        compiled("ui-static.jsx", UI_STATIC).to_owned(),
    ));
    modules.push(("@artist/ui-full", compiled("ui.jsx", UI).to_owned()));
    modules.push(("@artist/react", compiled("hooks.js", HOOKS).to_owned()));
    modules
}

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

/// Takes no key or slug any more: those existed to address the dep proxy, and
/// nothing is proxied now.
fn import_map(manifest: &Manifest) -> String {
    let mut imports: BTreeMap<&str, String> = BARE
        .iter()
        .map(|(specifier, file)| (*specifier, format!("/@vendor/{file}")))
        .chain(
            OURS.iter()
                .map(|(specifier, path)| (*specifier, (*path).to_owned())),
        )
        .collect();
    // Declared packages are linked at the CDN rather than proxied.
    //
    // Proxying looked safer and did not work. A CDN's entry point is a stub
    // that re-exports from a root-relative path — esm.sh answers `nanoid` with
    // `export * from "/nanoid@5.0.7/es2022/nanoid.mjs"` — and the browser
    // resolves that against whatever origin served it. Behind the proxy that
    // is the canvas server, which has no such path, so the import 404s and the
    // canvas silently renders nothing.
    //
    // The allowlist is enforced here instead, which is the part that has to
    // keep working: it used to run inside the fetch, and with nothing fetching
    // server-side it would otherwise have stopped applying altogether. A
    // specifier whose URL is not allowed is left out of the map entirely, so
    // it fails as an unresolved import the model can see rather than as a
    // silent request to somewhere it should not reach.
    for (specifier, url) in &manifest.deps {
        if crate::deps::check(url).is_ok() {
            imports.insert(specifier.as_str(), url.clone());
        }
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
///
/// `rev` is the build the page starts life showing. It matters because a page
/// is not always current: it loads, the model edits, and until the swap lands
/// the DOM still describes the previous version. Without a number on it, a
/// digest of the old page is indistinguishable from proof that an edit did
/// nothing.
pub fn shell(slug: &str, manifest: &Manifest, key: &str, rev: u64) -> String {
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
      window.__ARTIST__ = {{ slug: {slug_json}, key: {key_json}, rev: {rev} }};
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
        map = import_map(manifest),
        slug_json = json_string(slug),
        key_json = json_string(key),
        rev = rev,
        tailwind = tailwind,
        entry = escape_html(&format!("./{}", manifest.entry.trim_start_matches("./"))),
    )
}

pub(crate) fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn json_string(value: &str) -> String {
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
        let map = import_map(&Manifest::default());
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
        let map = import_map(&Manifest::default());
        assert!(map.contains("\"react\": \"/@vendor/react.js\""), "{map}");
        assert!(map.contains("\"react/jsx-runtime\""), "{map}");
    }

    /// Declared deps link at the CDN. Proxying them was the intent, and it
    /// silently broke every one of them: a CDN entry point re-exports from a
    /// root-relative path, which resolves against whichever origin served the
    /// stub, so behind the proxy it pointed back at the canvas server and 404d.
    #[test]
    fn declared_deps_link_at_the_cdn() {
        let manifest = Manifest {
            deps: BTreeMap::from([("three".to_owned(), "https://esm.sh/three@0.170".to_owned())]),
            ..Manifest::default()
        };
        let map = import_map(&manifest);
        assert!(
            map.contains("\"three\": \"https://esm.sh/three@0.170\""),
            "{map}"
        );
        // The vendored set still resolves locally; only declared deps go out.
        assert!(map.contains("\"react\": \"/@vendor/react.js\""), "{map}");
    }

    /// The allowlist used to be enforced inside the fetch. Nothing fetches
    /// server-side any more, so if it did not move here it would have stopped
    /// applying — and a canvas could name any host it liked.
    #[test]
    fn a_dep_outside_the_allowlist_never_reaches_the_page() {
        let manifest = Manifest {
            deps: BTreeMap::from([
                ("good".to_owned(), "https://esm.sh/ok".to_owned()),
                (
                    "evil".to_owned(),
                    "https://evil.example/payload.js".to_owned(),
                ),
                ("plain".to_owned(), "http://esm.sh/insecure".to_owned()),
                (
                    "sneaky".to_owned(),
                    "https://esm.sh@evil.example/x".to_owned(),
                ),
            ]),
            ..Manifest::default()
        };
        let map = import_map(&manifest);

        assert!(map.contains("\"good\""), "{map}");
        // Left out entirely rather than rewritten: an unresolved specifier is a
        // failure the model can read, and nothing is requested meanwhile.
        assert!(!map.contains("evil.example"), "{map}");
        assert!(!map.contains("\"evil\""), "{map}");
        assert!(!map.contains("insecure"), "{map}");
        assert!(
            !map.contains("\"sneaky\""),
            "credentials trick admitted: {map}"
        );
    }

    #[test]
    fn the_shell_boots_the_declared_entry_and_carries_the_key() {
        let manifest = Manifest {
            entry: "app.tsx".into(),
            ..Manifest::default()
        };
        let html = shell("perf", &manifest, "s3cret", 0);

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
        let html = shell("perf", &Manifest::default(), "s3cret", 0);
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
        assert!(!shell("x", &manifest, "k", 0).contains("tailwind-browser.js"));
    }

    /// A canvas title is model-written text landing in markup.
    #[test]
    fn a_title_cannot_break_out_of_the_document() {
        let manifest = Manifest {
            title: "</title><script>alert(1)</script>".into(),
            ..Manifest::default()
        };
        let html = shell("x", &manifest, "k", 0);
        assert!(!html.contains("<script>alert(1)"), "{html}");
        assert!(html.contains("&lt;/title&gt;"), "{html}");
    }
}
