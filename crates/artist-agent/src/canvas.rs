//! The `canvas` tool.
//!
//! Canvases are authored with the ordinary `write` and `edit` tools — this tool
//! handles lifecycle and, more importantly, *feedback*. A model writing UI it
//! cannot see is writing blind, so `status` is the load-bearing mode: it
//! reports compile errors, runtime failures, what the user did, and what the
//! canvas currently holds.
//!
//! All authoring guidance lives in `description()` and in the JSON-Schema
//! property descriptions, per `tool_prompt`'s contract — never in the system
//! prompt.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use artist_canvas::{
    registry::{self, Registry},
    server::Server,
    templates,
};
use artist_session::{CanvasCreated, CanvasOpened, CanvasState};
use rig_core::tool::PortableTool;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Recorder;

#[derive(Clone)]
pub struct CanvasTool {
    project: PathBuf,
    server: Arc<Server>,
    recorder: Recorder,
}

impl CanvasTool {
    pub fn new(project: PathBuf, server: Arc<Server>, recorder: Recorder) -> Self {
        CanvasTool {
            project,
            server,
            recorder,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct CanvasArgs {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    entries: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    topic: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct CanvasError(String);

impl PortableTool for CanvasTool {
    const NAME: &'static str = "canvas";
    type Error = CanvasError;
    type Args = CanvasArgs;
    type Output = String;

    fn description(&self) -> String {
        description_text()
    }

    fn parameters(&self) -> Value {
        schema()
    }

    async fn call(&self, args: CanvasArgs) -> Result<String, CanvasError> {
        let mode = args.mode.as_deref().unwrap_or("status");

        // `docs` and `list` are the only modes that do not name a canvas.
        if matches!(mode, "docs") {
            return Ok(artist_canvas::docs::render(args.topic.as_deref()));
        }
        if matches!(mode, "list") {
            return Ok(self.list());
        }

        let raw = args.name.as_deref().unwrap_or_default();
        if raw.trim().is_empty() {
            return Err(CanvasError(format!("mode={mode} needs a `name`")));
        }
        let slug = registry::slugify(raw);

        match mode {
            "create" => self.create(&slug, args),
            "open" => self.open(&slug),
            "status" => Ok(self.status(&slug)),
            "eject" => self.eject(&slug),
            "state" => self.state(&slug, args),
            other => Err(CanvasError(format!(
                "unknown canvas mode: {other} (expected create, open, status, state, docs, list or eject)"
            ))),
        }
    }
}

/// The model-facing description.
///
/// A free function rather than a method because it depends on nothing but the
/// template list — which lets it be tested without standing up a server.
fn description_text() -> String {
    format!(
            "Build a live React app the user can open in a browser and interact with, hosted on \
             this machine. Use it when a reply would be better as an interface than as text: a \
             dashboard over changing data, a form that collects a decision, a diff to walk \
             through, a report with charts.\n\n\
             Lifecycle: `create` scaffolds a working app, then edit its files with the normal \
             write/edit tools — the open page reloads itself on every save. Call `status` after \
             editing; it is how you see compile errors, browser exceptions, and what the user \
             did. Push data with `state` rather than rewriting code.\n\n\
             Files live in .artist/canvas/<name>/ and persist across sessions, so prefer growing \
             an existing canvas over creating a new one.\n\n\
             Available imports (no other packages resolve): react, react-dom/client, uplot, \
             @tanstack/react-table, and Tailwind classes. Tailwind's palette is remapped to \
             artist's own colours, so ordinary classes like `bg-blue-100` or `text-red-700` are \
             already on-theme — use them, and avoid hard-coded hex, rgb() and named colours, \
             which bypass the theme and are reported back to you by `status`. Plus the kit — \
             prefer it over writing your own:\n\
             • @artist/ui — AppShell, Toolbar, Stack, Split, Card, EmptyState, Skeleton, Button, \
             Input, Select, Checkbox, Badge, Tabs, Dialog, Toaster/toast, ErrorBoundary, \
             DataTable, Plot, Code, Diff, SchemaForm, Transcript, ToolLog, AskDock, Approve\n\
             • @artist/react — useCanvasState, useAgent, useAgentEvents, useAsk, useTool, useTheme\n\
             • @artist/canvas — artist.send, artist.call, artist.state, artist.highlight\n\n\
             useCanvasState(key, initial) is shared state: the user's clicks land in it, you read \
             it with mode=state, and you write it with mode=state entries={{...}}. That is the \
             normal way to feed a canvas.\n\n\
             Templates: {}.\n\
             Call mode=docs for full component signatures and examples before writing anything \
             non-trivial.",
        templates::names().join(", ")
    )
}

fn schema() -> Value {
    json!({
            "type": "object",
            "properties": {
                "mode": {
                    "enum": ["create", "open", "status", "state", "docs", "list", "eject"],
                    "default": "status",
                    "description":
                        "create: scaffold a new canvas from a template. open: get its URL to give \
                         the user. status: compile errors, browser errors, console output and \
                         current state — call this after every edit. state: read shared state, or \
                         write it by passing `entries`. docs: component reference. list: every \
                         canvas in this project. eject: convert to a standalone Vite project when \
                         a canvas outgrows the built-in dependency set — after which Artist stops \
                         serving it."
                },
                "name": {
                    "type": "string",
                    "description":
                        "Canvas identifier, kebab-case, e.g. `test-dashboard`. Required for every \
                         mode except list and docs."
                },
                "title": {
                    "type": "string",
                    "description": "Human-readable title for create; shown in the browser tab."
                },
                "template": {
                    "enum": templates::names(),
                    "default": "blank",
                    "description":
                        "Starting point for create. Each is a working app, not a stub — pick the \
                         one closest to the intent and edit it rather than starting from blank."
                },
                "entries": {
                    "type": "object",
                    "description":
                        "For mode=state: keys to write into shared state, merged into what is \
                         already there. Values are arbitrary JSON. The open page re-renders \
                         immediately. This is how you feed a canvas data — do not rewrite the \
                         component to embed values.",
                    "additionalProperties": true
                },
                "topic": {
                    "type": "string",
                    "description":
                        "For mode=docs: a component or hook name (e.g. `Plot`, `useCanvasState`). \
                         Omit for the full reference."
                }
        },
        "additionalProperties": false
    })
}

impl CanvasTool {
    fn create(&self, slug: &str, args: CanvasArgs) -> Result<String, CanvasError> {
        let name = args.template.as_deref().unwrap_or("blank");
        let template = templates::find(name).ok_or_else(|| {
            CanvasError(format!(
                "unknown template `{name}` (have {})",
                templates::names().join(", ")
            ))
        })?;
        let title = args
            .title
            .clone()
            .unwrap_or_else(|| slug.replace('-', " "));

        let canvas = registry::scaffold(&self.project, slug, &title, template)
            .map_err(|error| CanvasError(error.to_string()))?;

        self.recorder.record(CanvasCreated {
            slug: slug.to_owned(),
            title: title.clone(),
            template: Some(template.name.to_owned()),
        });

        let files = template
            .files
            .iter()
            .map(|(path, _)| format!(".artist/canvas/{slug}/{path}"))
            .collect::<Vec<_>>()
            .join("\n  ");

        Ok(format!(
            "Created canvas `{slug}` from the {} template.\n  {files}\n\n\
             Open it for the user with mode=open. Edit those files with write/edit — the page \
             reloads on save — then call mode=status to see whether it compiled.\n\n\
             Entry point: {}",
            template.name,
            canvas.entry_path().display()
        ))
    }

    fn open(&self, slug: &str) -> Result<String, CanvasError> {
        let registry = Registry::discover(&self.project);
        if registry.get(slug).is_none() {
            return Err(CanvasError(self.unknown(slug, &registry)));
        }
        self.recorder.record(CanvasOpened {
            slug: slug.to_owned(),
            port: self.server.addr().port(),
        });

        let url = self.server.url(slug);
        let title = registry
            .get(slug)
            .map(|canvas| canvas.manifest.title.clone())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| slug.to_owned());

        // A window artist owns beats a tab the user has to find. Falling back
        // to the URL matters more than it sounds: over SSH, or in a build
        // without a webview, that is the only way in.
        if artist_canvas::window::available() {
            match artist_canvas::window::open(&url, &title) {
                Ok(_) => {
                    return Ok(format!(
                        "Opened `{slug}` in a window. It reloads itself whenever you edit the \
                         canvas, and closes when this session ends.\n\n\
                         If the window did not appear, give the user this URL instead:\n{url}"
                    ));
                }
                Err(error) => {
                    return Ok(format!(
                        "Could not open a window ({error}). Give the user this URL instead:\n{url}"
                    ));
                }
            }
        }

        Ok(format!(
            "{url}\n\nNo display is available, so give the user this URL. It stays live while \
             this session runs, and reloads itself whenever you edit the canvas."
        ))
    }

    /// The feedback loop. Everything the model needs to know about a canvas it
    /// cannot see, in one place.
    fn status(&self, slug: &str) -> String {
        let registry = Registry::discover(&self.project);
        let Some(canvas) = registry.get(slug) else {
            return self.unknown(slug, &registry);
        };

        let mut out = format!("canvas `{slug}` — {}\n", canvas.manifest.title);
        out.push_str(&format!("url: {}\n", self.server.url(slug)));

        let reports = self.server.take_reports(Some(slug));
        let (problems, rest): (Vec<_>, Vec<_>) = reports
            .iter()
            .partition(|report| matches!(report.level.as_str(), "error" | "build-error"));
        // Style drift is neither an error nor console noise; grouping it with
        // either would get it skimmed past.
        let (style, chatter): (Vec<&artist_canvas::server::Report>, Vec<_>) = rest
            .into_iter()
            .partition(|report| report.level == "style");

        if problems.is_empty() {
            out.push_str("\nNo errors reported since the last check.\n");
        } else {
            out.push_str(&format!("\n{} error(s):\n", problems.len()));
            for report in &problems {
                out.push_str(&format!("  [{}] {}\n", report.level, report.message));
                if let Some(detail) = &report.detail {
                    if let Some(stack) = detail.get("stack").and_then(|v| v.as_str()) {
                        for line in stack.lines().take(4) {
                            out.push_str(&format!("      {line}\n"));
                        }
                    }
                }
            }
        }

        if !style.is_empty() {
            out.push_str("\nOff-palette values (these bypass the canvas theme):\n");
            for report in &style {
                out.push_str(&report.message.trim_start_matches('\n').to_string());
            }
        }

        if !chatter.is_empty() {
            out.push_str(&format!("\n{} console message(s):\n", chatter.len()));
            for report in chatter.iter().take(20) {
                out.push_str(&format!("  [{}] {}\n", report.level, report.message));
            }
        }

        let snapshot = self.server.state(slug).snapshot();
        if snapshot.entries.is_empty() {
            out.push_str("\nShared state is empty.\n");
        } else {
            out.push_str(&format!("\nShared state (rev {}):\n", snapshot.rev));
            for (key, value) in snapshot.plain() {
                out.push_str(&format!("  {key} = {}\n", truncate(&value.to_string(), 200)));
            }
        }
        out
    }

    fn state(&self, slug: &str, args: CanvasArgs) -> Result<String, CanvasError> {
        let registry = Registry::discover(&self.project);
        if registry.get(slug).is_none() {
            return Err(CanvasError(self.unknown(slug, &registry)));
        }

        match args.entries {
            None => {
                let snapshot = self.server.state(slug).snapshot();
                Ok(serde_json::to_string_pretty(&json!({
                    "rev": snapshot.rev,
                    "entries": snapshot.plain(),
                }))
                .unwrap_or_else(|_| "{}".into()))
            }
            Some(entries) => {
                let values: BTreeMap<String, Value> = entries.into_iter().collect();
                let keys = values.keys().cloned().collect::<Vec<_>>().join(", ");
                let rev = self.server.publish_state(slug, values);
                let snapshot = self.server.state(slug).snapshot();
                self.recorder.record(CanvasState {
                    slug: slug.to_owned(),
                    rev,
                    entries: serde_json::to_value(snapshot.plain()).unwrap_or_default(),
                });
                Ok(format!(
                    "Wrote {keys} to `{slug}` (rev {rev}). Any open page has already re-rendered."
                ))
            }
        }
    }

    fn eject(&self, slug: &str) -> Result<String, CanvasError> {
        let registry = Registry::discover(&self.project);
        let Some(canvas) = registry.get(slug) else {
            return Err(CanvasError(self.unknown(slug, &registry)));
        };
        let written = templates::eject(&canvas.root, &canvas.manifest.title)
            .map_err(|error| CanvasError(error.to_string()))?;
        Ok(format!(
            "Ejected `{slug}` to a standalone Vite project.\n  wrote: {}\n\n\
             Artist no longer serves this canvas. Replace its @artist/* imports with local code, \
             then `npm install && npm run dev` in {}.",
            if written.is_empty() { "nothing (already ejected)".to_owned() } else { written.join(", ") },
            canvas.root.display()
        ))
    }

    fn list(&self) -> String {
        let registry = Registry::discover(&self.project);
        if registry.canvases.is_empty() {
            return "No canvases in this project yet. Create one with mode=create.".into();
        }
        let mut out = String::new();
        for canvas in &registry.canvases {
            out.push_str(&format!(
                "{} — {}\n  {}\n",
                canvas.slug,
                if canvas.manifest.title.is_empty() {
                    "(untitled)"
                } else {
                    &canvas.manifest.title
                },
                self.server.url(&canvas.slug)
            ));
        }
        for diagnostic in &registry.diagnostics {
            out.push_str(&format!("{} — BROKEN: {}\n", diagnostic.slug, diagnostic.message));
        }
        out
    }

    fn unknown(&self, slug: &str, registry: &Registry) -> String {
        let known: Vec<_> = registry.canvases.iter().map(|c| c.slug.as_str()).collect();
        if known.is_empty() {
            format!("No canvas named `{slug}`, and none exist yet. Create one with mode=create.")
        } else {
            format!(
                "No canvas named `{slug}`. This project has: {}.",
                known.join(", ")
            )
        }
    }
}

fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let head: String = value.chars().take(limit).collect();
    format!("{head}… ({} chars)", value.chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kit only prevents reinvention if the model is told it exists, and
    /// per `tool_prompt` this description is the only place it can be told.
    #[test]
    fn the_description_names_every_template_and_the_kit() {
        let description = description_text();
        for name in templates::names() {
            assert!(description.contains(name), "template {name} is undocumented");
        }
        for symbol in [
            "@artist/ui",
            "@artist/react",
            "DataTable",
            "SchemaForm",
            "useCanvasState",
            "artist.send",
            ".artist/canvas/",
        ] {
            assert!(description.contains(symbol), "{symbol} is undocumented");
        }
    }

    /// A template offered in the schema but absent from the registry would be
    /// an error the model can only discover by trying it.
    #[test]
    fn the_schema_offers_only_real_templates_and_modes() {
        let schema = schema();
        let listed: Vec<String> =
            serde_json::from_value(schema["properties"]["template"]["enum"].clone())
                .expect("template enum");
        assert_eq!(listed, templates::names());
        for name in &listed {
            assert!(templates::find(name).is_some(), "{name} is offered but missing");
        }

        let modes: Vec<String> =
            serde_json::from_value(schema["properties"]["mode"]["enum"].clone()).expect("mode enum");
        for mode in ["create", "open", "status", "state", "docs", "list"] {
            assert!(modes.contains(&mode.to_owned()), "{mode} missing from the schema");
        }
    }

    /// Every documented mode needs a property description; a bare enum tells
    /// the model nothing about when to use which.
    #[test]
    fn every_argument_is_described() {
        let schema = schema();
        for (name, spec) in schema["properties"].as_object().expect("properties") {
            let described = spec
                .get("description")
                .and_then(|d| d.as_str())
                .is_some_and(|d| d.len() > 20);
            assert!(described, "`{name}` needs a real description");
        }
    }

    #[test]
    fn long_state_values_are_truncated_with_their_real_size() {
        let long = "x".repeat(500);
        let shown = truncate(&long, 100);
        assert!(shown.starts_with(&"x".repeat(100)));
        assert!(shown.contains("500 chars"));
        assert_eq!(truncate("short", 100), "short");
    }
}
