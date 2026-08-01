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
        name: "CanvasLink",
        signature: "<CanvasLink to=\"review\" params={{id}}>Review it</CanvasLink>",
        body: "Link to another canvas in this project. Omit `to` to link to the lobby, which\n\
               lists everything.\n\n\
               Use this rather than building a URL: the session key lives in the path, so a\n\
               hand-built href is both fiddly and a way to leak the key somewhere it should\n\
               not go.\n\n\
               In an exported file there are no sibling canvases to reach unless the whole\n\
               project was exported together, so a link renders as plain text — the reader\n\
               still sees what it pointed at.",
    },
    Entry {
        name: "useCanvasStateOf",
        signature: "const entries = useCanvasStateOf(slug, key?)",
        body: "Another canvas's shared state, live — for a dashboard that reflects a decision\n\
               a form recorded, without either canvas knowing more about the other than its\n\
               name.\n\n\
               Requires a declaration in this canvas's canvas.toml:\n\n\
                 [permissions]\n\
                 canvases = [\"form\"]\n\n\
               Without it the read is refused by name rather than returning empty, so a\n\
               missing declaration reads as a missing declaration and not as no data.\n\n\
               Read-only. The canvas that owns a key writes it; that is what stops two\n\
               surfaces fighting over one store.",
    },
    Entry {
        name: "artist.static",
        signature: "if (artist.static) { … }",
        body: "True in an exported canvas, absent in a live one.\n\n\
               An export has no agent, so anything that reaches the harness is gone from it:\n\
               send, call, answering a question, opening a file. Everything those surfaces\n\
               were *showing* stays — the kit drops the buttons and keeps the content.\n\n\
               Branch on this if a canvas has to be good both ways. A button you wrote that\n\
               calls artist.call will reject in an export, so either guard it or accept that\n\
               the exported copy is the read-only version of the canvas.",
    },
    Entry {
        name: "useCanvasState",
        signature: "const [value, setValue] = useCanvasState(key, initial)",
        body: "Shared, durable state. The same key is seen by every open tab, by you (via\n\
               canvas mode=state), and by tomorrow's session. Use it for what the canvas is\n\
               about — the selection, the rows, the answer. Use plain useState for what only\n\
               one tab cares about.\n\n\
               setValue accepts a value or an updater, like useState.\n\n\
               Durable means a file in the user's repo, so it is capped — 4 MiB by default,\n\
               which is thousands of rows. Writing state on every render will meet that; a\n\
               canvas that genuinely holds more raises it with [limits] state_bytes in\n\
               canvas.toml. A write over the cap is refused whole, never truncated.",
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
        name: "Pending",
        signature: "<Pending label=\"Running the test suite…\" />",
        body: "Waiting, with a reason. Say what is being waited on: work you hand to the\n\
               agent can take a minute, and an unlabelled spinner for a minute is\n\
               indistinguishable from a hang.",
    },
    Entry {
        name: "Metric",
        signature: "<Metric label value hint variant trend={[…]} />",
        body: "One headline number. `variant` (ok|warn|danger|accent) turns `hint` into a\n\
               badge; `trend` draws a sparkline beside it. Put several in a\n\
               <Stack horizontal> for the row of figures a dashboard opens with.",
    },
    Entry {
        name: "Alert",
        signature: "<Alert variant=\"warn\" title=\"…\">…</Alert>",
        body: "A callout: default|accent|ok|warn|danger. Use it rather than a tinted div —\n\
               a hand-picked background does not flip with the colour scheme.",
    },
    Entry {
        name: "Markdown",
        signature: "<Markdown>{text}</Markdown>",
        body: "Markdown rendered by the harness, with fenced code highlighted by the same\n\
               highlighter <Code> uses. Reach for this whenever you are putting prose on\n\
               screen. Raw HTML in the source is dropped rather than rendered.",
    },
    Entry {
        name: "FileLink",
        signature: "<FileLink path=\"src/lib.rs\" line={131} />",
        body: "A path that opens in the user's editor when clicked. Use it every time a\n\
               canvas names a file — it is the shortest route from what you found to the\n\
               place the user fixes it. Paths are relative to the project.",
    },
    Entry {
        name: "DataTable",
        signature: "<DataTable rows columns onRowClick empty dense height filterable />",
        body: "Sortable, filterable table. `columns` is optional — omit it and the columns\n\
               are inferred from the rows. A column may be a string, or\n\
               {key, label, align, render(value, row)}. Numbers sort numerically and are\n\
               right-aligned with tabular figures. A filter box appears past a dozen rows;\n\
               past a few hundred the table windows, so large row counts are fine.",
    },
    Entry {
        name: "Plot",
        signature: "<Plot data={[xs, ys, …]} series height title kind />",
        body: "A chart. `data` is uPlot's column-major form: the first array is the x axis,\n\
               each subsequent array is a series. `kind` is \"line\" (default), \"area\", or\n\
               \"bars\". Axes and grid follow the theme. Resizes with its container.\n\n\
               Example: <Plot data={[[0,1,2],[3,1,4]]} series={[{}, {label:'rps'}]} />",
    },
    Entry {
        name: "Sparkline",
        signature: "<Sparkline values={[…]} width height />",
        body: "A bare trend line with no axes, sized to sit beside a number rather than to\n\
               be read off. For anything with a scale worth reading, use <Plot>.",
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
        body: "Put text into the conversation. `mode` is required and is either \"steer\" —\n\
               land it in the turn that is running now — or \"queue\", which waits for the\n\
               current turn to finish. There is no default: the two do visibly different\n\
               things, and a button that guesses wrong interrupts work the user is\n\
               watching. Returns {outcome} — \"steered\", \"queued\", or \"no-turn-running\".",
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

Components not listed below: Toolbar, Stack, Split, Card, EmptyState, Pending,
Button, Input, Select, Checkbox, Badge, Tabs, Dialog, Toaster/toast,
ErrorBoundary, AskDock. They behave as their names suggest.

Everything that comes in flavours takes `variant`: default | accent | ok | warn
| danger. Button adds `ghost`. Reach for the kit before styling by hand — a
prop it does not read is dropped silently, and `canvas status` will tell you.
";

/// Render the reference, whole or for one symbol.
pub fn render(topic: Option<&str>) -> String {
    let Some(topic) = topic.map(str::trim).filter(|topic| !topic.is_empty()) else {
        let mut out = String::from(PREAMBLE);
        for entry in ENTRIES {
            out.push_str(&format!(
                "\n{}\n  {}\n",
                entry.signature,
                entry.body.replace('\n', "\n  ")
            ));
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
        .map(|entry| {
            format!(
                "{}\n  {}",
                entry.signature,
                entry.body.replace('\n', "\n  ")
            )
        })
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
        assert!(render(Some("send")).contains("\"steer\""));
    }

    /// An unknown topic must say what does exist, not just fail.
    #[test]
    fn an_unknown_topic_lists_what_is_documented() {
        let response = render(Some("Histogram"));
        assert!(
            response.contains("No canvas docs for `Histogram`"),
            "{response}"
        );
        assert!(response.contains("DataTable"), "{response}");
    }

    /// The reference is how the model learns the kit exists at all, so a
    /// component it cannot find is a component it will rebuild by hand — which
    /// is the whole thing the kit is here to prevent.
    #[test]
    fn everything_worth_reaching_for_is_documented() {
        for component in ["Metric", "Alert", "Markdown", "FileLink", "Sparkline"] {
            assert!(
                render(Some(component)).contains('<'),
                "{component} has no docs entry"
            );
        }
    }

    /// The two send modes do visibly different things and the client refuses
    /// anything else, so the docs must not describe a third.
    #[test]
    fn the_send_docs_match_what_the_client_accepts() {
        let send = render(Some("artist.send"));
        assert!(
            send.contains("\"steer\"") && send.contains("\"queue\""),
            "{send}"
        );
        assert!(
            !send.contains("\"auto\""),
            "the removed auto mode is still documented"
        );
        assert!(
            !send.contains("\"next\""),
            "the renamed next mode is still documented"
        );
    }

    #[test]
    fn an_empty_topic_is_treated_as_no_topic() {
        assert_eq!(render(Some("   ")), render(None));
    }
}
