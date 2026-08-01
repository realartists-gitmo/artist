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
    server::{Lazy, Server},
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
    /// Not a running server: most sessions register this tool and never call
    /// it, and binding a port for those is a cost and an exposure with no
    /// matching benefit. The server appears on the first call that needs one.
    canvas: Arc<Lazy>,
    recorder: Recorder,
}

impl CanvasTool {
    pub fn new(project: PathBuf, canvas: Arc<Lazy>, recorder: Recorder) -> Self {
        CanvasTool {
            project,
            canvas,
            recorder,
        }
    }

    /// The server, started if this is the first call that needs it.
    async fn server(&self) -> Result<Arc<Server>, CanvasError> {
        self.canvas.server().await.map_err(|error| {
            CanvasError(format!(
                "could not start the canvas server: {error}. Canvases need a loopback port; \
                 everything else in this session still works."
            ))
        })
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

        // `docs` and `list` name no canvas, and — the point of the split — need
        // no server. A model orienting itself must not bind a port to do it.
        if matches!(mode, "docs") {
            return Ok(artist_canvas::docs::render(args.topic.as_deref()));
        }
        if matches!(mode, "list") {
            return Ok(self.list(self.canvas.started().await.as_deref()));
        }

        let raw = args.name.as_deref().unwrap_or_default();
        if raw.trim().is_empty() {
            return Err(CanvasError(format!("mode={mode} needs a `name`")));
        }
        let slug = registry::slugify(raw);

        match mode {
            // `create` writes files; nothing is served until someone opens it.
            "create" => self.create(&slug, args),
            "eject" => self.eject(&slug),
            // The only mode whose whole purpose is to serve something, and so
            // the only one that may bring a server into being.
            "open" => self.open(&*self.server().await?, &slug),
            // The rest report on, or write to, things that exist without a
            // server: windows this session opened, files on disk. Starting one
            // to answer them would undo what `Lazy` is for — and `status` is
            // the default mode, so `canvas(name=…)` alone would have bound a
            // port and stood up an RPC surface holding a session key.
            "close" => Ok(self.close(self.canvas.started().await.as_deref(), &slug)),
            "status" => Ok(self
                .status(self.canvas.started().await.as_deref(), &slug)
                .await),
            "state" => self.state(self.canvas.started().await.as_deref(), &slug, args),
            other => Err(CanvasError(format!(
                "unknown canvas mode: {other} \
                 (expected create, open, close, status, state, docs, list or eject)"
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
             write/edit tools — the open page picks up every save. Call `status` after editing; \
             it is how you see compile errors, browser exceptions, and what the user did. Push \
             data with `state` rather than rewriting code.\n\n\
             Write the app in App.jsx. Saving it swaps the component in place, so whatever the \
             user had typed, scrolled to or focused survives the edit — which is why App.jsx \
             must export components and nothing else. Export a constant, a helper or a store \
             from it and the page falls back to a full reload on every save, losing their work; \
             keep those module-local, or put them in their own file. main.jsx only mounts the \
             app and is not worth editing.\n\n\
             Files live in .artist/canvas/<name>/ and persist across sessions, so prefer growing \
             an existing canvas over creating a new one.\n\n\
             Available imports (no other packages resolve): react, react-dom/client, uplot, \
             @tanstack/react-table, and Tailwind classes. Tailwind's palette is remapped to \
             artist's own colours, so ordinary classes like `bg-blue-100` or `text-red-700` are \
             already on-theme — use them, and avoid hard-coded hex, rgb() and named colours, \
             which bypass the theme and are reported back to you by `status`. Composed class \
             names like `text-${{tone}}-700` do work here, unlike in a build-time Tailwind \
             setup: the palette is generated in the browser from the live DOM, so a class only \
             has to exist once it is rendered. Plus the kit — prefer it over writing your own:\n\
             • @artist/ui — AppShell, Toolbar, Stack, Split, Card, EmptyState, Pending, Button, \
             Input, Textarea, Select, Checkbox, Badge, Alert, Metric, Markdown, FileLink, Tabs, \
             Dialog, Toaster/toast, ErrorBoundary, DataTable, Plot, Sparkline, Code, Diff, \
             SchemaForm, Transcript, ToolLog, AskDock, Approve. Everything that comes in \
             flavours takes `variant`: default | accent | ok | warn | danger. Use <Markdown> for \
             prose and <FileLink path line> every time you name a file — it opens the user's \
             editor there.\n\
             • @artist/react — useCanvasState, useAgent, useAgentEvents, useAsk, useTool, useTheme\n\
             • @artist/canvas — artist.send, artist.call, artist.state, artist.highlight\n\n\
             artist.send(text, {{mode}}) needs a mode: \"steer\" corrects the turn that is \
             running (and is refused if none is), \"queue\" starts one after it. It resolves to \
             the outcome, so a button can tell the user what happened. artist.state.set(k, v, \
             {{notify: true}}) also raises a badge in the terminal, which is how a click reaches \
             you when no turn is running.\n\n\
             useCanvasState(key, initial) is shared state: the user's clicks land in it, you read \
             it with mode=state, and you write it with mode=state entries={{...}}. That is the \
             normal way to feed a canvas.\n\n\
             Templates: {}.\n\
             Call mode=docs for full component signatures and examples before writing anything \
             non-trivial.",
        templates::names().join(", ")
    )
    // ---------------------------------------------------------------------
    // Anti-guidance — when to *not* reach for a canvas — would go here.
    //
    // Deliberately absent for now. The failure mode it would guard against
    // (a canvas built for something a sentence would have answered) has not
    // been observed yet, and every line in this description is paid for on
    // every request of every turn. Writing the warning before seeing the
    // behaviour would be guessing at which way the model errs, and a wrong
    // guess here is worse than silence: it would push the model away from
    // the feature in exactly the cases we cannot predict.
    //
    // Add it when `status` output or user reports show canvases being made
    // for things that did not want one, and phrase it from those cases.
    // ---------------------------------------------------------------------
}

fn schema() -> Value {
    json!({
            "type": "object",
            "properties": {
                "mode": {
                    "enum": ["create", "open", "close", "status", "state", "docs", "list", "eject"],
                    "default": "status",
                    "description":
                        "create: scaffold a new canvas from a template. open: put it on screen in \
                         a window (or hand over a URL where there is no display). close: take that \
                         window down, leaving the files alone. status: compile errors, browser \
                         errors, console output and \
                         current state — call this after every edit. state: read shared state, or \
                         write it by passing `entries`. status also describes what is on \
                         screen — the text, contrast failures, overflow — which is the only \
                         way you can see what you built. docs: component reference. list: every \
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
        let title = args.title.clone().unwrap_or_else(|| slug.replace('-', " "));

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

    fn open(&self, server: &Server, slug: &str) -> Result<String, CanvasError> {
        let registry = Registry::discover(&self.project);
        if registry.get(slug).is_none() {
            return Err(CanvasError(self.unknown(slug, &registry)));
        }
        self.recorder.record(CanvasOpened {
            slug: slug.to_owned(),
            port: server.addr().port(),
        });

        let url = server.url(slug);
        let title = registry
            .get(slug)
            .map(|canvas| canvas.manifest.title.clone())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| slug.to_owned());

        // A window artist owns beats a tab the user has to find. Falling back
        // to the URL matters more than it sounds: over SSH, or in a build
        // without a webview, that is the only way in.
        if artist_canvas::window::available() {
            match server.show(slug, &title) {
                Ok(artist_canvas::window::Opened::Spawned) => {
                    return Ok(format!(
                        "Opened `{slug}` in a window. It reloads itself whenever you edit the \
                         canvas, and closes when this session ends.\n\n\
                         If the window did not appear, give the user this URL instead:\n{url}"
                    ));
                }
                // Saying so matters: the model's next move otherwise is to
                // open it again, and the user watches nothing happen twice.
                Ok(artist_canvas::window::Opened::Already) => {
                    return Ok(format!(
                        "`{slug}` is already open in a window — the user is looking at it now. \
                         Edits reload it in place, so there is nothing to reopen."
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

    /// Take a canvas's window off the screen.
    ///
    /// The files stay. Closing a window is putting something away, not
    /// discarding it — a canvas is durable, and deleting one is the user's
    /// call, made with the file tools like any other deletion.
    /// No server is no windows: nothing opened one this session, so there is
    /// nothing to put away — and no reason to bind a port to discover that.
    fn close(&self, server: Option<&Server>, slug: &str) -> String {
        if server.is_some_and(|server| server.hide(slug)) {
            format!("Closed the window showing `{slug}`. Its files are untouched.")
        } else {
            format!("`{slug}` did not have a window open, so there was nothing to close.")
        }
    }

    /// The feedback loop. Everything the model needs to know about a canvas it
    /// cannot see, in one place.
    async fn status(&self, server: Option<&Server>, slug: &str) -> String {
        let registry = Registry::discover(&self.project);
        let Some(canvas) = registry.get(slug) else {
            return self.unknown(slug, &registry);
        };

        // Nothing served this session means no reports to collect and no page
        // to inspect — every line below this would be empty. The durable half
        // still exists and is worth reporting: a model that just wrote state
        // and asked `status` should see it, not a blank report.
        let Some(server) = server else {
            let snapshot = artist_canvas::StateStore::open(&canvas.root).snapshot();
            let entries = serde_json::to_string(&snapshot.plain()).unwrap_or_else(|_| "{}".into());
            return format!(
                "canvas `{slug}` — {}\n\n\
                 Not being served: nothing has opened a canvas this session, so there is no \
                 page to report errors from and nothing on screen to inspect. Use mode=open \
                 to put it up.\n\n\
                 Durable state (rev {}): {entries}\n",
                canvas.manifest.title, snapshot.rev
            );
        };

        let mut out = format!("canvas `{slug}` — {}\n", canvas.manifest.title);
        out.push_str(&format!("url: {}\n", server.url(slug)));

        // Read before the drain, which closes the window it describes.
        let (since, seen) = server.report_window(slug);
        let reports = server.take_reports(Some(slug));
        let (problems, rest): (Vec<_>, Vec<_>) = reports
            .iter()
            .partition(|report| matches!(report.level.as_str(), "error" | "build-error"));
        // Style drift is neither an error nor console noise; grouping it with
        // either would get it skimmed past.
        let (style, chatter): (Vec<&artist_canvas::server::Report>, Vec<_>) =
            rest.into_iter().partition(|report| report.level == "style");

        if problems.is_empty() {
            // Unqualified, "no errors" reads as "none, ever" — and reports
            // drain, so it only ever meant "none since something last looked".
            // Two calls in a row would show errors and then silence, and the
            // silence reads as a fix.
            out.push_str(&match since {
                Some(elapsed) => format!(
                    "\nNo errors in the {} since the last check ({seen} report(s) so far this \
                     session).\n",
                    describe(elapsed)
                ),
                None => "\nNo errors since this canvas was first served.\n".to_owned(),
            });
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

        // What is actually on screen. Every other line above reports whether
        // the code ran; this is the only one that reports what it produced —
        // and a wrong number or an unreadable label is invisible without it.
        match server.request_digest(slug).await {
            None => out.push_str("\nNo open window, so nothing to inspect. Use mode=open.\n"),
            Some(digest) if digest.get("mounted").is_none() => out.push_str(&format!(
                "\nThe page did not mount: {}\n",
                digest
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("unknown")
            )),
            Some(digest) => {
                // Which build the page is showing. A canvas the model just
                // edited may not have swapped yet, and a digest of the previous
                // version is indistinguishable from proof the edit did nothing
                // — the single most likely way this report misleads.
                let current = server.revision(slug);
                let showing = digest.get("rev").and_then(|rev| rev.as_u64());
                match showing {
                    Some(showing) if showing < current => out.push_str(&format!(
                        "\nOn screen now — but this page is showing build {showing} and the \
                         canvas is at {current}, so it has not picked up your last {} edit(s) \
                         yet. Check again before concluding anything from what follows.\n",
                        current - showing
                    )),
                    _ => out.push_str("\nOn screen now:\n"),
                }
                if let Some(text) = digest.get("text").and_then(|t| t.as_array()) {
                    let visible: Vec<_> = text
                        .iter()
                        .filter_map(|line| line.as_str())
                        .take(40)
                        .collect();
                    out.push_str(&format!("  text: {}\n", visible.join(" | ")));
                }
                for (key, label) in [
                    ("elements", "elements"),
                    ("overflowing", "overflowing their container"),
                    ("unreadable", "below 4.5:1 contrast"),
                ] {
                    if let Some(items) = digest.get(key).and_then(|v| v.as_array())
                        && !items.is_empty()
                    {
                        let rendered: Vec<_> = items.iter().filter_map(|i| i.as_str()).collect();
                        out.push_str(&format!("  {label}: {}\n", rendered.join(", ")));
                    }
                }
            }
        }

        let snapshot = server.state(slug).snapshot();
        if snapshot.entries.is_empty() {
            out.push_str("\nShared state is empty.\n");
        } else {
            out.push_str(&format!("\nShared state (rev {}):\n", snapshot.rev));
            for (key, value) in snapshot.plain() {
                out.push_str(&format!(
                    "  {key} = {}\n",
                    truncate(&value.to_string(), 200)
                ));
            }
        }
        out
    }

    /// State is a file. Neither reading nor writing it needs a server — the
    /// server only adds the push to open pages, and with nothing served there
    /// are none. The store reconciles against disk whichever way it is opened,
    /// so a server starting later picks this up.
    fn state(
        &self,
        server: Option<&Server>,
        slug: &str,
        args: CanvasArgs,
    ) -> Result<String, CanvasError> {
        let registry = Registry::discover(&self.project);
        let Some(canvas) = registry.get(slug) else {
            return Err(CanvasError(self.unknown(slug, &registry)));
        };

        match args.entries {
            None => {
                let snapshot = match server {
                    Some(server) => server.state(slug).snapshot(),
                    None => artist_canvas::StateStore::open(&canvas.root).snapshot(),
                };
                Ok(serde_json::to_string_pretty(&json!({
                    "rev": snapshot.rev,
                    "entries": snapshot.plain(),
                }))
                .unwrap_or_else(|_| "{}".into()))
            }
            Some(entries) => {
                let values: BTreeMap<String, Value> = entries.into_iter().collect();
                let keys = values.keys().cloned().collect::<Vec<_>>().join(", ");
                // A refused write is returned as the tool's error, not reported
                // as a success with a caveat: the model has to know its data is
                // not there before it builds a reply on top of it.
                let (rev, snapshot, rendered) = match server {
                    Some(server) => {
                        let rev = server
                            .publish_state(slug, values)
                            .map_err(|over| CanvasError(over.to_string()))?;
                        (
                            rev,
                            server.state(slug).snapshot(),
                            "Any open page has already re-rendered.",
                        )
                    }
                    None => {
                        let snapshot = artist_canvas::StateStore::open(&canvas.root)
                            .merge(values)
                            .map_err(|over| CanvasError(over.to_string()))?;
                        (
                            snapshot.rev,
                            snapshot,
                            "No page is open, so nothing re-rendered — it is on disk for the \
                             next one.",
                        )
                    }
                };
                self.recorder.record(CanvasState {
                    slug: slug.to_owned(),
                    rev,
                    entries: serde_json::to_value(snapshot.plain()).unwrap_or_default(),
                });
                Ok(format!("Wrote {keys} to `{slug}` (rev {rev}). {rendered}"))
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
            if written.is_empty() {
                "nothing (already ejected)".to_owned()
            } else {
                written.join(", ")
            },
            canvas.root.display()
        ))
    }

    fn list(&self, server: Option<&Server>) -> String {
        let registry = Registry::discover(&self.project);
        if registry.canvases.is_empty() {
            return "No canvases in this project yet. Create one with mode=create.".into();
        }
        let mut out = String::new();
        for canvas in &registry.canvases {
            out.push_str(&format!(
                "{} — {}\n",
                canvas.slug,
                if canvas.manifest.title.is_empty() {
                    "(untitled)"
                } else {
                    &canvas.manifest.title
                },
            ));
            // Only once something is being served. Listing what exists must not
            // itself start a server, and a URL for a canvas nobody has opened
            // would be a URL that stops working the moment the session ends.
            if let Some(server) = server {
                out.push_str(&format!("  {}\n", server.url(&canvas.slug)));
            }
        }
        for diagnostic in &registry.diagnostics {
            out.push_str(&format!(
                "{} — BROKEN: {}\n",
                diagnostic.slug, diagnostic.message
            ));
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

/// An elapsed time the way a person would say it.
///
/// This lands mid-sentence in a report the model reads as prose; `83.4721s`
/// reads as machine output and gets skimmed past, which defeats the point of
/// saying it at all.
fn describe(elapsed: std::time::Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds == 0 {
        return "moment".to_owned();
    }
    let (count, unit) = match seconds {
        seconds if seconds < 60 => (seconds, "second"),
        seconds if seconds < 3600 => (seconds / 60, "minute"),
        seconds => (seconds / 3600, "hour"),
    };
    format!("{count} {unit}{}", if count == 1 { "" } else { "s" })
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
            assert!(
                description.contains(name),
                "template {name} is undocumented"
            );
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
            assert!(
                templates::find(name).is_some(),
                "{name} is offered but missing"
            );
        }

        let modes: Vec<String> =
            serde_json::from_value(schema["properties"]["mode"]["enum"].clone())
                .expect("mode enum");
        for mode in ["create", "open", "status", "state", "docs", "list"] {
            assert!(
                modes.contains(&mode.to_owned()),
                "{mode} missing from the schema"
            );
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

    /// Stand up a tool over a scaffolded canvas whose server has never run.
    fn unstarted(project: &std::path::Path) -> (CanvasTool, Arc<Lazy>) {
        let template = templates::find("blank").expect("blank template");
        registry::scaffold(project, "demo", "Demo", template).expect("scaffold");
        let canvas = Lazy::new(
            project.to_owned(),
            Arc::new(artist_canvas::bridge::DetachedHost),
        );
        let tool = CanvasTool::new(project.to_owned(), Arc::clone(&canvas), Recorder::noop());
        (tool, canvas)
    }

    /// Lazy start is a posture decision, not an optimisation: a session that
    /// never puts a canvas on screen must never bind a loopback port or stand
    /// up an RPC surface holding a session key.
    ///
    /// These are the modes that would quietly undo it, because none of them
    /// needs a server to answer — and `status` is the *default*, so a bare
    /// `canvas(name=…)` used to be enough to bind.
    #[tokio::test]
    async fn reporting_on_a_canvas_never_starts_a_server() {
        let project = tempfile::tempdir().expect("tempdir");
        let (tool, canvas) = unstarted(project.path());

        for args in [
            CanvasArgs {
                mode: Some("status".into()),
                name: Some("demo".into()),
                ..Default::default()
            },
            CanvasArgs {
                mode: Some("close".into()),
                name: Some("demo".into()),
                ..Default::default()
            },
            CanvasArgs {
                mode: Some("list".into()),
                ..Default::default()
            },
            // What `canvas(name=…)` alone resolves to.
            CanvasArgs {
                name: Some("demo".into()),
                ..Default::default()
            },
        ] {
            let mode = args.mode.clone().unwrap_or_else(|| "(defaulted)".into());
            tool.call(args)
                .await
                .unwrap_or_else(|error| panic!("mode={mode} failed: {error}"));
            assert!(
                canvas.started().await.is_none(),
                "mode={mode} started a server"
            );
        }
    }

    /// State is a file, so writing it works with nothing served — and must
    /// still not bind a port to do so. The next server to start reads it.
    #[tokio::test]
    async fn state_written_without_a_server_reaches_the_disk() {
        let project = tempfile::tempdir().expect("tempdir");
        let (tool, canvas) = unstarted(project.path());

        let mut entries = serde_json::Map::new();
        entries.insert("rows".into(), json!(42));
        tool.call(CanvasArgs {
            mode: Some("state".into()),
            name: Some("demo".into()),
            entries: Some(entries),
            ..Default::default()
        })
        .await
        .expect("write state");

        assert!(
            canvas.started().await.is_none(),
            "writing state started a server"
        );

        // Read back through the store rather than the tool, so this asserts it
        // is durable rather than merely remembered.
        let stored =
            artist_canvas::StateStore::open(&project.path().join(".artist/canvas/demo")).snapshot();
        assert_eq!(stored.plain().get("rows"), Some(&json!(42)));
    }

    /// The whole loop a model actually runs, in order, against a live server.
    ///
    /// Every mode is checked in isolation above, and all of it with nothing
    /// served — which is the state where `status` has the least to say. This
    /// is the arrangement the model meets in practice, and it asserts on what
    /// the *text* conveys rather than only on side effects, because that text
    /// is the entire interface: a model that cannot see the canvas has this
    /// and nothing else.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_model_facing_loop_says_something_usable_at_every_step() {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        let canvas = Lazy::new(
            root.to_owned(),
            Arc::new(artist_canvas::bridge::DetachedHost),
        );
        let tool = CanvasTool::new(root.to_owned(), Arc::clone(&canvas), Recorder::noop());

        let empty = tool
            .call(CanvasArgs {
                mode: Some("list".into()),
                ..Default::default()
            })
            .await
            .expect("list");
        assert!(
            empty.contains("mode=create"),
            "an empty project should say how to stop being one: {empty}"
        );

        let created = tool
            .call(CanvasArgs {
                mode: Some("create".into()),
                name: Some("Sales Report".into()),
                template: Some("dashboard".into()),
                ..Default::default()
            })
            .await
            .expect("create");
        // The three things the model needs next: which files it may edit, that
        // editing reloads the page, and how to find out whether it compiled.
        assert!(created.contains("main.jsx"), "{created}");
        assert!(created.contains("mode=open"), "{created}");
        assert!(created.contains("mode=status"), "{created}");
        assert!(
            root.join(".artist/canvas/sales-report/canvas.toml")
                .is_file(),
            "create reported success without writing the manifest"
        );

        canvas.server().await.expect("server starts");

        let status = tool
            .call(CanvasArgs {
                mode: Some("status".into()),
                name: Some("Sales Report".into()),
                ..Default::default()
            })
            .await
            .expect("status");
        assert!(status.contains("url: http://"), "{status}");
        assert!(status.contains("No errors reported"), "{status}");
        // The load-bearing line. A report that is merely empty reads as "all
        // well" — this one has to say *why* it is empty and what to do about
        // it, or the model concludes a canvas nobody can see is working.
        assert!(
            status.contains("No open window") && status.contains("mode=open"),
            "an unopened canvas must explain its own silence: {status}"
        );
        assert!(status.contains("Shared state is empty"), "{status}");

        let mut entries = serde_json::Map::new();
        entries.insert("rows".into(), json!(42));
        tool.call(CanvasArgs {
            mode: Some("state".into()),
            name: Some("Sales Report".into()),
            entries: Some(entries),
            ..Default::default()
        })
        .await
        .expect("write state");

        let status = tool
            .call(CanvasArgs {
                mode: Some("status".into()),
                name: Some("Sales Report".into()),
                ..Default::default()
            })
            .await
            .expect("status again");
        assert!(
            status.contains("rows = 42"),
            "state the model just wrote should come back: {status}"
        );

        // Nothing ever opened a window, and saying so plainly matters: the
        // alternative reads as a window that failed to close.
        let closed = tool
            .call(CanvasArgs {
                mode: Some("close".into()),
                name: Some("Sales Report".into()),
                ..Default::default()
            })
            .await
            .expect("close");
        assert!(closed.contains("nothing to close"), "{closed}");

        let listed = tool
            .call(CanvasArgs {
                mode: Some("list".into()),
                ..Default::default()
            })
            .await
            .expect("list again");
        assert!(listed.contains("sales-report"), "{listed}");
        assert!(
            listed.contains("http://"),
            "a served canvas should list its url: {listed}"
        );
    }

    /// A typo is the most likely thing to reach this tool, and the reply is the
    /// model's only way to recover: it has to name what does exist.
    #[tokio::test]
    async fn an_unknown_canvas_is_answered_with_the_ones_that_exist() {
        let project = tempfile::tempdir().expect("tempdir");
        let (tool, _canvas) = unstarted(project.path());

        let error = tool
            .call(CanvasArgs {
                mode: Some("status".into()),
                name: Some("dmeo".into()),
                ..Default::default()
            })
            .await
            .expect("status answers rather than failing");
        assert!(
            error.contains("demo"),
            "the real canvas went unmentioned: {error}"
        );
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
