# Artist — Production Readiness Plan

> Status: **living orientation document**. Long-term scope, not a sprint plan.
> Owner: the person reading this for orientation. Implementation is gated by the
> decision log in §4 and the mining gate in §7.

---

## 0. How to read this document

1. **§2** is the architectural north star. It is locked and does not change without
   revisiting this document.
2. **§4** is the decision log: what is locked, what is open, and what is gated.
   Everything downstream obeys it.
3. **§7** is the prior-art mining registry. No Gortnite code is ported without a
   checkpoint there and an explicit sign-off.
4. **§8** is the phased roadmap. Phases are dependency-ordered and each ends in an
   exit criterion, not just "stuff was written."
5. **§12** is the definition of "prod ready." If it is not true, the repo is not done.

---

## 1. Product vision

**Artist** is a Rust coding-agent harness built on one thesis: **everything is a
file, and everything beyond a tiny kernel is a WASM component extension.**

- The agent, its tools, its resources, and its extensions are all addressable
  through a single virtual filesystem (VFS).
- The kernel is aggressively minimal. It hosts exactly the four namespaces that
  cannot be bootstrapped as extensions; every other namespace is an extension
  mounted under `resources://`.
- Files are addressed *within* by **semantic anchors**, never by line numbers.
- Tools ("verbs") are **batch-native** at the ABI but **scalar** at the model
  surface; the harness does the vectorization, not the model.
- Model-facing data is **TOON**, not JSON.
- Security and sandboxing are **non-goals**. All code runs full-trust, as-is.

"Prod ready" means this vision is a real, installable, cross-platform product —
not a demo — with a working agent loop, a usable CLI, durable sessions, and a
green cross-platform CI. The concrete checklist is §12.

---

## 2. Locked architecture (north star)

### 2.1 Everything is a file

The platform-neutral VFS core (`artist-kernel`) exposes an inode-based
`Vfs`/`Namespace` surface. Backend bridges mount it to the host:

| Host | Bridge | Crate |
|---|---|---|
| Linux | FUSE (fuser, async API) | `artist-kernel/vfs` |
| Windows | WinFsp | `artist-kernel/vfs/windows` |
| macOS | macFUSE | `artist-kernel/vfs` (same fuser path, macFUSE target) |

Namespaces appear as ordinary directory trees you can `ls`, `cat`, and — per the
read-write decision in §4 — **write to**.

### 2.2 Three extension contract families

Extensions are WASM components. Their "class" is *which of the three contract
families their exported interfaces satisfy* (classified by WIT inspection at load
time, no manifest):

1. **Nouns** (`artist:nouns/namespace`) — addressable, filesystem-shaped resources.
   A shell session (`bash://`), a repo (`repo://`), an issue tracker (`issues://`).
2. **Verbs** (`artist:verbs/*`) — typed operations over resources, file-shaped in,
   file-shaped out. The tools: `read`, `write`, `edit`, ….
3. **Events** (`artist:events/subscriber`) — long-lived, reactive services: hooks,
   policies/vetoes, formatters, background tasks, sub-agents.

A single component can satisfy several families at once (e.g. `bash://` is a noun
**and** a service).

### 2.3 The kernel hosts exactly four namespaces

Only the namespaces that *cannot* be bootstrapped as WASM live in the kernel:

- **`files://`** — passthrough to the real OS filesystem. Must exist before any
  WASM exists (the kernel reads disk to find and compile extension source).
- **`resources://`** — mounts namespace (noun) extensions.
- **`tools://`** — hosts the verb surface and the source→wasm→mount tool-loading
  machinery. (If tool *loading* were a tool, infinite regress.)
- **`events://`** — the typed event broker every extension and the agent loop
  subscribes to.

Everything else (`bash://`, `repo://`, `process://`, `session://`, `issues://`, …)
is a resource extension mounted under `resources://`. Tools are verbs, not nouns:
`tools://` hosts verb implementations, nothing noun-shaped.

### 2.4 Anchors, not line numbers

Addressing within a file is by **semantic occurrence anchor**, not line number.

> Revised 2026-08-18 (per `design.md`): addressing is **not** kernel-resident. It
> is a tool-surface convention of the `read`/`edit`/`insert` verbs; the anchor
> engine (the former `teca` idea) lives in the **tool layer**. The kernel VFS is
> byte-oriented, exactly like a normal filesystem.

Consequence: the VFS moves bytes; the verbs resolve anchors → offsets against a
snapshot and commit bytes back. Anchor resolution is a verb concern, and the
`artist-ast` fork produces structured analysis *without* rendering line numbers so
anchors stay the canonical address.

### 2.5 Batch-native verbs, scalar model surface

Every verb exports one canonical function shaped like:

```wit
verb: func(requests: list<request>) -> list<result<response, error>>;
```

A one-item call is a one-item batch. The **model never serializes the outer list**:
it emits scalar sibling calls, and the harness coalesces one assistant turn's calls
into grouped native batches. Locked verb-shape principles (from
`kernel-wasm-scaffold.md`):

- flat scalar arguments over nested structures;
- homogeneous operations over unions / optional modes;
- batching is pushed into the implementation, never asked of the model;
- "boring" shapes are closer to model training — no clever encodings;
- **parsers tolerant, validators strict**.

### 2.6 TOON at the model boundary

Model-facing data is **TOON (Token-Oriented Object Notation)**, not JSON — a
token-efficient, human-readable serialization designed for LLM prompts (its win:
uniform arrays of objects collapse into tables that declare the field list once
and stream rows). Implementation dependency:

- `toon-format` v0.5.0 — the mature core (`github.com/toon-format/toon-rust`).
- `serde_toon_format` — serde-compatible encoder/decoder for integration with
  existing `serde` types.

The authoritative programmatic ABI stays **typed** (WIT/component-model types). TOON
is the *model-facing* rendering of those types (schema, tool arguments, `stdobs`).
The harness never parses `stdobs` back out of the model text to recover typed data;
typed data travels on the authoritative plane (§6.3).

### 2.7 Async-first, WASIp3, component-model async

- `wasmtime` / `wasmtime-wasi` 47.0.3, feature `p3` (WASIp3).
- `component-model-async` + `component-model-bytes`, `wasm_component_model_async(true)`.
- No sync path: the kernel is tokio + async-trait; the events contract is
  inherently async.

### 2.8 Full trust

No sandbox, no capability theater, no policy layer. Ordinary OS permission errors
map to structured errors; nothing else is added.

---

## 3. Current baseline

What is actually in the tree today, as context for the roadmap.

| Crate / path | State | Notes |
|---|---|---|
| `artist-kernel` | **working** | `Kernel`, `Namespace`, `Resources`, `Vfs`. Read-only ops only (`lookup/getattr/readdir/read/parent`). |
| `artist-kernel/vfs` | **working** | FUSE bridge via fuser's experimental async API. Mount test passes. Read-only. |
| `artist-kernel/vfs/windows` | scaffold | WinFsp bridge (build.rs + driver skeleton). |
| `artist-kernel/wasm` | **working** | Engine (`build_engine`, async), loader (`Extension::load`), `classify` (noun/verb/event union). |
| `artist-kernel/wasm/nouns` | **working (glue)** | `WasmNamespace` + `NamespaceGuest` trait; ino↔path `PathTable`; tests drive a Rust fake. |
| `artist-kernel/wasm/verbs` | **working (shape)** | `VerbDispatcher`/`VerbTool` — batch-native shape + dispatch, parameterized by Req/Res. |
| `artist-kernel/wasm/events` | **working (glue)** | `EventBroker` (subscribe/emit/unsubscribe, monotonic sequence, fan-out) + `Subscriber` trait. |
| `artist-kernel/wasm/filesystem` | **in progress** | Hand-rolled `wasi:filesystem` host over the kernel `Vfs` (bindgen + descriptor + streams + host + preopens). |
| `artist-ast` | **working, full** | ast-bro fork at upstream `d9bccef`. Analysis engine whole; renders without line numbers. |
| `vendor/fff` | **orphaned** | FFF search engine fork. `ARTIST_PATCHES.md` documents `grep_raw`, `fuzzy_line_matches`, `fuzzy_match_score`, all referencing a now-deleted `artist_kernel::SearchService`. |
| `artist-agent` | **WIPED STUB** | Empty; `Cargo.toml` intact (Rig 0.41, llm-provider, session, component deps). |
| `artist-component` | **WIPED STUB** | Empty; wasmtime-wasi p3 + wit-parser deps intact. |
| `llm-provider` | **WIPED STUB** | Empty; was "ChatGPT subscription provider authentication." |
| `artist-session` | **WIPED STUB** | Empty; Rig 0.41 + fs2 (file locks) deps intact. |
| `artist-cli` | **WIPED STUB** | Empty; ratatui/clap/dialoguer/image/syntect deps intact. |

The prior-art branch **`Gortnite`** (92 commits, ~224k non-vendor lines) contains the
subsystems enumerated in §7. Its code is ~8 months divergent from this tree; ports
require adaptation, not copy.

---

## 4. Decision log

### 4.1 Locked decisions

| # | Decision | Outcome | Rationale |
|---|---|---|---|
| D1 | Product tier | **Full product** (ambitious, long-term). | Owner wants a scope doc to orient a multi-wave build. |
| D2 | Prior-art disposition | **Mine Gortnite selectively**; every port gated on sign-off (§7). | Reuse proven subsystems; avoid resurrecting the mistakes that led to the wipe. |
| D3 | Model data | **TOON now**, via `toon-format`. | Token efficiency is a first-class design goal, not an optimization. |
| D4 | Provider strategy | **Agnostic layer** (subscription + API-key + local). | Future-proof; Rig already ships auth-backed clients to lean on. |
| D5 | Platforms | **Linux + macOS + Windows** from the start. | The two VFS bridges already exist; macFUSE closes the third. |
| D6 | VFS writability | **Read-write mount** (write/create/unlink/mkdir in the trait). | The `design.md` "plug into bash" end-state requires tools to mutate through the mount. |
| D7 | WASM | Async-first, WASIp3, component-model async. | Already implemented; consistent with events contract. |
| D8 | Kernel scope | Four namespaces, no more. | `extension-contracts.md`; bootstrapping argument. |
| D9 | Anchors | Tool-layer convention; VFS is byte-oriented. | `design.md` revision 2026-08-18. |
| D10 | Sandbox | Full trust, none. | Explicitly a non-goal. |

### 4.2 Open decision — agent runtime foundation (owner decides later)

The one deliberate fork left open. Three options, with full implications. The plan
does not commit until the owner chooses; §8 marks where the choice becomes
binding (Phase 4, the agent loop).

**Option A — Keep Rig 0.41, hand-drive `AgentRun`.**

- **What:** Rig 0.41 splits into `rig-core` (portable contracts) + `rig-agent`
  (classic runtime). The loop is a *sans-IO, steppable, serializable* state machine
  (`AgentRun`). Drive it with `next_step()`:
  - `CallModel` → send completion → `model_response()`
  - `CallTools { calls }` → execute the whole same-turn set → `tool_results()`
  - `Done`
- **Pros:**
  - `CallTools` hands Artist the complete pending-call set, which is *exactly* the
    batch boundary verbs need. "Execute with whatever concurrency the driver
    chooses" — deterministic batching is first-class, not an override.
  - `AgentRun` is `Serialize + Deserialize`: persist/resume a run across processes
    → maps onto durable sessions and the session-host daemon.
  - Reuse Rig's provider clients (incl. ChatGPT/Copilot auth-backed), message/tool
    contracts, and the typed `AgentHook` trait.
- **Cons:**
  - Hand-driving means **we own the hook stack** (AgentRun "deliberately contains
    no … hook stack"); we re-wire `AgentHook` dispatch ourselves.
  - We own memory orchestration too (or wire `rig-memory`).
  - `AgentRun`'s serialized format "carries no cross-version stability guarantee"
    → pin Rig's version for any persisted run.
- **Implications for batching:** ideal — the batch executor replaces the
  `CallTools` arm of the driver loop.
- **Implications for hooks/steering:** reuse `AgentHook` events
  (`ToolCallAction`, `ToolResultAction`, `ObservationAction`, `ModelTurnAction`,
  `InvalidToolCallAction`, `RequestPatch`); map them onto the `events://` broker so
  *extensions* can hook the loop.
- **Implications for subagents:** a sub-agent is itself an `AgentRun` driven by the
  same executor; the events contract makes it an addressable service.

**Option B — Keep Rig 0.41, use `AgentRunner` as-is.**

- **What:** the batteries-included `Agent::runner()` path (hooks, tools, retrieval,
  memory handled by Rig).
- **Pros:** least work; hooks + memory + RAG for free.
- **Cons:** its tool-execution path does **not** hand Artist the batch set — this is
  precisely the per-tool-callback problem the previous build hit. Batching must be
  bolted on (e.g. `tool_concurrency`), which parallelizes but does not guarantee one
  vectorized component call per verb group.
- **Implications:** faster to a working loop, but the deterministic-batch invariant
  (§2.5) is compromised; likely needs a later rewrite to Option A.

**Option C — Hand-roll the loop, drop Rig.**

- **Pros:** total control; no version-pinning constraint; the loop's state model is
  ours to persist however we want.
- **Cons:** reimplement turn accounting, tool-call validation, invalid-call
  recovery, streaming, usage aggregation, and provider transports that Rig already
  provides — a large, low-value surface area. Provider work (D4) balloons.
- **Implications:** maximum extensibility, maximum cost; only justified if a
  concrete Rig limitation surfaces that Options A/B cannot work around.

**Recommendation (non-binding):** **Option A**. It is the only option that satisfies
both the batch invariant and reuses Rig's mature provider/contract surface. Decide
at Phase 4; the roadmap's Phase 1–3 work is runtime-agnostic either way.

---

## 5. Target crate & module layout

Dependency direction is top-down: `cli` → `agent` → `component`/`session`/`provider`
→ `kernel` (and its `vfs`/`wasm` subcrates). Nothing above `kernel` may depend on a
UI crate; nothing in `kernel` may depend on the agent/provider layer.

```
crates/
  artist-kernel/            # platform-neutral VFS core + 4 kernel namespaces (READ-WRITE)
    vfs/                    # FUSE bridge (unix + macFUSE)
    vfs/windows/            # WinFsp bridge
    wasm/                   # wasm host: engine, loader, classification
      nouns/                # noun contract host glue + WIT
      verbs/                # verb contract host glue + WIT
      events/               # event broker + WIT
      filesystem/           # wasi:filesystem host over the kernel Vfs
  artist-component/         # (rebuild) component discovery/build/tool loading: tools:// machinery
  artist-ast/               # (exists) AST navigation engine (anchors' structural substrate)
  artist-session/           # (rebuild) session store: history, replay, locks, persistence
  llm-provider/             # (rebuild) provider-agnostic client layer
  artist-agent/             # (rebuild) the agent loop + batch executor + hooks→events bridge
  artist-cli/               # (rebuild) ratatui TUI, args, slash commands, profiles

  # mined from Gortnite, gated by §7 (proposed, not yet created):
  artist-tools/             # verb implementations (read/edit/find/locate/outline/skeleton/...)
  hashline-tools/           # mnemonic anchor state + attribution across processes
  artist-session-host/      # durable session daemon (serialize/resume AgentRun)
  artist-memory/            # embedding + assertion/graph stores
  artist-mcp-server/        # MCP daemon + tunnel
  artist-canvas/            # React-in-WASM UI (wef/CEF)
  artist-computer/          # computer-use (OCR/screen/PTY/CDP)
  artist-logic/             # knowledge/logic kernel
  artist-config/ artist-registry/ artist-rules/ artist-ui-core/ artist-tool-api/
  artist-gpui/              # zed-gpui vendored UI (defer/optional)
```

The **event stream** (`events://`) is the cross-cutting spine: the agent loop and
every extension publish typed events (`turn-started`, `tool-called`,
`result-produced`, `file-changed`, …) to the same broker. This replaces ad-hoc
hook plumbing with one addressable substrate.

---

## 6. Core subsystems (design detail)

### 6.1 VFS — read-write (D6)

Today's `Vfs` trait is read-only. The read-write decision expands it. Minimum new
operations on both `Vfs` and `Namespace`:

```rust
async fn write(&self, ino: Ino, offset: u64, data: &[u8]) -> Result<u32, VfsError>;   // bytes written
async fn create(&self, parent: Ino, name: &OsStr, kind: NodeKind) -> Result<Attrs, VfsError>;
async fn unlink(&self, parent: Ino, name: &OsStr) -> Result<(), VfsError>;
async fn mkdir(&self, parent: Ino, name: &OsStr) -> Result<Attrs, VfsError>;
async fn setattr(&self, ino: Ino, size: Option<u64>) -> Result<Attrs, VfsError>;        // truncate
// likely: rename(&self, ...) once files:// and process:// both need it
```

Design notes:

- **Byte-oriented.** The VFS moves bytes/offsets. Anchor resolution lives in the
  verb layer (§2.4). bash tools that read/write the mount get ordinary file
  semantics; artist verbs get anchors on top.
- **ino stability.** Inos must be stable across mutations (FUSE caches them).
  The `Kernel` assigns namespace-root inos sequentially; each `Namespace` owns its
  internal ino↔path mapping (`WasmNamespace` already has a `PathTable`). Writes must
  not renumber siblings.
- **Write-through to the noun.** `resources://` routes write ops to the mounted
  noun extension (the noun WIT grows write/create/unlink/mkdir in §6.2); `files://`
  routes them to the OS; `WasmNamespace` translates inos→paths and calls the guest.
- **FUSE bridge** maps the new ops to `AsyncFilesystem::write/create/unlink/mkdir/
  setattr`. Open/flush can be no-ops initially (no per-open state) until file
  handles matter for locking.
- **Windows bridge** mirrors the same ops against WinFsp. macOS reuses the fuser
  path with the macFUSE target.
- **Concurrency & atomicity** are *verb-layer* concerns (snapshot → validate → one
  commit for sibling edits). The VFS provides the primitive reads/writes; it does
  not implement edit transactions.

### 6.2 WASM host & contracts

- **Noun contract** (`artist:nouns/namespace`): add the write side
  (`write`, `create`, `unlink`, `mkdir`) to the WIT and to `NamespaceGuest`. The
  `WasmNamespace` glue already translates inos↔paths; extend it.
- **Verb contract** (`artist:verbs/*`): keep the family shape. Concrete verbs
  (read/write/edit/…) are added as interfaces in this package or in tool
  extensions' own packages (§6.3). `VerbDispatcher` stays parameterized per family.
- **Event contract** (`artist:events/*`): broker already works. Remaining work is
  the bindgen adapters (real component instances, not the test fakes) and defining
  the standard topic taxonomy (the event names every subsystem can rely on).
- **`wasi:filesystem` host** (`filesystem/`): finish the hand-rolled host so
  extensions read **and write** any namespace through ordinary WASI file APIs.
  This is the piece that makes the whole design cohere — extensions see the entire
  namespace universe as one filesystem, rooted at `resources://`.
- **Classification** (`classify.rs`): already correct (exported-interface-name
  union). Extend `names` to cover the new write-side noun interface version.
- **Lifecycle:** minimal — load-on-demand, one live instance per extension.
  Hot-reload is deferred to its own subcrate (explicitly out of scope until a
  later phase).

### 6.3 Tool/verb surface + TOON + search

- **Verb set (target):** `read`, `write`, `edit`, `insert`, `find`, `grep`, `run`,
  `poll`, `abort`, `delete`. These are derived from active `tools://` packages —
  **not** a hardcoded closed enum in the agent.
- **Two planes, one per result:**
  - **Authoritative plane** — the exact typed WIT value; consumed by Rust/Python/
    WASM/nested tools/tests.
  - **Observation plane (`stdobs`)** — the package-owned rendering of one scalar
    response into model context; compact, lossy, may differ from the typed value.
    The observer belongs to the tool package (`observe: func(response) -> string`),
    validated at activation, pinned to the producing generation. No host-side
    `match tool_name` renderer.
- **TOON at the boundary (D3):** tool schemas, arguments, and `stdobs` are TOON.
  The pipeline is: tolerant-parse TOON → normalize against the scalar `DynamicType`
  → strict-validate → typed value. Where a provider/transport *hard-requires* a
  JSON-schema tool definition (Rig's tool registry), generate the schema from the
  same typed contract and serialize arguments as TOON; verify this seam in a spike
  during Phase 4 (see §11 Q1).
- **Search (`find`/`grep`):** re-establish the search service that `vendor/fff`
  patches reference (`grep_raw`, `fuzzy_line_matches`, `fuzzy_match_score`). Decide
  whether it is a kernel service or a `tools://` verb backed by fff; the
  `ARTIST_PATCHES.md` contract (pattern grammar owned by Artist, changed-file
  rejection before anchor conversion) is the spec to preserve.
- **Deterministic batching (§2.5):** the boundary is *all tool calls in one
  committed assistant turn*. Group valid calls by exact tool identity + generation;
  one native batch per group; map results back in source-call order. Same-resource
  mutation rules (sibling edits/inserts coalesce into one snapshot+commit; write
  mixed with edit/insert = `Conflict`) are verb-layer, not kernel-layer.

### 6.4 Kernel namespaces + resource extensions

- **`files://`** — OS passthrough, now read-write (backed by real openat/pwrite).
  Bare OS paths normalize through one address/path resolver (no second parser).
- **`resources://`** — mounts noun extensions; write ops routed to the noun.
- **`tools://`** — verb surface + source→wasm→mount loading. Tools are
  self-inspectable/self-modifiable: read the source through the namespace, edit it,
  it recompiles.
- **`events://`** — the broker (§6.2).
- **Resource extensions to build (nouns, mounted under `resources://`):**
  - `process://N` with `/stdin /stdout /stderr /ctl` (distinct append-only streams,
    monotonic anchors, exit status on the root, `abort` as hard termination, no
    capability theater).
  - `session://name` with `/inbox` (input/steering events; retained output readable
    after abort).
  - `repo://` (git-aware tree; mine `artist-tools` workspace/drift), `bash://`
    (a PTY-backed shell session — a noun **and** a service; mine
    `artist-computer`'s PTY surface), `issues://`, and any future `mcp://`.

### 6.5 Agent loop

- **Runtime:** per §4.2 (open). The batch executor is runtime-agnostic; it sits at
  the `CallTools` boundary (Rig `AgentRun` in Option A) or replaces the loop's tool
  step (Option C).
- **Model/authoritative split:** Rig structured result = `stdobs` (model_output) +
  outcome (structured success/error) + extensions (authoritative `DynamicValue` +
  `VerbId` + generation). Capture/recorder reads authoritative metadata, never
  parses `stdobs`.
- **Hooks → events bridge:** map the loop's hook points onto `events://` topics so
  policies/vetoes/formatters/subagents are ordinary extensions, not agent-code
  special cases.
- **Persistence/replay:** the loop state (Option A: `AgentRun`) is
  serializable; the session store persists it, and `artist-session-host` can resume
  a run in another process.
- **Subagents:** a long-lived service that subscribes to events **and** exports verb
  interfaces; driven by the same executor.

### 6.6 Provider layer (D4)

One agnostic trait over backends: **subscription auth** (ChatGPT/Copilot — lean on
Rig's native auth-backed clients), **API-key** (OpenAI/Anthropic/etc. via Rig
providers), and **local** (Ollama/llamafile via Rig). `llm-provider` becomes the
thin orchestration + credential-store layer; it does **not** reimplement provider
transports from scratch. The trait surface must expose streaming, usage/token
accounting, and identity/lineage (which provider produced which response) so the
session store can attribute correctly.

### 6.7 Session & storage

- **Session store** (`artist-session`): append-only log + replay, file-locked
  (fs2), writer-lock handoff on reopen (a Gortnite fix), compaction.
- **Durable sessions** (`artist-session-host`): a daemon that owns sessions,
  serializes/resumes loop state, and serves concurrent clients.
- **Conversation history** is a first-class noun (visible/editable via `session://`),
  not an opaque blob.

### 6.8 CLI & product surface

- `artist-cli`: ratatui TUI, clap args, slash commands, profiles, diffs with
  anchor gutters (mine the Gortnite `tool_ui` + `mnemonic-anchor gutter` work).
- `artist-canvas`: the React-in-WASM UI layer, mined and mounted as a noun/service
  (§7).

### 6.9 Prior-art subsystems

Canvas, computer-use, logic, memory, MCP, session-host, registry, rules, config,
ui-core, tool-api. All are mining targets (§7), not rebuild-from-scratch targets.

---

## 7. Prior-art mining registry

> **Gate:** no Gortnite code is ported without a checkpoint here and the owner's
> explicit sign-off. Each entry states what it is, what it maps to, adaptation
> cost, and what to discard.

Ordering is dependency-driven (earlier rows unblock later rows), not a value
ranking. Recommended waves:

| Subsystem (Gortnite) | ~Lines | Maps to | Priority | Adapt. cost | Notes / discard |
|---|---|---|---|---|---|
| `artist-tools` + `hashline-tools` | 10k + 1.8k | verb implementations + mnemonic anchors | **Wave 1** | High | The anchor/edit/find/coalesce/outline/skeleton tools are the concrete verb layer; keep the anchor model, drop any line-number rendering. |
| `artist-session` + `artist-session-host` | 4.5k + 2.1k | session store + durable daemon | **Wave 1** | Medium | Keep log/replay/lock-handoff; align the store with serializable loop state. |
| `artist-agent` loop files (tool_registry, tool_set, steering, capture, ttsr, statefulness, fallback, profiles, thinking, todo) | ~13k | the agent loop | **Wave 1** | High | Reference for the loop; re-targeted at the new runtime + `events://`, not copied. |
| `llm-provider` (provider.rs, set.rs) | ~200 | provider layer | **Wave 1** | Low | Small; superseded by the agnostic trait, keep the auth approach as reference. |
| `artist-ast` | 40k | anchor substrate | **done** | — | Already ported; survives as-is. |
| `artist-memory` | 8.5k | memory/retrieval | **Wave 2** | Medium | Embedding + assertion/graph stores; re-host retrieval behind the ranking trait in `artist-ast`. |
| `artist-mcp-server` | 4k | MCP daemon + tunnel | **Wave 2** | Medium | Keep typed harness/paging/progress; re-home under `resources://`/`events://`. |
| `artist-canvas` | 38k | React-in-WASM UI | **Wave 3** | High | Keep; the bridge is the prior art for UI-as-a-noun. Vendored wef/CEF re-evaluated. |
| `artist-computer` | 31k | computer-use (OCR/screen/PTY/CDP) | **Wave 3** | High | Feature, not core; PTY surface shared with `bash://`. |
| `artist-logic` | 20k | knowledge/logic kernel | **Wave 4** | High | Distinctive; port only if the full product wants it. Soundness tests are the valuable part. |
| `artist-registry` / `artist-rules` / `artist-config` / `artist-ui-core` / `artist-tool-api` | ~5k | supporting crates | **Wave 2** | Low-Med | Port as needed by the crates above. |
| `artist-gpui` (+ vendored `zed-gpui`, `wgpu`) | ~5k + 250k vendored | alternative GPU UI | **defer** | Very high | Optional; revisit only if canvas is insufficient. |
| vendored `wef` (CEF webview) | 11k | canvas runtime | **Wave 3** | High | Re-evaluate vs. plain system webview before re-vendoring. |

**Process per checkpoint:** (1) enumerate what the subsystem did and what it maps
to; (2) list the files to port and the files to discard; (3) state the adaptation
delta to the new kernel/events/TOON; (4) owner signs off → port begins. Ports land
behind the same test bar as new code (§9).

---

## 8. Phased roadmap

Dependency-ordered. Each phase ends at a verifiable exit criterion. Decision gates
(`G:`) are points where the owner must make a call before the next phase is
meaningful.

### Phase 0 — Green baseline + read-write VFS
- Make `cargo build`/`cargo test` green across all existing crates (unix).
- Add the read-write ops to `Vfs`/`Namespace` (§6.1); extend the FUSE and WinFsp
  bridges; add macFUSE to the fuser path.
- Stand up cross-platform CI (Linux/macOS/Windows) with the mount-test gating
  strategy in §9.
- **Exit:** read-write `files://` + `resources://` pass VFS contract tests on all
  three platforms; CI green.

### Phase 1 — WASM host vertical slice
- Finish the real bindgen adapters for noun/verb/event (replace the test fakes).
- Complete the `wasi:filesystem` host (read + write) over the kernel VFS.
- Wire classification → mount: a demo noun extension appears in the mounted fs and
  is readable/writable.
- **Exit:** a WASM noun component is mounted, listed, read, and written through
  FUSE/WinFsp end-to-end; component conformance tests restored.

### Phase 2 — Tool loading + verb surface
- Build `tools://`: source→wasm→mount, activation metadata, generation pinning.
- Define the concrete verb WIT contracts (§6.3) and the TOON serialization layer.
- Re-establish the fff-backed `find`/`grep` search service.
- **Exit:** `read/write/edit/insert/find/grep/run/poll/abort/delete` execute as
  batch-native wasm components; tolerant-parse → strict-validate path proven;
  TOON round-trips typed values.

### Phase 3 — Kernel namespaces + resource extensions
- Complete `files://`, `resources://`, `tools://`, `events://`.
- Build `process://` and `session://` as noun extensions (mine CRORTNITE redesign +
  Gortnite session store).
- **Exit:** a process and a session are addressable, readable, writable, pollable,
  and abortable through the VFS and the verb surface.

### Phase 4 — Agent loop + provider (G: runtime decision)
- **G:** choose §4.2 Option A/B/C.
- Rebuild `llm-provider` (agnostic trait) and `artist-session` (log/replay/locks).
- Implement the loop, the batch executor at the `CallTools` boundary, the
  hooks→events bridge, capture/steering, subagents.
- Verify the TOON-vs-JSON tool-schema seam (§11 Q1).
- **Exit:** a CLI-driven agent completes a real multi-turn coding task using the
  verb surface, with a persisted, resumable session and authoritative capture.

### Phase 5 — CLI + product surface
- Rebuild `artist-cli` (TUI, args, slash commands, profiles, anchor-gutter diffs).
- **Exit:** the product is usable end-to-end from the terminal on all three
  platforms.

### Phase 6 — Gortnite mining waves (gated, §7)
- Wave 1 (tools/session/agent reference/provider) — largely consumed by Phases 3–4.
- Wave 2 (memory, MCP, registry/rules/config/ui-core/tool-api).
- Wave 3 (canvas, computer-use).
- Wave 4 (logic), and the defer decision on `artist-gpui`.
- **Exit:** each wave's ported subsystem passes its own prior-art test suite under
  the new kernel.

### Phase 7 — Hardening & release
- Property tests (input/invariant machinery), fuzz the parsers, perf budgets
  (mount latency, search latency), packaging (systemd units, installers),
  documentation, release tooling.
- **Exit:** §12 fully true.

---

## 9. Testing & CI

- **CI matrix:** GitHub Actions on `ubuntu`, `macos`, `windows`. Two jobs per OS:
  pure-unit (no mount) and, where the mount backend is available, mount tests.
- **Mount tests:** Linux FUSE requires `/dev/fuse` (privileged job or
  `fusermount`); Windows requires WinFsp installed on the runner; macOS requires
  macFUSE. When unavailable, mount tests **skip** (not fail) — the VFS contract is
  also exercised against an in-memory implementation so behavior is covered
  everywhere.
- **Component conformance:** a WIT-conformance harness (restore the deleted
  `conformance/` idea): for each verb, a typed guest + host test asserts the
  batch contract (`len(results) == len(requests)`, one-for-one, ordered).
- **Property tests:** the input atoms / pairing / anchor invariants (Gortnite had
  `proptest` for these); the TOON tolerant-parse → strict-validate path.
- **`artist-ast`:** its existing e2e/adapter suite stays green (it survived the
  wipe; keep it).
- **Deterministic-batching proof:** an instrumented test proves one wasm
  component export executes N requests (not N scalar calls).
- **Perf budget (Phase 7):** repeated search stays in-process warm (fff), mount
  latency bounded, TOON token savings measured against a JSON baseline.

---

## 10. Risks & mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| FUSE/WinFsp/macFUSE flaky in CI | false red | skip-when-absent + in-memory VFS contract tests as the always-on safety net |
| wasmtime WASIp3 + component-model async is bleeding-edge | API churn | pin wasmtime 47.0.3; isolate host glue behind the trait boundaries already in place |
| TOON tool-call comprehension by models | unreliable tool calls | spike/eval (§11 Q1) before Phase 4 locks the schema format; keep tolerant-parse as the safety net |
| `AgentRun` serialization not version-stable | resumable sessions break on upgrade | pin Rig's version for persisted runs; version the session format |
| Mining Gortnite: 8-month divergence | ports mis-map assumptions | per-port checkpoint (§7); ports must pass new tests, not just compile |
| `vendor/fff` patches reference deleted `SearchService` | search is dead until rewired | Phase 2 re-establishes the service; treat `ARTIST_PATCHES.md` as the contract |
| Read-write VFS expands the kernel surface before the verb layer exists | rework risk | do write ops against `files://` (simplest backing) first, then generalize to nouns |
| Ambitious scope (full product) | never ships | phased exit criteria + gated mining keep each increment shippable |

---

## 11. Open questions (non-blocking, resolved at the named gate)

1. **TOON vs JSON for tool-call arguments specifically** (Phase 4 spike): Rig's
   tool registry historically uses JSON-schema; verify whether models emit valid
   TOON tool calls reliably, and whether Rig's tool-definition path can advertise
   TOON. Fallback: JSON at the Rig tool-registry seam, TOON for everything else.
2. **Search service home** (Phase 2): kernel service vs. `tools://` verb backed by
   fff. Recommendation: verb (keeps the kernel minimal per §2.3).
3. **macOS mount backend** (Phase 0): macFUSE (kext, user-installed) vs. a
   socket/NFS alternative for the rare headless case. Recommendation: macFUSE.
4. **`bash://` PTY ownership** (Phase 3/6): whether `bash://` reuses
   `artist-computer`'s PTY surface or a dedicated one. Recommendation: share it.
5. **Canvas runtime** (Phase 6): re-vendor `wef`/CEF vs. a system webview.
   Deferred to the canvas port checkpoint.
6. **Hot-reload** (post-Phase 7): explicitly deferred; a dedicated subcrate later.

---

## 12. Definition of done — "prod ready"

The repo is prod-ready when **all** of the following are true:

1. `cargo build --workspace` and `cargo test --workspace` are green on Linux,
   macOS, and Windows in CI, with mount tests skip-gated where the backend is
   absent.
2. The VFS is read-write; `files://`, `resources://`, `tools://`, and `events://`
   work through the mounted filesystem on all three platforms.
3. A WASM noun extension mounts, lists, reads, and writes end-to-end; a WASM verb
   extension executes as a batch (`list<request>` → `list<result>`), proven by an
   instrumented test.
4. The full verb set (`read/write/edit/insert/find/grep/run/poll/abort/delete`)
   is available, batch-native, derived from active `tools://` packages (no
   hardcoded tool enum).
5. Model-facing schemas/arguments/results are TOON; the tolerant-parse →
   strict-validate path is property-tested; `stdobs` is package-owned and the
   authoritative typed value is captured separately.
6. The agent completes real multi-turn coding tasks; the loop's batch boundary is
   deterministic (same-turn calls → grouped native batches, source-ordered
   results).
7. Sessions are durable: persisted, resumable across processes, replayable, with
   writer-lock handoff and compaction.
8. The provider layer is agnostic over subscription/API-key/local backends with
   streaming, usage accounting, and response lineage.
9. Anchors (not line numbers) are the addressing convention throughout, with
   `artist-ast` supplying structure.
10. Gortnite mining waves are complete (or explicitly deferred per §7), each
    port behind the same test bar.
11. Packaging and release tooling exist (systemd units, installers) and are
    documented.
12. A human can install Artist on any supported platform and drive it through the
    §1 vision without reading this document.

---

## 13. Design decision catalog (open forks)

> **How to use this section.** These are the open design forks for you to mull
> over. Each entry is a multiple-choice decision with a **scorecard** ranking the
> options on the merits that matter for that decision. Scores are 1–5 (higher =
> better on that merit); the **Total** row sums them; the **Verdict** is my
> recommendation, **not binding** — it defaults to the owner, exactly like §4.2.
>
> **Legend.** ✅ = recommended · ⚖️ = depends / owner call · ❌ = avoid. A
> `(binds Phase N)` tag tells you when the decision stops being deferrable.

---

### 13.1 Kernel & VFS

#### F1 — VFS write surface (binds Phase 0)

**Question:** How much of POSIX should the read-write VFS expose?

**Options:**
- **A. Minimal** — `write`, `create`, `unlink`, `mkdir`, `truncate`.
- **B. POSIX core** — A + `rename`, `symlink`, `hardlink`, `chmod`.
- **C. Full** — B + `xattr`, `ioctl`, `flock`, `mmap`-adjacent semantics.

| Merit | A | B | C |
|---|---|---|---|
| Kernel minimalism (§2.3) | 5 | 3 | 1 |
| bash / unix-tool compat | 2 | 4 | 5 |
| Implementation cost | 5 | 3 | 1 |
| Noun-extension simplicity | 5 | 3 | 1 |
| **Total** | **17** | **13** | **8** |

**Verdict:** ✅ **A** — ship minimal; add rename/symlink only when `files://` or a
noun actually needs them.

#### F2 — Inode allocation (binds Phase 0)

**Question:** How are inode numbers assigned across namespaces?

**Options:**
- **A. Global sequential** — one kernel counter, namespaces take the next range.
- **B. Per-namespace ranges** — fixed high bits, low bits per namespace.
- **C. Path-derived stable** — hash the (namespace, path) pair to a stable ino.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 3 | 1 |
| ino stability across reload | 1 | 3 | 5 |
| Collision risk | 3 | 4 | 2 |
| FUSE cache friendliness | 3 | 4 | 5 |
| **Total** | **12** | **14** | **13** |

**Verdict:** ✅ **B** — ranges give stable, collision-free inos without hashing;
matches the current sequential-register design with a namespace prefix.

#### F3 — Path & name representation (binds Phase 0)

**Question:** How does the kernel represent paths/names internally?

**Options:**
- **A. `OsStr`** — platform-native, non-UTF8-safe (current).
- **B. `String`** — UTF-8 only, lossy at the edges.
- **C. URI-first** — `ResourceUri` canonical everywhere; OsStr only at the mount.

| Merit | A | B | C |
|---|---|---|---|
| Non-UTF8 file support | 5 | 1 | 3 |
| Noun/WIT simplicity | 2 | 5 | 4 |
| Uniform addressing | 1 | 3 | 5 |
| Interop with FUSE | 5 | 4 | 3 |
| **Total** | **13** | **13** | **15** |

**Verdict:** ⚖️ **C for the kernel API, A at the mount boundary** — URI canonical
inside, OsStr only where FUSE requires it. Decide after the URI type exists.

#### F4 — File-handle model (binds Phase 0)

**Question:** Does the VFS track per-open state, or is every read/write stateless?

**Options:**
- **A. Stateless** — reads/writes carry ino+offset, no open() state.
- **B. Per-open handles** — `open` allocates a handle with flags/position.

| Merit | A | B |
|---|---|---|
| Simplicity | 5 | 3 |
| POSIX correctness (O_APPEND, locks) | 2 | 5 |
| Anchor/edit verbs need | 4 | 4 |
| Noun-extension surface | 5 | 3 |
| **Total** | **16** | **15** |

**Verdict:** ✅ **A now, B later** — stateless covers verbs and reads; add handles
only if `files://` locking or `bash://` PTY semantics demand it.

#### F5 — Symlinks (binds Phase 0)

**Question:** Should the VFS support symbolic links?

**Options:**
- **A. No symlinks** — nouns are trees only.
- **B. Symlinks surfaced** — `files://` exposes OS symlinks read-only.
- **C. Full symlink support** — create/readlink across namespaces.

| Merit | A | B | C |
|---|---|---|---|
| Kernel minimalism | 5 | 3 | 1 |
| Real-repo fidelity | 2 | 4 | 5 |
| Complexity (cycles, escapes) | 5 | 3 | 1 |
| **Total** | **12** | **10** | **7** |

**Verdict:** ✅ **B** — expose what `files://` already has, but do not build
symlink *creation* into the kernel; keep nouns tree-only.

#### F6 — Userspace caching & TTL (binds Phase 0)

**Question:** What caching does the bridge layer do beyond the kernel page cache?

**Options:**
- **A. None** — pass every op through to the VFS.
- **B. Attribute TTL** — cache attrs for a short TTL (current 1s).
- **C. Content cache** — cache file bodies + invalidation on write.

| Merit | A | B | C |
|---|---|---|---|
| Correctness (freshness) | 5 | 4 | 2 |
| Latency on hot paths | 2 | 3 | 5 |
| Complexity | 5 | 4 | 1 |
| **Total** | **12** | **11** | **8** |

**Verdict:** ✅ **B** — keep attribute TTL; let noun extensions own their content
freshness. Content cache only as a Phase 7 optimization with measurements.

#### F7 — Namespace registration (binds Phase 0)

**Question:** How do namespaces get registered with the kernel?

**Options:**
- **A. Compile-time** — namespaces are wired in code.
- **B. Runtime dynamic** — `register()` at startup/load.
- **C. Both** — kernel-resident ones compile-time; extensions dynamic.

| Merit | A | B | C |
|---|---|---|---|
| Extension story (§2.2) | 1 | 5 | 5 |
| Testability | 3 | 5 | 5 |
| Simplicity | 5 | 3 | 4 |
| **Total** | **9** | **13** | **14** |

**Verdict:** ✅ **C** — kernel namespaces fixed, `resources://` mounts are dynamic.
Current `Kernel::register` already supports this.

#### F8 — Mount topology (binds Phase 0)

**Question:** One global mount, or per-session mounts?

**Options:**
- **A. Single global mount** — one FUSE/WinFsp mount for the whole kernel.
- **B. Per-session mounts** — each session mounts its own view.
- **C. Nested** — one root with per-session subtrees.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 3 | 2 |
| Multi-session isolation | 2 | 5 | 4 |
| `bash://` interop | 3 | 3 | 4 |
| **Total** | **10** | **11** | **10** |

**Verdict:** ⚖️ **A for v1, C as the durable-session model** — decide at Phase 3
when `session://` and the session-host land.

#### F9 — Case sensitivity & Windows normalization (binds Phase 0)

**Question:** How are names normalized on case-insensitive hosts?

**Options:**
- **A. Case-sensitive everywhere** — treat names as exact bytes.
- **B. Case-insensitive mode** — normalize on Windows, sensitive on unix.
- **C. Configurable per-namespace** — each noun declares its policy.

| Merit | A | B | C |
|---|---|---|---|
| Cross-platform consistency | 3 | 4 | 5 |
| Simplicity | 5 | 3 | 2 |
| Real-OS fidelity | 2 | 5 | 5 |
| **Total** | **10** | **12** | **12** |

**Verdict:** ⚖️ **B** — normalize at the Windows bridge, keep the kernel
exact; revisit C if a noun needs deviation.

#### F10 — Error-model richness (binds Phase 0)

**Question:** How detailed should `VfsError` be?

**Options:**
- **A. Minimal enum** — NotFound/NotDir/IsDir/Io (current).
- **B. Errno-aligned** — map to a broad POSIX-like set.
- **C. Structured + detail** — category enum plus a message payload.

| Merit | A | B | C |
|---|---|---|---|
| Kernel minimalism | 5 | 3 | 2 |
| Debuggability | 1 | 3 | 5 |
| Noun/WIT mapping | 3 | 4 | 4 |
| **Total** | **9** | **10** | **11** |

**Verdict:** ✅ **C, but narrow** — keep the enum small and add an optional
context string only; do not model every errno.

#### F11 — Write-atomicity home (binds Phase 2)

**Question:** Where do multi-edit atomicity and sibling-edit coalescing live?

**Options:**
- **A. Kernel transactions** — the VFS exposes begin/commit.
- **B. Verb layer only** — verbs snapshot, validate, commit.
- **C. Both** — kernel provides a primitive, verbs compose it.

| Merit | A | B | C |
|---|---|---|---|
| Kernel minimalism | 1 | 5 | 3 |
| Correctness guarantees | 5 | 3 | 5 |
| Implementation cost | 1 | 4 | 2 |
| **Total** | **7** | **12** | **10** |

**Verdict:** ✅ **B** — matches §2.4 (byte-oriented kernel, verb-layer anchoring).
Revisit C only if `files://` concurrent-write guarantees become a hard requirement.

#### F12 — ino stability across reload (binds Phase 3)

**Question:** Must inos survive a process restart?

**Options:**
- **A. Ephemeral** — inos reassigned each boot.
- **B. Session-stable** — stable within a session, not across.
- **C. Durable** — persisted and stable forever.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 3 | 1 |
| Durable-session replay | 1 | 4 | 5 |
| Anchor addressing needs | 3 | 5 | 5 |
| **Total** | **9** | **12** | **11** |

**Verdict:** ⚖️ **B** — anchors are content/occurrence-based, so durability is
rarely needed; decide at Phase 3 against the session-store design.

#### F13 — Metadata / xattr channel (binds Phase 6)

**Question:** How do anchors/git-status/extra metadata ride on files?

**Options:**
- **A. None** — metadata lives in the verb result, not the fs.
- **B. xattr** — expose metadata as extended attributes.
- **C. Side-channel** — a parallel `://meta` namespace.

| Merit | A | B | C |
|---|---|---|---|
| Kernel minimalism | 5 | 2 | 3 |
| Tool/bridge reuse | 2 | 5 | 4 |
| Standard-ness | 1 | 5 | 2 |
| **Total** | **8** | **12** | **9** |

**Verdict:** ✅ **A for now** — the authoritative plane (§6.3) carries metadata;
revisit xattr at Phase 6 if `bash://` tooling needs it.

#### F14 — Concurrent writers (binds Phase 3)

**Question:** Policy when two writers touch one file.

**Options:**
- **A. Last-write-wins** — plain POSIX.
- **B. Lock + serialize** — flock semantics.
- **C. Conflict-error** — reject the second writer.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 3 | 3 |
| Safety for edits | 2 | 4 | 5 |
| POSIX fidelity | 5 | 5 | 1 |
| **Total** | **12** | **12** | **9** |

**Verdict:** ⚖️ **A at the VFS, C at the verb layer** — the kernel stays POSIX;
verbs reject stale-snapshot writes.

### 13.2 WASM & extensions

#### F15 — Guest language support (binds Phase 1)

**Question:** Which languages must be able to author extensions?

**Options:**
- **A. Rust-only** — cargo-component + wit-bindgen.
- **B. Rust + Python + JS** — add componentize-py and jco.
- **C. Broad** — B + Go/TinyGo, C, Zig.

| Merit | A | B | C |
|---|---|---|---|
| Delivery cost | 5 | 3 | 1 |
| Extension author reach | 2 | 4 | 5 |
| Host-side testing surface | 5 | 3 | 1 |
| **Total** | **12** | **10** | **7** |

**Verdict:** ✅ **A now, B as a documented fast-follow** — the host is language
agnostic (WIT), so nothing forecloses B/C; just do not block on them.

#### F16 — Extension discovery (binds Phase 1)

**Question:** How does the loader find extension components?

**Options:**
- **A. Directory scan** — look for `.wasm`/`.wit` in a known tree.
- **B. Manifest index** — an explicit registry file lists components.
- **C. Registry service** — a remote/local registry resolves names.

| Merit | A | B | C |
|---|---|---|---|
| Zero-config | 5 | 3 | 1 |
| Determinism / reproducibility | 2 | 5 | 4 |
| Future marketplace (§F102) | 1 | 3 | 5 |
| **Total** | **8** | **11** | **10** |

**Verdict:** ✅ **B** — a manifest index (or `tools://` listing) keeps activation
deterministic; directory scan can be a convenience fallback.

#### F17 — Extension packaging (binds Phase 1)

**Question:** What is the unit of an installed extension?

**Options:**
- **A. Bare `.wasm`** — single compiled component.
- **B. WIT package** — `.wit` + source + compiled artifacts.
- **C. Archive** — a zip/tar with source, WIT, and wasm.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 3 | 2 |
| Self-inspect/edit (§6.4) | 1 | 5 | 5 |
| Distribution | 3 | 3 | 5 |
| **Total** | **9** | **11** | **12** |

**Verdict:** ⚖️ **B for tools:// (needs source), C for shipped extensions** —
source-editability is a locked goal, so bare wasm is insufficient.

#### F18 — Tool-source compilation (binds Phase 2)

**Question:** Who compiles `tools://` source into wasm?

**Options:**
- **A. Host-managed build** — invoke cargo-component/rustc in a child process.
- **B. In-guest compile** — a builder extension does it.
- **C. Precompiled only** — ship wasm, edit via a separate build step.

| Merit | A | B | C |
|---|---|---|---|
| Self-modifiability (§6.4) | 4 | 5 | 1 |
| Control / determinism | 5 | 3 | 5 |
| Complexity | 3 | 2 | 5 |
| **Total** | **12** | **10** | **11** |

**Verdict:** ✅ **A** — host-managed cargo-component builds (the `artist-component`
rebuild) are the pragmatic path; B is a later elegance.

#### F19 — Runaway-extension limits (binds Phase 1)

**Question:** How do we stop a hung/looping extension?

**Options:**
- **A. None** — full trust, let it run.
- **B. Fuel + epoch interruption** — wasmtime fuel/epoch to preempt.
- **C. Hard resource limits** — memory caps, timeouts, kill.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 3 | 2 |
| Liveness of the harness | 1 | 5 | 5 |
| Alignment with full-trust (§2.8) | 5 | 4 | 3 |
| **Total** | **11** | **12** | **10** |

**Verdict:** ✅ **B** — fuel/epoch is not sandboxing, it is liveness protection;
consistent with full trust.

#### F20 — Verb result shape (binds Phase 2)

**Question:** How does a batch return per-item errors?

**Options:**
- **A. `list<result<response, error>>`** — one result per request.
- **B. `list<response>` + out-of-band error index**.
- **C. Single result** — all-or-nothing.

| Merit | A | B | C |
|---|---|---|---|
| One-for-one / ordered (§2.5) | 5 | 3 | 1 |
| WIT simplicity | 4 | 4 | 5 |
| Partial-success fidelity | 5 | 3 | 1 |
| **Total** | **14** | **10** | **7** |

**Verdict:** ✅ **A** — already locked; restated here for completeness.

#### F21 — Noun write-contract breadth (binds Phase 1)

**Question:** How much of the write surface must a noun WIT expose?

**Options:**
- **A. write/create/unlink/mkdir** — minimal parity with F1-A.
- **B. + rename/truncate** — closer to POSIX.
- **C. Full** — B + symlink + xattr.

| Merit | A | B | C |
|---|---|---|---|
| Noun authoring burden | 5 | 3 | 1 |
| bash/tool compat | 2 | 4 | 5 |
| **Total** | **7** | **7** | **6** |

**Verdict:** ✅ **A, plus optional rename** — keep noun authors' lives easy; a
noun that cannot rename just returns Unsupported.

#### F22 — Event delivery model (binds Phase 1)

**Question:** Push, pull, or both for the event contract?

**Options:**
- **A. Push** — broker calls `subscriber.handle-event` (current).
- **B. Pull** — subscribers poll a queue.
- **C. Both** — push for live, pull for replay.

| Merit | A | B | C |
|---|---|---|---|
| Latency | 5 | 2 | 4 |
| Backpressure | 2 | 5 | 5 |
| Replay/durability | 2 | 4 | 5 |
| **Total** | **9** | **11** | **14** |

**Verdict:** ✅ **C** — push for live fan-out (current), add a pull/replay API for
durable consumers (session-host, subagents) at Phase 3.

#### F23 — Event durability (binds Phase 3)

**Question:** Is the event stream persisted?

**Options:**
- **A. Volatile** — in-memory broker only.
- **B. Append-log persisted** — replayable, best-effort delivery.
- **C. Transactional** — exactly-once, durable subscriptions.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 3 | 1 |
| Replay/recovery | 1 | 5 | 5 |
| Cost | 5 | 4 | 2 |
| **Total** | **11** | **12** | **8** |

**Verdict:** ✅ **B** — persist an append log behind `events://`; exactly-once is
overkill for a local agent harness.

#### F24 — Shared WIT types package (binds Phase 2)

**Question:** When to factor a shared WIT types crate?

**Options:**
- **A. Now** — `artist:types` with uri/verb-id/anchor/error.
- **B. When duplicated** — first real duplicate triggers it.
- **C. Never** — each contract owns its types.

| Merit | A | B | C |
|---|---|---|---|
| Coherence | 5 | 4 | 2 |
| Avoid premature abstraction | 2 | 5 | 3 |
| **Total** | **7** | **9** | **5** |

**Verdict:** ✅ **B** — already the scaffold's stance; the verb WITs in Phase 2 are
the likely trigger point.

#### F25 — Hot-reload eventual design (binds post-Phase 7)

**Question:** When hot-reload eventually lands, what is its shape?

**Options:**
- **A. Generation-pin in-place** — new calls see gen N+1, in-flight stay N.
- **B. Snapshot + swap** — freeze, replace, roll forward.
- **C. No reload** — restart the harness.

| Merit | A | B | C |
|---|---|---|---|
| Zero-downtime edit (§6.4) | 5 | 4 | 1 |
| Complexity | 2 | 3 | 5 |
| **Total** | **7** | **7** | **6** |

**Verdict:** ✅ **A** — the generation-pin invariant (§2.5) already assumes this.
Explicitly deferred; documented here for when it is picked up.

#### F26 — Classification source (binds Phase 1)

**Question:** How is an extension's class decided?

**Options:**
- **A. WIT-inspection only** — exported interface names (current).
- **B. WIT + metadata** — a sidecar declares intent/hints.
- **C. Manifest only** — explicit class field.

| Merit | A | B | C |
|---|---|---|---|
| No-manifest simplicity | 5 | 3 | 1 |
| Intent capture (beyond WIT) | 3 | 5 | 4 |
| **Total** | **8** | **8** | **5** |

**Verdict:** ✅ **A** — already locked; B only if a component needs hints WIT cannot
carry (e.g. display name, ordering).

#### F27 — Extension instantiation lifecycle (binds Phase 1)

**Question:** How many live instances per extension?

**Options:**
- **A. One per extension** — load-on-demand, long-lived (current).
- **B. Pool** — several warm instances.
- **C. Per-call instantiate** — fresh instance per invocation.

| Merit | A | B | C |
|---|---|---|---|
| Stateful nouns/services | 5 | 2 | 1 |
| Isolation between calls | 3 | 4 | 5 |
| Throughput | 3 | 5 | 1 |
| **Total** | **11** | **11** | **7** |

**Verdict:** ✅ **A** — nouns/services are inherently stateful; revisit a pool only
for hot stateless verbs with measurements.

#### F28 — `wasi:filesystem` host depth (binds Phase 1)

**Question:** How much of `wasi:filesystem` should the hand-rolled host implement?

**Options:**
- **A. Read + write + readdir + stat** — enough for extension needs.
- **B. + rename/remove/create-dir** — full tree mutation.
- **C. Full POSIX** — everything in the WIT, incl. symlinks/times.

| Merit | A | B | C |
|---|---|---|---|
| Effort | 5 | 3 | 1 |
| Guest capability | 3 | 5 | 5 |
| Alignment with F1 | 4 | 5 | 2 |
| **Total** | **12** | **13** | **8** |

**Verdict:** ✅ **B** — mirrors F1 (minimal + rename); do not chase full POSIX.

### 13.3 Verbs & tools

#### F29 — Verb set finalization (binds Phase 2)

**Question:** Which verbs ship in the default surface?

**Options:**
- **A. Core 10** — read/write/edit/insert/find/grep/run/poll/abort/delete.
- **B. Core + harness** — A + `bash`, `ask`, `web`, `todo`, `glob`.
- **C. Minimal 4** — read/write/edit/run only.

| Merit | A | B | C |
|---|---|---|---|
| Capability coverage | 4 | 5 | 2 |
| Surface discipline (§2.5) | 5 | 3 | 5 |
| Ship velocity | 4 | 2 | 5 |
| **Total** | **13** | **10** | **12** |

**Verdict:** ✅ **A first, then B as extensions** — ship the core 10, add harness
verbs as *their own* tool packages so the surface stays derived, not hardcoded.

#### F30 — find/grep engine (binds Phase 2)

**Question:** What backs `find` and `grep`?

**Options:**
- **A. fff in-process** — the vendored search engine.
- **B. ripgrep subprocess** — shell out to `rg`.
- **C. Hybrid** — fff warm path, rg fallback.

| Merit | A | B | C |
|---|---|---|---|
| Warm repeated-search latency | 5 | 1 | 4 |
| Simplicity / no vendoring | 2 | 5 | 3 |
| Git-aware / frecency / fuzzy | 5 | 2 | 4 |
| **Total** | **12** | **8** | **11** |

**Verdict:** ✅ **A** — fff is already vendored and patched for Artist; it is the
whole point of the in-process search decision.

#### F31 — Search service home (binds Phase 2)

**Question:** Where does the fff search engine live?

**Options:**
- **A. Kernel service** — `artist_kernel::SearchService`.
- **B. `tools://` verb** — a find/grep verb backed by fff.
- **C. Noun** — a `search://` resource extension.

| Merit | A | B | C |
|---|---|---|---|
| Kernel minimalism (§2.3) | 1 | 5 | 4 |
| Verb semantics (find/grep are verbs) | 3 | 5 | 2 |
| Reuse by other extensions | 5 | 3 | 4 |
| **Total** | **9** | **13** | **10** |

**Verdict:** ✅ **B** — verbs, not kernel (§2.3: tools are verbs). Re-wire the
orphaned fff patches behind the verb layer; expose a noun wrapper only if
another extension must search programmatically.

#### F32 — Anchor model (binds Phase 2)

**Question:** What is an anchor, exactly?

**Options:**
- **A. Occurrence-count** — `#3` = 3rd match of a pattern.
- **B. Mnemonic/content-hash** — hashline-style stable id per line/region.
- **C. AST-structural** — anchor onto a declaration/statement via `artist-ast`.
- **D. Hybrid** — structural + content-hash fallback.

| Merit | A | B | C | D |
|---|---|---|---|---|
| Stability under edit | 2 | 4 | 5 | 5 |
| Model ergonomics | 4 | 3 | 3 | 4 |
| Implementation cost | 5 | 3 | 2 | 2 |
| Language coverage | 5 | 5 | 3 | 4 |
| **Total** | **16** | **15** | **13** | **15** |

**Verdict:** ⚖️ **D, with B as the always-available base** — structural anchors are
best where `artist-ast` has an adapter; content-hash covers the rest. This is the
single most important verb-layer decision; mine hashline-tools before locking.

#### F33 — Anchor stability under edit (binds Phase 2)

**Question:** How do anchors behave after an edit moves lines around?

**Options:**
- **A. Content-addressed** — anchor follows the content.
- **B. Occurrence-based** — anchor is positional, may drift.
- **C. Best-effort** — resolve against the snapshot at call time.

| Merit | A | B | C |
|---|---|---|---|
| Determinism | 5 | 3 | 4 |
| Model predictability | 4 | 3 | 3 |
| Cost | 2 | 5 | 4 |
| **Total** | **11** | **11** | **11** |

**Verdict:** ⚖️ **A for durable addressing, C for the edit verb's snapshot model**
— fold into F32.

#### F34 — Diff representation (binds Phase 2)

**Question:** What diff shape do `edit`/`insert` return?

**Options:**
- **A. Unified** — standard `@@` hunks.
- **B. Mnemonic-anchored** — hunks keyed to anchors, not line numbers.
- **C. Both** — anchored for the model, unified for tooling.

| Merit | A | B | C |
|---|---|---|---|
| Tooling compat | 5 | 2 | 5 |
| Anchor consistency (§2.4) | 1 | 5 | 5 |
| Cost | 5 | 3 | 2 |
| **Total** | **11** | **10** | **12** |

**Verdict:** ✅ **C** — the authoritative plane carries an anchored diff; `stdobs`
renders it with an anchor gutter (mine the Gortnite diff/tool_ui work).

#### F35 — run execution (binds Phase 3)

**Question:** How does `run` actually execute?

**Options:**
- **A. Direct exec** — `execvp` with argv, pipes for stdio.
- **B. Shell** — run through `sh -c`/`cmd /c`.
- **C. PTY** — pseudo-terminal for full interactive fidelity.

| Merit | A | B | C |
|---|---|---|---|
| Determinism (argv-only §2.5) | 5 | 2 | 3 |
| Interactive/bash compat | 1 | 3 | 5 |
| Cost | 5 | 4 | 2 |
| **Total** | **11** | **9** | **10** |

**Verdict:** ✅ **A for `run`, C for `bash://`** — `run` stays argv-only (locked);
interactive shells are the `bash://` noun's job, not `run`'s.

#### F36 — poll matching (binds Phase 3)

**Question:** What is `poll`'s stop condition?

**Options:**
- **A. Regex** — pattern over accumulated text.
- **B. Literal substring** — exact match.
- **C. Both** — auto-detect or explicit mode.

| Merit | A | B | C |
|---|---|---|---|
| Power | 5 | 2 | 4 |
| Model simplicity | 3 | 5 | 4 |
| Cost | 4 | 5 | 3 |
| **Total** | **12** | **12** | **11** |

**Verdict:** ✅ **C** — a `match` string that is regex by default, with an explicit
literal escape; keep the accumulated-text semantics from the prior spec.

#### F37 — write semantics (binds Phase 2)

**Question:** What does `write` mean?

**Options:**
- **A. Replace only** — content is the new file.
- **B. Replace + append mode** — an explicit append flag.
- **C. Replace + modes** — append/prepend/truncate flags.

| Merit | A | B | C |
|---|---|---|---|
| Homogeneity (§2.5) | 5 | 3 | 1 |
| Capability | 2 | 4 | 5 |
| **Total** | **7** | **7** | **6** |

**Verdict:** ✅ **A** — exact replace, no implicit newline, no modes (locked).
Appending is `insert(bottom)` / writing to an append-only noun like stdin/inbox.

#### F38 — delete semantics (binds Phase 2)

**Question:** How does `delete` handle recursion?

**Options:**
- **A. Single resource only** — recursion is composition.
- **B. Recursive flag** — `recursive: bool`.

| Merit | A | B |
|---|---|---|
| Homogeneity (§2.5) | 5 | 2 |
| Model ergonomics | 3 | 5 |
| **Total** | **8** | **7** |

**Verdict:** ✅ **A** — one resource per call; recursive deletion is the model
composing multiple calls (locked).

#### F39 — Tool-schema derivation (binds Phase 2)

**Question:** Where do model-facing tool schemas come from?

**Options:**
- **A. WIT-derived** — generate from the typed contract.
- **B. Hand-authored** — write schemas per tool.
- **C. Both** — WIT-derived base, hand-authored overrides.

| Merit | A | B | C |
|---|---|---|---|
| Single source of truth | 5 | 2 | 4 |
| Expressiveness | 3 | 5 | 5 |
| Cost | 3 | 5 | 2 |
| **Total** | **11** | **12** | **11** |

**Verdict:** ✅ **A with C's escape hatch** — derive from WIT so schema cannot drift
from the ABI; allow a hand-written description/example override only.

#### F40 — stdobs location (binds Phase 2)

**Question:** Where does the `stdobs` observer live?

**Options:**
- **A. In-component export** — `observe: func(response) -> string` in the same
  component.
- **B. Adjacent observer component** — a sibling component in the same package.
- **C. Host fallback** — generic deterministic renderer.

| Merit | A | B | C |
|---|---|---|---|
| Package ownership (§6.3) | 5 | 4 | 1 |
| Simplicity | 5 | 3 | 5 |
| WIT ergonomics | 3 | 4 | 5 |
| **Total** | **13** | **11** | **11** |

**Verdict:** ✅ **A, with C as activation-time fallback** — in-component observers,
a generic fallback for extensions that omit them.

#### F41 — Batching scope (binds Phase 4)

**Question:** What is the batching boundary?

**Options:**
- **A. Same-turn only** — one assistant turn's sibling calls.
- **B. Cross-turn coalescing** — merge across turns opportunistically.
- **C. Streaming partial** — start batches as calls arrive.

| Merit | A | B | C |
|---|---|---|---|
| Determinism (§2.5) | 5 | 3 | 2 |
| Latency | 3 | 2 | 5 |
| Correctness (snapshot semantics) | 5 | 3 | 2 |
| **Total** | **13** | **8** | **9** |

**Verdict:** ✅ **A** — same-turn is the locked semantic boundary; C is a latency
optimization to consider only after correctness is proven.

#### F42 — Tolerant-parse fallback policy (binds Phase 2)

**Question:** When normalization cannot repair a malformed call, what happens?

**Options:**
- **A. Strict-fail** — return an item error to the model.
- **B. Best-effort repair** — guess the closest valid shape.
- **C. Ask the model** — emit a clarification turn.

| Merit | A | B | C |
|---|---|---|---|
| Safety (no wrong op) | 5 | 2 | 4 |
| Cost / turn economy | 4 | 5 | 1 |
| §2.5 tolerance principle | 3 | 4 | 3 |
| **Total** | **12** | **11** | **8** |

**Verdict:** ✅ **A** — normalize the unambiguous, strictly validate the result;
never guess between materially different operations. C only for user-facing ask.

### 13.4 Agent loop & runtime

#### F43 — Agent runtime foundation (binds Phase 4)

**Question:** Rig hand-driven, Rig batteries-included, or hand-rolled? (Restates §4.2.)

**Options:**
- **A. Rig, hand-drive `AgentRun`** — own the `CallTools` batch boundary.
- **B. Rig `AgentRunner`** — batteries-included.
- **C. Hand-roll** — drop Rig.

| Merit | A | B | C |
|---|---|---|---|
| Batch invariant (§2.5) | 5 | 2 | 5 |
| Reuse provider/contract surface | 5 | 5 | 1 |
| Hook/steering control | 4 | 3 | 5 |
| Persistence/resume | 5 | 4 | 5 |
| Implementation cost | 3 | 5 | 1 |
| **Total** | **22** | **19** | **17** |

**Verdict:** ✅ **A** — the only option satisfying the batch invariant while
reusing Rig. **Open** until the owner picks (see §4.2).

#### F44 — Subagent model (binds Phase 4)

**Question:** How are sub-agents represented?

**Options:**
- **A. Nested `AgentRun`** — each subagent drives its own loop.
- **B. Events-service** — a long-lived service that subscribes + exports verbs.
- **C. Both** — services drive their own nested runs.

| Merit | A | B | C |
|---|---|---|---|
| Isolation | 5 | 4 | 5 |
| Addressability (noun + service) | 2 | 5 | 5 |
| Cost | 4 | 3 | 2 |
| **Total** | **11** | **12** | **12** |

**Verdict:** ⚖️ **C** — the events contract already defines a sub-agent as a
participant that can be driven; decide at Phase 4 against F43.

#### F45 — Memory & context model (binds Phase 6)

**Question:** What feeds the model's context beyond the conversation?

**Options:**
- **A. History only** — sliding conversation window.
- **B. RAG index** — retrieve relevant files/chunks on demand.
- **C. Semantic store** — a memory of assertions/facts.
- **D. Hybrid** — B + C.

| Merit | A | B | C | D |
|---|---|---|---|---|
| Simplicity | 5 | 3 | 2 | 1 |
| Long-task recall | 1 | 4 | 4 | 5 |
| Cost | 5 | 3 | 2 | 1 |
| **Total** | **11** | **10** | **8** | **7** |

**Verdict:** ⚖️ **B first, D at Phase 6** — mine `artist-memory` + the
`artist-ast` ranking trait; a semantic store is a Wave-2 feature, not core.

#### F46 — Context management (binds Phase 4)

**Question:** How is the growing context kept within budget?

**Options:**
- **A. Sliding window** — drop oldest turns.
- **B. Summarize** — compress older turns via the model.
- **C. Squeeze/TOON** — lossless token compression.
- **D. Hybrid** — compress text, summarize semantics.

| Merit | A | B | C | D |
|---|---|---|---|---|
| Simplicity | 5 | 3 | 3 | 2 |
| Recall fidelity | 2 | 3 | 5 | 4 |
| Cost (tokens) | 5 | 3 | 5 | 4 |
| **Total** | **12** | **9** | **13** | **10** |

**Verdict:** ✅ **C as the baseline** — `artist-ast` already ships `squeeze`
(reversible log/text compression); layer summarize only where lossless runs out.

#### F47 — Steering & capture (binds Phase 4)

**Question:** How do hooks, policies, and recording attach to the loop?

**Options:**
- **A. Hooks** — Rig `AgentHook` in-process callbacks.
- **B. Events** — `events://` subscriptions.
- **C. Both** — in-process hooks mirrored to events.

| Merit | A | B | C |
|---|---|---|---|
| Latency (vetoes) | 5 | 3 | 5 |
| Extensibility (§2.2) | 2 | 5 | 5 |
| Cost | 5 | 3 | 2 |
| **Total** | **12** | **11** | **12** |

**Verdict:** ✅ **C** — the §6.5 hooks→events bridge; synchronous in-process hooks
for vetoes, events for observers/services.

#### F48 — Thinking / scratchpad (binds Phase 4)

**Question:** How does the model keep long-running state?

**Options:**
- **A. Hidden chain-of-thought** — provider thinking blocks.
- **B. Explicit todo** — a `todo` tool/state file.
- **C. Both** — todo plus thinking.

| Merit | A | B | C |
|---|---|---|---|
| Transparency/steerability | 2 | 5 | 5 |
| Provider dependence | 2 | 5 | 3 |
| **Total** | **4** | **10** | **8** |

**Verdict:** ✅ **B** — mine the Gortnite `todo`/`thinking` work; an explicit,
addressable todo keeps state visible and replayable. A only as a provider perk.

#### F49 — Hallucinated-tool handling (binds Phase 4)

**Question:** What happens when the model calls a nonexistent tool?

**Options:**
- **A. Drop + note** — discard the call, tell the model.
- **B. Repair** — fuzzy-match to the nearest real tool.
- **C. Ask** — surface a correction prompt.

| Merit | A | B | C |
|---|---|---|---|
| Safety (no wrong op) | 5 | 2 | 4 |
| Turn economy | 4 | 5 | 2 |
| **Total** | **9** | **7** | **6** |

**Verdict:** ✅ **A** — consistent with §2.5 (no guessing between operations); a
Gortnite fix already handled this.

#### F50 — Parallel-tool wrapper expansion (binds Phase 4)

**Question:** How do provider-injected parallel-tool wrappers get handled?

**Options:**
- **A. Expand** — unwrap into sibling calls then batch.
- **B. Reject** — treat the wrapper as an error.
- **C. Passthrough** — hand it to the tool as-is.

| Merit | A | B | C |
|---|---|---|---|
| Batch correctness (§2.5) | 5 | 2 | 1 |
| Robustness | 4 | 2 | 3 |
| **Total** | **9** | **4** | **4** |

**Verdict:** ✅ **A** — a Gortnite fix already expanded OpenAI's injected wrapper;
the batch executor must unwrap before grouping.

#### F51 — Cancellation & orphaned calls (binds Phase 4)

**Question:** What happens to in-flight tool calls on user abort?

**Options:**
- **A. Abort all** — cancel every pending call.
- **B. Abort targeted** — cancel only the named resource.
- **C. Leave** — let them finish, drop results.

| Merit | A | B | C |
|---|---|---|---|
| Responsiveness | 5 | 4 | 1 |
| Resource safety (processes) | 3 | 5 | 2 |
| **Total** | **8** | **9** | **3** |

**Verdict:** ✅ **B** — `abort` is resource-addressed (locked); the loop aborts the
turn's resources, with all-orphaned-calls as the fallback.

#### F52 — Multi-agent orchestration (binds Phase 6)

**Question:** How do agents coordinate beyond subagents?

**Options:**
- **A. Delegate** — a supervising agent spawns workers.
- **B. Messaging** — agents exchange messages via `session://` inboxes.
- **C. Both** — delegate + messaging + ask-outbox.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 3 | 1 |
| Capability | 2 | 4 | 5 |
| **Total** | **7** | **7** | **6** |

**Verdict:** ⚖️ **C, but late** — mine the Gortnite delegate/messaging/ask_outbox
work at Phase 6; not core to the first working loop.

#### F53 — Model routing (binds Phase 4)

**Question:** How is the model chosen per call/turn?

**Options:**
- **A. Static** — one configured model.
- **B. Dynamic selection** — route by task/turn via `ModelSelectionAction`.
- **C. Fallback chain** — try primary, fail over on error.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 2 | 3 |
| Cost/latency optimization | 2 | 5 | 3 |
| Robustness | 2 | 3 | 5 |
| **Total** | **9** | **10** | **11** |

**Verdict:** ✅ **A first, C soon after** — the agnostic provider layer (D4) makes
a fallback chain cheap; dynamic selection is a later optimization.

#### F54 — Tool concurrency within a turn (binds Phase 4)

**Question:** How are independent tool calls executed?

**Options:**
- **A. Serial** — one at a time.
- **B. Parallel per resource** — concurrent across distinct resources.
- **C. Parallel per group** — concurrent across verb groups.

| Merit | A | B | C |
|---|---|---|---|
| Determinism | 5 | 4 | 4 |
| Latency | 1 | 4 | 5 |
| Same-resource safety (§2.5) | 3 | 5 | 4 |
| **Total** | **9** | **13** | **13** |

**Verdict:** ✅ **C** — independent verb groups may run concurrently; same-resource
mutations coalesce. Results return in source order regardless.

### 13.5 Provider layer

#### F55 — Provider abstraction granularity (binds Phase 4)

**Question:** How thick is the `llm-provider` abstraction?

**Options:**
- **A. CompletionModel trait** — implement Rig's trait per backend.
- **B. Full client** — own client with transport/auth/streaming.
- **C. Thin facade** — wrap Rig providers behind one enum.

| Merit | A | B | C |
|---|---|---|---|
| Reuse (Rig providers) | 4 | 1 | 5 |
| Control over quirks | 3 | 5 | 2 |
| Cost | 4 | 1 | 5 |
| **Total** | **11** | **7** | **12** |

**Verdict:** ✅ **C** — the agnostic layer is a facade over Rig's providers + the
subscription-auth backend, with per-provider adapters where quirks leak (F58).

#### F56 — Subscription auth (binds Phase 4)

**Question:** How does the ChatGPT/Copilot subscription backend authenticate?

**Options:**
- **A. Rig native** — use Rig's auth-backed clients.
- **B. Web-mesh** — custom cookie/OAuth flow.
- **C. Device OAuth** — standard OAuth device flow.

| Merit | A | B | C |
|---|---|---|---|
| Maintenance | 5 | 1 | 4 |
| Control / fidelity | 3 | 5 | 3 |
| Novelty risk | 5 | 1 | 4 |
| **Total** | **13** | **7** | **11** |

**Verdict:** ✅ **A** — Rig already ships auth-backed clients; keep a web-mesh path
only as an escape hatch if Rig's is insufficient.

#### F57 — Credential storage (binds Phase 4)

**Question:** Where do provider credentials live?

**Options:**
- **A. OS keyring** — platform secret store.
- **B. Config file** — plaintext/encrypted file.
- **C. Environment** — env vars only.

| Merit | A | B | C |
|---|---|---|---|
| Security | 5 | 2 | 3 |
| Portability/simplicity | 3 | 5 | 5 |
| **Total** | **8** | **7** | **8** |

**Verdict:** ✅ **A with C fallback** — keyring for interactive use, env for
CI/headless; never persist plaintext secrets in the session log (see F83).

#### F58 — Streaming transport (binds Phase 4)

**Question:** How are streamed completions consumed?

**Options:**
- **A. SSE** — server-sent events.
- **B. Chunked JSON** — newline-delimited deltas.
- **C. Both** — per-provider transport behind one stream trait.

| Merit | A | B | C |
|---|---|---|---|
| Uniformity | 4 | 4 | 5 |
| Provider fidelity | 3 | 3 | 5 |
| Cost | 4 | 4 | 3 |
| **Total** | **11** | **11** | **13** |

**Verdict:** ✅ **C** — Rig already abstracts this; expose one stream type and let
providers map their transport.

#### F59 — Failover (binds Phase 4)

**Question:** What happens when the primary provider errors?

**Options:**
- **A. None** — surface the error.
- **B. Manual** — user retries/chooses.
- **C. Automatic** — fall back to the next configured backend.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 4 | 1 |
| Robustness | 1 | 2 | 5 |
| **Total** | **6** | **6** | **6** |

**Verdict:** ⚖️ **B first, C once the agnostic layer has ≥2 backends** — automatic
failover needs lineage/accounting (F60) to be safe.

#### F60 — Usage & lineage accounting (binds Phase 4)

**Question:** How are tokens/costs/identity tracked?

**Options:**
- **A. Per-call** — each response records tokens + provider.
- **B. Aggregate only** — session totals.
- **C. None** — skip accounting.

| Merit | A | B | C |
|---|---|---|---|
| Attribution/replay (§6.6) | 5 | 3 | 1 |
| Cost | 4 | 5 | 5 |
| **Total** | **9** | **8** | **6** |

**Verdict:** ✅ **A** — lineage is a locked requirement (which provider produced
which response); per-call is the natural granularity.

#### F61 — Local-model backend (binds Phase 4)

**Question:** Which local backend ships first?

**Options:**
- **A. Ollama** — simplest, managed runtime.
- **B. llamafile** — single-file, no daemon.
- **C. Pluggable** — trait, defer a concrete choice.

| Merit | A | B | C |
|---|---|---|---|
| UX | 5 | 4 | 2 |
| Simplicity | 4 | 5 | 3 |
| **Total** | **9** | **9** | **5** |

**Verdict:** ✅ **C with A first** — the agnostic trait is the deliverable; Ollama
is the easiest concrete impl to prove it.

### 13.6 Session & storage

#### F62 — Session format (binds Phase 3)

**Question:** What is the on-disk session representation?

**Options:**
- **A. JSONL append-log** — one JSON event per line.
- **B. TOON log** — token-efficient event log.
- **C. Bincode** — binary, fast, non-portable.
- **D. SQLite** — relational, queryable.

| Merit | A | B | C | D |
|---|---|---|---|---|
| Human-readable / debuggable | 5 | 5 | 1 | 3 |
| Token/disk efficiency | 3 | 5 | 5 | 3 |
| Queryability | 2 | 2 | 1 | 5 |
| Tooling compat | 5 | 3 | 1 | 4 |
| **Total** | **15** | **15** | **8** | **15** |

**Verdict:** ⚖️ **A for the append-log, D for derived indexes** — keep the
source-of-truth log human-readable; SQLite (or a derived index) for search.
TOON is tempting but adds coupling (D3) to the durability layer.

#### F63 — Session concurrency (binds Phase 3)

**Question:** How is concurrent access to a session arbitrated?

**Options:**
- **A. File lock (fs2)** — advisory lock on the log.
- **B. Daemon-owned** — only the session-host mutates.
- **C. Both** — lock now, daemon later.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 3 | 4 |
| Multi-client safety | 2 | 5 | 4 |
| **Total** | **7** | **8** | **8** |

**Verdict:** ✅ **C** — fs2 now (the dependency is already there), migrate to the
daemon when `artist-session-host` lands.

#### F64 — Session host (binds Phase 6)

**Question:** In-process, daemon, or both?

**Options:**
- **A. In-process** — the CLI owns sessions.
- **B. Daemon** — `artist-session-host` owns them.
- **C. Both** — in-process for single-user, daemon for shared/durable.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 2 | 3 |
| Cross-process resume (§6.5) | 2 | 5 | 5 |
| **Total** | **7** | **7** | **8** |

**Verdict:** ✅ **C** — mine `artist-session-host`; in-process first, daemon when
durable/resumable sessions across processes become the requirement.

#### F65 — Persistence granularity (binds Phase 3)

**Question:** How often is loop state persisted?

**Options:**
- **A. Per-turn** — after each model turn.
- **B. Per-call** — after each tool result.
- **C. Per-token** — continuously during streaming.

| Merit | A | B | C |
|---|---|---|---|
| Crash durability | 3 | 4 | 5 |
| Overhead | 5 | 4 | 2 |
| **Total** | **8** | **8** | **7** |

**Verdict:** ✅ **B** — persist per tool-result (the expensive, non-replayable
work); per-token is unnecessary for a local harness.

#### F66 — Replay model (binds Phase 3)

**Question:** How is a session replayed?

**Options:**
- **A. Event-sourced** — replay the append-log deterministically.
- **B. Snapshot + delta** — checkpoint + replay recent events.
- **C. Recorded transcript only** — replay the rendered text.

| Merit | A | B | C |
|---|---|---|---|
| Fidelity | 5 | 5 | 2 |
| Replay speed | 2 | 5 | 5 |
| Simplicity | 4 | 2 | 5 |
| **Total** | **11** | **12** | **12** |

**Verdict:** ⚖️ **A for correctness, B for speed** — start event-sourced, add
snapshots when replay gets slow (mine the Gortnite replay/compaction work).

#### F67 — Compaction (binds Phase 6)

**Question:** How is a long session's log bounded?

**Options:**
- **A. Lossless** — compact without dropping events.
- **B. Lossy summarize** — summarize old turns.
- **C. None** — the log grows unbounded.

| Merit | A | B | C |
|---|---|---|---|
| Fidelity | 5 | 2 | 5 |
| Size bound | 3 | 5 | 1 |
| **Total** | **8** | **7** | **6** |

**Verdict:** ✅ **A first** — lossless compaction (a Gortnite fix existed); pair
with F46 summarize for the *context*, not the *log*.

#### F68 — Multi-client session (binds Phase 6)

**Question:** Can multiple clients read/write one session?

**Options:**
- **A. Read-only many** — one writer, many readers.
- **B. One writer** — single active client.
- **C. Full multi-writer** — concurrent clients.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 4 | 5 | 1 |
| Capability (collab) | 3 | 2 | 5 |
| **Total** | **7** | **7** | **6** |

**Verdict:** ⚖️ **B first, A via the daemon** — collaborative editing is a canvas
concern (F77), not the session log's.

#### F69 — Attachments & inline images (binds Phase 4)

**Question:** How are images/attachments attached to turns?

**Options:**
- **A. Inline** — base64 in the message.
- **B. Filesystem ref** — a `file://`/attachment URI.
- **C. Both** — refs in the log, inline at the provider.

| Merit | A | B | C |
|---|---|---|---|
| Log size | 1 | 5 | 5 |
| Provider compat | 5 | 2 | 5 |
| **Total** | **6** | **7** | **10** |

**Verdict:** ✅ **C** — store attachments as resources (mine Gortnite
`attachments`/`inline_images`), inline only at the transport boundary.

### 13.7 CLI & UI

#### F70 — Primary UI (binds Phase 5)

**Question:** Which interface is primary?

**Options:**
- **A. TUI** — ratatui terminal app.
- **B. GUI canvas** — React-in-WASM (Phase 6).
- **C. Both** — TUI now, canvas later.

| Merit | A | B | C |
|---|---|---|---|
| Ship velocity | 5 | 2 | 5 |
| Richness | 3 | 5 | 5 |
| **Total** | **8** | **7** | **10** |

**Verdict:** ✅ **C** — TUI is the Phase 5 deliverable; canvas rides on the same
noun/service layer later.

#### F71 — Terminal stack (binds Phase 5)

**Question:** What rendering stack does the TUI use?

**Options:**
- **A. ratatui** — widgets, layout (current dep).
- **B. Raw crossterm** — full manual control.
- **C. ink/other** — alternative framework.

| Merit | A | B | C |
|---|---|---|---|
| Productivity | 5 | 2 | 3 |
| Control | 3 | 5 | 3 |
| Ecosystem | 5 | 4 | 2 |
| **Total** | **13** | **11** | **8** |

**Verdict:** ✅ **A** — already the dependency; mine the Gortnite `chat_ui`/
`tool_ui` work.

#### F72 — Syntax highlighting (binds Phase 5)

**Question:** How is code highlighted in diffs/previews?

**Options:**
- **A. syntect** — Sublime grammars (current dep).
- **B. tree-sitter** — structural highlight via `artist-ast` deps.
- **C. None** — plain text.

| Merit | A | B | C |
|---|---|---|---|
| Quality | 4 | 5 | 1 |
| Dependency weight | 3 | 3 | 5 |
| **Total** | **7** | **8** | **6** |

**Verdict:** ✅ **A** — syntect is already wired; tree-sitter is already present
via `artist-ast`, so B is a zero-new-dep upgrade later.

#### F73 — Inline images in the terminal (binds Phase 5)

**Question:** Render images inline?

**Options:**
- **A. Yes (sixel/kitty)** — inline graphics.
- **B. No** — open externally / ascii fallback.
- **C. Conditional** — inline where supported, fallback elsewhere.

| Merit | A | B | C |
|---|---|---|---|
| UX | 5 | 2 | 5 |
| Portability | 2 | 5 | 4 |
| **Total** | **7** | **7** | **9** |

**Verdict:** ✅ **C** — mine Gortnite `inline_images`; degrade gracefully.

#### F74 — Input model (binds Phase 5)

**Question:** How does the CLI capture input?

**Options:**
- **A. Line editor** — simple readline-style.
- **B. Full-TUI atoms** — Gortnite's input_atoms model.
- **C. Hybrid** — atoms with a raw line fallback.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 2 | 3 |
| Multi-line/paste fidelity | 2 | 5 | 5 |
| **Total** | **7** | **7** | **8** |

**Verdict:** ✅ **C** — mine the `input_atoms` + proptest invariants; keep a raw
fallback for dumb terminals.

#### F75 — Canvas runtime (binds Phase 6)

**Question:** What runs the React-in-WASM canvas? (Restates §11 Q5.)

**Options:**
- **A. wef/CEF** — vendored Chromium webview.
- **B. System webview** — OS webview (wry/webview2).
- **C. gpui** — zed-gpui vendored GPU UI.

| Merit | A | B | C |
|---|---|---|---|
| Control / no external dep | 5 | 3 | 4 |
| Bundle size | 1 | 4 | 3 |
| Maintenance | 1 | 5 | 2 |
| **Total** | **7** | **12** | **9** |

**Verdict:** ⚖️ **B if it suffices, A if CEF fidelity is required** — decide at the
canvas mining checkpoint; re-evaluate the wef vendoring cost.

#### F76 — Canvas ↔ agent bridge (binds Phase 6)

**Question:** How does the canvas talk to the agent?

**Options:**
- **A. Messages** — typed request/response over the bridge.
- **B. Shared state** — a synchronized state object.
- **C. Both** — messages + shared state for live sync.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 3 | 1 |
| Live collab (Gortnite) | 2 | 4 | 5 |
| **Total** | **7** | **7** | **6** |

**Verdict:** ⚖️ **C** — mine the Gortnite bridge + `state`/`peer` (two-people-one-
canvas) only if collaboration is in scope; else A.

#### F77 — Canvas multi-user sync (binds Phase 6)

**Question:** Is the canvas collaborative?

**Options:**
- **A. Single-user** — one agent, one canvas.
- **B. Collaborative** — shared, convergent state (Gortnite).

| Merit | A | B |
|---|---|---|
| Scope control | 5 | 2 |
| Ambition | 2 | 5 |
| **Total** | **7** | **7** |

**Verdict:** ⚖️ **A first** — collaboration is a Wave-3 stretch, not core.

#### F78 — Config format (binds Phase 5)

**Question:** What format do config/rules/profiles use?

**Options:**
- **A. TOML** — Rust-native, existing dep.
- **B. YAML** — flexible, human-friendly.
- **C. TOON** — consistent with the model boundary.

| Merit | A | B | C |
|---|---|---|---|
| Rust tooling | 5 | 3 | 2 |
| Human ergonomics | 4 | 5 | 3 |
| Consistency (D3) | 3 | 2 | 5 |
| **Total** | **12** | **10** | **10** |

**Verdict:** ✅ **A** — config is for humans/tooling, not the model; TOML is the
Rust default. TOON is for model-facing data, not config.

#### F79 — Profiles & rules storage (binds Phase 5)

**Question:** Where do profiles/rules live?

**Options:**
- **A. Files** — discovered from a config dir.
- **B. Registry** — an `artist-registry` store.
- **C. Both** — files are the source, registry the index.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 2 | 3 |
| Query/edit-ability | 2 | 4 | 5 |
| **Total** | **7** | **6** | **8** |

**Verdict:** ✅ **A** — files-as-source (mine Gortnite `profiles`/`rules`
discovery); a registry only if it earns its keep.

### 13.8 Distribution & ops

#### F80 — Release channel (binds Phase 7)

**Question:** How do users install Artist?

**Options:**
- **A. cargo install** — source build.
- **B. Prebuilt binaries** — GitHub releases + installers.
- **C. Package managers** — brew/winget/npm.

| Merit | A | B | C |
|---|---|---|---|
| Reach | 2 | 4 | 5 |
| Build determinism | 4 | 5 | 3 |
| Cost | 5 | 3 | 1 |
| **Total** | **11** | **12** | **9** |

**Verdict:** ✅ **B first, C as fast-follow** — prebuilt cross-platform binaries are
the right prod bar; brew/winget scripts already partially existed in Gortnite.

#### F81 — Packaging (binds Phase 7)

**Question:** What packaging is shipped?

**Options:**
- **A. Portable binary** — single executable.
- **B. systemd units** — daemon/service install.
- **C. App bundle** — platform installer (dmg/msi).

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 3 | 2 |
| Daemon/session-host ops | 2 | 5 | 3 |
| **Total** | **7** | **8** | **5** |

**Verdict:** ✅ **A + B** — portable binary plus the systemd units Gortnite already
sketched for the session-host/MCP daemons; C only for a GUI-first release.

#### F82 — Update mechanism (binds Phase 7)

**Question:** How does Artist update itself?

**Options:**
- **A. None** — user reinstalls.
- **B. Self-update** — built-in updater.
- **C. Package-manager** — defer to brew/winget.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 2 | 4 |
| Freshness | 1 | 5 | 3 |
| **Total** | **6** | **7** | **7** |

**Verdict:** ⚖️ **A first, C as the channel matures** — a self-updater is a
security/complexity cost that can wait.

#### F83 — Telemetry (binds Phase 7)

**Question:** Does Artist phone home?

**Options:**
- **A. None** — fully offline.
- **B. Opt-in** — explicit consent, minimal.
- **C. Always-on** — default metrics.

| Merit | A | B | C |
|---|---|---|---|
| Privacy / trust | 5 | 4 | 1 |
| Product signal | 1 | 4 | 5 |
| **Total** | **6** | **8** | **6** |

**Verdict:** ✅ **B** — opt-in, local-first; aligns with a full-trust, self-hosted
tool. Never telemetry by default.

#### F84 — Secrets redaction (binds Phase 4)

**Question:** Are secrets scrubbed from logs/sessions?

**Options:**
- **A. Yes** — redact known secret patterns.
- **B. No** — log everything.

| Merit | A | B |
|---|---|---|
| Security | 5 | 1 |
| Fidelity | 4 | 5 |
| **Total** | **9** | **6** |

**Verdict:** ✅ **A** — redact credential material from the session log and
`stdobs`; authoritative capture can keep a separately-secured copy.

#### F85 — CI provider (binds Phase 0)

**Question:** Where does CI run?

**Options:**
- **A. GitHub Actions** — standard, 3-OS matrix.
- **B. Self-hosted** — control + custom runners (FUSE/WinFsp).
- **C. Both** — Actions for unit, self-hosted for mount.

| Merit | A | B | C |
|---|---|---|---|
| Zero infra | 5 | 1 | 3 |
| Mount-test capability | 2 | 5 | 5 |
| **Total** | **7** | **6** | **8** |

**Verdict:** ✅ **C** — Actions for the always-on matrix, self-hosted only if the
mount backends cannot be satisfied in Actions (see §9).

#### F86 — Build matrix (binds Phase 0)

**Question:** Cross-compile, or build natively per OS?

**Options:**
- **A. Per-OS runners** — native builds.
- **B. Cross-compile** — one host, target triples.
- **C. Both** — native for tests, cross for release.

| Merit | A | B | C |
|---|---|---|---|
| Test fidelity | 5 | 2 | 5 |
| Release speed | 3 | 5 | 5 |
| Complexity | 5 | 2 | 2 |
| **Total** | **13** | **9** | **12** |

**Verdict:** ✅ **A for CI, C for releases** — native runners for green tests;
cross-compile (the `.cargo/config.toml` already sets a Windows linker) for release
artifacts.

#### F87 — Quality gates (binds Phase 7)

**Question:** What gates a merge?

**Options:**
- **A. Tests only** — unit + integration.
- **B. Tests + coverage** — a minimum coverage threshold.
- **C. Tests + fuzz + property** — the full §9 surface.

| Merit | A | B | C |
|---|---|---|---|
| Cheap / fast | 5 | 3 | 1 |
| Confidence | 2 | 4 | 5 |
| **Total** | **7** | **7** | **6** |

**Verdict:** ✅ **C for kernel/parser code, A for the rest** — property/fuzz the
high-risk invariants (§9); do not gate feature code on coverage lines.

#### F88 — Docs & release notes (binds Phase 7)

**Question:** How are docs maintained?

**Options:**
- **A. Manual md** — hand-written.
- **B. Generated API docs** — rustdoc + changelog tooling.
- **C. Both** — md for concepts, generated for API.

| Merit | A | B | C |
|---|---|---|---|
| Concept quality | 5 | 1 | 5 |
| API freshness | 2 | 5 | 5 |
| **Total** | **7** | **6** | **10** |

**Verdict:** ✅ **C** — keep the design docs (this file's style) hand-written;
generate API/changelog.

### 13.9 Future-scope subsystems

#### F89 — Computer-use OCR backend (binds Phase 6)

**Question:** What does computer-use use for text recognition?

**Options:**
- **A. rten** — Rust, on-device (Gortnite used it).
- **B. tesseract** — classic, broad language support.
- **C. Cloud** — external OCR API.

| Merit | A | B | C |
|---|---|---|---|
| On-device / offline | 5 | 5 | 1 |
| Rust-native | 5 | 2 | 1 |
| Accuracy / languages | 3 | 4 | 5 |
| **Total** | **13** | **11** | **7** |

**Verdict:** ✅ **A** — Gortnite already patched rten; keep the model fetch script
(`scripts/fetch-ocr-models.sh`) as part of the port.

#### F90 — Computer-use capture surface (binds Phase 6)

**Question:** How does computer-use see the screen?

**Options:**
- **A. Wayland** — native compositor protocol.
- **B. X11/CDP/atspi** — per-surface protocols.
- **C. Pixels** — framebuffer capture.
- **D. Hybrid** — best available per platform.

| Merit | A | B | C | D |
|---|---|---|---|---|
| Modern Linux fidelity | 5 | 3 | 3 | 5 |
| Breadth | 2 | 4 | 5 | 5 |
| Cost | 2 | 3 | 4 | 1 |
| **Total** | **9** | **10** | **12** | **11** |

**Verdict:** ⚖️ **D** — Gortnite already had `surface/` with wayland/cdp/atspi/
pixels + pty; port the matrix, pick primary per platform at the checkpoint.

#### F91 — Logic kernel disposition (binds Phase 6/Wave 4)

**Question:** What happens to the Gortnite logic kernel?

**Options:**
- **A. Port as crate** — keep as a library.
- **B. Port as extension** — noun/verb over the logic store.
- **C. Skip** — out of scope.

| Merit | A | B | C |
|---|---|---|---|
| Scope control | 4 | 3 | 5 |
| Architecture alignment (§2.2) | 2 | 5 | 1 |
| **Total** | **6** | **8** | **6** |

**Verdict:** ⚖️ **C by default, B if the full product wants it** — it is the most
distinctive and least load-bearing subsystem; do not port on reflex.

#### F92 — Memory retrieval backend (binds Phase 6)

**Question:** How does memory find relevant past facts?

**Options:**
- **A. BM25/dense (artist-ast search)** — the built-in backend.
- **B. Embedding store** — vector similarity (artist-memory).
- **C. Hybrid** — both, RRF-fused.

| Merit | A | B | C |
|---|---|---|---|
| No external model | 5 | 2 | 3 |
| Semantic recall | 2 | 5 | 5 |
| Cost | 4 | 2 | 1 |
| **Total** | **11** | **9** | **9** |

**Verdict:** ⚖️ **A first** — `artist-ast` already ships the ranking/RRF layer with
a pluggable `CandidateSource`; plug `artist-memory`'s embedder in behind it later.

#### F93 — MCP role (binds Phase 6)

**Question:** Is Artist an MCP server, client, or both?

**Options:**
- **A. Server** — expose Artist to MCP clients.
- **B. Client** — consume MCP servers as tools/nouns.
- **C. Both** — server + client.

| Merit | A | B | C |
|---|---|---|---|
| Interop reach | 4 | 4 | 5 |
| Cost | 3 | 3 | 1 |
| **Total** | **7** | **7** | **6** |

**Verdict:** ✅ **B first, A after** — consuming MCP servers as nouns is the
higher-value direction; Gortnite already built a typed MCP daemon to mine.

#### F94 — MCP transport (binds Phase 6)

**Question:** How does MCP connect?

**Options:**
- **A. stdio** — local subprocess.
- **B. HTTP/tunnel** — remote servers.
- **C. Both** — stdio + tunnel daemon.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity | 5 | 3 | 2 |
| Remote reach | 2 | 5 | 5 |
| **Total** | **7** | **8** | **7** |

**Verdict:** ✅ **C** — mine Gortnite's `mcp-tunnel` + systemd units; stdio first.

#### F95 — Issue/PR namespace (binds Phase 6)

**Question:** Does `issues://` exist, and how is it backed?

**Options:**
- **A. Native** — a dedicated git-forge client.
- **B. MCP-backed** — consume an issue MCP server.
- **C. Skip** — not in scope.

| Merit | A | B | C |
|---|---|---|---|
| Effort | 2 | 3 | 5 |
| Value (full product) | 5 | 4 | 1 |
| **Total** | **7** | **7** | **6** |

**Verdict:** ⚖️ **B** — MCP-backing (F93) is the cheapest path to a forge noun;
skip unless a real use case appears.

#### F96 — GPUI direction (binds Phase 6/Wave 4)

**Question:** What happens to the vendored zed-gpui/wgpu UI?

**Options:**
- **A. Defer** — revisit only if canvas is insufficient.
- **B. Adopt** — build the GUI on gpui.
- **C. Drop** — remove the vendored code.

| Merit | A | B | C |
|---|---|---|---|
| Scope control | 4 | 1 | 5 |
| Capability | 3 | 5 | 1 |
| **Total** | **7** | **6** | **6** |

**Verdict:** ✅ **A (do not drop yet)** — it is ~250k vendored lines of real
prior art; keep it archived until canvas is proven adequate.

#### F97 — Extension marketplace (binds post-Phase 7)

**Question:** Is there a remote extension registry?

**Options:**
- **A. Local-only** — extensions installed by hand.
- **B. Remote registry** — download + verify extensions.
- **C. Both** — local first, registry later.

| Merit | A | B | C |
|---|---|---|---|
| Simplicity / trust | 5 | 1 | 4 |
| Ecosystem growth | 1 | 5 | 4 |
| **Total** | **6** | **6** | **8** |

**Verdict:** ✅ **C** — full-trust (§2.8) makes a registry easy but also risky;
ship local-only, add a registry only with provenance/signing.

#### F98 — Event taxonomy standardization (binds Phase 1)

**Question:** Is there a canonical set of event topics?

**Options:**
- **A. Open strings** — any topic goes.
- **B. Namespaced convention** — `artist.turn.started` etc.
- **C. Typed enum** — a closed topic enum.

| Merit | A | B | C |
|---|---|---|---|
| Extensibility | 5 | 5 | 1 |
| Discoverability | 2 | 5 | 5 |
| **Total** | **7** | **10** | **6** |

**Verdict:** ✅ **B** — a namespaced string convention (with documented core
subjects) gets discoverability without closing the set.

#### F99 — Time & clocks in the harness (binds Phase 1)

**Question:** How do components get time?

**Options:**
- **A. WASI clocks** — the standard `wasi:clocks` imports.
- **B. Host-injected** — time passed through host calls.
- **C. Both** — WASI now, host wall-clock where determinism matters.

| Merit | A | B | C |
|---|---|---|---|
| Standard-ness | 5 | 2 | 4 |
| Deterministic replay | 2 | 5 | 5 |
| **Total** | **7** | **7** | **9** |

**Verdict:** ✅ **A now** — the filesystem host already imports `wasi:clocks`;
revisit host-injected time only if replay determinism becomes a hard requirement.

---

### 13.10 How to triage this catalog

- **Bind immediately (Phase 0):** F1, F2, F3, F4, F5, F6, F7, F9, F10, F85, F86.
- **Bind at Phase 1 (WASM host):** F15, F16, F19, F21, F22, F24, F26, F27, F28, F98.
- **Bind at Phase 2 (verbs/TOON):** F11, F29, F30, F31, F32, F33, F34, F37, F38, F39, F40, F42.
- **Bind at Phase 3 (namespaces/session):** F8, F12, F14, F23, F35, F36, F62, F63, F65, F66.
- **Bind at Phase 4 (loop/provider):** F41, F43, F44, F46, F47, F48, F49, F50, F51, F53, F54, F55, F56, F57, F58, F60, F61, F69, F84.
- **Bind at Phase 5 (CLI):** F70, F71, F72, F73, F74, F78, F79.
- **Bind at Phase 6+ (mining/future):** F13, F45, F52, F59, F64, F67, F68, F75, F76, F77, F89, F90, F91, F92, F93, F94, F95, F96.
- **Bind at Phase 7 (release):** F80, F81, F82, F83, F87, F88, F97.

Every fork with a `binds Phase N` tag is deliberately *deferrable until that
phase*. The scorecards give a reasoned default so you can either accept or
override quickly in the morning; nothing here is binding until you say so.
