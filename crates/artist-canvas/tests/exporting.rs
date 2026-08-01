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
#[test]
fn an_export_reaches_for_nothing() {
    let project = Project::new("selfcontained");
    project.scaffold("demo", "dashboard");

    let flattened = export::export(&project.root, "demo").expect("export");
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

/// Fast Refresh is for a page a watcher is editing underneath. An exported file
/// has neither, so shipping the runtime would be weight in the one artifact
/// whose whole job is to be small enough to send.
#[test]
fn an_export_carries_no_hot_reload_machinery() {
    let project = Project::new("norefresh");
    project.scaffold("demo", "blank");

    let html = export::export(&project.root, "demo").expect("export").html;
    for absent in ["$RefreshReg$", "__ARTIST_REFRESH__", "@artist/refresh"] {
        assert!(
            !html.contains(absent),
            "the export still carries `{absent}`"
        );
    }
}

/// Every template has to survive the trip, because the model picks one without
/// knowing which of them the exporter happens to have been tried against.
#[test]
fn every_template_exports() {
    for name in templates::names() {
        let project = Project::new(&format!("t-{name}"));
        project.scaffold("demo", name);

        let flattened = export::export(&project.root, "demo")
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
#[test]
fn state_travels_with_the_canvas() {
    let project = Project::new("state");
    project.scaffold("demo", "blank");

    let store = artist_canvas::StateStore::open(&project.root.join(".artist/canvas/demo"));
    store
        .set("rows", serde_json::json!(42))
        .expect("write state");

    let html = export::export(&project.root, "demo").expect("export").html;
    assert!(
        html.contains("\"rows\""),
        "the state the canvas was showing did not travel"
    );
    assert!(html.contains("42"), "the state value did not travel");
}

/// A canvas that links to another exports, on its own, into a document with a
/// dead link in it. The set export is the answer, and the property that matters
/// is that each canvas is still a whole working page inside it.
#[test]
fn a_project_exports_as_one_document_with_every_canvas_in_it() {
    let project = Project::new("set");
    project.scaffold("alpha", "dashboard");
    project.scaffold("beta", "form");

    let flattened = export::export_project(&project.root).expect("export project");
    assert_eq!(flattened.canvases, ["alpha", "beta"]);

    let html = &flattened.html;
    // One frame per canvas, and the nav that switches between them.
    assert_eq!(
        html.matches("<iframe data-canvas=").count(),
        2,
        "{html:.200}"
    );
    assert!(html.contains("data-for=\"alpha\""));
    assert!(html.contains("data-for=\"beta\""));

    // Each frame holds a real export, escaped into the attribute. If the
    // escaping were wrong the document would end early and the rest would be
    // rendered as text, so finding both intact is the check.
    assert_eq!(
        html.matches("&lt;div id=&quot;root&quot;&gt;&lt;/div&gt;")
            .count(),
        2,
        "each canvas needs its own mount point inside its frame"
    );
    assert!(
        !html.contains("/@vendor/"),
        "a set export still points at the server"
    );
}

/// An empty project is a real state the model can reach, and it must not
/// produce a file that looks like a working export of nothing.
#[test]
fn an_empty_project_exports_nothing_rather_than_something_broken() {
    let project = Project::new("emptyset");
    let flattened = export::export_project(&project.root).expect("export project");
    assert!(flattened.canvases.is_empty());
    assert!(flattened.html.contains("no canvases"), "{}", flattened.html);
}

/// A canvas that reads a file from outside its own directory would inline it
/// into a document meant to be handed to other people.
#[test]
fn a_canvas_cannot_export_a_file_from_outside_itself() {
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

    let error = export::export(&project.root, "demo").expect_err("should refuse");
    assert!(
        matches!(error, export::ExportError::Escapes { .. }),
        "unexpected error: {error}"
    );
}
