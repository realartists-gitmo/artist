# Artist architecture

This document describes the Artist coding-agent harness: how the crates fit
together, how a session runs, and how the two headline systems — **stream
rules** (time-traveling, runtime-extensible) and the **event-sourced session
store** — work. Updated on branch `Gortnite` (2026-07) after the TTSR
rework; the prior version of this file was the 2026-07 architecture review
log, whose recommendations this branch implemented.

## What it is

Artist is a local terminal coding agent. The binary is `artist`
(`crates/artist-cli`). It provides:

- Interactive multi-turn chat (ratatui TUI) with mid-run steering
- One-shot prompts (`-p`) without the chat UI
- Session create/resume/fork (`-r`, `/rewind`), with full tool context
  preserved across turns and restarts
- **Stream rules**: dormant, zero-context-cost rules that abort the model
  mid-token when it goes off-script, inject a reminder, and retry from the
  same point — declaratively (regex) or programmably (WASM plugins)
- ChatGPT OAuth login and model selection (`provider --login chatgpt`,
  `model`)

It is close in spirit to Claude Code / Codex-style harnesses, implemented in
Rust on top of [Rig](https://github.com/0xPlaygrounds/rig) (rig-core 0.40).

---

## Workspace layout

```text
artist/                          # Cargo workspace
├── crates/
│   ├── artist-cli/              # Binary: TUI, session UX, commands, provider config
│   ├── artist-agent/            # Agent loop + TTSR driver, hooks, MCP, delegate
│   ├── artist-rules/            # Stream rules: parsing, matching, WASM host
│   ├── artist-session/          # Event-sourced session store + projections
│   ├── artist-memory/           # Durable memory: Mnestic store, local embeddings, code index
│   ├── artist-tools/            # Built-in tools: bash, read, write, edit, find, grep
│   ├── artist-computer/         # Computer use: surfaces, anchors, the isolated Stage
│   ├── hashline-tools/          # Mnemonic line anchors + multi-agent file coordination
│   └── llm-provider/            # ChatGPT OAuth, SavedProvider, Secret
└── docs/
    ├── architecture.md          # This file
    ├── computer-use.md          # The computer-use subsystem
    ├── computer-use-audit.md    # Adversarial review: defects, by severity
    ├── computer-use-landscape.md # What every other harness does
    └── computer-use-steal-list.md # What to take from them, and what not to
```

| Crate | Responsibility |
|-------|----------------|
| **artist-cli** | UX surface: CLI args, config I/O, chat UI, event recording/replay wiring, `/rules` `/rewind` and custom commands, session maintenance subcommands. |
| **artist-agent** | The model loop: builds the Rig agent, drives the TTSR abort/inject/retry loop, owns the capture/steering/TTSR hooks, `delegate` subagents, MCP. |
| **artist-rules** | The rules engine: declarative rule files, discovery + hot reload, streaming matcher, per-session state, retro scanning, wasmtime plugin host (feature `wasm`). |
| **artist-extensions** | Trusted WASM extensions: persistent component instances discovered from `<config>/extensions` manifests, with a powerful host interface (run/spawn commands, steer, queue prompts, stop the agent, live context, event bus). Distinct trust model from rule plugins — extensions are trusted and capable; rule plugins are untrusted and sandboxed. Both hosts share one wasmtime (46). |
| **artist-session** | Rig `ConversationMemory` persistence in `events.jsonl`, operational events, legacy converters, and lossy display/rewind projections. |
| **artist-memory** | Durable cross-session memory: a Mnestic (CozoDB fork) store holding curated facts and an embedded code index, local CPU embeddings via rten, and AST-aware chunking. |
| **artist-tools** | Tool implementations bound to a `Workspace` (project-jailed file tools, PTY bash, FFF find/grep). |
| **artist-computer** | Generalized computer use: one observation contract over every application surface, an isolated graphical Stage (headless Wayland compositor + private session bus), and the abstraction ladder that picks the cheapest rung per surface. See [computer use](computer-use.md). |
| **hashline-tools** | Standalone file-tool core: mnemonic anchors, hidden line hashes, SQLite anchor state, cross-process path locks. Its mnemonic allocator is shared with `artist-computer`, so file anchors and screen anchors mint identically. |
| **llm-provider** | ChatGPT subscription auth (PKCE), provider records, redacted-but-serializable secrets. |

Workspace edition is Rust 2024; MSRV `1.88`. License: MIT OR Apache-2.0.

---

## The agent loop

`artist_agent::stream_chat` executes one user turn. Its shape is an **outer
retry loop** around a Rig streaming run:

1. The CLI hands it a Rig `ConversationMemory` plus a `SessionHandles`
   bundle: conversation id, steering handle, rules handle + compiled rule
   set, event recorder, and a cancellation token. Rig loads the native
   messages and appends the successful run delta.
2. Each iteration builds a fresh Rig agent and one ordered tool registry from
   built-ins, MCP, and extensions. The denylist filters that registry once, and
   provider registration consumes the final list. Tool definitions reach the
   model through the API's `tools` field rather than the system prompt — that is
   the provider-native channel and the single source of truth for name,
   description and schema — so per-tool guidance lives in each tool's own
   `description()`. The agent then installs three hooks, in order:
   - **SteeringHook** — injects queued user corrections into tool results
     as `<user_steering>` blocks.
   - **CaptureHook** — captures structured tool outcome/timing metadata for
     the live UI; conversation persistence is handled by Rig memory.
   - **TtsrHook** — the stream-rules matcher (see below).
3. The drive loop translates Rig stream items into `PromptEvent`s for the
   UI, `tokio::select!`-ing against the cancellation token: Esc cancels
   cooperatively and records `run.finished{cancelled}`; partial model output
   is intentionally absent from resumed conversation memory.
4. When a rule fires, the run terminates and the loop reseeds (see TTSR
   mechanics) — `continue 'retry`.

Multi-turn tool loops are unbounded by design (`default_max_turns(MAX)`);
bash remains fully privileged by design.

Subagents via the `subagent` tool run the same streaming drive (with TTSR active and
the same rules handle, so once-per-session semantics span main + delegates),
record into the log under a child lineage (`main/delegate-<uuid>`), and
cannot delegate further.

---

## Stream rules (TTSR)

Rules sit dormant at **zero context cost**. When one matches the model's
streaming output, the in-flight completion aborts mid-token, the rule
injects itself as a reminder, and the request retries from the same point —
the offending partial output never enters context. Each rule fires at most
once per session (default) or once per user turn.

### Mechanics (why no Rig fork is needed)

- The `TtsrHook` watches `CompletionCall`, `TextDelta`, `ToolCallDelta`, and
  `ToolCall` step events. On an armed-rule match it returns
  `Flow::terminate("ttsr:<rule>")`.
- Rig surfaces that as `PromptCancelled { chat_history }`, where
  `chat_history` is exactly the committed turns (including in-run tool
  calls/results) **minus the aborted partial turn** — rig only commits
  messages at turn boundaries. That is the retry seed, free of the mistake.
- The driver re-runs with the reminder as the new prompt: a **user-role**
  message wrapped in `<system-reminder rule="...">` tags (never
  `Message::System` — the ChatGPT provider hoists system messages into
  `instructions`, away from the failure point).
- Reasoning-summary rules match on the driver side (rig has no reasoning
  hook event), seeded from history the hook captured at each
  `CompletionCall`.
- Tool-argument matches abort **before the tool executes**.
- `persistence: session` reminders re-inject on every completion call via
  `RequestPatch.extra_context`, outside ordinary history.
- A retry budget (4 per user prompt) backstops loops; exhausted firings
  degrade to inject-only. Rule state (fired set, injections, hit counts)
  restores from the event log on resume.

### Declarative rules

Markdown files in `~/.config/artist/rules/`, `~/.agents/rules/`,
`<project>/.artist/rules/`, or `<project>/.agents/rules/` (later scopes
shadow earlier by name; hot-reloaded between turns via an mtime
fingerprint). Scaffold one with `artist rules new <name>`.

```markdown
---
name: no-mock-data
description: Stop inventing mock/placeholder fallbacks
targets: [assistant-text, tool-args]   # + reasoning-summary
patterns:                               # linear-time regexes
  - '(?i)\bmock(ed)?\s+data\b'
tools: [write, edit]                    # tool-args filter (empty = all)
window: 4096                            # matching window in bytes
fire: once                              # once | per-turn
persistence: session                    # session | message
scope: [main, delegate]
---
Do not invent mock data. If real data is unavailable, stop and say so.
```

Matching is a `RegexSet` prescreen over a rolling tail window, evaluated
only when ≥64 new bytes or a newline arrive; per-call-id accumulators
handle streamed tool arguments. One curated built-in ships enabled:
`builtin:no-swallowed-errors` (disable with `/rules disable`).

### WASM plugins (programmable rules)

For what regex can't express — stateful or temporal matching — a rule can
be a wasmtime **component** implementing the `artist:rules/rule-plugin` WIT
world (`crates/artist-rules/wit/rule-plugin.wit`). A plugin ships as
`<name>.wasm` + `<name>.toml` in any rules directory:

```toml
description = "Fires on the third strike (stateful)"
prefilter = ['strike zone']     # mandatory native regexes
targets = ["assistant-text"]
fire = "per-turn"
```

The **prefilter is mandatory**: it compiles into the ordinary rule set, and
the guest is only consulted to *judge* prefilter hits — plugin quality can
never slow the raw token stream. The guest exports `meta()` (id sanity
check) and `on-event(event) -> verdict` (`pass` or `fire{reminder,
persistence}`); host imports are `log` plus a bounded session KV. Sandbox:
WASI linked with an empty context (no preopens/env/args/network), ~50ms
epoch deadline, 64 MiB memory cap. Any trap poisons the plugin for the
session (shown in `/rules`); a broken rule never breaks the agent.

Guests build with plain cargo — `tests/fixtures/rule-guest/` is a working
starter template (`rustup target add wasm32-wasip2 && cargo build --release
--target wasm32-wasip2`). The `wasm` feature is on in `artist-cli` builds
and off in `artist-rules`' own tests.

### Tooling

- `/rules` — live panel: every rule with armed/fired/disabled/poisoned
  state, session hit counts, loader diagnostics.
- `/rules enable|disable <rule>` — session-scoped toggles.
- `/rules scan` — on-demand retro evaluation of all rules over this
  session's committed model output (never automatic; findings are
  informational and recorded as `rule.retro_findings` events).
- `/rules dry-run <file>` — evaluate a candidate rule file against the
  session without activating it ("would have fired 3×, excerpts…").
- `artist rules new <name>` — scaffold a commented rule template.

Out of scope so far, by decision: tool-result match target (current rules are
pure abort-retry), user-prompt matching, trust prompts for project rules
(consistent with unsandboxed bash).

---

## Event-sourced sessions

The canonical record of a session is an append-only JSONL event log;
everything else is a projection. Nothing is ever deleted — rewind events mask
history ranges — which is what makes retroactive rule scans, `/rewind`, and
forking possible.

```text
<config_root>/sessions/<project-key>/<session-id>/
  events.jsonl      # canonical log (envelope per line)
  transcript.md     # derived markdown, incrementally appended, regenerable
  attachments/<sha> # content-addressed image blobs
  writer.lock       # exclusive while a process owns the session
```

**Envelope:** `{v, seq, ts, session, run, lineage, kind, payload}`. `seq`
is the ordering key; `lineage` scopes agents (`main`,
`main/delegate-<id>`); `run` identifies one `stream_chat` invocation (TTSR
retries mint new runs, so aborted branches stay distinguishable). Unknown
kinds/fields are tolerated on read — an older binary can open a newer
session, degraded.

**Event kinds:** `session.created`, `run.started/finished`,
`conversation.messages`, `conversation.compacted`, `steering.delivered`,
`delegate.started/finished`, `history.rewind`, `rule.fired`, `rule.injection`,
and `rule.retro_findings`.
Older `turn.user`, `model.turn`, `tool.result`, and `legacy.turn` records remain
readable for migration.

**Conversation memory:** the main agent uses Rig's `ConversationMemory` API.
After a successful run Rig appends its native `Vec<Message>` delta as one
`conversation.messages` event; subsequent runs load those messages directly.
The first successful turn in an older session writes a reset snapshot containing
the legacy projection plus the new Rig delta. Failed and cancelled runs do not
enter model memory. Images therefore persist in Rig's own message representation;
the attachment store remains for older sessions.

**Compaction:** Artist follows Pi's turn-aware checkpoint design directly over
Rig messages. Before a turn, automatic compaction triggers when projected
context exceeds `context_window - reserve_tokens`; `/compact [instructions]`
triggers it manually. The planner walks backward to retain approximately
`keep_recent_tokens`, never cuts at a tool result, and separately summarizes an
early prefix when one oversized turn must be split. Summarization uses labelled
conversation serialization, truncates tool results to 2,000 characters, updates
the previous structured checkpoint on repeated compactions, and carries
cumulative read/modified file lists. For OpenAI Responses and ChatGPT, compaction uses the standalone
`/responses/compact` endpoint with `store: false`; encrypted reasoning and the
opaque canonical compaction item remain in the provider sidecar, while Rig
memory is reset so superseded history is not resent. Explicit 400/404/422
capability failures fall back to the local summarizer; auth, network, and server
failures leave both memory and sidecar untouched. `/compact` custom instructions
always select the local summarizer. Other providers remain local. A successful
compaction appends non-opaque `conversation.compacted` audit metadata and a
hidden reset snapshot. The append-only transcript remains intact; only model
context is replaced.

Defaults are enabled with 16,384 reserve tokens and 20,000 recent tokens. They
can be overridden globally or per project in `settings.toml`:

```toml
[compaction]
enabled = true
reserve_tokens = 16384
keep_recent_tokens = 20000
```

**Writer:** all producers (CLI, hooks, delegates) send through a clonable
`Recorder` into one writer task — total order, O(1) durable appends
(`sync_data` per event), torn-tail repair on open, exclusive per-session
lock (a second `artist -r` fails fast). A flush barrier gives
read-your-writes at turn boundaries.

**Projections:**
- *Model memory* — Rig loads native messages through `SessionMemory`; rewind
  masks select the active batches. Legacy events are converted only when needed.
- *Markdown transcript* — a lossy display projection appended by the writer;
  regenerate any time with `artist sessions render <id>`.
- *TUI replay* — reconstructed from Rig messages plus operational events. Fine
  timing and cancelled partial output are intentionally not resume state.

**Time travel:** `/rewind` lists recent user turns; `/rewind <n>` appends a
`history.rewind` mask (projections hide the range; the log keeps it) and
pre-fills the turn's text for editing; `/rewind <n> fork` creates a new
session whose log is the verbatim event prefix (stable seqs, parent pointer
in `session.created`, attachments copied) — the parent is untouched. Forks
are annotated in the `-r` picker.

**Migration:** legacy markdown sessions convert on first open (one
`legacy.turn` per parsed turn, idempotent, old file becomes
`transcript.md`); listing never migrates. Retention is manual:
`artist sessions gc [--keep N] [--older-than-days D] [--dry-run]`, plus
`artist sessions list` with on-disk sizes.

---

## Durable memory

Sessions are event-sourced but nothing read them back, so every session started
cold. `artist-memory` gives the harness two corpora that outlive a session:
**facts** (curated preferences, constraints, decisions and their rationale) and
an optional **code index** (AST-chunked, embedded source).

This generalizes the invariant the todo list already established — *harness-owned
state survives a context wipe without being summarized into lossy prose*
(`docs/profiles.md`, invariant 7). Memory is never part of the conversation: it
lives in its own store, is recorded as `memory.written` events, and is injected
only when relevant.

**Storage.** One [Mnestic](https://github.com/shuruheel/mnestic) database per
scope — a global one under `<config>/memory/` for preferences that follow you
between checkouts, and a project one beside the existing tool state at
`<config>/tools/<project-hash>/memory.rocks`. They are separate databases rather
than one relation with a scope column because a query-time HNSW `filter:` is
only a post-filter over the `ef` candidate pool, so partitioning by filter would
silently drop results.

Relations: `fact` (with a `<F32; 768>` embedding, `live`, `superseded_by`),
`chunk`, and `meta`. Retrieval fuses an HNSW leg with a BM25 leg through
Mnestic's built-in `ReciprocalRankFusion`.

Supersession is an ordinary current-state table with a `live` flag and partial
indexes — Slowly Changing Dimension Type 2, in warehousing terms — rather than a
temporal table. That is not a workaround for the constraint below so much as the
shape this system wants anyway: the event log already holds a richer history
than a temporal sibling could (origin, session, sequence), and essentially every
query asks what is true *now*.

Three constraints are load-bearing and were verified against a live database
rather than read from documentation:

- A `TxTime` relation **rejects every index and trigger**, and search atoms take
  no validity clause — so bitemporality and search cannot coexist on one
  relation. No production system offers as-of ANN search; an HNSW index is a
  precomputed graph, and a temporal predicate is a filter, so satisfying one
  means rebuilding the other.
- HNSW `filter:`/`radius:` are **post-filters over the `ef` pool**, contradicting
  the upstream docs. The `live` condition is built into the index at creation
  time so superseded facts leave recall automatically on `:update`.
- Every *searchable* index over `fact` needs that condition, not just the vector
  one: an unfiltered BM25 index resurrects superseded facts through the other
  half of the fused query. `::fts` spells it `extract_filter`. The `::lsh`
  dedupe index is the exception — an `extract_filter` there empties it, so
  liveness is enforced by a join in the query instead.

**The store is a projection, not the record.** Mnestic's RocksDB backend commits
without syncing its WAL and the durability opt-in does not exist in the pinned
release, so facts are recorded as `memory.written` session events and
reconciled on open. That is also what makes memory **rewind-aware for free**:
replay runs over `visible_events`, so rewinding past a write removes the fact,
exactly as with `todo.updated`.

**Embeddings are local.** `rten` (pure-Rust ONNX, no C++ runtime) runs
CodeRankEmbed on CPU. Measured on a 13th-gen i7: ~6–7 sequences/sec at batch 32,
82–127 ms for a single query, numerically identical to ONNX Runtime to ~1e-6.
No code leaves the machine and no API key is involved. The published int8 export
is deliberately **not** used — it measured 0.909 mean cosine against fp32 and
moved 14% of top-1 results.

**Writes.** A `memory` tool for deliberate recording, plus three automatic
triggers chosen because they carry signal rather than because they are
convenient: a user correction (a preference being stated out loud), a model
decision stated mid-stream with its rationale, and a successful `git commit`.
All three are fire-and-forget; none may delay a stream or the input box.
Compaction is deliberately *not* a trigger.

**Reads.** Three channels, all pre-existing mechanisms: the `memory` tool
(model-pull), a prompt-conditioned prepend riding the user turn beside skill
sections (never the preamble, which must stay a stable prompt-cache prefix), and
`RequestPatch::extra_context` injection from a hook — the only channel that
survives compaction and handoff by construction. The hook shares the stream
rules' matcher shape but **never aborts**: rules terminate a run to keep a
mistake out of context, whereas memory only ever adds context.

Configured under `[memory]` in `settings.toml`, off by default because the
subsystem needs a local embedding model and silently degrading to lexical-only
recall would read as memory simply not working.

> **Build note.** `mnestic-rocks 0.1.10` vendors a RocksDB whose headers rely on
> `<cstdint>` arriving transitively, which GCC 13+ stopped doing. The workspace
> `.cargo/config.toml` sets `CXXFLAGS = "-include cstdint"` so cold builds work
> without per-developer setup. It must not also be set for `CFLAGS` — that
> breaks `zstd-sys`.

---

## Tools and workspace

| Tool | Role |
|------|------|
| **bash** | One-shot `exec` or persistent PTY sessions. Stopped/exited sessions are reaped from the map (one tombstone appearance in `list`). Unsandboxed by design. |
| **read** | Bounded text with mnemonic line anchors; images report metadata in the tool channel (image results surface a count marker in the UI). |
| **edit** | Atomic replacements keyed by mnemonic anchors from the latest read. |
| **write** | Atomic full-file create/overwrite. |
| **find/grep** | FFF index queries. The project index builds in the **background**; an absolute scope outside the project creates a watcher-free transient index for that path. |

All file tools accept project-relative paths and unrestricted absolute paths;
bash also accepts either form for `cwd`. Relative paths remain rooted in the
project and cannot traverse or escape it through symlinks. On stale/unknown anchors the model must re-read then retry; this guidance is
conditionally generated only when the relevant tools are registered.

---

## CLI surface

- **Interactive:** `artist` / `artist <dir>`; **one-shot:** `artist -p "…"`;
  **resume:** `-r [id]`.
- **Slash commands:** `/model`, `/statusbar`, `/skills`, `/tools`, `/mcp`,
  `/rewind`, `/compact`, `/rules`, `/help`, extension-declared commands, and `!` bang
  commands routed to the persistent input shell — plus **custom commands**:
  markdown prompt templates in
  `<project>/.artist/commands/*.md` or `~/.config/artist/commands/` with
  optional frontmatter (`description`) and `$ARGUMENTS` expansion; they
  join the completion menu (built-in names always win).
- **Maintenance:** `artist rules new`, `artist sessions list|render|gc`.
- Status bar `Context` shows remaining context versus capacity; the separate
  `Session tokens` item shows cumulative request volume. `/statusbar` can toggle
  and reorder them independently.
- Tool transcript rows use per-built-in glyphs. Extension tools may set optional
  UI metadata in their manifest (`icon = "🚀"` inside `[[tools]]`); icons must be
  a printable one- or two-column glyph. Missing or invalid icons fall back to
  `🛠` without affecting the model-facing tool schema.

## Configuration

Global state lives in `~/.config/artist/` (override with `$ARTIST_CONFIG_DIR`); a
one-time migration moves a pre-existing `~/.artist/` in, preferring
destination files on conflict so a partial home is never clobbered
(`store.rs`). `providers.toml` v4 holds provider identity, explicitly tagged credentials,
provider-local model/protocol choices, the status bar, and the base
`disabled_tools`. See the [provider guide](../crates/llm-provider/README.md) for
the schema, safe examples, all 23 completion providers, and Voyage's
embedding-only limitation.

Behaviour is layered through **`settings.toml`**, resolved from a global
`~/.config/artist/settings.toml` and a project `<repo>/.artist/settings.toml`, plus an
optional highest-precedence override layer (CLI/session):

```toml
model = "gpt-5-codex"        # the model to use (sole home; moved out of providers.toml)
reasoning_effort = "high"    # reasoning effort

[permissions]
deny = ["write", "edit"]     # tools the agent may not use

[memory]                     # durable cross-session memory; off by default
enabled = true
model_dir = "models/code-embed"  # holds model.onnx + tokenizer.json; relative to the config root
dim = 768                    # must match the model; changing it forces a rebuild
recall_limit = 8             # facts returned per recall
auto_write = true            # let the automatic triggers store facts (the tool works either way)
index_code = false           # maintain the embedded code index
```

Resolution rules (`settings.rs`): **scalars** (`model`, `reasoning_effort`)
take the highest-precedence layer that sets them (override > project >
global); **restriction lists** (`permissions.deny`) are **unioned** with each
other and with `providers.toml`'s `disabled_tools`, so a project can tighten
access but never silently loosen it.

A provider record's `model` and `reasoning_effort` are its durable defaults;
`artist model` and `/model` edit the active default provider. Project/global
`settings.toml` values remain layered runtime overrides when present.

## Auth, providers, MCP

Login is performed during first-run setup or with the TUI `/login` command.
ChatGPT uses Authorization Code + PKCE and an eligible ChatGPT subscription;
API-key OpenAI Responses is configured as an OpenAI login. Copilot supports API
key, GitHub token, and cached device-OAuth modes. Other clients enforce their
protocol-specific API-key/bearer/no-auth requirements.
Pre-v4 credentials migrate to v4's tagged union without discarding secrets;
Unix config/token permissions are tightened to `0700` directories and `0600`
files. Full operational and security details are in the provider guide.

MCP (`mcp.toml`, cached schemas, startup/manual/on-call activation) wraps
oversized tool output in a valid JSON envelope with an explicit `truncated`
marker and degrades server-map access gracefully. The tool set is snapshotted
per turn; `/mcp start` binds on the next message.

The ChatGPT web integration uses one published bootstrap app whose remote MCP
surface contains only `artist_open`. That call mounts an immutable reviewed
component. The component then registers the authorized Artist tools directly
with the ChatGPT host through MCP Apps, signs and encrypts each exact invocation,
and sends it over Veilid to `artistd`. Sensitive arguments and results never
traverse the shared remote MCP service. ChatGPT remains the sole agent;
the component is deterministic protocol/transport code, and `artistd` is the
local authority and canonical tool executor. See `docs/chatgpt-web-mesh.md`.

---

## Testing

`cargo test --workspace` (~160 tests): content-schema round-trips, event
log torn-tail/locking/seq recovery, history/replay/markdown projections
with rewind+fork fixtures, matcher windowing/coalescing, rule state
semantics, legacy migration, and the **TTSR integration harness** — eight
scenarios against rig's scripted `MockCompletionModel` asserting the actual
requests sent (offending text absent from retry context, committed tool
round-trips preserved, tools never executing on arg matches, once-per-
session, steering delivered exactly once across an abort, budget
exhaustion, reasoning-side aborts, session-persistent re-injection).

WASM tier: `cargo test -p artist-rules --features wasm` (builds the fixture
guest; needs `rustup target add wasm32-wasip2`) — stateful firing via host
KV, epoch-deadline trap on an infinite loop, memory-bomb poisoning,
manifest validation.

Manual: `cargo test -p artist-agent --test codex_replay_spike -- --ignored`
validates cross-process replay of tool history + encrypted reasoning
against the live backend (needs a logged-in provider); if the backend
rejects encrypted reasoning, flip `HistoryOptions::drop_encrypted_reasoning`
for cross-run replay.

## Open items

- Codex replay spike not yet run against a live login (degrade path ready).
- Tool-result rule target (inject-only semantics) remains an open item.
- Subagent activity is recorded in the log but not yet surfaced in the TUI.
- Full clean-rewind rendering of aborted partial output in scrollback
  (currently the unflushed tail clears and an amber card marks the rewind).

## Related docs

- `crates/artist-rules/wit/rule-plugin.wit` — the plugin interface
- `crates/artist-rules/tests/fixtures/rule-guest/` — plugin starter template
- `crates/llm-provider/README.md` — OAuth and secret handling notes
- `crates/hashline-tools/FRANKENSTEIN.md` / `docs/mnemonic-anchors.md`
- `crates/artist-agent/src/system_prompt.md` — model-facing tool policy
- `docs/chatgpt-web-mesh.md` — component-owned ChatGPT tools over the Veilid mesh
