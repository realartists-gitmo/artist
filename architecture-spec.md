# Artist Harness: Foundational Architecture Specification

Status: working specification

This document records the architecture currently understood from
`refactorplans.md`, `gort(1).md`, and the reduced workspace. It is deliberately
narrow: only decisions that constrain the foundational kernel belong here.
Questions that can be answered while implementing a handler are not treated
as architecture blockers.

## 1. Direction

Everything the harness touches is addressed as a resource. Resources are
exposed through one filesystem-shaped API, while the kernel hides the
backend-specific machinery.

The purpose of the redesign is to reduce the model-facing surface to a small
set of verbs while making the noun space extensible. Files, repositories,
agents, shell sessions, REPLs, forge objects, tools, and other future systems
are all resources addressed by URIs.

The current workspace contains:

- `artist-agent`: provider-facing agent loop and conversation execution
- `artist-session`: append-only session logs, memory, replay, and attachments
- `llm-provider`: provider configuration and authentication
- `artist-cli`: terminal application shell
- `artist-herdr`: lifecycle/activity integration

The previous tool, rule, extension, MCP, subagent, TTSR, compaction, and
skills surfaces have been removed or reduced. They are not to be restored in
their old form.

## 2. The kernel

The next foundational component is a kernel/VFS crate, tentatively named
`artist-kernel` or `artist-vfs`.

It provides:

- URI routing and resource resolution
- handler registration
- the universal verb dispatch layer
- batching
- structured results and errors
- native filesystem resources
- live-resource coordination
- the host API used by handlers and extensions

The CLI and agent loop must eventually call this kernel instead of carrying
their own tool-specific dispatch paths.

The kernel is not required to know every future noun in advance. Its native
responsibility is to route URIs to handlers and enforce the universal
contracts. Self-modifiable handlers may be added later through the extension
surface.

## 3. Resources and URIs

Every addressed noun is a resource. A resource is reached by a URI such as:

```text
repo://...
agent://...
bash://...
eval://...
debug://...
canvas://...
profile://...
todo://...
tools://...
```

Schemes are address roots, not separate tool APIs. A handler behind a scheme
implements the same universal verbs as every other handler.

The resource vocabulary includes:

- ordinary file and directory resources;
- derived views, such as repository symbols;
- pointer resources, whose value is another resource URI;
- live session resources, such as agents and shells;
- extension resources under `tools://`.

The initial URI rules are intentionally conventional:

- paths are slash-separated and case-sensitive;
- ordinary URI percent-encoding is used;
- `find()` owns wildcard/glob traversal;
- URIs identify current resource state unless a future resource explicitly
  exposes snapshots;
- no separate display-URI versus identity-URI distinction is required at the
  initial layer;
- aliases, redirects, `.`/`..` normalization, query parameters, and other
  nonessential URI features are deferred unless a concrete handler needs them.

There are currently no deliberate deviations from ordinary URI and path
behavior.

## 4. Universal verbs

The model-facing verb set is exactly:

```text
read write edit send poll abort delete find grep
```

The same grammar applies to every resource. All verbs are batching-native.
Every request item receives its own result. Batching does not create a second
tool mode; it is part of each verb's normal request shape.

The kernel may have internal operations for mounting, resolving, registering,
subscribing, snapshotting, and committing. Those are implementation APIs, not
additional model-facing verbs.

### `read()`

`read()` is non-blocking and returns the current serialization of a resource.
It can select a location and a relative window.

Locators are:

```text
top | bottom | #anchor
```

Windows are relative to the locator, such as `(-50,50)`, `(-N,0)`,
`(0,+N)`, or a bare count.

### `poll()`

`poll()` is the blocking counterpart to `read()`. It is bottom-oriented and
returns the newly available state together with the condition that ended the
wait.

The initial termination conditions are:

```text
idle
match:<regular-expression>
timeout:<milliseconds>
lines:<count>
```

Polling is multi-target and supports `any` and `all` completion modes.

### `write()`

`write()` creates a resource in a writable namespace or overwrites a mutable
resource according to that handler's contract.

Writing agent resources may eventually provide creation and pre-boot inbox
seeding, but agent creation is deferred until agents are rebuilt.

### `edit()`

`edit()` modifies mutable textual resources using Teca anchors. Ordinary
model-visible line-number addressing is not part of the contract.

Batched edits are one atomic transaction. The filesystem kernel payload is an
`args.edits` array; each item has a `start` anchor, an optional inclusive
`end` anchor, and replacement text. The exact anchor mathematics belong to
Teca; the harness supplies the relevant source text and CST/AST type-kind
information, resolves every edit against one current snapshot, rejects stale,
colliding, reversed, or overlapping ranges before mutation, and commits the
rendered file with an atomic same-directory replacement.

### `send()`

`send()` supplies interactive input to a live resource: an agent inbox, shell,
REPL, canvas, or similar session.

### `abort()`

`abort()` stops a live resource while preserving its record/history.

### `delete()`

`delete()` removes a resource. For live resources, deletion first aborts the
resource and then removes its record according to the handler's contract.

### `find()` and `grep()`

`find()` traverses resource paths and supports wildcard queries. `grep()`
searches inside resources. Both range over derived subtrees as well as
ordinary files. There is no separate structural-search tool surface.

## 5. Structured requests and results

Every operation has a machine-readable request and result. Human-readable
rendering belongs to projections such as the CLI.

Each batch item reports success or failure independently. A result carries the
resource value or a structured error; live operations additionally report
their current/terminal status as needed.

The request/result envelope is a straightforward implementation of the
universal contract: a batch is a sequence of request items, and the response
contains one corresponding result per item. Each result carries either a
value or a structured error, with operation-specific fields for windows,
polling conditions, pointers, streams, and live-resource state.

The exact Rust types and serialized field names belong to the kernel
implementation and provider-schema adapter; they do not represent a separate
architecture decision.

## 6. Teca and anchors

Teca is the anchor authority. It is not a tentative future crate and the
harness should not reproduce its anchor mathematics.

The harness/Teca boundary is:

1. the file/resource handler obtains the relevant source text;
2. the AST/CST layer supplies the exact line text and type-kind chain required
   by Teca;
3. Teca generates the lazy content-derived address stream;
4. the harness adapter computes shortest unique prefixes and resolves them
   against the current item set, reporting collisions or stale addresses
   loudly;
5. the kernel applies a resolved edit set as one atomic transaction.

The accepted anchor properties are:

- anchors are content-derived;
- anchors are designed to remain stable through heavy unrelated churn;
- identical content/type-kind entries use deterministic bidirectional ordinal
  identity;
- ranges are inclusive;
- stale or colliding anchors are errors, with enough information to repair
  the address;
- immutable resources accept anchors for uniformity;
- the external model contract does not expose ordinary line numbers.

Internal byte offsets, line indexes, and parser spans remain implementation
details.

## 7. Repository and AST resources

Repository structure is exposed as derived resource views. The adapter owns
the resource contract and delegates language analysis to the vendored
`artist-ast` fork of ast-bro. The fork exposes structured declarations,
language metadata, source spans, imports, and calls as a library API; the
kernel translates those source spans into Teca-compatible file anchors.

The VFS should expose ast-bro capabilities through paths resembling:

```text
repo://project/symbols/Foo
repo://project/symbols/Foo/callees
repo://project/symbols/Foo/callers
repo://project/symbols/Foo/impact
repo://project/symbols/Foo/trace/to/Bar
file:///work/project/src/main.rs/symbols/Foo
```

The exact returned shape is not invented here. The repository handler should
use ast-bro's library and machine-readable output contracts, then translate
source locations into Teca-compatible anchors where mutation needs to point
back to a file.

Repository reads are live derivations. There is no separate resource-cache
architecture in the foundational plan: reading a derived URI invokes its
handler against current repository state.

The adapter should remain thin: invoke the vendored library API for a resource
read, expose its useful structured result, and translate source locations into
Teca-compatible anchors where mutation needs to point back to a file. The
kernel does not replace the AST declaration model; it adds only resource and
address fields. Parsing failures and partial analysis are reported using
ordinary structured results, with unsupported or invalid files degrading to
the kernel's line-addressing layer.

## 8. Live resources

Agents, shells, REPLs, debuggers, canvases, and similar objects are sessions:
resources with live state and readable output.

The minimum shared behavior is:

- `send()` supplies input;
- `read()` returns current state/output;
- `poll()` waits for state/output;
- `abort()` stops execution and preserves the record;
- `delete()` removes the resource according to its handler;
- closed resources remain readable.

The existing `artist-session` event log is a useful persistence mechanism for
these resources and should eventually sit behind a session handler. The
existing `artist-herdr` activity layer should consume kernel/session
lifecycle events rather than define separate tool semantics.

The session handler directly implements the five live-resource operations
above. Questions about crash recovery, retention, event-version migration,
and writer locking belong in that handler's implementation rather than in the
foundational architecture.

## 9. Tool-to-tool calls

Every backend must eventually use the same kernel API:

- an agent reads repository resources;
- a shell invokes harness resources;
- a REPL reads and edits files;
- an extension creates or polls sessions;
- a forge handler returns structured resources.

No backend exposes an independent model-facing tool schema. The kernel is the
single route to addressed nouns.

Handler registration is straightforward: a handler claims/resolves an address,
declares supported verbs, receives a universal request, returns a universal
result, and receives the shared host API for nested calls. Capability policy,
recursive-call limits, tracing detail, and WASM ABI design are not
foundational questions yet; they can be specified when those systems are
implemented.

## 10. Self-modification and `tools://`

Self-modification is a first-class later extension. The intended behavior is:

1. tools are represented as files under `tools://`;
2. the agent can read, write, and edit those files;
3. the trusted host compiles and loads the resulting extension;
4. the active registry is snapshotted per turn;
5. changes become live on the next turn.

The kernel must provide the handler/host boundary that makes this possible,
but WASM ABI, trust, state migration, rollback, and resource limits are not
required to begin the VFS implementation.

## 11. Deferred systems

The following are downstream resource handlers or projections. Their previous
implementations remain deleted:

- bash/brush and terminal sessions
- subagents and agent profiles
- MCP
- rules and TTSRs
- old tool policy concepts
- skills and authored skill discovery
- compaction and memory improvements
- todo-specific tool surfaces
- forge integrations
- richer AST/repository projections beyond the foundational `repo://` surface
- WASM extensions and `tools://`
- LSP and DAP integration
- computer use and the uni-CLI rung-0 redesign
- certified subagent `yield()` output
- model ingress filtering such as snapcompact

These systems should return only through the resource/verb model. None should
be reintroduced as a parallel bespoke tool surface.

## 12. Foundational implementation order

1. [x] Create the kernel/VFS crate with a minimal in-memory handler test
   double. The first slice now includes conventional URI parsing, the nine
   universal verbs, handler registration/routing, structured per-item results,
   ordered batching, registry metadata, and nested handler calls.
2. [x] Implement native filesystem resources as native OS-path targets, not a
   `file://` namespace. The kernel distinguishes `ResourceAddress::Path`
   from virtual URIs: bare OS paths are the model-facing spelling, while
   equivalent `file://` URIs are accepted for interoperability. AST
   projections use the same suffix grammar on either spelling.
3. [x] Add the Teca-backed anchor adapter and structural analysis boundary.
   The kernel now has a CST-provider interface, a Rust tree-sitter provider,
   and an explicit line-only fallback. Teca inputs therefore degrade from
   structural type-kind addressing to exact line/duplicate addressing only
   when no usable CST exists.
4. [x] Integrate the adapter into atomic filesystem edits. The filesystem
   handler now resolves CST/type-kind or line-fallback anchors, supports
   inclusive anchor ranges and simultaneous edits, rejects invalid edit sets
   before writing, and commits through a temporary-file replacement.
5. [x] Implement one minimal live-session handler. `session://` resources
   support creation through `write`, append-only input through `send`,
   immediate event snapshots through `read`, blocking `poll` with line,
   match, timeout, and terminal conditions, plus `abort` and `delete`.
   Sessions are synchronized in memory and intentionally do not spawn agents,
   processes, or providers yet.
6. [x] Route the agent and CLI through the kernel and derive named tool schemas
   from the same registry. The CLI retains its kernel-backed resource command,
   while the agent receives runtime-defined named tools whose descriptions and
   input schemas come from the registered tool packages. Named calls dispatch
   through the shared kernel with ordinary resource targets; interactive and
   non-interactive agent paths both receive the same registered kernel.
7. [x] Add the repository/AST resource adapter. `repo://` now exposes source
   files, multi-language symbol projections through the vendored `artist-ast`
   fork, `find`, and `grep` through the universal handler contract, with symbol
   definitions pointing back to file anchors. The adapter analyzes the exact
   source buffer read by the handler and degrades unsupported or invalid input
   to ordinary line addressing.
8. [x] Add the self-modifying `tools://` handler. Tool package files are
   ordinary virtual files backed by the native file handler. The handler
   exposes package frontmatter as named harness-tool definitions and dispatches
   those names to typed or generic WASM components through the transactional
   component registry. Components receive the kernel handle for nested resource
   operations. Writes and edits invalidate the package's active build for the
   next invocation, while failed compilation preserves the prior version.
9. Rebuild the remaining deferred systems as handlers and projections.

There are no remaining architecture questions blocking the foundational
implementation. Handler-specific details should be settled as each handler
is built.
