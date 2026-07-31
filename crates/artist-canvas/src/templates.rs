//! Starters a canvas is grown from.
//!
//! This is the strongest lever against the failure mode that makes rendered
//! artifacts tedious: the model rebuilding an app shell, a theme, an error
//! boundary, and a loading state every single time. A template is not a stub —
//! it is a working app, so the model's first act is an edit rather than a
//! scaffold.

use crate::manifest::Manifest;

/// One starter.
pub struct Template {
    pub name: &'static str,
    /// Shown in the tool schema, so the model picks by intent.
    pub description: &'static str,
    pub entry: &'static str,
    /// `(relative path, contents)`.
    pub files: &'static [(&'static str, &'static str)],
}

pub const TEMPLATES: &[Template] = &[
    Template {
        name: "blank",
        description: "An empty page with the shell, theme and error boundary already wired.",
        entry: "main.jsx",
        files: &[("main.jsx", BLANK)],
    },
    Template {
        name: "dashboard",
        description: "Metrics, a live chart and a sortable table. For watching something change.",
        entry: "main.jsx",
        files: &[("main.jsx", DASHBOARD)],
    },
    Template {
        name: "review",
        description: "Side-by-side list and diff viewer with approve/reject. For walking a change set.",
        entry: "main.jsx",
        files: &[("main.jsx", REVIEW)],
    },
    Template {
        name: "form",
        description: "A form whose result is sent back into the conversation. For collecting a decision.",
        entry: "main.jsx",
        files: &[("main.jsx", FORM)],
    },
    Template {
        name: "report",
        description: "A long-form document with headings, code blocks and tables. For explaining something.",
        entry: "main.jsx",
        files: &[("main.jsx", REPORT)],
    },
];

pub fn find(name: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|template| template.name == name)
}

pub fn names() -> Vec<&'static str> {
    TEMPLATES.iter().map(|template| template.name).collect()
}

/// The manifest a new canvas starts with.
pub fn manifest_for(title: &str, template: &Template) -> Manifest {
    Manifest {
        title: title.to_owned(),
        entry: template.entry.to_owned(),
        ..Manifest::default()
    }
}

const BLANK: &str = r#"import { createRoot } from "react-dom/client";
import { AppShell, Card, ErrorBoundary, Toaster } from "@artist/ui";
import { useCanvasState } from "@artist/react";

function App() {
  // Shared with the agent and every other open tab, and still here tomorrow.
  const [note, setNote] = useCanvasState("note", "");

  return (
    <AppShell title="New canvas" subtitle="Edit main.jsx — the page reloads itself">
      <Card title="Start here">
        <input
          value={note}
          onChange={(event) => setNote(event.target.value)}
          placeholder="Type something; the agent can read it"
          style={{ width: "100%", padding: 8 }}
        />
      </Card>
    </AppShell>
  );
}

createRoot(document.getElementById("root")).render(
  <ErrorBoundary>
    <App />
    <Toaster />
  </ErrorBoundary>,
);
"#;

const DASHBOARD: &str = r#"import { createRoot } from "react-dom/client";
import {
  AppShell, Badge, Button, Card, DataTable, ErrorBoundary, Plot, Stack, Toaster,
} from "@artist/ui";
import { useAgent, useCanvasState } from "@artist/react";

/** One number, stated plainly. */
function Metric({ label, value, tone }) {
  return (
    <Card style={{ flex: 1, minWidth: 140 }}>
      <div style={{ fontSize: 12, color: "var(--a-muted)" }}>{label}</div>
      <div style={{ fontSize: 26, fontWeight: 600, marginTop: 4 }}>{value}</div>
      {tone && <div style={{ marginTop: 6 }}><Badge tone={tone}>{tone}</Badge></div>}
    </Card>
  );
}

function App() {
  // The agent fills these with: canvas(mode="state", entries={...})
  const [rows] = useCanvasState("rows", []);
  const [seriesX] = useCanvasState("x", [0, 1, 2, 3, 4]);
  const [seriesY] = useCanvasState("y", [0, 4, 2, 6, 3]);
  const { busy, send } = useAgent();

  return (
    <AppShell
      title="Dashboard"
      subtitle={busy ? "agent is working" : "idle"}
      actions={
        <Button variant="primary" onClick={() => send("Refresh the dashboard data.")}>
          Refresh
        </Button>
      }
    >
      <Stack gap={4}>
        <Stack gap={3} horizontal style={{ alignItems: "stretch", flexWrap: "wrap" }}>
          <Metric label="Rows" value={rows.length} />
          <Metric label="Peak" value={Math.max(0, ...seriesY)} />
          <Metric label="Status" value={busy ? "running" : "ready"} tone={busy ? "warn" : "ok"} />
        </Stack>

        <Card title="Over time">
          <Plot data={[seriesX, seriesY]} series={[{}, { label: "value" }]} height={220} />
        </Card>

        <Card title="Detail">
          <DataTable rows={rows} empty="No rows yet — ask the agent to fill them in" />
        </Card>
      </Stack>
    </AppShell>
  );
}

createRoot(document.getElementById("root")).render(
  <ErrorBoundary>
    <App />
    <Toaster />
  </ErrorBoundary>,
);
"#;

const REVIEW: &str = r#"import { createRoot } from "react-dom/client";
import {
  AppShell, Badge, Button, Card, Diff, EmptyState, ErrorBoundary, Split, Stack, Toaster, toast,
} from "@artist/ui";
import { useCanvasState } from "@artist/react";
import { artist } from "@artist/canvas";

function App() {
  // Each: {path, patch, status}. The agent supplies them.
  const [changes, setChanges] = useCanvasState("changes", []);
  const [selected, setSelected] = useCanvasState("selected", 0);
  const current = changes[selected];

  const decide = (status) => {
    setChanges(changes.map((c, i) => (i === selected ? { ...c, status } : c)));
    toast(`${current.path} ${status}`, status === "rejected" ? "danger" : "default");
    artist.send(`I ${status} the change to ${current.path}.`);
  };

  return (
    <AppShell title="Review" subtitle={`${changes.length} file(s)`}>
      <div style={{ height: "100%" }}>
        <Split initial={28}>
          <Stack gap={1}>
            {changes.map((change, index) => (
              <button
                key={change.path}
                onClick={() => setSelected(index)}
                style={{
                  textAlign: "left", padding: 8, cursor: "pointer", borderRadius: 6,
                  border: "1px solid var(--a-border)",
                  background: index === selected ? "var(--a-subtle)" : "transparent",
                  color: "var(--a-fg)", font: "12px var(--a-mono)",
                }}
              >
                <Stack gap={2} horizontal>
                  <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis" }}>
                    {change.path}
                  </span>
                  {change.status && (
                    <Badge tone={change.status === "approved" ? "ok" : "danger"}>
                      {change.status}
                    </Badge>
                  )}
                </Stack>
              </button>
            ))}
            {!changes.length && <EmptyState title="Nothing to review" />}
          </Stack>

          {current ? (
            <Card
              title={current.path}
              actions={
                <Stack gap={2} horizontal>
                  <Button variant="primary" onClick={() => decide("approved")}>Approve</Button>
                  <Button variant="danger" onClick={() => decide("rejected")}>Reject</Button>
                </Stack>
              }
            >
              <Diff patch={current.patch ?? ""} />
            </Card>
          ) : (
            <EmptyState title="Select a file" />
          )}
        </Split>
      </div>
    </AppShell>
  );
}

createRoot(document.getElementById("root")).render(
  <ErrorBoundary>
    <App />
    <Toaster />
  </ErrorBoundary>,
);
"#;

const FORM: &str = r#"import { createRoot } from "react-dom/client";
import { AppShell, Card, ErrorBoundary, SchemaForm, Toaster, toast } from "@artist/ui";
import { useCanvasState } from "@artist/react";
import { artist } from "@artist/canvas";

// Any JSON Schema works here, including one lifted straight off a tool.
const SCHEMA = {
  type: "object",
  required: ["name"],
  properties: {
    name: { type: "string", description: "What should this be called?" },
    scale: { type: "integer", description: "How many workers?" },
    region: { type: "string", enum: ["us-east", "us-west", "eu"] },
    autoscale: { type: "boolean", description: "Grow under load" },
  },
};

function App() {
  const [answers, setAnswers] = useCanvasState("answers", {});

  return (
    <AppShell title="Configure" subtitle="Submitting sends this back to the agent">
      <Card title="Options">
        <SchemaForm
          schema={SCHEMA}
          value={answers}
          onChange={setAnswers}
          submitLabel="Send to agent"
          onSubmit={(values) => {
            setAnswers(values);
            toast("Sent");
            artist.send(`Here are my choices:\n${JSON.stringify(values, null, 2)}`);
          }}
        />
      </Card>
    </AppShell>
  );
}

createRoot(document.getElementById("root")).render(
  <ErrorBoundary>
    <App />
    <Toaster />
  </ErrorBoundary>,
);
"#;

const REPORT: &str = r#"import { createRoot } from "react-dom/client";
import { AppShell, Card, Code, DataTable, ErrorBoundary, Stack, Toaster } from "@artist/ui";
import { useCanvasState } from "@artist/react";

function Section({ title, children }) {
  return (
    <section style={{ maxWidth: 760 }}>
      <h2 style={{ fontSize: 17, margin: "0 0 8px" }}>{title}</h2>
      {children}
    </section>
  );
}

function App() {
  const [report] = useCanvasState("report", {
    summary: "The agent writes this section.",
    snippet: "fn main() {\n    println!(\"hello\");\n}",
    language: "rust",
    rows: [],
  });

  return (
    <AppShell title="Report">
      <Stack gap={5}>
        <Section title="Summary">
          <p style={{ lineHeight: 1.7, color: "var(--a-fg)" }}>{report.summary}</p>
        </Section>

        <Section title="Code">
          {/* Highlighted by the same Rust that colours the transcript. */}
          <Code language={report.language} showLines>{report.snippet}</Code>
        </Section>

        <Section title="Detail">
          <Card>
            <DataTable rows={report.rows} empty="No rows" dense />
          </Card>
        </Section>
      </Stack>
    </AppShell>
  );
}

createRoot(document.getElementById("root")).render(
  <ErrorBoundary>
    <App />
    <Toaster />
  </ErrorBoundary>,
);
"#;

/// Turn a canvas into a standalone Vite project.
///
/// The escape hatch for the one canvas that outgrows the vendored set. After
/// this the canvas is an ordinary npm project and Artist stops serving it —
/// which is the point: the toolchain being small is worth more than the
/// toolchain being universal, provided there is a door out.
pub fn eject(root: &std::path::Path, title: &str) -> std::io::Result<Vec<String>> {
    let package = format!(
        r#"{{
  "name": "{name}",
  "private": true,
  "type": "module",
  "scripts": {{
    "dev": "vite",
    "build": "vite build"
  }},
  "dependencies": {{
    "react": "^19.2.8",
    "react-dom": "^19.2.8",
    "uplot": "^1.6.32",
    "@tanstack/react-table": "^8.21.3"
  }},
  "devDependencies": {{
    "@vitejs/plugin-react": "^6.0.0",
    "@tailwindcss/vite": "^4.3.3",
    "vite": "^8.0.0"
  }}
}}
"#,
        name = title
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
    );

    let config = r#"import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwind from "@tailwindcss/vite";

// `@artist/*` only exists inside Artist. An ejected canvas has to stand on its
// own, so those imports must be replaced with local code before `npm run dev`.
export default defineConfig({
  plugins: [react(), tailwind()],
});
"#;

    let index = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Canvas</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="./main.jsx"></script>
  </body>
</html>
"#;

    let mut written = Vec::new();
    for (name, contents) in [
        ("package.json", package.as_str()),
        ("vite.config.js", config),
        ("index.html", index),
    ] {
        let target = root.join(name);
        // Never clobber: an ejected canvas may already have been edited.
        if target.exists() {
            continue;
        }
        std::fs::write(&target, contents)?;
        written.push(name.to_owned());
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::{Options, transform};
    use std::path::Path;

    /// A template that does not compile is worse than no template: the model
    /// starts from broken code and has to debug someone else's app.
    #[test]
    fn every_template_compiles() {
        for template in TEMPLATES {
            for (path, source) in template.files {
                let result = transform(Path::new(path), source, Options::default());
                assert!(
                    result.is_ok(),
                    "{}/{path} failed: {:?}",
                    template.name,
                    result.err()
                );
            }
        }
    }

    /// Each template must actually ship the entry its manifest names, or the
    /// canvas 404s on first load.
    #[test]
    fn every_template_ships_its_entry() {
        for template in TEMPLATES {
            assert!(
                template.files.iter().any(|(path, _)| *path == template.entry),
                "{} declares entry {} but does not include it",
                template.name,
                template.entry
            );
            assert_eq!(
                manifest_for("Title", template).entry,
                template.entry,
                "{} manifest disagrees with its entry",
                template.name
            );
        }
    }

    /// Templates only import what the import map resolves; a bad specifier
    /// fails at load with no useful message.
    #[test]
    fn templates_import_only_known_specifiers() {
        let known = [
            "react", "react-dom/client", "react/jsx-runtime", "uplot",
            "@tanstack/react-table", "@artist/ui", "@artist/react", "@artist/canvas",
        ];
        for template in TEMPLATES {
            for (path, source) in template.files {
                for line in source.lines().filter(|line| line.trim_start().starts_with("import ")) {
                    let Some(start) = line.rfind(" from \"") else { continue };
                    let specifier = line[start + 7..].trim_end_matches("\";");
                    assert!(
                        known.contains(&specifier) || specifier.starts_with("./"),
                        "{}/{path} imports unknown specifier {specifier}",
                        template.name
                    );
                }
            }
        }
    }

    #[test]
    fn ejecting_writes_a_standalone_project_without_clobbering() {
        let root = std::env::temp_dir().join(format!("artist-eject-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");

        let written = eject(&root, "Perf Explorer").expect("eject");
        assert!(written.contains(&"package.json".to_owned()));
        let package = std::fs::read_to_string(root.join("package.json")).expect("package.json");
        assert!(package.contains("\"react\""), "{package}");
        assert!(package.contains("perf-explorer"), "{package}");

        // A second eject must not overwrite edits made after the first.
        std::fs::write(root.join("package.json"), "{\"edited\": true}").expect("edit");
        let again = eject(&root, "Perf Explorer").expect("eject twice");
        assert!(!again.contains(&"package.json".to_owned()));
        assert_eq!(
            std::fs::read_to_string(root.join("package.json")).expect("read"),
            "{\"edited\": true}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn blank_is_available_and_named_consistently() {
        assert!(find("blank").is_some());
        assert!(find("nope").is_none());
        assert!(names().contains(&"dashboard"));
        for template in TEMPLATES {
            assert!(!template.description.is_empty(), "{} needs a description", template.name);
        }
    }
}
