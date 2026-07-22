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
│   ├── artist-tools/            # Built-in tools: bash, read, write, edit, find, grep
│   ├── hashline-tools/          # Mnemonic line anchors + multi-agent file coordination
│   └── llm-provider/            # ChatGPT OAuth, SavedProvider, Secret
└── docs/
    └── architecture.md          # This file
```

| Crate | Responsibility |
|-------|----------------|
| **artist-cli** | UX surface: CLI args, config I/O, chat UI, event recording/replay wiring, `/rules` `/rewind` and custom commands, session maintenance subcommands. |
| **artist-agent** | The model loop: builds the Rig agent, drives the TTSR abort/inject/retry loop, owns the capture/steering/TTSR hooks, `delegate` subagents, MCP. |
| **artist-rules** | The rules engine: declarative rule files, discovery + hot reload, streaming matcher, per-session state, retro scanning, wasmtime plugin host (feature `wasm`). |
| **artist-extensions** | Trusted WASM extensions: persistent component instances discovered from `<config>/extensions` manifests, with a powerful host interface (run/spawn commands, steer, queue prompts, stop the agent, live context, event bus). Distinct trust model from rule plugins — extensions are trusted and capable; rule plugins are untrusted and sandboxed. Both hosts share one wasmtime (46). |
| **artist-session** | Rig `ConversationMemory` persistence in `events.jsonl`, operational events, legacy converters, and lossy display/rewind projections. |
| **artist-tools** | Tool implementations bound to a `Workspace` (project-jailed file tools, PTY bash, FFF find/grep). |
| **hashline-tools** | Standalone file-tool core: mnemonic anchors, hidden line hashes, SQLite anchor state, cross-process path locks. |
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
   built-ins, MCP, and extensions. The denylist filters that registry once;
   provider registration and the generated system-prompt tool section consume
   the same final list, so descriptions and tool-specific guidance cannot name
   disabled tools and automatically include extension/MCP tools. The agent then
   installs three hooks, in order:
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

Subagents via `delegate` run the same streaming drive (with TTSR active and
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

Out of scope so far, by decision: tool-result match target (v1 rules are
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
cumulative read/modified file lists. A successful compaction appends
`conversation.compacted` audit metadata and a hidden reset snapshot containing
the summary plus the recent suffix. The append-only transcript remains intact;
only model context is replaced. Summary failures leave memory untouched.

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
(`store.rs`). `providers.toml` holds provider identity, secrets, the status
bar, and the base `disabled_tools` — **not** model choice.

Behaviour is layered through **`settings.toml`**, resolved from a global
`~/.config/artist/settings.toml` and a project `<repo>/.artist/settings.toml`, plus an
optional highest-precedence override layer (CLI/session):

```toml
model = "gpt-5-codex"        # the model to use (sole home; moved out of providers.toml)
reasoning_effort = "high"    # reasoning effort

[permissions]
deny = ["write", "edit"]     # tools the agent may not use
```

Resolution rules (`settings.rs`): **scalars** (`model`, `reasoning_effort`)
take the highest-precedence layer that sets them (override > project >
global); **restriction lists** (`permissions.deny`) are **unioned** with each
other and with `providers.toml`'s `disabled_tools`, so a project can tighten
access but never silently loosen it.

Model/reasoning are settings, not per-provider fields: `artist model` writes
the global `settings.toml`, and a first launch after upgrade migrates any
per-provider model out of `providers.toml` (which then drops the field on its
next save — the `SavedProvider` fields are runtime-only carriers now,
`skip_serializing`). At session time the resolved model/reasoning are applied
to a throwaway provider clone, so switching accounts (`/accounts`) keeps the
project's model and nothing settings-derived is ever persisted back.

## Auth, providers, MCP

Unchanged in shape: Authorization Code + PKCE against the ChatGPT/Codex
public client id (documented dependency risk), tokens in `0o600` TOML,
JWT identity decoded without signature verification (acceptable given the
token source). MCP (`mcp.toml`, cached schemas, startup/manual/on-call
activation) hardened: oversized tool output is wrapped in a **valid JSON
envelope** with an explicit `truncated` marker (never cut mid-byte), and
server-map access degrades gracefully instead of panicking. The tool set
is still snapshotted per turn; `/mcp start` binds on the next message.

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
- Tool-result rule target (inject-only semantics) deferred from v1.
- Delegate activity is recorded in the log but not yet surfaced in the TUI.
- Full clean-rewind rendering of aborted partial output in scrollback
  (currently the unflushed tail clears and an amber card marks the rewind).

## Related docs

- `crates/artist-rules/wit/rule-plugin.wit` — the plugin interface
- `crates/artist-rules/tests/fixtures/rule-guest/` — plugin starter template
- `crates/llm-provider/README.md` — OAuth and secret handling notes
- `crates/hashline-tools/FRANKENSTEIN.md` / `docs/mnemonic-anchors.md`
- `crates/artist-agent/src/system_prompt.md` — model-facing tool policy
