//! Flattening a canvas into a file that needs nothing.
//!
//! The unit tests cover the arithmetic — base64, path resolution, stamps. What
//! only shows up once a whole canvas goes through is whether the result is
//! genuinely self-contained: one dangling `/@vendor/` reference and the file
//! works perfectly on the machine that made it and is blank everywhere else,
//! which is the failure this feature exists to avoid.

use std::path::PathBuf;

use artist_canvas::{export, registry, templates};

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("artist-export-it-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("project root");
        Project { root }
    }

    fn scaffold(&self, slug: &str, template: &str) -> &Self {
        let template = templates::find(template).expect("template");
        registry::scaffold(&self.root, slug, "Export probe", template).expect("scaffold");
        self
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Nothing in an exported file may point anywhere. This is the whole contract:
/// the reader may be on a plane, on a phone, or on a machine that has never had
/// artist installed.
#[tokio::test(flavor = "multi_thread")]
async fn an_export_reaches_for_nothing() {
    let project = Project::new("selfcontained");
    project.scaffold("demo", "dashboard");

    let flattened = export::export(&project.root, "demo").await.expect("export");
    let html = &flattened.html;

    // The paths a live canvas is served from. Any of them surviving means a
    // request to a server that will not be there.
    for absent in ["/@vendor/", "/@artist/", "/c/", "http://127.0.0.1"] {
        assert!(
            !html.contains(absent),
            "the export still points at `{absent}`"
        );
    }
    assert!(
        !html.contains("localhost"),
        "the export still points at localhost"
    );

    // And the positive: modules really were inlined rather than omitted.
    assert!(
        html.contains("data:text/javascript;base64,"),
        "no module was inlined"
    );
    assert!(
        flattened.modules.len() >= 2,
        "the dashboard has an entry and an app: {:?}",
        flattened.modules
    );
    assert_eq!(
        flattened.modules.first().map(String::as_str),
        Some("main.jsx"),
        "the entry should lead: {:?}",
        flattened.modules
    );
}

/// The document has to be able to start.
///
/// Every other test here checks that things are *present*. None of them noticed
/// when the entry was namespaced twice — `canvas:canvas:main.jsx` — because the
/// pieces were all still in the file, just wired to a specifier no map key
/// matched. The page went blank with nothing in the console, which is the
/// worst possible way for an exported file to fail: on someone else's machine,
/// silently.
#[tokio::test(flavor = "multi_thread")]
async fn the_entry_the_document_imports_is_one_the_map_can_resolve() {
    let project = Project::new("entrywired");
    project.scaffold("demo", "dashboard");

    let html = export::export(&project.root, "demo")
        .await
        .expect("export")
        .html;

    let map: serde_json::Value = {
        let start = html.find(r#"<script type="importmap">"#).expect("map");
        let body = &html[start + r#"<script type="importmap">"#.len()..];
        let end = body.find("</script>").expect("map end");
        serde_json::from_str(&body[..end]).expect("valid import map")
    };

    // The one module the document names itself. Scanned from *after* the tag,
    // because the first quote inside it belongs to `type="module"`.
    let entry = {
        let marker = "<script type=\"module\">import ";
        let at = html.rfind(marker).expect("entry") + marker.len();
        let rest = &html[at..];
        let open = rest.find('"').expect("quote");
        let close = rest[open + 1..].find('"').expect("quote");
        rest[open + 1..open + 1 + close].to_owned()
    };

    assert!(
        map["imports"].get(&entry).is_some(),
        "the document imports `{entry}`, which the map cannot resolve. Keys: {:?}",
        map["imports"]
            .as_object()
            .map(|imports| imports.keys().collect::<Vec<_>>())
    );

    // And every target has to be a URL. A map value that is not one becomes a
    // null entry, and the browser then refuses the specifier — which is how
    // React once ended up unreachable in a file that contained all of it: the
    // vendored modules had been put in as raw JavaScript, and every test that
    // asked "is react-dom present?" said yes.
    for (specifier, target) in map["imports"].as_object().expect("imports") {
        let target = target.as_str().unwrap_or_default();
        assert!(
            target.starts_with("data:") || target.starts_with("https://"),
            "`{specifier}` maps to something that is not a URL: {:.60}",
            target
        );
    }
}

/// Fast Refresh is for a page a watcher is editing underneath. An exported file
/// has neither, so shipping the runtime would be weight in the one artifact
/// whose whole job is to be small enough to send.
#[tokio::test(flavor = "multi_thread")]
async fn an_export_carries_no_hot_reload_machinery() {
    let project = Project::new("norefresh");
    project.scaffold("demo", "blank");

    let html = export::export(&project.root, "demo")
        .await
        .expect("export")
        .html;
    for absent in ["$RefreshReg$", "__ARTIST_REFRESH__", "@artist/refresh"] {
        assert!(
            !html.contains(absent),
            "the export still carries `{absent}`"
        );
    }
}

/// Every template has to survive the trip, because the model picks one without
/// knowing which of them the exporter happens to have been tried against.
#[tokio::test(flavor = "multi_thread")]
async fn every_template_exports() {
    for name in templates::names() {
        let project = Project::new(&format!("t-{name}"));
        project.scaffold("demo", name);

        let flattened = export::export(&project.root, "demo")
            .await
            .unwrap_or_else(|error| panic!("`{name}` failed to export: {error}"));
        assert!(
            flattened.html.contains("<div id=\"root\"></div>"),
            "`{name}` exported without a mount point"
        );
        assert!(
            !flattened.html.contains("/@vendor/"),
            "`{name}` exported with a dangling vendor reference"
        );
    }
}

/// State is what the canvas was showing, so it is content and travels with it.
#[tokio::test(flavor = "multi_thread")]
async fn state_travels_with_the_canvas() {
    let project = Project::new("state");
    project.scaffold("demo", "blank");

    let store = artist_canvas::StateStore::open(&project.root.join(".artist/canvas/demo"));
    store
        .set("rows", serde_json::json!(42))
        .expect("write state");

    let html = export::export(&project.root, "demo")
        .await
        .expect("export")
        .html;
    assert!(
        html.contains("\"rows\""),
        "the state the canvas was showing did not travel"
    );
    assert!(html.contains("42"), "the state value did not travel");
}

/// A canvas that links to another exports, on its own, into a document with a
/// dead link in it. The set export is the answer, and the property that matters
/// is that each canvas is still a whole working page inside it.
#[tokio::test(flavor = "multi_thread")]
async fn a_project_exports_as_one_document_with_every_canvas_in_it() {
    let project = Project::new("set");
    project.scaffold("alpha", "dashboard");
    project.scaffold("beta", "form");

    let flattened = export::export_project(&project.root)
        .await
        .expect("export project");
    assert_eq!(flattened.canvases, ["alpha", "beta"]);

    let html = &flattened.html;
    // One realm, one mount point each, and the nav that switches between them.
    assert!(html.contains("id=\"root-alpha\""), "no mount for alpha");
    assert!(html.contains("id=\"root-beta\""), "no mount for beta");
    assert!(html.contains("data-for=\"alpha\""));
    assert!(html.contains("data-for=\"beta\""));
    assert!(
        !html.contains("<iframe"),
        "canvases should share a realm, not be isolated in frames"
    );

    // Namespaced per canvas, which is what makes one realm safe: both of these
    // have an App.jsx and both import @artist/ui.
    for specifier in [
        "canvas:alpha/main.jsx",
        "canvas:beta/main.jsx",
        "canvas:alpha/@artist/canvas",
        "canvas:beta/@artist/canvas",
    ] {
        assert!(html.contains(specifier), "`{specifier}` is missing");
    }

    // And the heavy vendored libraries are shared rather than duplicated,
    // which is the entire reason for sharing a realm.
    assert_eq!(
        html.matches("\"react-dom/client\":").count(),
        1,
        "react-dom was inlined more than once"
    );
    assert!(
        !html.contains("/@vendor/"),
        "a set export still points at the server"
    );
}

/// An empty project is a real state the model can reach, and it must not
/// produce a file that looks like a working export of nothing.
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_project_exports_nothing_rather_than_something_broken() {
    let project = Project::new("emptyset");
    let flattened = export::export_project(&project.root)
        .await
        .expect("export project");
    assert!(flattened.canvases.is_empty());
    assert!(flattened.html.contains("no canvases"), "{}", flattened.html);
}

/// A canvas that reads a file from outside its own directory would inline it
/// into a document meant to be handed to other people.
#[tokio::test(flavor = "multi_thread")]
async fn a_canvas_cannot_export_a_file_from_outside_itself() {
    let project = Project::new("escape");
    project.scaffold("demo", "blank");
    std::fs::write(
        project.root.join("secret.js"),
        "export const key = 'hunter2';",
    )
    .expect("write outside file");
    std::fs::write(
        project.root.join(".artist/canvas/demo/main.jsx"),
        "import { key } from '../../../secret.js';\nconsole.log(key);\n",
    )
    .expect("entry");

    let error = export::export(&project.root, "demo")
        .await
        .expect_err("should refuse");
    assert!(
        matches!(error, export::ExportError::Escapes { .. }),
        "unexpected error: {error}"
    );
}
