# Artist: Long-Horizon Production Architecture Plan

**Status:** architectural orientation document  
**Planning horizon:** complete production system, not a staged product strategy  
**Planning method:** horizontal slices, in dependency order  
**Source of truth for this document:** the current working tree’s source and configuration only

---

## 1. How to use this document

This is a decision document, not a feature backlog. It records the forks that can materially change correctness, compatibility, operability, security, or the shape of later work. It deliberately does **not** turn obvious requirements into choices: the finished system is expected to be cross-platform, extensible to every suitable WASM producer language, usable through more than one interface, and complete rather than a narrowly scoped release.

The sequence is architectural build order. Each slice establishes a reusable plane for every later capability. A slice is complete only when its contracts, failure behavior, persistence, tests, and observability are defined; a demo that works through one path is not completion.

### 1.1 Scorecard rubric

Every option below is scored from **1 (poor) to 5 (excellent)** on the following virtues:

| Abbreviation | Virtue | Meaning |
|---|---|---|
| **C** | Correctness | Ability to preserve exact semantics, ordering, atomicity, and recovery guarantees |
| **X** | Extensibility | Ability to add nouns, verbs, providers, languages, hosts, and transports without redesign |
| **S** | Safety | Containment, least authority, data protection, deterministic failure, and safe mutation |
| **O** | Operability | Inspectability, upgrades, diagnostics, supportability, and failure recovery |
| **P** | Performance | Latency, throughput, memory use, startup, and batching potential |
| **M** | Maintainability | Conceptual clarity, testability, dependency discipline, and implementation burden |

The **weighted score** is out of 100:

```text
score = 5C + 4X + 4S + 3O + 2P + 2M
```

Correctness, extensibility, and safety are intentionally weighted above raw speed. Scores are comparative architectural judgments, not benchmarks. A lower-scoring option can still be selected when it is required by a hard constraint; that exception must be recorded in the decision log.

### 1.2 Decision notation

- **Recommended** means the plan should implement that option unless a later proof invalidates it.
- **Required invariant** means it is not an implementation preference and must be tested continuously.
- **Deferred detail** means the fork is resolved, but the detailed schema or algorithm can be designed in the named slice.
- **Exit gate** means evidence required before the next dependent slice is allowed to solidify.

---

## 2. Current-tree orientation

The current tree provides useful boundaries but not yet a complete product:

- `artist-kernel` contains a minimal async `Vfs`, inode-like identifiers, namespaces, directory/file attributes, a kernel router, and an empty `resources` namespace.
- `artist-kernel/vfs` contains an async FUSE bridge; `artist-kernel/vfs/windows` contains a cfg-gated WinFsp bridge and a non-Windows unsupported stub.
- `artist-kernel/wasm` contains Wasmtime engine, component loading, and noun/verb/event family classification scaffolding.
- The WASM filesystem package is intended to project WASI filesystem operations over the virtual filesystem rather than directly over host paths.
- `artist-agent`, `artist-component`, `artist-cli`, `artist-session`, and `llm-provider` are intentionally empty library shells with dependency wiring preserved.
- `artist-ast` is substantially implemented as a structured analysis library: language adapters, declarations, anchors, project walking, dependency/call/public-surface analysis, impact/context assembly, AST-aligned chunking, hybrid BM25+dense retrieval, persistent indexes, incremental deltas, and versioned JSON representations.
- The workspace already assumes Rust 2024, async execution, Wasmtime component model support, and separate host bridges. Those are starting constraints, not proof that the surrounding contracts are complete.

The most important architectural implication is that **the kernel, contracts, persistence, extension host, agent loop, provider layer, and user interfaces must be rebuilt as one coherent system**. Filling each empty crate independently would create incompatible local designs.

---

# Part I — Architectural constitution

## D1. Product authority model

**Fork:** what is the authoritative boundary of the system?

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Agent-centric application** | The agent loop owns resources, tools, state, and UI; other surfaces adapt to it | 3 | 2 | 3 | 3 | 4 | 3 | 58 |
| **B. Kernel-centric resource system** | A host-neutral kernel owns identity, resources, operations, events, and transactions; agents and UIs are clients | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Distributed service fabric** | Every concern is a network service and the local process is one deployment of it | 4 | 5 | 3 | 4 | 2 | 2 | 75 |

**Recommendation: B.** The kernel is the semantic authority. The agent is an orchestration client, not the owner of file truth. A future remote deployment can expose kernel protocols without changing operation semantics.

**Required invariants:** no UI, provider, or model-specific type may define resource truth; all mutation and authorization decisions pass through kernel contracts; clients can be disconnected and reattached without corrupting state.

## D2. Execution topology

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. One in-process monolith** | Kernel, WASM, provider, agent, and UI share one process | 3 | 3 | 2 | 2 | **5** | 3 | 62 |
| **B. Supervisor plus local worker processes** | A durable supervisor owns lifecycle; kernel, extensions, and optional providers run in controlled workers | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Always-remote microservices** | Separate network services for kernel, agent, indexing, and providers | 4 | 5 | 3 | 4 | 2 | 2 | 75 |

**Recommendation: B, with an in-process fast path.** Keep the semantic API process-neutral, but use a supervisor/worker topology for untrusted or failure-prone components. A trusted single-process deployment should be an optimization, not a separate semantic implementation.

## D3. Trust zones

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Uniform trust** | All extensions and host integrations receive the same ambient authority | 2 | 4 | 1 | 2 | **5** | 4 | 53 |
| **B. Capability-scoped execution** | Each operation and extension receives explicit, revocable capabilities | **5** | **5** | **5** | 4 | 3 | 3 | **91** |
| **C. OS sandbox only** | Rely on process/container/OS boundaries; application contracts are permissive | 3 | 3 | 4 | 3 | 3 | 3 | 68 |

**Recommendation: B backed by C.** Application capabilities express intent and auditability; OS/WASM isolation is the enforcement backstop. Do not create a fake capability layer that merely carries strings while all host authority remains ambient.

## D4. Deployment shape

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Local-first only** | One machine owns all state and execution | 4 | 3 | 4 | 3 | **5** | 4 | 76 |
| **B. Local-first with protocol-equivalent remote mode** | Local is complete; remote kernel/workers use the same typed protocol | **5** | **5** | **5** | **5** | 3 | 3 | **94** |
| **C. Cloud-first** | Persistent remote service is the primary authority | 4 | 5 | 3 | 5 | 2 | 2 | 76 |

**Recommendation: B.** Design identity, leases, events, persistence, and transport so local and remote hosts are two deployments of one model. Do not make network presence mandatory for local correctness.

## D5. API authority versus convenience APIs

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. One dynamic JSON API** | Every caller uses JSON objects and runtime checking | 2 | 4 | 2 | 3 | 3 | **5** | 65 |
| **B. Typed core plus dynamic boundary adapters** | Core contracts are typed/versioned; JSON, CLI, Python, and network adapters normalize into them | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Separate API per client** | Rust, CLI, WASM, and remote clients each get independent contracts | 3 | 2 | 3 | 2 | 3 | 2 | 53 |

**Recommendation: B.** There are two legitimate surfaces: lossless programmatic contracts and tolerant human/model adapters. They must not be collapsed.

## D6. Error taxonomy

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. String errors** | `anyhow`-style text at every boundary | 2 | 3 | 2 | 2 | 4 | **5** | 58 |
| **B. Stable structured error envelope** | Machine code, category, retryability, blame, resource, operation, and safe detail | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Language-native error enums only** | Each crate keeps private typed errors and adapters translate ad hoc | 3 | 2 | 3 | 2 | 3 | 3 | 60 |

**Recommendation: B.** Human text is a projection. Every error must answer: what failed, whether state changed, whether retry is safe, what can be repaired, and which identifier to correlate.

---

# Part II — Horizontal build order

## Slice 0 — Freeze invariants, terminology, and dependency direction

Before implementing features, establish a short architectural constitution in code comments and tests:

1. Kernel owns semantics; clients request operations.
2. Every resource has a stable URI and an internal identity distinct from its display path.
3. Reads are snapshot-aware; writes are transactional; events are ordered per stream.
4. Programmatic operations are typed and batch-capable.
5. Model-facing tool calls are scalar and boring; batching happens at the assistant-turn boundary.
6. Authoritative results and compact model observations are separate planes.
7. No active path performs lazy schema compilation, component compilation, or provider discovery.
8. Every long-running operation has cancellation, timeout, progress, and terminal state semantics.
9. Untrusted code is capability-scoped and cannot escape its assigned resource graph.
10. All persistent formats have explicit schemas, versions, migrations, and corruption behavior.

### Slice 0 exit gate

- A dependency graph proves that UI/provider code cannot be imported by the kernel.
- Contract tests can run without an LLM, terminal, FUSE, WinFsp, or network.
- Every later decision in this document has an owner module and a test category.

---

## Slice 1 — Resource identity, URI, namespace, and lifetime

The current kernel has inode-like IDs and namespaces but only a skeletal `resources` namespace. Complete the resource model before implementing verbs.

### D7. External resource address

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. OS paths as universal identity** | `PathBuf` is passed through every layer | 3 | 2 | 2 | 3 | **5** | 4 | 62 |
| **B. Typed hierarchical URI** | Scheme, authority, path, query/fragment rules, and canonicalization are explicit | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Opaque IDs only** | Callers use UUIDs and ask a catalog to resolve them | 4 | 4 | 4 | 4 | 3 | 2 | 78 |

**Recommendation: B, with opaque internal IDs.** Use URIs for stable interchange and typed IDs for efficient internal routing. Bare OS paths are accepted only by an explicit adapter that resolves them into a URI.

**URI rules to settle in this slice:** schemes are registered, normalization is deterministic, percent encoding is not double-applied, case rules are scheme-owned, path traversal is rejected after decoding, and display strings cannot be used as authorization keys.

### D8. Namespace composition

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Static enum of built-in namespaces** | Kernel knows filesystem, process, session, and extension types | 3 | 2 | 3 | 4 | 4 | **5** | 72 |
| **B. Registry of namespace providers** | Schemes/providers register route, lifecycle, metadata, and operation capabilities | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. One generic key/value namespace** | All resources are records behind a generic store | 2 | 4 | 2 | 2 | 3 | 4 | 56 |

**Recommendation: B.** Provider registration is open-ended, but each provider must declare its capabilities and lifecycle explicitly. Do not make namespace discovery an untyped `Any` downcast.

### D9. Resource lifetime and identity

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. URI is identity forever** | Recreated resources with the same URI are indistinguishable | 3 | 3 | 2 | 4 | 5 | 4 | 67 |
| **B. URI plus generation/resource incarnation** | URI locates; incarnation/version identifies the observed object | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Ephemeral handles only** | Every operation uses a leased handle | 4 | 4 | 4 | 2 | 3 | 2 | 70 |

**Recommendation: B.** A URI is the user-facing address; resource identity includes provider, incarnation, and observed revision where needed. Stale handles must fail explicitly rather than silently target a replacement.

### D10. Namespace visibility

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Global namespace** | Every mounted resource is visible to every caller | 3 | 5 | 1 | 4 | **5** | 4 | 68 |
| **B. Per-session projected namespace** | A caller sees a capability-filtered projection of the global graph | **5** | **5** | **5** | **5** | 3 | 3 | **94** |
| **C. Copy resources into each session** | Isolation is achieved by duplication | 2 | 2 | 4 | 2 | 2 | 2 | 47 |

**Recommendation: B.** The kernel can maintain one graph while each invocation receives a stable projection. This supports least authority without duplicating file contents.

### Slice 1 deliverables

- `ResourceUri`, canonicalization, scheme registry, `ResourceId`, `Incarnation`, and `Revision`.
- Provider registration and capability discovery.
- Namespace mount/unmount lifecycle with parent/child invariants.
- Stable inode mapping for VFS projections; no inode becomes a public semantic identity.
- Resource metadata model: kind, mutability, size, timestamps, encoding, content type, capabilities, and retention.
- Fuzz tests for URI parsing and canonicalization.

### Slice 1 exit gate

A test can register two providers, mount overlapping-looking names, resolve canonical URIs unambiguously, project different capabilities to two callers, and prove that unmounting one provider cannot redirect an old URI to another resource.

---

## Slice 2 — State, snapshots, revisions, and transactions

Reads, edits, process output, and AST anchors all require a coherent notion of observed state. Build this before high-level tools.

### D11. State authority

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Direct host state** | Each provider reads the host and callers accept races | 2 | 4 | 2 | 2 | **5** | 4 | 62 |
| **B. Kernel snapshot ledger** | Providers expose revisions; kernel records snapshots and transaction outcomes | **5** | **5** | **5** | **5** | 3 | 3 | **93** |
| **C. Immutable copy-on-write workspace** | Every change produces a complete new tree | 5 | 4 | 5 | 4 | 1 | 2 | 77 |

**Recommendation: B.** Use provider-native snapshots when available, content hashes and revision tokens otherwise. Copy-on-write remains an implementation technique for selected stores, not the universal model.

### D12. Mutation transaction model

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Sequential scalar writes** | Each edit is immediately committed | 2 | 3 | 2 | 3 | 4 | **5** | 59 |
| **B. Per-resource atomic transactions** | All compatible mutations against one snapshot validate then commit together | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Global multi-resource transactions** | Arbitrary resources commit in one distributed transaction | 5 | 5 | 5 | 4 | 1 | 1 | 77 |

**Recommendation: B, with explicit composition for cross-resource workflows.** Same-resource edit/insert batches are atomic. Independent resources may execute concurrently. Mixed write/edit conflicts unless a provider can prove a safe transaction.

### D13. Conflict strategy

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Last writer wins** | New content replaces earlier content | 1 | 3 | 1 | 3 | 5 | **5** | 54 |
| **B. Revision-checked commit** | Mutation names the observed revision; stale commit returns conflict data | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Automatic three-way merge** | Kernel tries to reconcile all stale mutations | 3 | 4 | 3 | 3 | 3 | 2 | 64 |

**Recommendation: B.** Automatic merge belongs in an explicit edit tool or user policy, never as invisible kernel behavior. Conflict results include current revision, requested base revision, affected ranges, and safe next actions.

### D14. Content representation

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. UTF-8 text only** | All writable resources are strings | 3 | 3 | 3 | 4 | **5** | **5** | 75 |
| **B. Typed bytes with text projection** | Binary is authoritative; text operations require a validated encoding projection | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. MIME/document object model** | Every resource is a rich typed document tree | 4 | 5 | 4 | 3 | 2 | 2 | 75 |

**Recommendation: B.** Text is first-class because coding tools need it, but the kernel must not corrupt binary resources or silently decode lossy data.

### D15. Revision granularity

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---|---:|---:|---:|---:|---:|---:|
| **A. Whole-resource hash only** | Any change invalidates the complete snapshot | 4 | 3 | 4 | 4 | **5** | 4 | 79 |
| **B. Whole-resource revision plus anchors/ranges** | Revision protects commits; stable anchors improve navigation and small diffs | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Line-level CRDT identity** | Every line/character has collaborative identity | 5 | 5 | 4 | 4 | 2 | 1 | 80 |

**Recommendation: B.** Reserve CRDT identity for an explicitly collaborative store. Do not force distributed text machinery into every provider.

### Slice 2 deliverables

- Revision ledger and snapshot handles with retention rules.
- Read-at-revision and commit-at-revision interfaces.
- Atomic mutation planner with overlap, ordering, and conflict validation.
- Byte/text projections, line ending policy, Unicode rules, and exact-write semantics.
- Stable anchors defined as semantic occurrences plus fallback byte/line coordinates; anchors are never raw line numbers alone.
- Diff model that is machine-readable, bounded, and tied to revisions.
- Crash-safe commit protocol and recovery journal for providers that need it.

### Slice 2 exit gate

Concurrent tests prove: stale edits never overwrite newer content; compatible sibling edits commit once; incompatible operations leave the resource unchanged; read results identify their revision; restart recovery produces either the old or new committed state, never a torn hybrid.

---

## Slice 3 — Operation algebra and asynchronous execution

Define operations independently of any particular verb name or model provider.

### D16. Operation model

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Ad hoc methods** | Each provider exposes custom async methods | 2 | 2 | 2 | 2 | 4 | 3 | 50 |
| **B. Typed operation algebra** | Request, response, error, context, progress, cancellation, and capabilities are explicit | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Generic command bus** | Everything is a string-keyed command/event | 3 | 5 | 3 | 3 | 3 | 3 | 70 |

**Recommendation: B.** A generic dispatcher may implement the algebra, but the semantic layer must retain typed operation identities and payloads.

### D17. Batch dimension

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Scalar everywhere** | One request per component/provider call | 3 | 3 | 4 | 4 | 1 | **5** | 64 |
| **B. Typed batch ABI, scalar presentation** | Programmatic operation is `list<Request> -> list<Result>`; model/UI calls remain scalar | **5** | **5** | **5** | **5** | **5** | 3 | **98** |
| **C. Model-visible batch JSON** | Callers serialize an outer requests array | 3 | 4 | 3 | 3 | 4 | 3 | 68 |

**Recommendation: B.** The assistant turn is the model-side batch envelope. Preserve source order and one-for-one results while grouping by exact operation/provider/generation.

### D18. Cancellation and timeout ownership

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Caller drops the future** | Cancellation is implicit and providers may continue | 2 | 2 | 2 | 2 | 4 | 4 | 50 |
| **B. Structured cancellation context** | Parent/child cancellation, deadlines, budgets, and terminal acknowledgment are explicit | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. External kill only** | Supervisor kills the worker on timeout | 3 | 3 | 4 | 3 | 2 | 3 | 63 |

**Recommendation: B backed by C.** Every operation receives a cancellation token and deadline; a supervisor can terminate a wedged worker, and the resulting state is recorded as termination rather than mistaken for ordinary cancellation.

### D19. Idempotency and retry

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Retry all failures** | Caller retries based on generic error text | 1 | 3 | 1 | 2 | 4 | 4 | 51 |
| **B. Operation-owned idempotency class and key** | Each request states whether retry is safe; durable operations deduplicate keys | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Exactly-once distributed execution** | Infrastructure guarantees exactly-once side effects | 5 | 4 | 5 | 4 | 1 | 1 | 76 |

**Recommendation: B.** Exactly-once is promised only for a provider transaction that can prove it; otherwise the contract distinguishes at-most-once, at-least-once, and unknown outcome.

### D20. Progress and observation

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Logs only** | Long-running operations emit unstructured logs | 2 | 3 | 2 | 2 | 4 | 4 | 54 |
| **B. Typed progress stream plus terminal result** | Progress is bounded, sequence-numbered, resumable, and separate from result | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Polling status records** | Callers repeatedly read a status resource | 4 | 4 | 4 | 4 | 2 | 4 | 76 |

**Recommendation: B plus a status resource.** Streaming is efficient; a durable status projection makes reconnect and support possible.

### Slice 3 exit gate

A fake provider can execute scalar and batch requests, emit progress, honor a deadline, be cancelled, return item-level errors, and prove retry behavior without any model or WASM runtime.

---

## Slice 4 — Contract language, schema derivation, and compatibility

The current tree is already oriented toward Wasmtime components and WIT-like family boundaries. Make the contract system authoritative before implementing extension packages.

### D21. Contract representation

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Rust traits as ABI** | Rust types and trait objects define all extensions | 4 | 2 | 3 | 3 | 4 | 4 | 70 |
| **B. WIT/component contracts** | Language-neutral typed records, variants, resources, and functions | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. JSON Schema plus RPC** | JSON schemas and a generic transport define extensions | 3 | 4 | 3 | 4 | 3 | **5** | 75 |

**Recommendation: B with generated Rust and JSON adapters.** WIT is the lossless programmatic contract; JSON Schema is derived for model, CLI, and remote convenience.

### D22. Type system for dynamic boundaries

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Serde-only conversion** | Deserialize into hand-written Rust structs | 3 | 3 | 2 | 3 | 4 | 4 | 64 |
| **B. Type-directed dynamic values** | Dynamic values carry their contract type; normalization precedes strict validation | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Untyped JSON passthrough** | Components/providers receive arbitrary JSON | 1 | 5 | 1 | 2 | 4 | 5 | 57 |

**Recommendation: B.** Normalization may tolerate unique naming/casing conventions, but validation is exact. Unknown fields, ambiguous aliases, invalid ranges, malformed URIs, and union-shape errors are item-level repairable failures.

### D23. Contract evolution

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. In-place version changes** | Consumers track the latest schema | 2 | 2 | 2 | 2 | 4 | 4 | 50 |
| **B. Versioned packages with compatibility declarations** | Major changes are new interfaces; adapters declare supported ranges | **5** | **5** | **5** | **5** | 3 | 3 | **93** |
| **C. Permanent translation gateway** | One canonical schema with translators for all historical forms | 4 | 4 | 4 | 5 | 2 | 2 | 76 |

**Recommendation: B, with bounded translation for important historical formats.** Activation rejects incompatible contracts before publication; active invocations pin the generation they started on.

### D24. Model schema derivation

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Hand-written per-tool schemas** | Agent code maps names to schemas | 3 | 1 | 3 | 2 | 4 | 2 | 52 |
| **B. Generic derivation from scalar request type** | Validate batch ABI, retain batch types, advertise scalar request schema | **5** | **5** | **5** | **5** | 4 | **5** | **98** |
| **C. Expose the full batch type** | Model is told to emit request arrays | 3 | 4 | 3 | 3 | 4 | 4 | 67 |

**Recommendation: B.** Require exactly one batch input and one batch output of `list<Result<...>>` for advertised tool packages. Derive the scalar schema from the item request type; never add a model-visible batch wrapper.

### D25. Authoritative result versus model observation

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. One serialized result for everyone** | Model, recorder, and programmatic callers receive the same payload | 3 | 3 | 3 | 3 | 2 | **5** | 59 |
| **B. Dual plane** | Typed authoritative result plus package-owned compact `stdobs` | **5** | **5** | **5** | **5** | **5** | 3 | **97** |
| **C. Model text only** | Result is rendered into text and reparsed if needed | 1 | 2 | 1 | 1 | **5** | 4 | 43 |

**Recommendation: B.** Capture and automation consume typed values; the model consumes bounded observations. The observer is activated and generation-pinned with the operation, not hard-coded in the agent by tool name.

### Slice 4 deliverables

- Shared WIT package for URI, revision, anchors, errors, progress, capabilities, and operation context.
- Per-operation WIT packages with typed batch functions.
- `DynamicType`/`DynamicValue` representation and type-directed normalization.
- Schema fingerprinting, contract manifest, compatibility checker, and activation validator.
- Generated bindings checked into build output deterministically or generated in a reproducible build step.
- Observer contract and bounded rendering rules.

### Slice 4 exit gate

A deliberately malformed extension is rejected before publication. A valid extension produces a scalar model schema, a typed batch ABI, an observer, and a compatibility fingerprint without hand-written agent special cases.

---

## Slice 5 — WASM runtime, component lifecycle, and capability host

The current Wasmtime engine and extension-family crates are scaffolding. Complete the runtime as a product subsystem, not merely a loader.

### D26. Extension packaging

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Raw component bytes** | The user points at a `.wasm` file | 3 | 3 | 2 | 2 | 5 | **5** | 62 |
| **B. Signed package manifest plus component** | Metadata, contract fingerprints, permissions, hashes, provenance, and component are one installable unit | **5** | **5** | **5** | **5** | 3 | 3 | **94** |
| **C. Native dynamic libraries** | Extensions load as host-language libraries | 3 | 3 | 1 | 2 | 5 | 3 | 56 |

**Recommendation: B.** Raw components remain a low-level import format, but activation requires a manifest or derives one into a quarantined inspection state.

### D27. Component lifecycle

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. One live instance forever** | Component state is global to its loaded instance | 3 | 3 | 2 | 2 | **5** | 3 | 62 |
| **B. Prepared generation plus scoped instances** | Compilation, validation, and resources are prepared; invocation instances are scoped and replaceable | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Fresh process for every call** | Maximum reset, maximum startup and IPC | 5 | 4 | 5 | 5 | 1 | 2 | 82 |

**Recommendation: B.** Keep compiled components and validated metadata hot; scope mutable store state to an invocation or declared actor. Use worker processes for components requiring stronger fault isolation.

### D28. Extension family model

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. One universal extension interface** | Nouns, verbs, and events share one giant interface | 3 | 3 | 3 | 3 | 3 | 2 | 61 |
| **B. Orthogonal noun/verb/event contracts** | A component may implement one or more small contract families | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Separate product per family** | Family implementations cannot share a component or host lifecycle | 4 | 2 | 4 | 3 | 2 | 3 | 66 |

**Recommendation: B.** Classification may remain useful, but exported interfaces—not a closed host enum—define capabilities. Specific verbs are packages under the verb family.

### D29. WASI filesystem view

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Direct host directory preopens** | WASI maps directly to real directories | 3 | 4 | 2 | 4 | **5** | 4 | 69 |
| **B. Kernel VFS-backed WASI projection** | WASI filesystem imports operate over the capability-filtered kernel graph | **5** | **5** | **5** | **5** | 4 | 3 | **95** |
| **C. No filesystem in extensions** | Extensions use custom host calls only | 4 | 2 | 5 | 3 | 2 | 3 | 72 |

**Recommendation: B.** It preserves one resource authority and lets extensions read namespaces without learning host-path details. Mutating WASI APIs must be capability-checked and mapped to kernel transactions rather than silently becoming host writes.

### D30. Hot reload and generation pinning

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Replace in place** | New bytes immediately replace the old instance | 2 | 3 | 2 | 2 | 4 | 4 | 53 |
| **B. Immutable prepared generations** | Validate and prepare N+1, atomically publish, let N drain | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Restart the whole supervisor** | Every extension update restarts all work | 4 | 3 | 4 | 3 | 1 | 4 | 67 |

**Recommendation: B.** A call pins its generation for its complete batch and observer. New calls see the published generation; draining generations have explicit deadlines and termination records.

### Slice 5 exit gate

- Component fuel, memory, table, stack, wall-clock, output, and concurrency budgets are enforced.
- A component cannot access a resource outside its capability projection.
- Invalid exports, missing observers, incompatible types, and unsafe manifests fail activation atomically.
- A hot swap does not change the result of an in-flight call.
- Runtime metrics identify compile, instantiation, host-call, fuel, and termination costs.

---

## Slice 6 — Provider implementations and universal resource operations

Implement providers behind the kernel contracts. The initial provider set should be broad in architectural coverage: host files, directories, processes, sessions, in-memory/transient streams, extension namespaces, and remote proxies.

### D31. Filesystem provider strategy

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Kernel owns every OS syscall** | Filesystem semantics and syscalls live in the kernel crate | 4 | 2 | 4 | 3 | 4 | 3 | 72 |
| **B. Provider adapter with explicit capability root** | Kernel defines semantics; OS adapter owns platform details | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Shell/CLI subprocess adapter** | Use external commands for filesystem behavior | 2 | 3 | 1 | 2 | 2 | 4 | 47 |

**Recommendation: B.** Keep host path resolution, symlink policy, permissions, atomic rename, and platform-specific metadata in the filesystem provider. The kernel sees typed operations and structured failures.

### D32. Process resource representation

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. One process resource with merged output** | Process and all streams are one readable object | 2 | 3 | 2 | 3 | 5 | 4 | 61 |
| **B. Process root plus stdin/stdout/stderr/control child resources** | Streams and control are addressable independently | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Terminal emulator as the only process API** | PTY bytes are the canonical process model | 3 | 3 | 3 | 3 | 4 | 2 | 61 |

**Recommendation: B, with PTY as an optional provider mode.** Root exposes lifecycle and exit status; child streams preserve separation, anchors, backpressure, and retention. Control commands are typed writes, not magic stdin bytes.

### D33. Process spawning authority

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Ambient host process execution** | Any caller can launch an OS process | 2 | 4 | 1 | 3 | **5** | 4 | 62 |
| **B. Declarative command policy plus capability grant** | Executable, arguments, environment, cwd, resources, and limits are explicit | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Container-only execution** | Every process is inside an external container | 4 | 4 | 5 | 4 | 2 | 2 | 78 |

**Recommendation: B backed by C where available.** Do not add a shell parser to the universal run operation. Arguments are argv; shell composition is an explicit shell/command provider with a separate policy.

### D34. Session resource model

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. In-memory chat transcript** | Session is a list held by the agent process | 3 | 3 | 2 | 2 | **5** | **5** | 63 |
| **B. Durable append-only event log plus projections** | Messages, tool calls, state transitions, and approvals are events | **5** | **5** | **5** | **5** | 4 | 3 | **95** |
| **C. Relational conversation database** | Session tables are canonical; events are secondary | 4 | 4 | 4 | 4 | 3 | 3 | 80 |

**Recommendation: B with a queryable projection.** The event log preserves replay and audit; projections serve fast context assembly and UI. Secrets and large binary outputs are referenced, not blindly embedded in transcript rows.

### D35. Universal operation vocabulary

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Closed built-in verb enum** | Kernel and agent hard-code all operations | 4 | 1 | 4 | 4 | 4 | **5** | 70 |
| **B. Open versioned operation IDs with built-in packages** | Built-ins are ordinary packages; registry and contracts remain open | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Fully opaque extension commands** | Even basic read/write semantics are extension-defined | 3 | 5 | 3 | 2 | 3 | 2 | 62 |

**Recommendation: B.** Ship a coherent universal set of resource operations, but do not encode that set as the system’s ontology. `find` and `grep` are operations over addressable resources; process and session input are writable child resources, not special hidden channels.

### Required universal semantic families

The exact package names can evolve, but the semantic families must cover:

- **Observation:** read, list, metadata, find, grep, structured inspect.
- **Mutation:** exact write, anchored edit, anchored insert, delete, rename/move where the provider supports it.
- **Execution:** spawn/run with argv, process state, stream reads, control, abort, and retention.
- **Coordination:** poll/wait, event subscription, claims/locks, approvals, and leases.
- **Analysis:** AST map/show, dependency/call/public-surface queries, context packing, structural search/rewrite, semantic retrieval.

Each operation declares resource capabilities, batch behavior, transaction behavior, retry class, side effects, result size limits, and observer.

### Slice 6 exit gate

The same kernel operation tests pass against in-memory, host filesystem, process, session, and extension-backed providers where applicable. FUSE and WinFsp are only projections: they cannot invent semantics or bypass kernel transactions.

---

## Slice 7 — AST, index, anchors, and code intelligence as kernel clients

The AST subsystem already has strong independent machinery. Integrate it without letting search or parsing become a second source of resource truth.

### D36. Analysis ownership

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. AST engine owns file walking and writes** | Analysis directly reads and mutates host paths | 3 | 3 | 2 | 3 | 4 | 4 | 64 |
| **B. AST engine is a provider/client over kernel snapshots** | Kernel supplies revisioned bytes; AST returns structured derived resources | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Remote indexing service only** | Local code intelligence always calls a separate service | 4 | 4 | 3 | 4 | 2 | 2 | 70 |

**Recommendation: B.** Preserve `artist-ast`’s language adapters and index algorithms, but give them snapshot identity and make stale results explicit.

### D37. Index storage

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Rebuild in memory for each query** | No durable index | 3 | 3 | 4 | 2 | 1 | **5** | 57 |
| **B. Versioned local persistent index with incremental deltas** | Content hashes, tombstones, atomic files, and model/schema metadata | **5** | **5** | **5** | **5** | **5** | 3 | **96** |
| **C. External search database** | Delegate storage and retrieval to a database service | 4 | 5 | 4 | 4 | 4 | 2 | 80 |

**Recommendation: B with a pluggable candidate-source seam.** The current AST index’s content hashing, incremental updates, tombstones, atomic persistence, BM25+dense candidates, and ranking split are good foundations; bind every record to workspace, source revision, parser schema, and embedding model identity.

### D38. Retrieval architecture

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Dense embeddings only** | Semantic model controls candidate generation and ranking | 3 | 3 | 2 | 3 | 4 | 4 | 64 |
| **B. Hybrid candidate sources plus independent ranking** | Lexical, dense, graph, exact-symbol, and future sources feed a common ranker | **5** | **5** | **5** | **5** | 4 | 3 | **95** |
| **C. External hosted search** | Search quality and data leave the local authority | 3 | 5 | 2 | 5 | 3 | 2 | 67 |

**Recommendation: B.** Keep candidate generation replaceable and ranking policy explicit. Search results must expose evidence, source revision, score components where useful, and scope filters.

### D39. Analysis freshness policy

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Best effort, silently stale** | Query returns whatever cached result exists | 2 | 4 | 2 | 2 | **5** | 4 | 59 |
| **B. Revision-aware freshness with explicit stale result** | Caller chooses wait, stale-allowed, or refresh policy | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Synchronous rebuild before every query** | Results are always fresh at high latency | 5 | 3 | 4 | 3 | 1 | 4 | 72 |

**Recommendation: B.** Never present stale symbols or anchors as current without labeling them. A tool can return a bounded stale result plus a refresh operation, or await a known index generation.

### D40. Structural rewrite semantics

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Text substitution** | Regex/string replacement is the rewrite primitive | 2 | 3 | 2 | 3 | **5** | **5** | 62 |
| **B. AST match -> anchored patch plan -> kernel transaction** | Structural matches produce explicit, reviewable edits | **5** | **5** | **5** | **5** | 4 | 3 | **95** |
| **C. Full compiler/refactoring service** | Compiler semantic model owns all rewrites | 5 | 4 | 5 | 4 | 2 | 2 | 80 |

**Recommendation: B.** AST code may propose patches; only the kernel commits them against a revision. Every rewrite reports matches, skipped/ambiguous nodes, diff, and new anchors.

### Slice 7 exit gate

- A parsed file and every derived result identify the source revision.
- Index invalidation is deterministic on content, parser/schema, configuration, and model changes.
- Search, graph, context, and structural rewrite calls work through resource/operation contracts.
- A stale analysis cannot commit an edit without revision validation.

---

## Slice 8 — Agent orchestration and model-facing tool ABI

Only after kernel operations, batching, observers, and resource providers are stable should the agent crate be implemented.

### D41. Agent loop ownership

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Provider SDK drives tool callbacks** | The LLM library invokes one tool at a time | 2 | 3 | 2 | 3 | 2 | 4 | 51 |
| **B. Artist-owned turn runner around provider events** | Provider streaming is preserved; Artist owns CallModel/CallTools/Done and batch planning | **5** | **5** | **5** | **5** | **5** | 3 | **97** |
| **C. Custom model protocol and transport** | Artist replaces provider SDKs entirely | 4 | 4 | 3 | 3 | 2 | 1 | 65 |

**Recommendation: B.** Use the provider library for transports and model protocol details, but intercept the complete same-turn tool-call set before execution. Never approximate a turn batch with timing or tool concurrency alone.

### D42. Same-turn batching

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Parallel scalar execution** | Each call runs independently, possibly concurrently | 3 | 3 | 3 | 4 | 3 | 4 | 64 |
| **B. Deterministic plan/group/execute/remap** | Resolve, normalize, pin, group by operation+generation, invoke typed batch, restore source order | **5** | **5** | **5** | **5** | **5** | 3 | **98** |
| **C. Timing/window heuristic** | Calls arriving within a time window are considered a batch | 2 | 3 | 2 | 1 | 3 | 2 | 43 |

**Recommendation: B.** Preserve internal and provider call IDs, source index, authoritative value, observation, operation ID, and generation for every result.

### D43. Model input normalization

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Strict provider JSON only** | Any naming or shape mismatch fails immediately | 4 | 2 | 4 | 3 | 4 | 4 | 70 |
| **B. Type-directed tolerant normalization then strict validation** | Unique aliases and harmless spelling differences are repairable; ambiguity is rejected | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Broad coercion** | Strings become numbers/bools/objects as needed | 2 | 4 | 2 | 2 | 4 | 3 | 56 |

**Recommendation: B.** Record both original and normalized arguments for audit. Do not guess between materially different operations.

### D44. Tool catalog publication

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Rebuild catalog during invocation** | Active tools are discovered lazily | 2 | 3 | 2 | 1 | 2 | 3 | 45 |
| **B. Immutable prepared catalog generations** | Schemas, observers, components, permissions, and dispatch handles are validated before publication | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Static catalog baked into the agent binary** | Extension tools require a binary rebuild | 4 | 1 | 4 | 4 | 4 | **5** | 72 |

**Recommendation: B.** A catalog snapshot is atomically published. Calls pin a catalog generation. There are no non-timeout cold misses for schema derivation or component preparation.

### D45. Agent result protocol

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Text-only tool result** | The model receives rendered text and the agent reparses it | 1 | 2 | 1 | 1 | **5** | 4 | 43 |
| **B. Model observation + structured outcome + authoritative extension** | Rig/provider gets compact output while recorder and control logic get typed metadata | **5** | **5** | **5** | **5** | **5** | 3 | **97** |
| **C. Full typed JSON sent to model** | Exact values are serialized into model context | 4 | 3 | 3 | 3 | 1 | 4 | 60 |

**Recommendation: B.** Keep observations short, bounded, and package-owned. Never force a model to carry all authoritative data merely to preserve it for the host.

### D46. Agent state machine

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Recursive prompt/tool calls** | Each tool call recursively invokes another prompt loop | 2 | 2 | 2 | 2 | 3 | 3 | 48 |
| **B. Explicit turn state machine** | Model call, tool planning, batch execution, result insertion, steering, cancellation, and completion are states | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Workflow engine as the agent** | Durable workflow runtime owns all conversation control | 4 | 4 | 4 | 5 | 2 | 2 | 75 |

**Recommendation: B, with workflow integration for durable long-running tasks.** The core loop remains inspectable and testable without a workflow service.

### Slice 8 exit gate

A fake provider emits multiple sibling tool calls in one assistant turn. The runner groups equivalent calls into one native batch, executes mixed tools concurrently where safe, coalesces same-resource mutations correctly, maps results back in source order, preserves IDs, and records both planes of result data.

---

## Slice 9 — LLM provider abstraction, credentials, and model policy

The current provider crate is empty but already separated from the agent. Preserve that boundary and make provider behavior replaceable.

### D47. Provider abstraction

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. One vendor SDK everywhere** | Agent types depend directly on one provider | 3 | 1 | 2 | 3 | 4 | 4 | 59 |
| **B. Provider-neutral event and capability protocol** | Vendor adapters translate streaming, tools, usage, errors, and model metadata | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Generic OpenAI-compatible HTTP only** | All providers are forced through one approximate protocol | 3 | 4 | 3 | 4 | 4 | **5** | 73 |

**Recommendation: B.** Compatibility adapters are welcome, but the internal protocol must represent provider-specific capabilities honestly rather than flattening them into lowest-common-denominator behavior.

### D48. Credential storage

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Environment/config files** | Secrets are read from ordinary process configuration | 3 | 3 | 2 | 2 | 5 | **5** | 61 |
| **B. OS secret store plus explicit ephemeral injection** | Durable credentials use platform secret storage; workers receive scoped short-lived material | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Central secret service always required** | All credentials live in a remote vault | 5 | 5 | 4 | 5 | 2 | 2 | 80 |

**Recommendation: B.** Environment variables remain an import path, never the only storage guarantee. Secrets are redacted from logs, events, crash reports, tool observations, and session projections.

### D49. Model context policy

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Transcript truncation** | Drop oldest messages when the provider limit is reached | 2 | 3 | 2 | 2 | **5** | **5** | 60 |
| **B. Typed context planner** | Budgeted selection of messages, tool observations, source evidence, summaries, and durable references | **5** | **5** | **5** | **5** | 4 | 3 | **95** |
| **C. External memory database decides context** | Retrieval service owns all context selection | 3 | 5 | 3 | 4 | 3 | 2 | 70 |

**Recommendation: B.** Reuse AST context packing concepts, but make context entries typed, revision-aware, provenance-bearing, and redaction-aware. The model should receive evidence, not untraceable summaries alone.

### D50. Usage and budget accounting

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Provider-reported tokens only** | Trust vendor usage fields | 3 | 3 | 2 | 3 | 4 | 4 | 63 |
| **B. Normalized usage ledger with estimates and provider facts** | Requests, responses, tools, embeddings, wall time, and cost are recorded with provenance | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Local byte heuristics only** | Estimate all usage from serialized data | 2 | 3 | 2 | 2 | 5 | 4 | 54 |

**Recommendation: B.** Distinguish provider facts from estimates, preserve privacy policy, and enforce budgets before calls where possible.

### Slice 9 exit gate

The agent can swap providers without changing kernel operations, tool contracts, session event shape, or cancellation semantics. Provider failures classify retryability and preserve partial streaming state safely.

---

## Slice 10 — Durable sessions, replay, approvals, and collaboration

### D51. Session event storage

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Mutable transcript rows** | Current state is updated in place | 3 | 3 | 2 | 2 | 4 | 4 | 58 |
| **B. Append-only log with snapshots and projections** | Every significant transition is durable and replayable | **5** | **5** | **5** | **5** | 4 | 3 | **95** |
| **C. CRDT log for all state** | Every session field is collaborative and mergeable | 5 | 5 | 4 | 4 | 2 | 1 | 79 |

**Recommendation: B.** A collaborative projection may later use CRDT techniques, but the authoritative event model should remain explicit and auditable.

### D52. Approval and steering

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Boolean confirmation callback** | A UI returns yes/no for a tool call | 2 | 2 | 2 | 2 | 4 | **5** | 52 |
| **B. Typed approval policy and decision resource** | Proposed action, scope, expiry, actor, evidence, and decision are durable | **5** | **5** | **5** | **5** | 4 | 3 | **95** |
| **C. Human review through external ticketing only** | Every approval is delegated to another system | 4 | 4 | 4 | 4 | 2 | 2 | 72 |

**Recommendation: B.** Approval is a resource/event with scope and expiry. A UI, CLI, API client, or policy engine can render or decide it.

### D53. Replay semantics

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Best-effort transcript replay** | Re-prompt from recorded text | 2 | 3 | 2 | 2 | 4 | 4 | 57 |
| **B. Deterministic event replay with external-effect markers** | Internal state can replay; external calls are represented as recorded or re-executed by policy | **5** | **5** | **5** | **5** | 3 | 3 | **93** |
| **C. Full time-travel sandbox** | Replay reconstructs every host effect | 5 | 5 | 5 | 4 | 1 | 1 | 77 |

**Recommendation: B.** Label nondeterministic provider, process, clock, and network effects. Replaying a session must never accidentally repeat an irreversible effect.

### D54. Multi-actor coordination

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Single-owner session** | One agent and one UI own all writes | 4 | 2 | 4 | 3 | 4 | **5** | 70 |
| **B. Claims, leases, and ordered event streams** | Multiple agents/users can coordinate without hidden write races | **5** | **5** | **5** | **5** | 3 | 3 | **93** |
| **C. Collaborative CRDT workspace** | Concurrent edits merge automatically everywhere | 4 | 5 | 4 | 4 | 2 | 1 | 75 |

**Recommendation: B.** Explicit claims and leases cover agents, humans, background indexers, and remote clients. Automatic merging is an operation policy, not an invisible default.

### Slice 10 exit gate

A session can be stopped, restarted, resumed from a new client, inspected as events and projections, and replayed without repeating external side effects. Approval decisions, capability grants, tool calls, results, and mutations are correlated by stable IDs.

---

## Slice 11 — User-facing surfaces and transport adapters

Implement CLI/TUI, machine API, remote transport, and filesystem mounts as adapters over the same kernel/agent protocols.

### D55. Machine transport

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. CLI stdout protocol** | Scripts invoke the binary and parse output | 3 | 3 | 2 | 3 | 3 | **5** | 62 |
| **B. Versioned typed RPC plus CLI adapter** | Streaming request/event protocol with structured errors and resumable cursors | **5** | **5** | **5** | **5** | 4 | 3 | **95** |
| **C. GraphQL/document API** | Clients query a flexible resource graph | 4 | 4 | 3 | 4 | 3 | 2 | 72 |

**Recommendation: B.** The CLI is a first-class client, not the wire contract. Provide local IPC and network transports with the same envelopes, authentication, backpressure, and event cursors.

### D56. CLI/TUI architecture

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. UI owns workflows** | Commands directly manipulate provider/session internals | 2 | 2 | 2 | 2 | 4 | 3 | 50 |
| **B. UI as a pure protocol client** | TUI, batch CLI, and future GUI render typed events and submit commands | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Embedded web UI** | Browser frontend is the primary client | 4 | 4 | 3 | 4 | 3 | 2 | 73 |

**Recommendation: B.** A web UI can be added later without changing semantics. Terminal rendering must not become the only observation format.

### D57. Filesystem mount role

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Mount is the primary API** | All operations are forced through POSIX/Windows filesystem calls | 2 | 3 | 2 | 3 | 4 | 4 | 56 |
| **B. Mount is a projection** | VFS mount exposes compatible read/list semantics; typed API remains authoritative for richer operations | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. No mount adapter** | Product uses only RPC/CLI APIs | 4 | 2 | 5 | 4 | 2 | 4 | 72 |

**Recommendation: B.** Keep FUSE and WinFsp bridges thin. Document what cannot be represented through filesystem semantics, especially revisions, approvals, process control, and atomic multi-call batches.

### D58. Event delivery

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Ephemeral notifications** | Missed events are simply missed | 2 | 3 | 2 | 2 | **5** | 4 | 58 |
| **B. Sequence-numbered streams with replay cursors** | Subscribers resume from a durable cursor and receive gap markers | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Polling only** | Clients repeatedly query current state | 4 | 3 | 4 | 4 | 2 | **5** | 74 |

**Recommendation: B plus polling snapshots.** Event streams are ordered per resource/session/tenant partition; clients can detect compaction and rehydrate from a snapshot.

### Slice 11 exit gate

The same operation can be invoked from Rust tests, CLI, local RPC, remote RPC, and supported filesystem projections. Every client handles reconnect, backpressure, structured errors, and capability-denied operations consistently.

---

## Slice 12 — Persistence, migrations, and recovery

### D59. Durable storage engine

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Files and ad hoc JSON** | Each crate owns its own files | 2 | 3 | 2 | 2 | 4 | **5** | 55 |
| **B. Storage abstraction with transactional local backend** | Events, metadata, indexes, blobs, and projections have explicit stores | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. External relational database everywhere** | Even local installs require a database service | 5 | 5 | 4 | 5 | 3 | 2 | 83 |

**Recommendation: B.** A local backend may use an embedded database plus content-addressed files; remote deployments can map the same logical stores to durable services. Do not let storage choice leak into operation semantics.

### D60. Large content and artifacts

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Inline all content in events** | Messages and file bodies are copied into logs | 3 | 2 | 2 | 2 | 1 | 4 | 48 |
| **B. Content-addressed blob store plus references** | Events carry digest, metadata, retention, and access capability | **5** | **5** | **5** | **5** | **5** | 3 | **95** |
| **C. Temporary files with path references** | Payloads live in uncontrolled temp locations | 2 | 3 | 1 | 2 | 4 | 4 | 53 |

**Recommendation: B.** Redact or encrypt sensitive blobs, enforce size/retention policies, and make references portable across local/remote deployments.

### D61. Migration policy

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Delete and rebuild caches/state** | Incompatible data is discarded | 2 | 2 | 2 | 2 | 4 | **5** | 52 |
| **B. Versioned forward migrations with quarantine fallback** | Data migrates transactionally; unsupported/corrupt data is preserved for repair | **5** | **5** | **5** | **5** | 3 | 3 | **93** |
| **C. Read every historical version forever** | Runtime contains all old readers | 4 | 4 | 4 | 5 | 2 | 1 | 75 |

**Recommendation: B.** Every persisted artifact has schema, producer, dependency/model fingerprints, checksum, creation time, and migration path. Failed migration never destroys the source.

### D62. Crash recovery

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Best effort** | Restart and hope partial files are harmless | 1 | 3 | 1 | 1 | 4 | 4 | 46 |
| **B. Journal/manifest commit protocol** | Durable commit intent and atomic publication identify valid generations | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Full replicated log** | Every local state change is replicated before acknowledgement | 5 | 5 | 5 | 5 | 1 | 1 | 77 |

**Recommendation: B.** Use directory or manifest generation swaps where possible, fsync required metadata, and expose recovery status rather than hiding it.

### Slice 12 exit gate

Power-loss and injected-failure tests cover event log, blob writes, index updates, session projections, extension catalogs, and resource mutations. Recovery either resumes, rolls back, or quarantines with a structured explanation.

---

## Slice 13 — Security, privacy, and policy plane

Security is architectural because it changes every resource and operation contract.

### D63. Policy placement

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. UI-side confirmation** | UI decides whether a tool looks safe | 2 | 2 | 1 | 2 | 4 | 4 | 47 |
| **B. Kernel policy decision point** | Kernel evaluates actor, resource, operation, capability, and context | **5** | **5** | **5** | **5** | 4 | 3 | **95** |
| **C. OS/container policy only** | Application trusts the host sandbox | 3 | 3 | 4 | 3 | 3 | 3 | 68 |

**Recommendation: B backed by host policy.** UI approval is an input to policy, not the enforcement point.

### D64. Secret and sensitive-content handling

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Redact only at display time** | Internal logs and events contain raw content | 2 | 3 | 1 | 3 | 4 | 4 | 57 |
| **B. Sensitivity labels and propagation** | Resources, blobs, events, context entries, and observations carry redaction/export policy | **5** | **5** | **5** | **5** | 3 | 3 | **93** |
| **C. Encrypt everything with no labels** | Encryption is universal but consumers cannot reason about handling | 4 | 4 | 4 | 4 | 2 | 2 | 76 |

**Recommendation: B with encryption at rest and in transit.** Labels must prevent accidental model, log, event, or remote export.

### D65. Telemetry policy

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Collect everything by default** | Full prompts, source, outputs, and environment are sent to telemetry | 2 | 3 | 1 | 5 | 4 | 4 | 60 |
| **B. Local structured telemetry, opt-in content diagnostics** | Metrics/traces are safe by default; payload capture is explicit and redacted | **5** | **5** | **5** | **5** | 4 | 3 | **95** |
| **C. No telemetry** | Product emits no operational signals | 3 | 2 | 5 | 1 | 4 | **5** | 67 |

**Recommendation: B.** Define a telemetry schema, sampling, retention, tenant boundaries, and a “never capture” class before adding integrations.

### Slice 13 exit gate

Threat-model tests cover path traversal, symlink escape, capability confusion, stale capability reuse, malicious component imports, secret leakage through observations, prompt/tool injection from repository text, event authorization, and remote replay.

---

## Slice 14 — Concurrency, resource limits, and scheduler

### D66. Scheduling model

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Unbounded Tokio spawning** | Every request starts a task | 2 | 3 | 1 | 2 | 4 | 4 | 53 |
| **B. Typed fair scheduler with budgets and lanes** | Work is admitted by tenant/session/resource class and backpressure | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. One global queue** | All work shares one FIFO queue | 3 | 3 | 3 | 3 | 2 | **5** | 61 |

**Recommendation: B.** Distinguish interactive agent work, indexing, process IO, extension calls, persistence, and maintenance. Fairness and cancellation are observable policy decisions.

### D67. Backpressure

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Drop when buffers fill** | New events/results are discarded | 1 | 3 | 1 | 2 | 5 | 4 | 52 |
| **B. Bounded queues with explicit overflow policy** | Block, coalesce, spool, or fail according to event class | **5** | **5** | **5** | **5** | 4 | 3 | **94** |
| **C. Unbounded durable queues** | Never drop; disk grows without a declared cap | 4 | 4 | 3 | 4 | 2 | 3 | 69 |

**Recommendation: B.** No silent data loss. A progress event may coalesce; an authoritative result or mutation event may spool or fail the operation.

### D68. Resource budgets

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Timeouts only** | Wall clock is the only limit | 2 | 3 | 2 | 3 | 4 | 4 | 61 |
| **B. Multi-dimensional budget ledger** | Time, CPU/fuel, memory, output, disk, network, concurrency, and tokens are tracked | **5** | **5** | **5** | **5** | 4 | 3 | **95** |
| **C. Host cgroups/job objects only** | OS resource controls are the only budget interface | 3 | 3 | 4 | 3 | 3 | 3 | 68 |

**Recommendation: B backed by OS controls.** Budgets propagate parent-to-child and are included in terminal outcomes.

### Slice 14 exit gate

Load tests demonstrate bounded memory and queues, fair progress under indexing and process output, cancellation propagation, and deterministic terminal results when any budget is exhausted.

---

## Slice 15 — Verification, compatibility, and release engineering

### D69. Contract verification strategy

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Unit tests in each crate** | Local implementation tests are the primary evidence | 3 | 3 | 3 | 3 | 4 | **5** | 69 |
| **B. Layered tests plus cross-language conformance** | Unit, property, model, fault-injection, component, transport, and end-to-end tests share fixtures | **5** | **5** | **5** | **5** | 4 | 3 | **95** |
| **C. Golden end-to-end transcripts only** | A small number of complete scenarios define behavior | 3 | 3 | 2 | 3 | 2 | 2 | 52 |

**Recommendation: B.** Add a conformance suite that exercises every operation through Rust, generated bindings, a non-Rust WASM guest, dynamic JSON, and at least one remote transport.

### D70. Determinism policy

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Best-effort concurrency** | Ordering varies but callers are expected to tolerate it | 2 | 3 | 2 | 2 | 4 | 4 | 55 |
| **B. Deterministic semantic ordering, concurrent execution where independent** | IDs, source order, transaction ordering, and replay are stable | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Serialize all work** | Avoid nondeterminism by removing concurrency | 5 | 2 | 5 | 4 | 1 | 4 | 76 |

**Recommendation: B.** Determinism applies to result ordering, conflict resolution, event sequence, generated identifiers under test, catalog publication, and persistence—not to wall-clock completion order.

### D71. Release compatibility

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Lockstep binary releases** | Kernel, extensions, clients, and indexes upgrade together | 4 | 2 | 4 | 3 | 4 | 4 | 72 |
| **B. Compatibility matrix and migration tooling** | Protocol, WIT, package, persisted data, and client versions have explicit support ranges | **5** | **5** | **5** | **5** | 3 | 3 | **94** |
| **C. Rolling compatibility forever** | Every component must work with every historical version | 4 | 5 | 4 | 5 | 2 | 1 | 76 |

**Recommendation: B.** Publish supported version ranges and refusal diagnostics. Compatibility is a product capability with tests, not a hope.

### D72. Supply-chain and artifact provenance

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Build from local source without attestations** | Users trust whatever was built | 3 | 3 | 2 | 2 | 4 | 4 | 60 |
| **B. Reproducible builds, signed artifacts, dependency audit, SBOM** | Release artifacts and extension packages have verifiable provenance | **5** | **5** | **5** | **5** | 3 | 3 | **93** |
| **C. Central vendor-only distribution** | All artifacts are served by one trusted distributor | 4 | 3 | 4 | 5 | 3 | 2 | 75 |

**Recommendation: B.** Verify WASM component hashes, manifests, WIT fingerprints, and dependency licenses before activation or installation.

### D73. Platform adapter policy

| Option | Shape | C | X | S | O | P | M | Weighted |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **A. Platform-neutral lowest common denominator** | Avoid capabilities not shared by every host | 3 | 2 | 4 | 4 | 3 | **5** | 67 |
| **B. Shared semantic contract plus platform capability matrix** | Every host implements the common contract and declares native extensions/limitations | **5** | **5** | **5** | **5** | 4 | 4 | **96** |
| **C. Platform-specific implementations with separate behavior** | Each OS gets its own semantics | 3 | 3 | 3 | 2 | 4 | 2 | 58 |

**Recommendation: B.** FUSE, WinFsp, process APIs, path rules, notifications, and credential stores differ; the contract must expose capability differences rather than pretend they do not exist.

### Slice 15 exit gate

The release pipeline can build reproducibly, run the complete conformance/fault/security/performance suites, produce SBOM and signed artifacts, migrate or quarantine persisted data, and verify cross-platform capability declarations.

---

# Part III — Cross-cutting contracts that must exist everywhere

## 3.1 Identity and correlation

Every externally visible action carries:

- actor/principal ID;
- workspace/session ID;
- request ID;
- operation invocation ID;
- parent/causation ID;
- resource URI and incarnation where applicable;
- observed revision and committed revision where applicable;
- catalog/extension generation;
- provider and host identity;
- deadline, budget, and cancellation lineage.

IDs are opaque, stable, non-secret, and safe to log. Correlation must survive agent turn boundaries, remote reconnects, process restarts, and event projection.

## 3.2 Result shape

Every operation result distinguishes:

1. **Outcome:** success, item failure, cancellation, deadline, conflict, or unknown external outcome.
2. **Authoritative value:** typed and lossless where successful.
3. **Observation:** bounded rendering for a model or human surface.
4. **Effects:** resources changed, revisions, spawned work, emitted events, and retained artifacts.
5. **Provenance:** provider, generation, source revision, and policy decisions.

Never infer authoritative state by parsing the observation string.

## 3.3 Event shape

Events have a schema ID, sequence, timestamp source, causation, actor, resource, revision, sensitivity, payload/reference, and retention class. Event consumers must handle duplicates, gaps, compaction, and unknown future fields.

## 3.4 Resource capability declaration

Capabilities are structured, not booleans scattered across adapters. At minimum distinguish:

- read metadata/content;
- list/resolve children;
- append versus replace versus anchored mutation;
- delete/move;
- poll/subscribe;
- execute/spawn;
- control/abort;
- snapshot/read revision;
- transaction/conditional commit;
- export/share;
- sensitive-content classes.

A provider may deny a capability for policy, host limitation, resource state, or operation incompatibility; those causes must remain distinguishable.

## 3.5 Naming and discovery

Names exposed to models and users are projections of active package metadata. The registry must support aliases only when they are unique and non-ambiguous. Discovery must not expose internal host paths, secrets, component handles, or giant type graphs.

## 3.6 Resource retention

Process streams, session events, blobs, indexes, tool outputs, and snapshots need independent retention policies. A resource remains readable after abort only when its provider’s retention policy says so. Deletion is a state transition with audit evidence, not merely unlinking a path.

---

# Part IV — Concrete implementation sequence

This is the recommended order for implementation across crates. It is deliberately horizontal: every phase strengthens the entire system rather than shipping one user-visible feature in isolation.

## Phase 1 — Foundation and contracts

1. Add the shared domain crate or modules for URI, identity, revisions, errors, capabilities, operation context, budgets, and event envelopes.
2. Define the dependency direction: domain -> kernel -> providers/runtime -> agent/session/surfaces.
3. Add WIT packages and generated-binding policy.
4. Implement dynamic value/type validation and schema fingerprints.
5. Build contract/property tests before provider implementations.

**Gate:** all later crates can depend on stable domain contracts without importing agent/provider/UI types.

## Phase 2 — Kernel registry and projected resource graph

1. Replace vector-only namespace lookup with provider registration and canonical URI routing.
2. Add mounts, resource incarnations, capability projections, leases, and lifecycle events.
3. Keep inode mapping as a VFS concern.
4. Add in-memory provider fixtures that exercise every route and lifecycle transition.

**Gate:** two isolated callers can observe different projections of one resource graph.

## Phase 3 — Revision ledger and transaction engine

1. Implement snapshot handles, content hashes, revision tokens, and conditional reads/writes.
2. Implement same-resource mutation planning, overlap detection, atomic commit, and diff generation.
3. Add crash-safe provider commit helpers.
4. Integrate anchors with revision identity and stale-anchor errors.

**Gate:** concurrency, conflict, restart, and property tests pass independently of any model.

## Phase 4 — Async operation runtime

1. Implement typed scalar and batch invocation interfaces.
2. Add operation metadata: capabilities, side effects, idempotency, budgets, progress, and retry class.
3. Add structured cancellation, deadlines, terminal state, and item-level result mapping.
4. Add fair admission and bounded queues.

**Gate:** a synthetic operation proves batching, cancellation, progress, backpressure, and ordered results.

## Phase 5 — WASM component platform

1. Complete component manifest parsing, package fingerprints, export validation, and contract-family classification.
2. Prepare immutable component generations before publication.
3. Implement scoped stores, WASI filesystem projection, capability checks, fuel/memory/output limits, and worker termination.
4. Implement noun, verb, and event host adapters over the kernel contracts.
5. Add generation pinning and atomic hot swap.

**Gate:** a valid guest can use only its projected resources; an invalid or malicious guest cannot publish or escape; in-flight generations remain stable.

## Phase 6 — Core providers

1. Host filesystem provider with path/symlink/permission policy and atomic writes.
2. In-memory and transient stream providers for tests and process output.
3. Process provider with root/stdin/stdout/stderr/control children, argv semantics, exit state, retention, and OS-specific signal mapping.
4. Durable session provider with event log, projections, approvals, leases, and replay markers.
5. Remote proxy provider using the typed transport.

**Gate:** provider conformance suite is green on all supported host families and reports capability differences honestly.

## Phase 7 — AST and code-intelligence integration

1. Make AST reads revision-aware and route file access through kernel snapshot APIs.
2. Bind parse results, graphs, context packs, chunks, embeddings, and indexes to source revision and configuration fingerprints.
3. Preserve incremental index behavior but make persistence transactional and recovery-aware.
4. Expose AST/search/graph operations as normal typed packages with observers.
5. Route structural rewrites through patch plans and kernel conditional commits.

**Gate:** stale analysis cannot mutate current resources; search results carry provenance; index recovery is tested.

## Phase 8 — Agent runner and model tool surface

1. Implement active catalog generation and scalar schema derivation.
2. Implement type-directed normalization and concise repairable errors.
3. Implement the Artist-owned turn state machine around provider streaming.
4. Collect the complete same-turn call set; resolve, normalize, pin, group, batch, and remap.
5. Implement same-resource edit/insert coalescing and write/mutation conflict rules.
6. Add authoritative result extensions and package-owned observations.
7. Preserve steering, cancellation, memory, capture, provider call IDs, and event ordering.

**Gate:** adversarial fake-provider tests prove deterministic native batching and no model-visible batch wrapper.

## Phase 9 — Provider/auth/session integration

1. Implement provider-neutral streaming events, usage, errors, capability declarations, and model metadata.
2. Add secure credential storage and scoped worker injection.
3. Add context planner with source provenance, sensitivity propagation, and budget accounting.
4. Connect session events to agent turns, tool calls, approvals, and replay.
5. Test provider interruption, reconnect, partial output, and unknown external outcomes.

**Gate:** a session resumes with equivalent semantic state after agent/provider restart without replaying irreversible actions.

## Phase 10 — Transports and interfaces

1. Implement local typed IPC/RPC.
2. Implement remote streaming transport with authentication, authorization, cursor replay, backpressure, and version negotiation.
3. Rebuild CLI/TUI as a protocol client.
4. Complete FUSE and WinFsp projections and clearly document projection limits.
5. Add machine-readable batch CLI and diagnostics output.

**Gate:** every surface exercises the same kernel and agent contracts; no surface has privileged bypasses.

## Phase 11 — Persistence, migrations, security, and operations

1. Add storage abstraction, manifests, journals, atomic generation publication, and repair/quarantine flows.
2. Add schema migrations and compatibility matrix tests.
3. Add policy decision point, capability audit, secret redaction, sensitivity propagation, and export controls.
4. Add structured logs, metrics, traces, health/readiness, diagnostic bundles, and safe crash reports.
5. Add scheduler fairness, quotas, retention, and resource budget enforcement.

**Gate:** injected failures and security tests are first-class release blockers.

## Phase 12 — Release proof

1. Run cross-language WIT conformance, including a guest produced outside Rust.
2. Run host matrix tests for filesystem, process, clock, path, signal, notification, credential, and mount differences.
3. Run deterministic replay, fuzz, model-checking/property, load, soak, and power-loss tests.
4. Build signed reproducible artifacts and SBOMs.
5. Verify upgrade, rollback, migration, extension revocation, catalog swap, and session recovery procedures.
6. Publish operator and extension-author contracts from the same schemas used in validation.

**Gate:** production readiness is a proof bundle, not a green compile alone.

---

# Part V — Test architecture

## 5.1 Contract tests

- URI normalization and alias ambiguity.
- Dynamic type normalization for every primitive, option, list, tuple, record, enum, variant, flags, and result case.
- Batch one-for-one ordering and item-level failure.
- Generation pinning and catalog publication.
- Observer/authoritative-result agreement on provenance, not textual equality.
- Compatibility rejection and supported-version negotiation.

## 5.2 Resource and transaction tests

- Path traversal, symlink loops/escapes, Unicode names, case behavior, and permission errors.
- Read/write at revisions, stale anchors, overlapping edits, inserts at equal boundaries, delete races, and mixed mutation conflicts.
- Crash at every commit step; restart and inspect state.
- Provider capability matrix and unsupported-operation mapping.

## 5.3 Runtime and extension tests

- Malformed components, wrong exports, missing observers, incompatible WIT, oversized memory, fuel exhaustion, host-call denial, and cancellation.
- Component generation swap during active invocation.
- WASI filesystem projection over multiple namespaces.
- Event subscriber cancellation, replay, gap, compaction, and duplicate handling.

## 5.4 Agent and provider tests

- Multiple sibling calls in source order with mixed tools.
- Same-resource mutation batching and independent-resource concurrency.
- Provider stream interruption and late tool results.
- Invalid model arguments with concise repair instructions.
- Context budget pressure, sensitive-content redaction, and provenance preservation.
- Session restart and safe replay of external effects.

## 5.5 AST and index tests

- Parser adapter output against fixture languages already represented in the tree.
- Source revision mismatch and index invalidation.
- Incremental add/modify/remove, tombstone compaction, model/schema change, and corrupt index recovery.
- Search candidate-source contract, out-of-range IDs, deterministic ranking, and scope filtering.
- Structural rewrite preview, stale commit rejection, and anchor refresh.

## 5.6 Host and release tests

- Unix FUSE and Windows WinFsp projection behavior.
- Process signal, quoting/argv, environment, output separation, partial lines, and exit state across host families.
- Remote authentication, reconnect, cursor replay, backpressure, and version negotiation.
- Reproducible build, artifact signature, dependency/license audit, upgrade, downgrade refusal, and rollback.

---

# Part VI — Operational model

## Health dimensions

Health is multidimensional rather than one boolean:

- kernel registry and policy health;
- persistence/recovery health;
- extension catalog generation and draining generations;
- provider availability and freshness;
- session store and event lag;
- index freshness and model availability;
- scheduler queue depth and budget pressure;
- provider connectivity and credential validity;
- host adapter availability;
- security/audit pipeline health.

## Diagnostic bundle

A support bundle must be safe by construction. It includes versions, contract fingerprints, capability matrix, generation IDs, structured failure summaries, queue/budget metrics, persistence checksums, and redacted traces. It excludes source, prompts, credentials, raw tool payloads, and sensitive blobs unless the caller explicitly grants an export capability.

## Upgrade choreography

1. Inspect current persisted/catalog/protocol fingerprints.
2. Validate migrations and extension compatibility without publication.
3. Prepare the new kernel/provider/component/catalog generations.
4. Publish atomically.
5. Drain old invocations to their deadlines.
6. Migrate projections and indexes independently where possible.
7. Retain rollback/quarantine material until health gates pass.
8. Emit a durable upgrade event with operator-visible diagnostics.

---

# Part VII — Decision register and unresolved implementation details

The major forks above are resolved for planning purposes. These details still need design work inside the named slices; they are not invitations to reopen the whole architecture without evidence:

1. Exact URI grammar and scheme registration API.
2. Revision token encoding and provider-specific snapshot adapters.
3. WIT package layout, generated binding location, and package fingerprint algorithm.
4. Dynamic type memory representation and error path formatting.
5. Component manifest fields, signature algorithm, and trust-store configuration.
6. WASM store-data composition for filesystem, resource, event, and budget host views.
7. Worker supervision protocol and crash classification.
8. Process-group/job-object strategy per host family.
9. Session event schema and projection indexes.
10. Content-addressed blob layout, encryption/key rotation, and garbage collection.
11. AST parser/configuration fingerprint and index migration strategy.
12. Candidate-source/ranker plugin interface and model artifact cache policy.
13. Agent runner integration point with the selected provider library while preserving streamed events.
14. Provider capability negotiation and normalized usage schema.
15. Remote transport framing, authentication, cursor, and flow-control details.
16. CLI/TUI event rendering, terminal resize, and non-interactive output contracts.
17. Policy language versus programmatic policy API.
18. Telemetry schemas, sampling, redaction, and retention.
19. Release artifact channels, revocation, and emergency disablement.
20. Formal support matrix for host OS, filesystem, process, credentials, mounts, and runtime features.

Each detail must be resolved by a design record containing: rejected alternatives, scorecard update if assumptions changed, compatibility impact, migration impact, security impact, test plan, and implementation owner.

---

# Part VIII — Final target architecture in one page

The finished system should have this shape:

```text
                    ┌─────────────────────────────────────┐
                    │     CLI / TUI / RPC / mounts         │
                    └──────────────────┬──────────────────┘
                                       │ typed commands/events
                    ┌──────────────────▼──────────────────┐
                    │      Agent runner + session client   │
                    │ scalar model tools, turn batching,  │
                    │ context, observations, approvals    │
                    └──────────────────┬──────────────────┘
                                       │ typed operation ABI
          ┌────────────────────────────▼────────────────────────────┐
          │              Kernel resource and operation authority      │
          │ URI graph, capabilities, revisions, transactions,       │
          │ scheduler, events, policy, persistence, catalog          │
          └───────────────┬───────────────┬───────────────┬──────────┘
                          │               │               │
               ┌──────────▼──────┐ ┌────▼─────────┐ ┌────▼──────────┐
               │ Native providers │ │ WASM guests  │ │ Remote proxy  │
               │ files/processes/ │ │ nouns/verbs/ │ │ providers and │
               │ sessions/streams │ │ events/WASI  │ │ workers        │
               └──────────┬──────┘ └────┬─────────┘ └────┬──────────┘
                          │               │               │
                          └───────────────▼───────────────┘
                                  revisioned resources

          AST parsing / graphs / context / hybrid retrieval are providers
          over revisioned resources, never a second mutation authority.
```

The essential properties are:

- **One semantic authority:** the kernel owns resources, revisions, transactions, policy, and events.
- **Two deliberate interfaces:** lossless typed programmatic ABI and scalar model-facing JSON derived from it.
- **One extension story:** versioned WIT/component contracts, with orthogonal noun, verb, and event families.
- **One lifecycle story:** prepared immutable generations, pinned in-flight calls, atomic publication, and explicit draining.
- **One state story:** revision-aware reads, conditional writes, atomic same-resource mutations, durable events, and recoverable persistence.
- **One intelligence story:** AST/search/index results are revisioned derived resources and produce patch plans rather than privileged writes.
- **One operations story:** structured errors, budgets, cancellation, backpressure, provenance, audit, health, migrations, and diagnostics apply at every boundary.
- **Many projections:** CLI, TUI, RPC, remote clients, FUSE, WinFsp, providers, and model observations remain adapters rather than competing authorities.

That is the architecture to build outward toward, horizontally and in order, until every plane has a complete contract and a production proof rather than merely a working vertical demo.
