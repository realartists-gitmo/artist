# Artist Production Architecture Roadmap

## Summary

Artist is a durable, local, multi-workspace daemon with a minimal URI/resource
kernel, WASM Component Model extensions, provider-neutral model normalization,
and multiple client projections.

Production means the platform is complete and reliable enough for later
tool/resource/frontend work. Built-in tools, built-in resources, GPUI, TUI, and
MCP integration are explicitly deferred.

The implementation order is horizontal: each plane establishes contracts,
persistence, failure behavior, tests, and observability for every subsystem that
depends on it. A demo path is not an exit criterion.

## Horizontal architecture slices

### 1. Architectural constitution

- The kernel owns URI identity, routing, provider registration, lifecycle, and
  internal VFS projection.
- Native bootstrap roots are `files://`, `resources://`, `tools://`, and
  `events://`.
- The VFS is internal to the agent/kernel; there is no user-facing OS mount
  requirement.
- Resources are file-shaped but may be static, mutable, computed, remote,
  append-only, or live. The kernel does not impose universal snapshot,
  revision, polling, or stream semantics.
- WASM Component Model + WIT is the extension ABI.
- Frontend protocols, provider APIs, and MCP do not define kernel semantics.

### 2. Core platform primitives

Stabilize canonical resource URIs, structured errors, metadata, IDs, async
interfaces, cancellation, deadlines, clocks, and deterministic serialization
boundaries. These foundational types must not depend on UI, providers, or
concrete tools.

### 3. Durable daemon state

- One daemon owns many workspaces and sessions.
- Workspaces have explicit identity, root configuration, and lifecycle.
- Each durable stream has its own append-only structured text log.
- Each line is a self-describing, versioned record.
- Projections are rebuildable from logs.
- Recovery handles partial writes, duplicate delivery, truncation, schema
  evolution, and conflicting writers.

### 4. Resource kernel

Complete provider registration, URI claim arbitration, native namespace
publication, provider-owned subtrees, current-byte reads, metadata, lookup,
listing, parent traversal, structured failures, and internal inode projection.
The kernel has no universal cache or universal stream abstraction.

### 5. WASM runtime and extension lifecycle

Complete component loading, WIT validation, noun/verb/event contracts, trusted
host bindings, claims, activation, replacement, generation pinning, invocation
leases, retirement, cleanup, and extension-owned providers. Specialized stream
and byte-flow contracts are added only where needed.

### 6. Extension build and packaging

Keep artifact execution separate from source compilation. A build subsystem
discovers source, invokes language toolchains, captures diagnostics, and emits
validated component artifacts. The ABI remains language-neutral.

### 7. Provider-neutral model plane

Create a dedicated normalization module for model requests, streaming deltas,
tool calls, structured content, usage, cancellation, retries, rate limits,
capabilities, and provider errors. Provider-specific data remains typed
extension data and never leaks into the agent loop.

Add a separate deterministic observation renderer: authoritative resource data
remains typed and addressable, while compact `stdobs` output is only a
model-facing projection. Exact tool schemas and `poll()` semantics are deferred.

### 8. Agent/session orchestration

Build the reusable session state machine, prompt/turn lifecycle, model
invocation, extension dispatch, events, cancellation, steering, replay/resume,
context projection, compaction interfaces, lineage, and usage accounting.
Concrete built-in tools and resources remain out of scope.

### 9. Client projection layer

Expose one canonical engine through:

- an embedded Rust SDK;
- NDJSON RPC for Herdr, custom clients, tests, and process isolation;
- an ACP adapter for compatible editors such as Zed.

Protocol normalization lives in adapter modules. GPUI and the later TUI consume
these client boundaries rather than kernel internals.

### 10. Production hardening

Provide Linux, macOS, and Windows builds; state migrations; parser fuzzing;
routing/replay/lifecycle/cancellation property tests; observability; secret
redaction; reliability limits; deterministic diagnostics; packaging; upgrades;
rollback; and cross-platform CI.

## Explicitly deferred

Concrete coding verbs, concrete resource extensions, `poll()` semantics,
process/session resource details, search/edit/anchor contracts, GPUI, TUI, MCP,
browser/computer-use, subagent semantics, memory/retrieval, and marketplace UX.

## Acceptance criteria

The platform is production-ready when one daemon can host multiple workspaces,
sessions, providers, clients, and extension generations; state survives restart
through replayable logs; the kernel is independent of models, frontends, MCP,
and concrete tools; prebuilt and user-built components share one validated
lifecycle; provider differences are isolated; embedded, NDJSON, and ACP clients
observe one canonical event model; and Linux, macOS, and Windows builds are
tested and packaged.
