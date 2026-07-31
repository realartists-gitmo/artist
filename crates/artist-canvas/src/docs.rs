//! The component reference `canvas(mode="docs")` returns.
//!
//! The tool description carries a catalog; this carries the signatures. That
//! split mirrors how skills work — a name and a one-liner ride in the always-on
//! text, and the full content is fetched only when it is about to be used. A
//! kit this size cannot fit in a tool description without crowding out
//! everything else the model needs to know.

/// One documented symbol.
struct Entry {
    name: &'static str,
    signature: &'static str,
    body: &'static str,
}

const ENTRIES: &[Entry] = &[
    Entry {
        name: "useCanvasState",
        signature: "const [value, setValue] = useCanvasState(key, initial)",
        body: "Shared, durable state. The same key is seen by every open tab, by you (via\n\
               canvas mode=state), and by tomorrow's session. Use it for what the canvas is\n\
               about — the selection, the rows, the answer. Use plain useState for what only\n\
               one tab cares about.\n\n\
               setValue accepts a value or an updater, like useState.",
    },
    Entry {
        name: "useAgent",
        signature: "const {context, events, busy, send, call} = useAgent()",
        body: "The running agent. `busy` is true mid-turn, `events` is a bounded stream of\n\
               recent activity, `send(text)` puts text into the conversation and `call(tool,\n\
               args)` invokes a tool the canvas declared.",
    },
    Entry {
        name: "useAsk",
        signature: "const {questions, answer} = useAsk()",
        body: "Questions awaiting an answer — the same ones the terminal is showing.\n\
               answer(questionId, selected, notes). Whoever answers first wins. AppShell\n\
               already renders these via AskDock, so most canvases need not call this.",
    },
    Entry {
        name: "useTool",
        signature: "const {run, loading, output, error} = useTool(name)",
        body: "A tool call with request state attached. Stale responses are dropped, so a\n\
               fast second call cannot be overwritten by a slow first one.",
    },
    Entry {
        name: "AppShell",
        signature: "<AppShell title subtitle actions sidebar>…</AppShell>",
        body: "The page frame: header, optional sidebar, scrolling body, and the question\n\
               dock. Start every canvas with it rather than laying out a header by hand.",
    },
    Entry {
        name: "DataTable",
        signature: "<DataTable rows columns onRowClick empty dense />",
        body: "Sortable, filterable table. `columns` is optional — omit it and the columns\n\
               are inferred from the rows. A column may be a string, or\n\
               {key, label, render(value, row)}. Numbers sort numerically.",
    },
    Entry {
        name: "Plot",
        signature: "<Plot data={[xs, ys, …]} series height title />",
        body: "A chart. `data` is uPlot's column-major form: the first array is the x axis,\n\
               each subsequent array is a series. Resizes with its container.\n\n\
               Example: <Plot data={[[0,1,2],[3,1,4]]} series={[{}, {label:'rps'}]} />",
    },
    Entry {
        name: "Code",
        signature: "<Code language=\"rust\" showLines wrap>{source}</Code>",
        body: "Syntax-highlighted code, coloured by the harness's own highlighter — no\n\
               grammar files are downloaded. Any language syntect knows works.",
    },
    Entry {
        name: "Diff",
        signature: "<Diff patch={unifiedDiff} />",
        body: "Renders a unified diff with added/removed lines tinted. Pass the patch text\n\
               you already have from an edit result; this does not compute a diff.",
    },
    Entry {
        name: "SchemaForm",
        signature: "<SchemaForm schema value onChange onSubmit submitLabel />",
        body: "A form derived from a JSON Schema, with required-field validation. Every\n\
               Artist tool publishes a schema, so this puts a real interface in front of any\n\
               tool. Supports string, number, integer, boolean and enum properties.",
    },
    Entry {
        name: "Approve",
        signature: "<Approve questionId label>…</Approve>",
        body: "Approve/reject buttons wired to a pending question, so a decision in the\n\
               canvas lands directly in your turn.",
    },
    Entry {
        name: "Transcript",
        signature: "<Transcript events height />",
        body: "Live agent output. Pass `events` from useAgent or useAgentEvents.",
    },
    Entry {
        name: "ToolLog",
        signature: "<ToolLog events limit />",
        body: "Tool calls as they happen, with timings.",
    },
    Entry {
        name: "artist.send",
        signature: "artist.send(text, {mode})",
        body: "Put text into the conversation. mode is \"auto\" (default), \"steer\" to\n\
               correct a running turn, or \"next\" to queue a new one. Auto steers when a\n\
               turn is running and prompts when idle, which is what a button wants.",
    },
    Entry {
        name: "artist.call",
        signature: "await artist.call(tool, args)",
        body: "Invoke a tool. The canvas must list it in [permissions] allow in canvas.toml,\n\
               and it must also be permitted for you — a canvas cannot widen its own reach.",
    },
    Entry {
        name: "artist.highlight",
        signature: "await artist.highlight(source, language, dark)",
        body: "Highlight code, returning {language, lines:[[{text,color,bold,italic}]]}.\n\
               <Code> uses this; call it directly only for custom rendering.",
    },
];

const PREAMBLE: &str = "\
Canvas component reference.

Imports:
  import { … } from \"@artist/ui\";      components
  import { … } from \"@artist/react\";   hooks
  import { artist } from \"@artist/canvas\"; imperative bridge

Also resolvable: react, react-dom/client, uplot, @tanstack/react-table.
Tailwind utility classes work. No other package resolves — add one under
[deps] in canvas.toml if you truly need it (that requires network at load).

Components not listed below: Toolbar, Stack, Split, Card, EmptyState, Skeleton,
Button, Input, Select, Checkbox, Badge, Tabs, Dialog, Toaster/toast,
ErrorBoundary, AskDock. They behave as their names suggest.
";

/// Render the reference, whole or for one symbol.
pub fn render(topic: Option<&str>) -> String {
    let Some(topic) = topic.map(str::trim).filter(|topic| !topic.is_empty()) else {
        let mut out = String::from(PREAMBLE);
        for entry in ENTRIES {
            out.push_str(&format!("\n{}\n  {}\n", entry.signature, entry.body.replace('\n', "\n  ")));
        }
        return out;
    };

    // Match loosely: the model may ask for `Plot`, `plot`, or `artist.send`.
    let needle = topic.to_lowercase();
    let found: Vec<_> = ENTRIES
        .iter()
        .filter(|entry| {
            let name = entry.name.to_lowercase();
            name == needle || name.ends_with(&format!(".{needle}")) || name.contains(&needle)
        })
        .collect();

    if found.is_empty() {
        return format!(
            "No canvas docs for `{topic}`. Documented: {}.\n\nCall mode=docs with no topic for \
             the full reference.",
            ENTRIES
                .iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    found
        .iter()
        .map(|entry| format!("{}\n  {}", entry.signature, entry.body.replace('\n', "\n  ")))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_full_reference_covers_every_entry() {
        let all = render(None);
        for entry in ENTRIES {
            assert!(all.contains(entry.signature), "{} is missing", entry.name);
        }
        assert!(all.contains("@artist/ui"));
    }

    #[test]
    fn a_topic_narrows_to_one_entry() {
        let plot = render(Some("Plot"));
        assert!(plot.contains("uPlot's column-major form"), "{plot}");
        assert!(!plot.contains("SchemaForm"), "{plot}");
    }

    #[test]
    fn lookup_is_forgiving_about_case_and_qualification() {
        assert!(render(Some("useCanvasState")).contains("Shared, durable state"));
        assert!(render(Some("usecanvasstate")).contains("Shared, durable state"));
        assert!(render(Some("send")).contains("mode is \"auto\""));
    }

    /// An unknown topic must say what does exist, not just fail.
    #[test]
    fn an_unknown_topic_lists_what_is_documented() {
        let response = render(Some("Sparkline"));
        assert!(response.contains("No canvas docs for `Sparkline`"), "{response}");
        assert!(response.contains("DataTable"), "{response}");
    }

    #[test]
    fn an_empty_topic_is_treated_as_no_topic() {
        assert_eq!(render(Some("   ")), render(None));
    }
}
