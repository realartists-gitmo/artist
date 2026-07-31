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

fn import_map(manifest: &Manifest) -> String {
    let mut imports: BTreeMap<&str, String> = BARE
        .iter()
        .map(|(specifier, file)| (*specifier, format!("/@vendor/{file}")))
        .collect();
    // A canvas may name extra packages; those are absolute URLs and need the
    // network, which is why they are opt-in per canvas rather than ambient.
    for (specifier, url) in &manifest.deps {
        imports.insert(specifier.as_str(), url.clone());
    }
    let entries = imports
        .iter()
        .map(|(specifier, target)| format!("    {}: {}", json_string(specifier), json_string(target)))
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
    <title>{title}</title>
    <link rel="stylesheet" href="/@vendor/uplot.css" />
    <script type="importmap">
{map}
    </script>
    <script>
      window.__ARTIST__ = {{ slug: {slug_json}, key: {key_json} }};
    </script>{tailwind}
    <script type="module" src="/@artist/client.js"></script>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="{entry}"></script>
  </body>
</html>
"#,
        title = escape_html(title),
        map = import_map(manifest),
        slug_json = json_string(slug),
        key_json = json_string(key),
        tailwind = tailwind,
        entry = escape_html(&format!("./{}", manifest.entry.trim_start_matches("./"))),
    )
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
    #[test]
    fn react_resolves_for_the_bundles_that_import_it() {
        let map = import_map(&Manifest::default());
        assert!(map.contains("\"react\": \"/@vendor/react.js\""), "{map}");
        assert!(map.contains("\"react/jsx-runtime\""), "{map}");
    }

    #[test]
    fn declared_deps_override_and_extend_the_vendored_map() {
        let manifest = Manifest {
            deps: BTreeMap::from([("three".to_owned(), "https://esm.sh/three".to_owned())]),
            ..Manifest::default()
        };
        let map = import_map(&manifest);
        assert!(map.contains("\"three\": \"https://esm.sh/three\""), "{map}");
        assert!(map.contains("\"react\""), "{map}");
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
