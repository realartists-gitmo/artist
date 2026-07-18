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
| **artist-session** | The canonical event log (`events.jsonl`), content schema + rig converters, recorder/writer task, and every projection (model history, markdown transcript, TUI replay, rewind targets). |
| **artist-tools** | Tool implementations bound to a `Workspace` (project-jailed file tools, PTY bash, FFF find/grep). |
| **hashline-tools** | Standalone file-tool core: mnemonic anchors, hidden line hashes, SQLite anchor state, cross-process path locks. |
| **llm-provider** | ChatGPT subscription auth (PKCE), provider records, redacted-but-serializable secrets. |

Workspace edition is Rust 2024; MSRV `1.88`. License: MIT OR Apache-2.0.

---

## The agent loop

`artist_agent::stream_chat` executes one user turn. Its shape is an **outer
retry loop** around a Rig streaming run:

1. The CLI hands it full-fidelity history (`Vec<rig::Message>` rebuilt from
   the event log — tool calls, tool results, reasoning included) plus a
   `SessionHandles` bundle: steering handle, rules handle + compiled rule
   set, event recorder, attachment store, and a cancellation token.
2. Each iteration builds a fresh Rig agent (system prompt, built-in tools,
   MCP tools, `delegate`) with three hooks, in order:
   - **SteeringHook** — injects queued user corrections into tool results
     as `<user_steering>` blocks.
   - **CaptureHook** — records committed model turns and tool results into
     the event log (see below). Ignores all delta events.
   - **TtsrHook** — the stream-rules matcher (see below).
3. The drive loop translates Rig stream items into `PromptEvent`s for the
   UI, `tokio::select!`-ing against the cancellation token: Esc cancels
   cooperatively (the run records `run.finished{cancelled}` and accumulated
   text is preserved as a partial model turn — nothing is abandoned).
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
  `RequestPatch.extra_context` — they live outside compactable history by
  construction, so any future context compaction cannot lose them.
- A retry budget (4 per user prompt) backstops loops; exhausted firings
  degrade to inject-only. Rule state (fired set, injections, hit counts)
  restores from the event log on resume.

### Declarative rules

Markdown files in `~/.artist/rules/`, `~/.agents/rules/`,
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
everything else is a projection. Nothing is ever deleted — rewind and
compaction are *mask events* — which is what makes retroactive rule scans,
`/rewind`, and forking possible.

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

**Event kinds:** `session.created`, `run.started/finished`, `turn.user`,
`model.turn` (the commit point: full assistant content incl. tool calls and
reasoning, `partial: true` when synthesized after a cancel), `tool.result`
(model-visible text + structured outcome + duration), `steering.delivered`,
`delegate.started/finished`, `history.rewind`, `history.compact`,
`legacy.turn`, `rule.fired`, `rule.injection`, `rule.retro_findings`.
**Deltas are never persisted** — capture happens at commit points via the
`CaptureHook` (`ModelTurnFinished` / `ToolResult` step events), so the log
is byte-faithful to what rig committed, including encrypted reasoning.

**Content schema:** own explicitly-tagged `ContentBlock` enum mirroring rig
types with exact round-trip converters; content we can't model degrades to
an `Opaque` block carrying verbatim rig serde (zero loss). Images are
content-addressed into `attachments/`.

**Writer:** all producers (CLI, hooks, delegates) send through a clonable
`Recorder` into one writer task — total order, O(1) durable appends
(`sync_data` per event), torn-tail repair on open, exclusive per-session
lock (a second `artist -r` fails fast). A flush barrier gives
read-your-writes at turn boundaries.

**Projections:**
- *Model history* — `build_history(events) -> Vec<rig::Message>` with tool
  results paired to committed tool-call ids (what rig validates on replay),
  rewind/compact masks honored, and a degrade option that drops encrypted
  reasoning if the backend rejects cross-process replay. This is what fixes
  the old "tool context lost between turns" problem.
- *Markdown transcript* — appended incrementally by the writer task;
  regenerate any time with `artist sessions render <id>`.
- *TUI replay* — resume shows tool activity, reasoning, steering, and rule
  firings, not just prose.

**Time travel:** `/rewind` lists recent user turns; `/rewind <n>` appends a
`history.rewind` mask (projections hide the range; the log keeps it) and
pre-fills the turn's text for editing; `/rewind <n> fork` creates a new
session whose log is the verbatim event prefix (stable seqs, parent pointer
in `session.created`, attachments copied) — the parent is untouched. Forks
are annotated in the `-r` picker.

**Compaction** has a designed hook point (`history.compact` replaces a
masked range with a summary message in projections) but no summarizer yet.

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
| **find/grep** | FFF index queries. The index builds in the **background** — session startup never blocks on it; results carry an "index still building" note until the scan lands (`ARTIST_INDEX_STRICT` restores the hard 30s wait). |

File tools remain jailed to the project root (two hardcoded layers:
`Workspace` path resolution and the hashline `FileToolConfig`); bash can
leave the tree. On stale/unknown anchors the model must re-read then retry
(system prompt encodes this).

---

## CLI surface

- **Interactive:** `artist` / `artist <dir>`; **one-shot:** `artist -p "…"`;
  **resume:** `-r [id]`.
- **Slash commands:** `/model`, `/statusbar`, `/skills`, `/tools`, `/mcp`,
  `/rewind`, `/rules`, `/help`, extension-declared commands, and `!` bang
  commands routed to the persistent input shell — plus **custom commands**:
  markdown prompt templates in
  `<project>/.artist/commands/*.md` or `~/.artist/commands/` with
  optional frontmatter (`description`) and `$ARGUMENTS` expansion; they
  join the completion menu (built-in names always win).
- **Maintenance:** `artist rules new`, `artist sessions list|render|gc`.
- Status bar `Context` segment shows current-context tokens *and* the
  session's cumulative total, so tool loops and TTSR retries aren't
  misread.

## Configuration

Global state lives in `~/.artist/` (override with `$ARTIST_CONFIG_DIR`); a
one-time migration moves a pre-existing `~/.config/artist/` in, preferring
destination files on conflict so a partial home is never clobbered
(`store.rs`). `providers.toml` holds provider identity, secrets, the status
bar, and the base `disabled_tools` — **not** model choice.

Behaviour is layered through **`settings.toml`**, resolved from a global
`~/.artist/settings.toml` and a project `<repo>/.artist/settings.toml`, plus an
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

**Multiple backends.** A `SavedProvider` carries a `ProviderKind`
(`chat_gpt` / `open_ai` / `anthropic` / `gemini`) and a tagged `Auth`
(`chat_gpt` OAuth tokens, or an `api_key`). ChatGPT signs in via Authorization
Code + PKCE against the ChatGPT/Codex public client id (tokens in `0o600` TOML,
JWT identity decoded without signature verification — acceptable given the
token source); other backends are added with `artist provider add` (key from
the provider's env var or a prompt). The agent's stream loop, TTSR, and capture
hooks are all generic over rig's traits, so `stream_chat`/the delegate just
dispatch on `ProviderKind` to build the right rig client and per-backend
`additional_params` (`params_for`) — the streaming layer is unchanged. Each
backend maps to a dedicated rig client: xAI/Grok and OpenAI over the Responses
path (`reasoning.effort`); Anthropic (`x-api-key`, `thinking` budget); Gemini;
and the OpenAI chat-completions family — Groq, DeepSeek, Together, OpenRouter,
Mistral, Perplexity (`/chat/completions`, top-level `reasoning_effort`). Add any
with `artist provider add`.

MCP (`mcp.toml`, cached schemas, startup/manual/on-call
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
- Context compaction: hook point exists; summarizer unbuilt.
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
