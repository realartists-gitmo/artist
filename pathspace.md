# The noun space: one path grammar for the whole harness

**Builds on `toolschemafix.md` as fully implemented.** That spec unified the *verbs* over session-shaped things. This spec unifies the *nouns*: every address in the harness — a live session, a settled artifact, a skill, a todo tree, the memory index — is a path. The verb layer stays exactly as specced (spawn tools return addresses; `poll`/`abort`/`send` take them). What changes is that the addresses are paths, and the already-existing filesystem tools (`read`, `find`, `grep`, `edit`) resolve those paths transparently.

The whole thing in one line:

> **Verbs split by axis; the noun space is one grammar.** Processes are *tracked* by session verbs (`poll` waits), facts and transcripts are *read* by filesystem verbs (`read`/`grep`/`find` snapshot), and both are addressed by the same `scheme://path` grammar.

## The two revelations, and the synthesis

OMP's revelation: many things are **filesystem-shaped** (facts — PRs, issues, skills, rules, agent outputs). Give them `scheme://` addresses and the FS tools carry the variation: *read* a PR like a file, *search* a diff like a directory, the tool count stays flat, and second reads come back free from a disk cache.

Our revelation (the previous spec): many things are **session-shaped** (processes — subagents, bash, ask, canvas, computer). They run, resolve, can be cancelled, and their read changes with time. Give them one set of verbs (`spawn`/`poll`/`abort`/`send`) and the same tools cover everything with that shape.

The synthesis is that the verb split is exactly what makes the noun space one grammar:

- A **process** is a path whose content is its transcript: `agent://goethe`. At any moment the transcript has a well-defined whole — it is just *incomplete the next moment*. `read`/`grep` snapshot it as-is; `poll` is the tool that tracks it over time (blocks on the state transition, reports `Activity`/`Status`). `send`/`abort` touch the process itself.
- The **facts a process emits** are child paths that are *frozen* once settled: `agent://goethe/artifact`, `ask://should-we-ship/answer`. These are read by `read`, cache on disk, and are second-reads-free.

`read agent://goethe/artifact` returns the same shape as `read src/foo.ts`. There is no `agent get`, no `skill show`, no `todo read` — the path space carries the variation, and the model already knows `read`.

## The path grammar

An address is `scheme://address` — the scheme names the kind of thing, the address names the instance. A `://` means virtual; anything else is a real file, addressed exactly as today (relative or absolute). The FS-shaped tools resolve both transparently.

### Scheme root = the process (its content is the transcript)

A session IS its scheme root. Spawn returns the path; `poll`/`abort`/`send` take it back. The root's content is the session's transcript — the accumulated record of what happened so far. It is **append-only while running, frozen once `Idle`**: at any instant it has a well-defined whole, and a `read`/`grep` returns exactly that whole. `poll` is what waits on the next unit to resolve and reports status; `read` is a snapshot, not a wait.

| scheme | spawned by | address |
|---|---|---|
| `agent://` | `subagent` | `agent://goethe` (roster name) |
| `bash://` | `bash` | `bash://build` (content-derived slug) |
| `ask://` | `ask` | `ask://should-we-ship` (content-derived slug) |
| `canvas://` | `canvas` | `canvas://dashboard` (registry name) |
| `computer://` | `computer` `launch`/`attach` | `computer://desktop` (spawned) |

A `read` of a root that is still `Running` returns the transcript so far, and says so — it is a snapshot of an incomplete whole, and the honesty is part of the content. A root that is `Idle` reads frozen.

### Path children = the facts a process emits (frozen or append-only)

| path | content | cache |
|---|---|---|
| `agent://goethe/artifact` | the subagent's final output, frozen once it settles | disk |
| `agent://goethe/history` | the transcript, rendered as concise markdown | disk (append-only while running) |
| `bash://build/output` | accumulated output (the transcript) | prefix-cache; re-read returns the tail |
| `ask://should-we-ship/answer` | the settled answer, once `Idle{Completed}` | disk |

The root is the raw transcript; the children are *projections* of it (artifact = the settled final output, history = rendered form, answer = the settled slice). Reading the root reads the transcript as-is at this moment; reading a child reads the projection.

### Registry schemes = things that pre-exist, not spawned

These were the "read-only surface" — now they are files, read with `read`/`find`/`search`:

| scheme | reads as |
|---|---|
| `skill://<name>` | a skill's instructions; exact referent resolves (the one-shot rule carries over); `find skill:// <query>` is the fuzzy search |
| `todo://<tree>` | the todo tree; `read todo://` gets the tree, `edit todo:// <patch>` applies the RFC 6902 ops |
| `memory://<query>` | the project memory index, surfaced through the same reader |
| `rule://<name>` | a single rule's body, for inspection without re-listing |
| `artifact://<id>` | any captured artifact (logs, traces, raw tool output) |
| `history://<id>` | any session's transcript by id; bare `history://` lists sessions |

Additive by construction: a new kind is a new scheme — `mcp://`, harness docs, anything later — with no tool changes and no root to touch.

## Sessions are paths: the bare-name exception is gone

The previous spec's one namespace exception — artist names stay bare (`Goethe`, not `agent:goethe`) because a name is a persona, not a prefix — is resolved rather than violated. The name **remains** the persona: it is what the prompt shows ("You are Goethe"), what the roster claims, what the mailbox keys on, what survives reconnects via stateless identity. But the *address* is now uniform: `subagent` returns `agent://goethe`. Persona and address decouple; the noun space has no exceptions.

Everything identity-shaped in the previous spec survives unchanged: the 736-name roster, idempotent claims, content-derived slugs (truncated whole at 24 chars, disambiguation suffix), the durable stateless path, mail delivered on the next tool result (the mailbox keys on the name, which the path embeds).

## The verb split, restated as the boundary

- **Creation is a verb, always.** You cannot `touch` a subagent into existence; PRs and skills pre-exist, processes do not. `spawn` stays.
- **Tracking is a verb.** `poll` is the *time-aware* reader: it waits for a state transition (or a timeout) and reports `Activity`/`Status`. `read` is a snapshot; `poll` is a wait. The two coexist on the same path — `read bash://build` gives the output so far, `poll bash://build` settles when the unit resolves. `read` never claims to be a wait, and `poll` never pretends to be a snapshot.
- **Injection and cancellation are verbs.** `send`, `abort`.
- **Facts are read by the FS tools.** `read`, `find`, `grep`, `search` resolve schemes transparently — and they work on running transcripts as well as frozen facts, because a transcript has a well-defined whole at every instant.
- **Enumeration is `find`, not a session verb.** `find agent://` lists subagents; `find bash://` lists bash sessions; `find skill:// <query>` is the skill search; `find artist://`... no — enumeration is per-scheme, and `list` is **deleted**. The old `list` was a special session tool doing what `find` already does; under one noun space it is just `find` on the scheme root. (This is OMP's "bare `history://` lists agents" — enumeration is a space operation, not a time operation.)

Two contract details carry over from the current `find`: on a scheme root the fuzzy path index is bypassed — the result is the scheme's own session list, ordered by recency, because a handful of live sessions are not a codebase. And `grep` is the content mirror of `find`: `grep` over a scheme reads each entry's transcript/body and returns matching lines with their `#anchor`s, so a hit is immediately addressable. `find` and `grep` are both per-scheme; a scheme root is a directory, not a file.

## The stability rule: the one real boundary

The filesystem beauty — stable content, disk cache, second reads free — applies *exactly* where content is stable. There are two classes, and they are determined by a single question: *can this content still change?*

1. **Frozen** — the content will never change again: settled artifacts, settled answers, skills, rules, an `Idle` session's transcript. Cached on disk, second reads free.
2. **Append-only** — the content can only *grow*: a `Running` session's transcript. The prefix is stable, so the prefix caches; a re-read returns only what is new. At any instant it is a well-defined whole — just an incomplete one, and the read says so.

There is no third class. "Live" is not a stability class — it is the *temporal* state of an append-only node. That is the whole point: a running session is exactly as readable as a growing log file, and `read`/`grep`/`poll` answer three different questions about it. What would be a misuse — caching an append-only node whole and serving it later as complete — is prevented by the class itself: an append-only read always returns the current tail, never a stale frozen copy.

## Selectors compose: `#anchor` is the durable address

The FS tools already accept selectors on real paths, and scheme paths take the same ones. But the selector Artist actually speaks is its own: **content-derived mnemonic anchors**, not positions. A line number is invalidated by any insert above it; an anchor survives. `read` renders every line as `ANCHOR: CONTENT`, and `edit` takes the bare anchor ("never use a line number"). Under one noun space that handshake becomes an address — the `#` fragment:

- `read src/foo.rs#campfire` — a real path with an anchor fragment: the line anchored `campfire` in `src/foo.rs`.
- `edit src/foo.rs#start <new content>` — edit at the anchored line.
- `read agent://goethe/artifact#findings` — the `findings` handle in that artifact's ledger.
- `read skill://rustperfopt#campfire` — a named handle inside the skill's ledger.
- `:raw` survives; `:50+150` line slices are dropped. The model never speaks position.

`#anchor` is the one-hop key the harness already uses internally — the ledger is keyed by `(path, handle)`, and the same handle legitimately means different lines in different files (locate.rs). The syntax just exposes that key. It parses on real paths and scheme paths alike, and it composes with the other selectors: `path#handle:raw`, `path#handle/2.field`.

### Anchors are deterministic, not persisted

The current allocator mints handles by cursor: two fresh actors get different mnemonics for the same line, and "same line, same anchor" holds only while the actor's anchor state survives. A noun-space address must resolve the same for anyone, so anchors become **deterministic functions of identity**: the handle is derived from the line's full hash, and the same line is the same anchor for any actor, any session, any process. Durability falls out of determinism — no allocator cursor to persist, no export/import of anchor state.

That identity is not the raw line text — identical lines would collide. The full hash already folds in the line's **semantic identity**: for structured languages, the tree-sitter AST ancestry path plus the raw bytes (`compute_semantic_identities_rust`); otherwise trimmed content. Duplicate identities are then disambiguated by occurrence index and both neighbors' hashes. Identical lines get distinct anchors; an edit that leaves a line's identity untouched leaves its anchor untouched. The mnemonic is a screened word derived deterministically from that full hash — the allocator's cursor is the only stage that was ever non-deterministic.

Two consequences, both deletions:

- The cross-file `SlotPreference` machinery (keeping a bare handle live in only one file) is dead. With `path#anchor`, the path is in the address; the same handle in different files is different addresses — exactly what the `(path, handle)` ledger already says.
- The persisted anchor store becomes a recomputable index. Resolving `path#anchor` reads the current content, derives each line's handle, and finds the match; if the handle no longer resolves, the anchor is stale and the read/edit says so.

## What is writable

`edit`/`write` resolve schemes only where the target is genuinely editable — registry facts with a defined patch contract: `todo://` (the RFC 6902 ops), `rule://`, `skill://` (authoring), `memory://` (store/supersede). Live session roots are never writable paths (injecting is `send`); frozen artifacts are never writable (they are a record).

## What survives unchanged (from the previous spec, as implemented)

- Spawn semantics: never block, always return an address immediately (now a path).
- The two-enum session state (`Activity`/`Status`), the derived terminal-ness, the `abandoned` tombstone.
- The cross-process `abort` protocol (registry marker + SIGUSR1 + hang escalation).
- The universal web profile, MCP stateless identity via `clientInfo`, mail-on-next-tool-result, canvas prompts riding the same mailbox.
- The MCP gate flags (only `subagent` + `memory` remain daemon flags; everything else is profile `permits`).
- The `computer` two-stage dynamic discovery — the heavy surface appears once a `computer://` session exists.
- The `skill`, `todo`, `ask`, `canvas` reworks — their operations map onto the scheme, they do not change.

## Tool count under the grammar

- spawn (5): `subagent`, `bash`, `ask`, `canvas`, `computer` — return paths.
- session verbs (3): `poll`, `abort`, `send`.
- FS tools (already exist, now scheme-aware): `read`, `find`, `grep`, `edit`, `write`.

`list` is deleted (→ `find <scheme>://`). The `skill`/`todo`/`memory` tools' read operations collapse into `read`/`find`/`search` on their schemes. The count stays flat because the path space carries the variation — exactly OMP's claim, on the noun side, without pretending processes are files.
