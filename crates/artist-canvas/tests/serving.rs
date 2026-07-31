//! End-to-end checks against a really-bound server.
//!
//! The unit tests cover the pieces in isolation; these exist because the parts
//! that bite — key checking, path jailing, and whether a `.jsx` actually comes
//! back as JavaScript — only exist once the router is assembled.

use std::path::PathBuf;

use artist_canvas::server::Server;

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("artist-canvas-it-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("project root");
        Project { root }
    }

    fn canvas(&self, slug: &str, manifest: &str, entry: &str) -> &Self {
        let directory = self.root.join(".artist/canvas").join(slug);
        std::fs::create_dir_all(&directory).expect("canvas dir");
        std::fs::write(directory.join("canvas.toml"), manifest).expect("manifest");
        std::fs::write(directory.join("main.jsx"), entry).expect("entry");
        self
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

const ENTRY: &str = r#"
import { createRoot } from "react-dom/client";
export default function App() { return <h1 className="title">hello</h1>; }
createRoot(document.getElementById("root")).render(<App />);
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_canvas_is_served_compiled_and_gated_by_its_key() {
    let project = Project::new("serve");
    project.canvas("demo", "title = \"Demo\"", ENTRY);

    let server = Server::start(project.root.clone()).await.expect("server starts");
    let base = server.url("demo");
    let http = reqwest::Client::new();

    // The shell carries the import map and boots the declared entry.
    let shell = http.get(&base).send().await.expect("shell").text().await.unwrap();
    assert!(shell.contains(r#"<script type="importmap">"#), "{shell}");
    assert!(shell.contains(r#""react": "/@vendor/react.js""#), "{shell}");
    assert!(shell.contains("./main.jsx"), "{shell}");

    // JSX arrives as JavaScript, already transformed.
    let module = http
        .get(format!("{base}main.jsx"))
        .send()
        .await
        .expect("module");
    assert_eq!(
        module
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/javascript; charset=utf-8")
    );
    let code = module.text().await.unwrap();
    assert!(!code.contains("<h1"), "still JSX:\n{code}");
    // Development runtime, so a React error names the model's own file.
    assert!(code.contains("react/jsx-dev-runtime"), "{code}");
    // ...and that name is canvas-relative, not the user's home directory.
    assert!(code.contains(r#""main.jsx""#), "{code}");
    assert!(!code.contains(project.root.to_str().unwrap()), "leaked path:\n{code}");

    // The vendored dependency the entry imports is actually there.
    let react = http
        .get(format!("http://{}/@vendor/react.js", server.addr()))
        .send()
        .await
        .expect("react");
    assert!(react.status().is_success());
    assert!(react.text().await.unwrap().len() > 1000);

    // A wrong key is indistinguishable from a missing canvas.
    let forged = base.replace(&base[base.find("/c/").unwrap() + 3..base.rfind("/demo/").unwrap()], "x".repeat(32).as_str());
    assert_eq!(
        http.get(&forged).send().await.expect("forged").status(),
        reqwest::StatusCode::NOT_FOUND
    );

    // Nothing outside the canvas directory is reachable.
    for escape in ["../../../../etc/passwd", "..%2f..%2fCargo.toml"] {
        let response = http
            .get(format!("{base}{escape}"))
            .send()
            .await
            .expect("traversal");
        assert!(
            response.status().is_client_error(),
            "{escape} returned {}",
            response.status()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_page_can_report_its_own_errors_back_to_the_harness() {
    let project = Project::new("report");
    project.canvas("demo", "", ENTRY);

    let server = Server::start(project.root.clone()).await.expect("server starts");
    let rpc = format!("http://{}/_artist/rpc", server.addr());
    let key = server.url("demo");
    let key = &key[key.find("/c/").unwrap() + 3..key.rfind("/demo/").unwrap()];
    let http = reqwest::Client::new();

    let response = http
        .post(format!("{rpc}?slug=demo"))
        .header("origin", format!("http://{}", server.addr()))
        .header("x-artist-key", key)
        .json(&serde_json::json!({
            "method": "canvas.report",
            "params": {"level": "error", "message": "boom", "detail": {"line": 3}},
        }))
        .send()
        .await
        .expect("report");
    assert!(response.status().is_success(), "{:?}", response.status());

    let reports = server.take_reports(Some("demo"));
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].level, "error");
    assert_eq!(reports[0].message, "boom");
    // Draining is destructive, so the model never re-reads a stale failure.
    assert!(server.take_reports(Some("demo")).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn another_page_on_loopback_cannot_drive_the_bridge() {
    let project = Project::new("origin");
    project.canvas("demo", "", ENTRY);

    let server = Server::start(project.root.clone()).await.expect("server starts");
    let url = server.url("demo");
    let key = &url[url.find("/c/").unwrap() + 3..url.rfind("/demo/").unwrap()];

    let response = reqwest::Client::new()
        .post(format!("http://{}/_artist/rpc?slug=demo", server.addr()))
        .header("origin", "http://127.0.0.1:1")
        .header("x-artist-key", key)
        .json(&serde_json::json!({"method": "canvas.report", "params": {}}))
        .send()
        .await
        .expect("cross-origin post");

    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    assert!(server.take_reports(None).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_compile_error_is_captured_for_the_model_and_shown_on_the_page() {
    let project = Project::new("broken");
    project.canvas("demo", "", "export default function App() { const x = ; }");

    let server = Server::start(project.root.clone()).await.expect("server starts");
    let code = reqwest::get(format!("{}main.jsx", server.url("demo")))
        .await
        .expect("module")
        .text()
        .await
        .unwrap();

    // The module throws rather than 404ing, so the browser surfaces it through
    // the page's own error path instead of a silently blank frame.
    assert!(code.contains("throw new Error"), "{code}");

    let reports = server.take_reports(Some("demo"));
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].level, "build-error");
    let detail = reports[0].detail.as_ref().expect("positioned diagnostic");
    assert_eq!(detail["path"], "main.jsx");
    assert_eq!(detail["line"], 1);
}

/// The slug is a query parameter, so it is attacker-controlled even from a page
/// holding the key. Every RPC handler joins it onto a path or uses it to pick a
/// permission set, so an unresolved slug was an arbitrary-directory write.
#[tokio::test(flavor = "multi_thread")]
async fn a_forged_slug_cannot_escape_the_canvas_directory() {
    let project = Project::new("slug");
    project.canvas("demo", "", ENTRY);

    let server = Server::start(project.root.clone()).await.expect("server starts");
    let url = server.url("demo");
    let key = &url[url.find("/c/").unwrap() + 3..url.rfind("/demo/").unwrap()];
    let http = reqwest::Client::new();

    let escape = project.root.join("ESCAPED");
    for slug in ["../../../ESCAPED", "..%2F..%2F..%2FESCAPED", "demo/../../.."] {
        let response = http
            .post(format!("http://{}/_artist/rpc?slug={slug}", server.addr()))
            .header("origin", format!("http://{}", server.addr()))
            .header("x-artist-key", key)
            .json(&serde_json::json!({
                "method": "canvas.state.set",
                "params": {"entries": {"pwned": true}},
            }))
            .send()
            .await
            .expect("post");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::NOT_FOUND,
            "slug `{slug}` was accepted"
        );
    }
    assert!(!escape.join("state.json").exists(), "a file was written outside the canvas");
    assert!(!escape.exists(), "a directory was created outside the canvas");
}

/// A canvas naming a more permissive sibling must not borrow its grants.
#[tokio::test(flavor = "multi_thread")]
async fn a_canvas_cannot_borrow_another_canvases_permissions() {
    let project = Project::new("borrow");
    project.canvas("locked", "", ENTRY);
    project.canvas("open-one", "[permissions]\nallow = [\"bash\"]", ENTRY);

    let server = Server::start(project.root.clone()).await.expect("server starts");
    let url = server.url("locked");
    let key = &url[url.find("/c/").unwrap() + 3..url.rfind("/locked/").unwrap()];

    // The detached host refuses everything, so a 403 here proves the request
    // reached the permission gate under the slug it claimed rather than being
    // silently resolved to the permissive sibling.
    let response = reqwest::Client::new()
        .post(format!("http://{}/_artist/rpc?slug=open-one", server.addr()))
        .header("origin", format!("http://{}", server.addr()))
        .header("x-artist-key", key)
        .json(&serde_json::json!({"method": "canvas.call", "params": {"tool": "bash"}}))
        .send()
        .await
        .expect("post");
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
}

/// The bridge had no end-to-end coverage at all — the headline feature of two
/// commits, exercised only by unit tests either side of the HTTP boundary.
#[tokio::test(flavor = "multi_thread")]
async fn shared_state_round_trips_and_pushes_only_what_changed() {
    let project = Project::new("state");
    project.canvas("demo", "", ENTRY);

    let server = Server::start(project.root.clone()).await.expect("server starts");
    let url = server.url("demo");
    let key = &url[url.find("/c/").unwrap() + 3..url.rfind("/demo/").unwrap()];
    let http = reqwest::Client::new();
    let rpc = |method: &str, params: serde_json::Value| {
        let request = http
            .post(format!("http://{}/_artist/rpc?slug=demo", server.addr()))
            .header("origin", format!("http://{}", server.addr()))
            .header("x-artist-key", key)
            .json(&serde_json::json!({"method": method, "params": params}));
        async move { request.send().await.expect("rpc") }
    };

    // A write from the page is visible to the harness...
    let response = rpc("canvas.state.set", serde_json::json!({"entries": {"rows": [1, 2, 3]}})).await;
    assert!(response.status().is_success());
    assert_eq!(server.state("demo").get("rows"), Some(serde_json::json!([1, 2, 3])));

    // ...and a write from the harness is visible to the page.
    server.publish_state("demo", std::collections::BTreeMap::from([
        ("selected".to_owned(), serde_json::json!(2)),
    ]));
    let body: serde_json::Value = rpc("canvas.state.get", serde_json::json!({}))
        .await
        .json()
        .await
        .expect("json");
    assert_eq!(body["entries"]["selected"], 2);
    assert_eq!(body["entries"]["rows"], serde_json::json!([1, 2, 3]));
    assert!(body["rev"].as_u64().expect("rev") >= 2, "revisions must advance");
}

/// `send` needs an explicit mode: `auto` resolved by whether a turn happened to
/// be running, which the page cannot see.
#[tokio::test(flavor = "multi_thread")]
async fn send_requires_an_explicit_mode() {
    let project = Project::new("send");
    project.canvas("demo", "", ENTRY);

    let server = Server::start(project.root.clone()).await.expect("server starts");
    let url = server.url("demo");
    let key = &url[url.find("/c/").unwrap() + 3..url.rfind("/demo/").unwrap()];
    let http = reqwest::Client::new();

    for (params, expected) in [
        (serde_json::json!({"text": "hi"}), reqwest::StatusCode::BAD_REQUEST),
        (serde_json::json!({"text": "hi", "mode": "auto"}), reqwest::StatusCode::BAD_REQUEST),
        (serde_json::json!({"text": "hi", "mode": "queue"}), reqwest::StatusCode::OK),
    ] {
        let response = http
            .post(format!("http://{}/_artist/rpc?slug=demo", server.addr()))
            .header("origin", format!("http://{}", server.addr()))
            .header("x-artist-key", key)
            .json(&serde_json::json!({"method": "canvas.send", "params": params}))
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), expected, "params were {params}");
    }
}

/// The key must travel in a header, not the URL. The window child takes its URL
/// as a command-line argument, so a key in the path is visible in `ps` to any
/// process on the machine — and the client was already sending the header.
#[tokio::test(flavor = "multi_thread")]
async fn the_rpc_endpoint_only_accepts_the_key_as_a_header() {
    let project = Project::new("keyhdr");
    project.canvas("demo", "", ENTRY);

    let server = Server::start(project.root.clone()).await.expect("server starts");
    let url = server.url("demo");
    let key = &url[url.find("/c/").unwrap() + 3..url.rfind("/demo/").unwrap()];
    let http = reqwest::Client::new();
    let endpoint = format!("http://{}/_artist/rpc", server.addr());
    let origin = format!("http://{}", server.addr());
    let body = serde_json::json!({"method": "canvas.state.get", "params": {}});

    // The old shape — key in the query string — is no longer enough.
    let query_only = http
        .post(format!("{endpoint}?k={key}&slug=demo"))
        .header("origin", &origin)
        .json(&body)
        .send()
        .await
        .expect("post");
    assert_eq!(query_only.status(), reqwest::StatusCode::NOT_FOUND);

    let with_header = http
        .post(format!("{endpoint}?slug=demo"))
        .header("origin", &origin)
        .header("x-artist-key", key)
        .json(&body)
        .send()
        .await
        .expect("post");
    assert!(with_header.status().is_success(), "{:?}", with_header.status());

    // And a wrong header value is refused.
    let wrong = http
        .post(format!("{endpoint}?slug=demo"))
        .header("origin", &origin)
        .header("x-artist-key", "x".repeat(32))
        .json(&body)
        .send()
        .await
        .expect("post");
    assert_eq!(wrong.status(), reqwest::StatusCode::NOT_FOUND);
}
