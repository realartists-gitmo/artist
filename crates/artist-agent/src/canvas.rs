//! Durable canvas registry search/open/clone surface.
//!
//! Canvas lifecycle and dynamic model I/O are universal-session concerns. This
//! tool only resolves a durable canvas/template referent and opens it.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use artist_canvas::{registry, server::Lazy, templates};
use artist_registry::{Reactivate, SessionStatus};
use artist_session::{CanvasCreated, CanvasOpened, Recorder};
use futures::future::BoxFuture;
use rig_core::tool::{PortableTool, ToolExecutionError};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::RwLock;

use crate::session_tools::{OwnedSession, OwnedState, SessionHub};

#[derive(Clone)]
pub(crate) struct CanvasTool {
    project: PathBuf,
    canvas: Arc<Lazy>,
    recorder: Recorder,
    sessions: SessionHub,
    states: artist_registry::ArtistStates,
}

impl CanvasTool {
    pub fn new(
        project: PathBuf,
        canvas: Arc<Lazy>,
        recorder: Recorder,
        sessions: SessionHub,
    ) -> Self {
        let states = artist_registry::Registry::for_project(&project).artist_states();
        Self {
            project,
            canvas,
            recorder,
            sessions,
            states,
        }
    }

    /// Open the canvas owning an explicitly addressed source file. This is the
    /// path-first counterpart to the search-first `canvas` tool: callers that
    /// already hold the canonical source path do not need a second discovery
    /// round trip merely to run it.
    pub(crate) async fn run_source(&self, source: &Path) -> Result<Option<String>, CanvasError> {
        let source =
            std::fs::canonicalize(source).map_err(|error| CanvasError(error.to_string()))?;
        if !source
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "js" | "jsx" | "ts" | "tsx"))
        {
            return Ok(None);
        }
        let registry = registry::Registry::discover(&self.project);
        for canvas in registry.canvases {
            let root = std::fs::canonicalize(&canvas.root)
                .map_err(|error| CanvasError(error.to_string()))?;
            if source.starts_with(&root) {
                return self
                    .open(&canvas.slug, &canvas.manifest.title)
                    .await
                    .map(Some);
            }
        }
        Ok(None)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CanvasArgs {
    query: String,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct CanvasError(String);

impl From<CanvasError> for ToolExecutionError {
    fn from(value: CanvasError) -> Self {
        ToolExecutionError::other(value.to_string()).with_code("canvas_error")
    }
}

#[derive(Clone, Debug)]
enum Referent {
    Canvas { slug: String, title: String },
    Template { name: String, description: String },
}

impl Referent {
    fn name(&self) -> &str {
        match self {
            Self::Canvas { slug, .. } => slug,
            Self::Template { name, .. } => name,
        }
    }

    fn canonical(&self) -> String {
        match self {
            Self::Canvas { slug, .. } => format!("canvas:{slug}"),
            Self::Template { name, .. } => format!("template:{name}"),
        }
    }

    fn description(&self) -> String {
        match self {
            Self::Canvas { title, .. } if !title.trim().is_empty() => title.clone(),
            Self::Canvas { .. } => "project canvas".into(),
            Self::Template { description, .. } => description.clone(),
        }
    }
}

struct CanvasSession {
    server: Arc<artist_canvas::server::Server>,
    slug: String,
    last: RwLock<Value>,
    window_expected: bool,
}

impl CanvasSession {
    fn control_snapshot(&self) -> Value {
        let handlers = self.server.app_handlers(&self.slug);
        json!({
            "url": self.server.url(&self.slug),
            "appPollRegistered": handlers.poll,
            "sendSupported": handlers.send,
        })
    }

    fn window_closed(&self) -> bool {
        self.window_expected
            && !self
                .server
                .showing()
                .iter()
                .any(|showing| showing == &self.slug)
    }

    fn harness_failure(
        &self,
        kind: &str,
        message: impl Into<String>,
        detail: Option<Value>,
    ) -> Value {
        let mut failure = serde_json::Map::new();
        failure.insert("kind".into(), Value::String(kind.to_owned()));
        failure.insert("message".into(), Value::String(message.into()));
        if let Some(detail) = detail {
            failure.insert("detail".into(), detail);
        }
        let mut snapshot = self.control_snapshot();
        if let Some(object) = snapshot.as_object_mut() {
            object.insert("harnessFailure".into(), Value::Object(failure));
        }
        snapshot
    }
}

impl OwnedSession for CanvasSession {
    fn state(&self) -> BoxFuture<'_, Result<OwnedState, String>> {
        Box::pin(async move {
            if self.window_closed() {
                return Ok(OwnedState::stopped(
                    SessionStatus::Cancelled,
                    self.last.read().await.clone(),
                ));
            }
            if let Some(report) = self.server.current_failure(&self.slug) {
                let snapshot = self.harness_failure("runtime", report.message, report.detail);
                *self.last.write().await = snapshot.clone();
                return Ok(OwnedState::live(snapshot));
            }
            let mut snapshot = self.last.read().await.clone();
            let control = self.control_snapshot();
            if let (Some(current), Some(control)) = (snapshot.as_object_mut(), control.as_object())
            {
                for (key, value) in control {
                    current.insert(key.clone(), value.clone());
                }
            }
            *self.last.write().await = snapshot.clone();
            Ok(OwnedState::live(snapshot))
        })
    }

    fn observe(&self) -> BoxFuture<'_, Result<OwnedState, String>> {
        Box::pin(async move {
            if self.window_closed() {
                return Ok(OwnedState::stopped(
                    SessionStatus::Cancelled,
                    self.last.read().await.clone(),
                ));
            }

            // Harness failures have unconditional precedence over app-authored poll code.
            if let Some(report) = self.server.current_failure(&self.slug) {
                let snapshot = self.harness_failure(
                    if report.level == "build-error" {
                        "build"
                    } else {
                        "runtime"
                    },
                    report.message,
                    report.detail,
                );
                *self.last.write().await = snapshot.clone();
                return Ok(OwnedState::live(snapshot));
            }

            // The client runtime always answers digest. Failure to do so means the page
            // did not mount or the browser/bridge runtime is unavailable.
            let digest = self.server.request_digest(&self.slug).await;
            let Some(digest) = digest else {
                let snapshot = self.harness_failure(
                    "bridge",
                    "canvas page failed to mount or did not answer the runtime bridge",
                    None,
                );
                *self.last.write().await = snapshot.clone();
                return Ok(OwnedState::live(snapshot));
            };
            if let Some(error) = digest
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                let snapshot = self.harness_failure("mount", error, Some(digest));
                *self.last.write().await = snapshot.clone();
                return Ok(OwnedState::live(snapshot));
            }

            let app = match self.server.app_poll(&self.slug).await {
                Ok(value) => value,
                Err(error) => {
                    let snapshot = self.harness_failure("poll", error, None);
                    *self.last.write().await = snapshot.clone();
                    return Ok(OwnedState::live(snapshot));
                }
            };
            let handlers = self.server.app_handlers(&self.slug);
            let snapshot = match app {
                Some(app) => json!({
                    "url": self.server.url(&self.slug),
                    "appPollRegistered": true,
                    "sendSupported": handlers.send,
                    "app": app,
                }),
                None => json!({
                    "url": self.server.url(&self.slug),
                    "appPollRegistered": false,
                    "sendSupported": handlers.send,
                    "app": null,
                    "note": "no app poll hook is registered"
                }),
            };
            *self.last.write().await = snapshot.clone();
            Ok(OwnedState::live(snapshot))
        })
    }

    fn send(&self, input: Value) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if self.window_closed() {
                return Err("canvas session is stopped".into());
            }
            self.server.app_send(&self.slug, input).await
        })
    }

    fn abort(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.server.hide(&self.slug);
            Ok(())
        })
    }
}

impl PortableTool for CanvasTool {
    const NAME: &'static str = "canvas";
    type Error = CanvasError;
    type Args = CanvasArgs;
    type Output = String;

    fn description(&self) -> String {
        "Search canvases and templates. Search is mandatory before opening/cloning an exact referent for this artist session; after it appears in results, repeat that exact query to open the canvas or clone/open the template.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{"query":{"type":"string"}},
            "required":["query"],
            "additionalProperties":false
        })
    }

    async fn call(&self, args: CanvasArgs) -> Result<String, CanvasError> {
        let query = args.query.trim();
        let entries = effective_entries(&self.project);
        let exact = entries.iter().find(|entry| entry.name() == query).cloned();
        let state = self
            .states
            .get(self.sessions.artist())
            .map_err(|error| CanvasError(error.to_string()))?;

        if let Some(referent) = exact.clone() {
            if state
                .seen_canvas_entries
                .iter()
                .any(|seen| seen == &referent.canonical())
            {
                return match referent {
                    Referent::Canvas { slug, title } => self.open(&slug, &title).await,
                    Referent::Template { name, .. } => self.clone_template(&name).await,
                };
            }
        }

        let matches = search_entries(query, &entries);
        self.states
            .update(self.sessions.artist(), |state| {
                for entry in &matches {
                    let canonical = entry.canonical();
                    if !state
                        .seen_canvas_entries
                        .iter()
                        .any(|seen| seen == &canonical)
                    {
                        state.seen_canvas_entries.push(canonical);
                    }
                }
            })
            .map_err(|error| CanvasError(error.to_string()))?;

        let mut lines = matches
            .iter()
            .map(|entry| format!("{} — {}", entry.canonical(), entry.description()))
            .collect::<Vec<_>>();
        if exact.is_some() {
            lines.push(format!(
                "Call canvas {{\"query\":{}}} again with the exact same query to open or clone it.",
                serde_json::to_string(query).unwrap_or_else(|_| "\"\"".into())
            ));
        }
        Ok(if lines.is_empty() {
            "No matching canvases or templates.".into()
        } else {
            lines.join("\n")
        })
    }
}

impl CanvasTool {
    async fn clone_template(&self, template_name: &str) -> Result<String, CanvasError> {
        let template = templates::find(template_name)
            .ok_or_else(|| CanvasError(format!("unknown template `{template_name}`")))?;
        let slug = clone_slug(&self.project, template.name);
        registry::scaffold(&self.project, &slug, &slug, template)
            .map_err(|error| CanvasError(error.to_string()))?;
        self.recorder.record(CanvasCreated {
            slug: slug.clone(),
            title: slug.clone(),
            template: Some(template.name.to_owned()),
        });
        self.open(&slug, &slug).await
    }

    async fn open(&self, slug: &str, title: &str) -> Result<String, CanvasError> {
        let registry = registry::Registry::discover(&self.project);
        let canvas = registry
            .get(slug)
            .ok_or_else(|| CanvasError(format!("no canvas named `{slug}`")))?;
        let title = if canvas.manifest.title.trim().is_empty() {
            title
        } else {
            &canvas.manifest.title
        };
        let id = format!("canvas:{slug}");
        let initial = json!({
            "app": null,
            "appPollRegistered": false,
            "sendSupported": false,
            "note": "no app poll hook is registered"
        });
        match self
            .sessions
            .registry()
            .reactivate(&id, "canvas", self.sessions.artist(), None, initial.clone())
            .map_err(|error| CanvasError(error.to_string()))?
        {
            Reactivate::AlreadyLive(_) => {
                return Ok(format!("canvas://{id}\nalready live"));
            }
            Reactivate::Opened(_) => {}
        }

        let server = self
            .canvas
            .server()
            .await
            .map_err(|error| CanvasError(error.to_string()))?;
        if let Err(error) = server.show(slug, title) {
            let snapshot = json!({"harnessFailure":{"kind":"window","message":error.to_string()}});
            let _ = self
                .sessions
                .registry()
                .finish(&id, SessionStatus::Failed, Some(snapshot));
            return Err(CanvasError(format!("cannot open `{id}`: {error}")));
        }
        self.recorder.record(CanvasOpened {
            slug: slug.to_owned(),
            port: server.addr().port(),
        });
        self.sessions.own(
            id.clone(),
            Arc::new(CanvasSession {
                server,
                slug: slug.to_owned(),
                last: RwLock::new(initial),
                window_expected: true,
            }),
        );
        Ok(format!("canvas://{id}"))
    }
}

fn clone_slug(project: &Path, name: &str) -> String {
    let occupied = effective_entries(project)
        .into_iter()
        .map(|entry| entry.name().to_owned())
        .collect::<BTreeSet<_>>();
    let mut ordinal = 1u64;
    loop {
        let candidate = format!("{name}-{ordinal}");
        if !occupied.contains(&candidate) {
            return candidate;
        }
        ordinal = ordinal.saturating_add(1);
    }
}

fn effective_entries(project: &Path) -> Vec<Referent> {
    // Global built-in templates are the lower layer.
    let mut entries = BTreeMap::<String, Referent>::new();
    for template in artist_canvas::TEMPLATES {
        entries.insert(
            template.name.to_owned(),
            Referent::Template {
                name: template.name.to_owned(),
                description: template.description.to_owned(),
            },
        );
    }
    // Project-local canvases shadow global referents by name.
    for canvas in registry::Registry::discover(project).canvases {
        let slug = canvas.slug.clone();
        entries.insert(
            slug.clone(),
            Referent::Canvas {
                slug,
                title: canvas.manifest.title,
            },
        );
    }
    entries.into_values().collect()
}

fn search_entries<'a>(query: &str, entries: &'a [Referent]) -> Vec<&'a Referent> {
    if query.is_empty() {
        return entries.iter().collect();
    }
    let haystacks = entries
        .iter()
        .map(|entry| format!("{} {}", entry.name(), entry.description()))
        .collect::<Vec<_>>();
    let config = neo_frizbee::Config {
        max_typos: Some((query.chars().count() / 3).max(1).min(u16::MAX as usize) as u16),
        casing: neo_frizbee::CaseMatching::Smart,
        ..Default::default()
    };
    let mut matches = neo_frizbee::match_list(query, &haystacks, &config)
        .into_iter()
        .take(40)
        .filter_map(|matched| entries.get(matched.index as usize))
        .collect::<Vec<_>>();
    if let Some(exact) = entries.iter().find(|entry| entry.name() == query) {
        if !matches
            .iter()
            .any(|entry| entry.canonical() == exact.canonical())
        {
            matches.insert(0, exact);
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_canvas::bridge::DetachedHost;

    fn tool(root: &Path) -> CanvasTool {
        let lazy = Lazy::new(root.to_path_buf(), Arc::new(DetachedHost));
        CanvasTool::new(
            root.to_path_buf(),
            lazy,
            Recorder::noop(),
            SessionHub::standard(root, "Goethe", None),
        )
    }

    #[test]
    fn schema_is_exactly_query() {
        let root = tempfile::tempdir().unwrap();
        let schema = tool(root.path()).parameters();
        assert_eq!(schema["required"], json!(["query"]));
        assert_eq!(schema["properties"].as_object().unwrap().len(), 1);
        for deleted in [
            "create", "open", "close", "status", "state", "docs", "list", "share", "join",
            "export", "eject", "mode",
        ] {
            assert!(schema["properties"].get(deleted).is_none());
        }
    }

    #[tokio::test]
    async fn run_source_declines_files_outside_a_canvas_root() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("ordinary.jsx");
        std::fs::write(&source, "export default null;\n").unwrap();

        assert!(
            tool(root.path())
                .run_source(&source)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn exact_template_is_search_first_and_clone_name_uses_lowest_free_integer() {
        let root = tempfile::tempdir().unwrap();
        let tool = tool(root.path());
        let first = tool
            .call(CanvasArgs {
                query: "blank".into(),
            })
            .await
            .unwrap();
        assert!(first.contains("template:blank"));
        assert!(first.contains("exact same query"));
        assert_eq!(clone_slug(root.path(), "blank"), "blank-1");
        registry::scaffold(
            root.path(),
            "blank-1",
            "occupied",
            templates::find("blank").unwrap(),
        )
        .unwrap();
        assert_eq!(clone_slug(root.path(), "blank"), "blank-2");
        registry::scaffold(
            root.path(),
            "blank-2",
            "occupied",
            templates::find("blank").unwrap(),
        )
        .unwrap();
        assert_eq!(clone_slug(root.path(), "blank"), "blank-3");
    }

    #[test]
    fn project_canvas_shadows_same_named_global_template() {
        let root = tempfile::tempdir().unwrap();
        registry::scaffold(
            root.path(),
            "blank",
            "Local blank",
            templates::find("blank").unwrap(),
        )
        .unwrap();
        let entries = effective_entries(root.path());
        let exact = entries
            .iter()
            .find(|entry| entry.name() == "blank")
            .unwrap();
        assert!(matches!(exact, Referent::Canvas { .. }));
    }
}
