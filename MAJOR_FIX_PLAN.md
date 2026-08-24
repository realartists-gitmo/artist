# Major fix implementation plan

## Status and authority

This document is the implementation contract for every finding in
[`majorfixlist.md`](majorfixlist.md). The fix list remains the diagnostic
rationale; this plan defines scope, sequencing, public contracts, and evidence.

Two project decisions are fixed:

- **Artist has no security boundaries.** Filesystem roots, URI routes, WASI
  configuration, and plugin boundaries are organizational mechanisms only.
  Do not add `cap-std`, permission policy, path-containment claims, sandbox
  policy, or security abstractions. Keep only the path mapping needed to decide
  whether a provider owns a URI, and document that it is not confinement.
- **The Rust plugin-host API is experimental and async-first.** Breaking the
  current synchronous API is allowed. Do not retain blocking compatibility
  wrappers, private runtimes, or `block_on` bridges.

Two findings are already permanently resolved and remain regression gates:

- Canonical transcript append and validation are incremental on the successful
  path; full replay is reserved for untrusted loading and exceptional rollback.
- `SessionStore::append` is an expected-sequence transaction. `FileStore`
  serializes independent instances and processes with a stable file lock and
  commits hash-chained batches.

Their contracts live in [`TRANSCRIPT_V1.md`](TRANSCRIPT_V1.md). This plan must
not replace those implementations with weaker in-process-only behavior.

## Architectural boundaries

- Artist owns the canonical event-sourced transcript. Rig may shape a copy for
  one model request but never owns or mutates durable conversation history.
- `ToolRegistry` remains below Rig. Every model-facing tool, including the six
  bundled universal verbs, registers from a WASM tool-provider component into
  the shared registry; Rig adapts registry definitions to dynamic model tools.
  The host itself registers no model tool. Do not replace this registry with
  Rig's `ToolSet`.
- `ResourceRouter` is the canonical resource model. FUSE is a Unix projection,
  FFF indexes that projection, and the default WASM component delegates native
  mechanics back to the router/provider layer.
- Provider/model details may be preserved in live execution and telemetry
  types, but the durable transcript remains backend-neutral.
- Text resource bodies remain UTF-8. This work does not introduce a binary
  resource protocol.

## Public contract changes

### Async plugin host

- Enable Wasmtime async support and the async WASI/component linker path.
- Make component loading and every socket that may reach an async Artist
  service `async`.
- Instantiate and call components with Wasmtime's async APIs.
- Host `list-tools`, `call-tool`, native-filesystem, and resource callbacks
  directly await the shared registry/router.
- Host resource-fabric imports expose generic routed requests and indexed
  search mechanics to tool-provider components without defining model tools.
- Remove `PluginHost`'s Tokio runtime, `block_on`, `block_in_place`, and all
  equivalent synchronous bridges.

### Resource metadata is deferred

Metadata needs a separate design discussion. The current resource protocol and
WIT package deliberately expose no generic metadata operation or metadata
record. FUSE temporarily classifies nodes through `children` and `read`; that
cost is accepted until the later design is agreed rather than freezing the
wrong public contract now.

The component contract still advances to `artist:plugin@0.4.0` for the other
breaking changes in this plan. Update every fixture together; no 0.3
compatibility shim is required.

### FUSE write lifecycle

Do not add byte-oriented writes to the text resource protocol. Instead, give
FUSE process-owned per-open text state:

- initialize an open file buffer once;
- apply kernel write/truncate callbacks to that buffer under a per-open lock;
- validate UTF-8 and commit through one routed whole-resource `write` at
  flush/release boundaries;
- ensure repeated flush/release is idempotent;
- keep open handles pinned independently from lookup-reference reclamation.

This removes a routed read/modify/write cycle from every kernel write callback
without pretending the canonical resource API is binary-safe.

### Model lifecycle and metadata

Separate these events explicitly:

- model emitted a tool request;
- Rig committed to executing a tool after validation and hooks;
- tool execution completed or failed.

Use Rig's internal call identity to correlate these stages with Artist's stable
call ID. `ToolInvoked` telemetry is emitted only for execution commitment, never
merely because the model produced a tool-call item.

Preserve Rig's normalized terminal metadata through the adapter, including:

- finish reason (`stop`, `length`, `tool-calls`, `content-filter`, or unknown);
- provider/model/response correlation identifiers when supplied;
- normalized failure class, retryability, provider code, HTTP status, and
  request/response identifiers when supplied.

The durable failed outcome may retain its stable human-readable error string;
structured backend details belong in live events and observability.

Preserve rich model/tool-result content with a backend-neutral content-part
type covering text, JSON, images, reasoning, and an opaque tagged payload for
unknown/provider-native parts. Live events retain those parts without lossy
string conversion. Parts needed to reconstruct later model history are also
canonical transcript data. This is a semantic record change: advance
`RECORD_VERSION`, migrate current string tool results to one text part, and
teach the v2 physical-frame decoder to migrate its declared older record
version. Do not silently discard an unknown part.

### Universal tool arguments

Define one typed Serde/Schemars request type per universal tool:

- `ReadArgs`
- `FindArgs`
- `GrepArgs`
- `WriteArgs`
- `MoveArgs`
- `PollArgs`

The bundled WASM tool-provider owns these types, schemas, defaults, validation,
and concise result rendering. Derive schemas from those types and deserialize
invocation arguments directly into them. Remove the handwritten schema objects
and manual `string_arg` / `u64_arg` family. Preserve the existing six tool
names and JSON field names.

## Implementation sequence

### 1. Correct resume reconstruction (P0)

Derive pending process state from the canonical transcript during resume:

- pending inputs are `Input` entries never referenced by `RunStarted`;
- pending steering items are `SteeringQueued` entries never included in a
  `SteeringDelivered` entry;
- both retain canonical transcript order;
- first close any active crashed run using the existing resume semantics, then
  continue pending work through the normal command/run loop;
- never append replacement queue entries during reconstruction.

Evidence:

- crash after acknowledged input but before `RunStarted`, then resume and run;
- multiple pending inputs preserve FIFO order;
- queued steering survives restart and is delivered exactly once;
- delivered steering is not reconstructed;
- active-run recovery plus queued input/steering composes correctly.

### 2. Align the Rig adapter with Rig 0.42 lifecycle (P0/P1)

First pin behavior with compile-time exhaustive matches and focused adapter
tests against the locked Rig version. Then:

- retain the model-requested tool-call event for canonical replay;
- translate Rig 0.42's `ToolExecutionCommitted` as a distinct live execution
  event with the correct internal call ID (the locked release has no separate
  `ToolExecutionStart` stream item);
- translate completed execution with that same internal call ID;
- preserve normalized final-response metadata instead of inventing `stop`;
- replace `ModelError(String)` at the adapter boundary with a backend-neutral
  structured failure and stringify only at the durable transcript boundary;
- test Rig's recoverable stream-error behavior. If `AgentRunner` forwards a
  recoverable item, continue draining until a genuine terminal item or stream
  end. If it guarantees only terminal errors, encode that verified assumption
  in a regression test and documentation.

Preserve reasoning and provider-native/unknown stream items through the
content-part contract. Reasoning remains distinct from visible assistant text.
Persist only parts required for faithful future model-history reconstruction;
telemetry-only raw response metadata remains outside the transcript.

Evidence:

- requested-but-rejected/skipped tool never emits execution-start telemetry;
- executed tool emits one start and one terminal event with stable correlation;
- every normalized finish reason round-trips, including unknown strings;
- structured provider failure fields survive adapter projection;
- recoverable error followed by valid data does not prematurely fail when Rig
  exposes that sequence;
- reasoning and unknown items are surfaced without corrupting text history.
- text, JSON, image, and opaque tool-result parts replay without flattening;
- existing v2 string tool results migrate to a single text part.

### 3. Remove `rig-tap` skew and rebuild observability (P0/P1)

- Remove `rig-tap` and its obsolete Rig 0.37 dependency from manifests/lockfile.
- Keep `artist-observe` as an Artist event consumer with Artist-owned,
  backend-neutral observation records or a small sink trait.
- Map model-requested tools separately from actual execution.
- Track only executions that actually started; clear state on completion,
  failure, interruption, and receiver lag where correlation can no longer be
  trusted.
- Report the real finish reason and structured failure/correlation metadata.
- Do not make telemetry necessary for replay or resume.

Evidence:

- the lockfile contains only the intended Rig 0.42 family;
- no false `ToolInvoked` for rejected calls and no leaked tool map entry;
- finish reasons are never hard-coded;
- success, provider failure, cancellation, and tool failure observations are
  deterministic unit tests.

### 4. Make the Wasmtime boundary natively async (A prerequisite for later work)

- Enable the minimum Wasmtime/Wasmtime-WASI async features.
- Convert `PluginHost`, `HostState`, component instantiation, socket calls, and
  examples/tests to async.
- Directly await registry and router operations in host imports.
- Preserve nested tool-call correlation and recursion rejection across async
  component callbacks.
- Remove the host-owned runtime entirely.

Evidence:

- source search finds no private Tokio runtime or blocking bridge in
  `artist-plugin`;
- all WIT 0.4 fixtures build and every advertised socket loads/calls;
- cross-plugin calls and direct/indirect recursion tests still pass;
- calls work both inside a multithreaded Tokio runtime and a current-thread
  test runtime without panic or deadlock.

### 5. Consolidate filesystem mechanics onto `artist-resource`

- Make the WIT native-filesystem host implementation a thin async adapter over
  the existing `FilesystemProvider`/resource request path.
- Remove direct duplicate `std::fs` read/list/write/move/remove mechanics from
  `artist-plugin::HostState`.
- Keep filesystem roots as organizational provider scope only. Retain lexical
  ownership checks if routing needs them, but name/document them as routing—not
  confinement—and add no symlink or capability policy.
- Ensure the default component still registers the terminal `file:///**`
  routes and delegates mechanics through focused host imports.

Evidence:

- one implementation defines filesystem behavior;
- direct provider calls and default-WASM calls produce equivalent replies and
  typed errors;
- symlinks receive ordinary host filesystem behavior, with no security claim;
- no capability/security filesystem dependency is introduced.

### 6. Repair FUSE lifecycle

- Leave generic resource metadata out of this slice pending the later design
  discussion recorded above.
- Implement per-open buffered write/truncate/flush/release behavior described
  above.
- Track lookup counts and implement `forget` plus `batch_forget`; reclaim an
  inode only when it has no lookup references and no open handles.
- Keep fixed process-owned attributes solely for the kernel interface.

Evidence on Linux with `/dev/fuse`:

- many small kernel writes cause one logical routed commit per flush cycle;
- truncate, overwrite, sparse extension, repeated flush, and invalid UTF-8
  behavior are deterministic;
- forgotten dynamic nodes are reclaimed and later lookup remains correct;
- projection directories and ordinary files still round-trip to canonical URIs.

Unit-level provider/router tests remain runnable without FUSE.

### 7. Give FFF ownership of index lifecycle and pagination

- Replace the reconstructed `Mutex<FilePicker>` with FFF 0.10.5's
  `SharedFilePicker` / `new_with_shared_state` long-running path.
- Let FFF perform initial indexing, filesystem watching, and ordinary content
  refreshes.
- Reserve Artist's router topology signal for route/projection changes that the
  mounted-filesystem watcher cannot observe. Do not trigger a complete logical
  rescan for every content write.
- Translate topology invalidation into FFF's supported rescan/resynchronization
  mechanism without discarding the shared picker.
- Push root/glob/depth constraints into FFF when its API can represent them.
- Use native path pagination for `find` and native bounded grep pagination /
  file offsets for `grep`; do not request `usize::MAX` and paginate a fully
  materialized result afterward.
- Preserve opaque Artist cursors that encode all FFF continuation state needed
  to resume deterministically.

Evidence:

- one shared picker survives refresh/topology events;
- ordinary file edits become searchable through FFF watching without Artist
  reconstructing the picker;
- virtual route changes become searchable after explicit resynchronization;
- bounded searches do not materialize the complete result set;
- pagination has no duplicates or omissions across ordinary and projected
  resources.

### 8. Make `grep.regex` a raw regex contract

- Stop passing `regex` through FFF's query-language parser.
- Construct the raw-regex search input supported by the pinned FFF API.
- Apply `include_glob` independently as its own constraint.
- Keep the public argument named `regex`; do not silently broaden it into FFF
  query syntax.

Evidence includes literal/escaped patterns resembling `*.rs`, exclusions,
paths, and `type:rust`, plus invalid-regex reporting and paginated context.

### 9. Move request-history shaping into Rig's request hook

- Continue projecting canonical Artist history into Rig messages.
- Compose the selected `rig-memory` policy/compactor with a Rig hook that runs
  for every completion request, including each iteration of a multi-turn tool
  loop.
- Supply shaped history through `RequestPatch::history`.
- Emit enough compaction information for Artist to append its existing
  `ContextCompacted`/compaction artifact after the request boundary without
  mutating prior transcript entries.
- Do not adopt Rig `ConversationMemory` as canonical persistence.

Evidence:

- no-op, sliding-window, token-window, and compaction policies shape only a
  request copy;
- a multi-turn tool loop applies shaping at every completion call;
- canonical transcript bytes are unchanged except for an explicitly appended
  compaction artifact;
- tool-call/result pairs and stable prompt prefix remain valid.

### 10. Derive universal tool contracts from typed arguments

- Add Schemars only to the narrow WASM tool-providers that own universal tools;
  the native resource crate must not define or register them.
- Implement the six argument structs and shared defaults/validation.
- Generate Rig/plugin-visible definitions from the derived schemas.
- Deserialize once at invocation and pass typed values to router/search calls.

Evidence:

- schema snapshots cover required fields, nullability, limits, and
  `additionalProperties` behavior;
- valid and invalid invocation tests exercise the same types;
- the model-facing tool surface remains exactly `read`, `find`, `grep`,
  `write`, `move`, and `poll` plus separately registered plugin tools;
- registry ownership identifies each tool as `artist.tool.<name>`, and a bare
  host has no model-facing definitions before those components are loaded.

### 11. Simplify synchronization and topology state

- Replace `ResourceRouter`'s separate atomic generation and watch value with
  the watch channel as the single source of truth using `borrow` and
  `send_modify`.
- Distinguish content mutation from logical topology mutation; only the latter
  drives explicit FFF resynchronization.
- Replace Tokio registry/route locks with `std::sync::RwLock` where the guarded
  operation is wholly synchronous and no lock crosses an await.
- Keep provider execution outside locks.

Evidence:

- generation is monotonic under concurrent registration/topology changes;
- content-only writes do not announce route topology changes;
- no synchronous lock is held across `.await`;
- registry definitions and ownership queries no longer require artificial
  async calls where no asynchronous work exists.

### 12. Small dependency/API cleanup

- Remove fuser's no-op `abi-7-31` feature while retaining pinned fuser 0.17.
  Treat an upgrade to 0.18 as separate work.
- Expose Rig tool concurrency as an opt-in `RigModel` configuration, defaulting
  to one. Do not enable concurrent side-effecting tools globally.
- Retain the already implemented transcript/store performance and concurrency
  tests as permanent regression gates.

Evidence:

- the no-op feature is absent from the manifest;
- default tool execution remains serial;
- an explicit concurrency setting reaches Rig and has a deterministic test;
- the lockfile has no unintended duplicate major/minor dependency families
  introduced by this work.

## Finding coverage

| Fix-list finding | Disposition |
| --- | --- |
| A. FFF lifecycle and pagination | Steps 7–8 |
| B. Rig-owned request-history shaping | Step 9 |
| C. Async Wasmtime embedding | Step 4 |
| D. Duplicate filesystem mechanics | Step 5 |
| E. Conditional capability filesystem | Explicitly excluded by project policy |
| F. Typed universal-tool schemas | Step 10 |
| Lost input/steering across resume | Step 1 |
| Stale Rig tool-execution lifecycle | Step 2 |
| False tool-executed telemetry | Steps 2–3 |
| Invented finish reason / lost metadata | Steps 2–3 |
| Recoverable stream errors | Step 2, behavior proven before implementation branch |
| Flattened `ModelError` | Steps 2–3 |
| Quadratic transcript append | Already resolved; permanent regression gate |
| FileStore write race | Already resolved with stronger cross-process semantics |
| FUSE metadata full reads | Deferred pending metadata design; Step 6 does not add a public contract |
| FUSE whole-resource work per write callback | Step 6 |
| Unreclaimed FUSE inode tables | Step 6 |
| `rig-tap` / Rig 0.37 dependency skew | Step 3 |
| No-op fuser ABI feature | Step 12 |
| Raw regex passed through FFF query parser | Step 8 |
| Duplicate router generation state | Step 11 |
| Content writes treated as topology changes | Steps 7 and 11 |
| Artificial async registry/router locks | Step 11 |
| Unexposed optional Rig tool concurrency | Step 12 |
| Lost rich/unknown Rig content | Step 2 plus the versioned content-part contract |

## Final verification matrix

Completion requires all of the following:

- `cargo fmt --all -- --check` for the host and plugin workspaces;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo test --workspace --all-targets`;
- all narrow production components build for `wasm32-wasip2` against WIT 0.4;
- the Wasmtime load smoke invokes every advertised socket and skips every
  unadvertised socket;
- resource/FUSE integration passes on Linux with a usable `/dev/fuse`;
- dependency-tree audit confirms removal of `rig-tap` and Rig 0.37;
- source audits confirm no private plugin runtime/blocking bridge, no duplicate
  native filesystem implementation, no `usize::MAX` search pagination, no FFF
  query parser on the raw regex path, and no handwritten universal-tool schema;
- transcript replay proves resource, model, and telemetry changes introduce no
  second durable conversation store;
- documentation states plainly that Artist provides no permission, security,
  sandbox, confinement, or isolation boundary.

## Explicit non-goals

- security, permissions, sandboxing, path confinement, plugin isolation, or
  capability filesystems;
- binary resource bodies or a general byte-stream resource protocol;
- replacing Artist's canonical transcript with Rig memory;
- replacing Artist's shared tool registry with Rig's tool registry;
- increasing tool concurrency by default;
- upgrading fuser beyond 0.17 merely as part of this plan;
- compatibility wrappers for the synchronous `PluginHost` API;
- frontend or product workflow work.
