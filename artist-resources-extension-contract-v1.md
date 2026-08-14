# Artist `resources://` Resource Extension Contract v1

Status: implementation contract  
Scope: resource extensibility and routing only  
Non-goal: Brush / `bash://` implementation

## 0. Purpose

Artist already makes model/programmatic tools editable as WASM Components under `tools://`.

This slice makes the **resource system itself extensible and self-modifying**.

`resources://` is **not a registry of URI schemes**. It is a registry of **resource extensions**.

A resource extension may:

- introduce a new namespace, e.g. `bash://...`;
- extend an existing namespace with new addressable child resources, e.g. `file:///src/main.rs/symbols/Foo`;
- add support for a universal verb to resources it recognizes;
- reserve a synthetic resource subtree so an unsupported operation does not fall through to an unrelated provider;
- call other Artist resources through the same typed universal resource interfaces;
- import other typed WASM Component contracts and explicitly granted host capabilities;
- publish machine-readable resource documentation plus full Markdown prose.

Many active resource extensions may operate on the same URI scheme.

The kernel must never assign one provider owner to an entire scheme.

The routing unit is:

```text
(verb, resource URI) -> exactly one handling resource extension
```

Multiple extensions may coexist for the same scheme and may handle different URI subspaces and/or different verbs.

---

# 1. Keep the universal verb surface unchanged

Do not add resource-specific model verbs.

The universal surface remains:

```text
read
write
edit
poll
send
run
abort
delete
find
grep
```

Resource extensions implement any subset that makes sense.

There is no universal `ResourceKind` enum.

Do **not** add a closed-world type taxonomy such as:

```rust
enum ResourceKind {
    File,
    PullRequest,
    Issue,
    Symbol,
    Shell,
    ...
}
```

A namespace may contain heterogeneous resources with unrelated internal shapes.

Examples:

```text
repo://project/
repo://project/issues/123
repo://project/pulls/456
repo://project/pulls/456/diff/3

file:///src/main.rs
file:///src/main.rs/symbols/
file:///src/main.rs/symbols/Foo
file:///src/main.rs/symbols/Foo/callers

bash://run-tests
```

Interoperability is provided by the universal verbs and their typed results, not by a universal resource-kind hierarchy.

---

# 2. Canonical Artist resource-address grammar

Artist resource addressing has three ordered layers:

```text
base resource path
    + optional query
    + optional textual anchor
```

Canonical serialized order:

```text
scheme://authority/path?query#anchor
```

Examples:

```text
file:///src/main.rs
file:///src/main.rs/symbols/Foo
file:///src/main.rs/symbols/Foo/callers?limit=20
file:///src/main.rs/symbols/Foo/callers?limit=20#BlueHorse

repo://artist/issues/?state=open&label=bug
pr://123/diff/4?context=10#Quartz17
```

This ordering is mandatory.

This is invalid Artist syntax:

```text
file:///src/main.rs#BlueHorse?limit=20
```

The parser must reject Artist addresses that attempt to append query syntax after the fragment.

## 2.1 Path means addressable hierarchy

If something can have children that the caller can continue traversing into, it belongs in the URI path.

Correct:

```text
file:///src/main.rs/symbols/
file:///src/main.rs/symbols/Foo
file:///src/main.rs/symbols/Foo/callers
```

Do not encode a second hierarchical path language inside the query:

```text
file:///src/main.rs?symbols/Foo/callers
```

A query may contain data, but it does not establish Artist child-resource hierarchy.

## 2.2 Query means extension-defined modifiers / selectors / filters

Queries hold information that modifies or parameterizes the selected resource without creating a child hierarchy.

Examples:

```text
repo://artist/issues/?state=open
repo://artist/issues/?author=adam&limit=20
file:///src/main.rs/symbols/?kind=function
pr://123/diff/4?context=10
file:///src/main.rs?stat
```

A bare key is valid:

```text
?stat
?verbose
```

A key/value item is valid:

```text
?state=open
?limit=20
```

Multiple items use `&`:

```text
?state=open&author=adam&label=bug
```

v1 query grammar:

```text
query := item ("&" item)*
item  := key | key "=" value
key   := ASCII identifier
```

Use URI percent encoding for arbitrary UTF-8 values and delimiter characters.

The kernel must:

- preserve query item order;
- preserve repeated keys;
- not deduplicate;
- not reorder;
- not interpret extension-defined keys or values.

Query semantics belong to the selected resource extension.

## 2.3 Anchor means position inside the fully resolved textual view

The fragment is not a child resource and does not participate in resource-extension routing.

It identifies a line in the textual representation obtained **after** the base path and query have been resolved.

Conceptually:

```text
file:///src/main.rs/symbols/Foo?expanded#BlueHorse
```

means:

1. resolve `file:///src/main.rs/symbols/Foo`;
2. apply `?expanded`;
3. address `#BlueHorse` in that resulting textual representation.

Anchors are scoped to the complete resource view.

An anchor from:

```text
file:///src/main.rs
```

has no implied identity relationship with an identically spelled anchor from:

```text
file:///src/main.rs/symbols
```

or:

```text
file:///src/main.rs?stat
```

The complete base path + query defines the anchored textual space.

Fragments must never be considered by provider claim routing.

The kernel/model adapter should lower a fragment into the relevant existing anchor/position semantics when a verb supports positional addressing.

A fragment supplied to a verb that does not accept an anchored target is `InvalidInput`.

A fragment applied to a non-textual result is `WrongKind` or `InvalidInput`, depending on whether resolution occurred before the mismatch became known.

---

# 3. `resources://` packages

Each resource extension is an editable package under:

```text
resources://<package>/
```

Canonical package layout:

```text
resources://ast/
├── resource.md
├── Cargo.toml
├── Cargo.lock
├── resource.wit
├── src/
└── resource.wasm
```

`Cargo.toml`, `Cargo.lock`, `resource.wit`, source, or precompiled artifact presence follows the same package/build conventions as the current tool component system.

Use the existing component build/cache/dependency/hot-swap machinery.

Do **not** build a second WASM runtime specifically for resources.

Generalize the current package/runtime machinery so `tools://` and `resources://` are two package classes backed by the same component infrastructure.

One package has one active generation.

That is the only exclusive ownership rule at activation time.

There is **no** one-package-per-scheme rule.

---

# 4. `resource.md`

`resource.md` uses YAML frontmatter followed by Markdown prose, analogous to `tool.md`.

Minimum frontmatter:

```yaml
---
name: artist-ast
description: Structured source-code resource projections
version: 0.1.0
contract: artist:resource:extension@1

routes:
  - schemes: [file]

capabilities:
  - resource.read

docs:
  - uri: "file://<path>/symbols/"
    summary: "Lists structured symbols defined by a source file."
    verbs: [read, find]

  - uri: "file://<path>/symbols/<symbol>"
    summary: "Structured information for one symbol."
    verbs: [read]

  - uri: "file://<path>/symbols/<symbol>/callers"
    summary: "Callers of the selected symbol."
    verbs: [read]
    query:
      - name: limit
        summary: "Maximum number of returned callers."
---

Full Markdown documentation follows here.
```

## 4.1 `routes` are only a coarse candidate index

v1 routing frontmatter is intentionally simple.

A route declares one or more URI schemes for which the component may have claims:

```yaml
routes:
  - schemes: [file]
  - schemes: [repo, pr, issue]
```

Do not invent a path-glob routing language in v1.

Do not use registration order as routing priority.

Do not use the `docs` URI patterns for dispatch.

The component's typed `claim` export is the authoritative resolver.

Frontmatter routes are allowed to produce false-positive candidates.

They must never produce false negatives for addresses the component intends to claim.

## 4.2 `docs` are documentation, not routing rules

`docs` describes model/programmer-visible resource forms.

Each entry contains:

```yaml
uri: "<human-readable URI pattern>"
summary: "<short explanation>"
verbs: [<supported universal verbs>]
query:
  - name: <query key>
    summary: <meaning>
```

Optional examples may be added.

The harness must not execute routing based on these strings.

They exist for documentation and model seeding.

## 4.3 Full prose remains inspectable

The Markdown body of `resource.md` is the full resource-extension documentation.

It must remain readable through `resources://`.

Example:

```text
read(resources://ast/resource.md)
```

The automatically seeded model documentation should remain compact; full prose is available on demand.

---

# 5. Shared typed WIT: `artist:resource`

Do not make resource extensions receive generic JSON.

Do not add:

```text
invoke(verb, uri, json)
```

The universal resource contracts remain typed WASM Component interfaces.

Factor the current universal operation types/interfaces into a shared resource package conceptually equivalent to:

```text
artist:resource@1
```

It owns the shared typed definitions for:

```text
Uri
Anchor
Position
AnchoredLine
AnchoredText
AnchoredDiff
Error
ReadRequest / ReadResult
WriteRequest / WriteResult
EditRequest / EditResult
FindRequest
GrepRequest / GrepSource
RunRequest
SendRequest
PollRequest / PollResult
...
```

and the ten universal resource operation interfaces.

The existing model/programmatic tool components become clients of these resource interfaces.

Conceptually:

```text
artist:tool/read component
    exports the model/programmatic read tool
    imports artist:resource/read
```

A resource extension may:

```text
export artist:resource/read
import artist:resource/read
```

This is required for composable projections.

Example:

```text
read(file:///src/main.rs/symbols/Foo)
    -> ast resource component
        -> resource.read(file:///src/main.rs)
        -> parse
        -> return AnchoredText for the symbol resource
```

There must be one set of shared operation types.

Do not copy/paste a second incompatible `AnchoredText`, `Error`, `PollRequest`, etc. into a new WIT package.

---

# 6. Resource-extension control interface

Every resource component exports one mandatory control interface:

```text
artist:resource/extension@1
```

Conceptual WIT:

```wit
enum verb {
    read,
    write,
    edit,
    poll,
    send,
    run,
    abort,
    delete,
    find,
    grep,
}

enum claim {
    pass,
    handle,
    reserve,
}

record claim-request {
    verb: verb,
    uri: uri,
}

interface extension {
    claim: func(request: claim-request) -> claim;
}
```

The exact WIT package syntax may follow existing project version conventions, but these semantics are locked.

The `uri` passed to `claim`:

- is canonical Artist URI syntax;
- includes base path and query;
- never includes a fragment/anchor.

## 6.1 `pass`

`pass` means:

> This component does not claim this `(verb, URI)` and does not prevent another extension from handling it.

Use `pass` for intentional composition.

## 6.2 `handle`

`handle` means:

> This component is the implementation for this exact `(verb, URI)`.

The kernel dispatches the typed verb interface to this component if it is the unique owner assertion.

## 6.3 `reserve`

`reserve` means:

> This URI belongs to a resource subspace recognized by this component, but this verb is not supported here. Do not fall through to another resource extension.

The external operation result is canonical `Unsupported`.

`reserve` exists primarily to protect synthetic resource trees.

Example:

```text
file:///src/main.rs/symbols/Foo
```

may be an AST-owned synthetic resource.

If the caller attempts:

```text
edit(file:///src/main.rs/symbols/Foo)
```

the AST extension may return `reserve`.

Artist must return `Unsupported`.

It must **not** allow the ordinary filesystem provider to interpret `/symbols/Foo` as a real path and create/edit it.

---

# 7. Deterministic routing algorithm

For every resource-targeted operation:

1. Parse and canonicalize the Artist resource address.
2. Remove/lower the fragment before resource routing.
3. Use the URI scheme only to select candidate resource-extension packages from `resource.md.routes`.
4. Invoke each candidate package's active generation `claim(verb, uri)`.
5. Evaluate all owner assertions independent of registration order.

Define:

```text
owner assertion := handle | reserve
```

Resolution:

```text
0 owner assertions
    -> no resource extension claims this address

1 handle
    -> dispatch to that component

1 reserve
    -> Unsupported

>1 owner assertions
    -> Conflict
```

A `handle` plus another component's `reserve` is a conflict.

Two `handle` claims are a conflict.

Two `reserve` claims are a conflict.

Never silently choose the first registered provider.

Never introduce implicit provider priority in v1.

Conflict diagnostics should identify the competing package names and URI.

## 7.1 Unknown scheme vs unclaimed address

If no active resource package advertises the URI scheme at all:

```text
Unsupported
```

If the scheme is known but every candidate returns `pass`:

```text
NotFound
```

This distinguishes unsupported namespace from missing address inside a known resource space.

## 7.2 Claim behavior

`claim` is routing logic, not the resource operation itself.

It should be fast and semantically side-effect-free.

It may inspect package configuration and, when necessary, read stable metadata required to determine whether an address belongs to it.

It must not mutate user resources.

Do not require `claim` to prove that the concrete resource currently exists.

Existence/state validation belongs in the actual verb implementation and may return `NotFound`, `WrongKind`, `Conflict`, etc.

A trap/failure while evaluating a candidate claim is not `pass`.

It is an `Internal`/provider failure for that operation so a broken extension is not silently bypassed.

---

# 8. Composition rules

Multiple packages may advertise and use the same scheme.

This is expected.

Example active packages:

```text
filesystem:
    routes: [file]

ast:
    routes: [file]

repo-core:
    routes: [repo]

github-prs:
    routes: [repo, pr]

bash:
    routes: [bash]
```

For:

```text
read(file:///src/main.rs)
```

possible claims:

```text
filesystem -> handle
ast        -> pass
```

For:

```text
read(file:///src/main.rs/symbols/Foo)
```

possible claims:

```text
filesystem -> pass
ast        -> handle
```

For:

```text
edit(file:///src/main.rs/symbols/Foo)
```

possible claims:

```text
filesystem -> pass
ast        -> reserve
```

No scheme monopoly exists.

The invariant is only:

> At most one extension may assert ownership of one concrete `(verb, URI)` at dispatch time.

If extensions need to cooperate, the handling extension calls other typed resource operations through its imported resource interfaces.

Do not build middleware ordering into the router.

---

# 9. Universal error ergonomics

Tools should not need compatibility matrices or resource-kind branching.

A tool calls the universal operation.

The resource layer returns canonical errors.

Use:

```text
Unsupported
```

The address/subspace is recognized but this verb is not available.

Use:

```text
NotFound
```

No resource exists at the requested address inside a known resource space.

Use:

```text
WrongKind
```

The resource exists but is the wrong semantic shape for the requested operation.

Use:

```text
Conflict
```

The operation conflicts with current resource state, or resource routing has multiple owner assertions.

Use the existing canonical errors for all other cases.

Do not add:

```text
NotLive
AlreadyLive
InvalidState
UnknownResourceKind
```

to the public algebra.

A custom tool should be able to call a typed resource verb and simply propagate/handle the typed `Error`.

---

# 10. Resource-to-resource calls

Resource components may import any universal typed resource verb they need.

Nested resource calls use the current `InvocationContext` unless the component intentionally constructs a narrower derived context.

Do not add working URI, environment, correlation ID, authority, or cancellation fields to ordinary model-facing resource requests.

They remain out-of-band invocation context.

## 10.1 Recursion protection

The kernel must maintain an internal routing stack.

At minimum, detect recursive re-entry of:

```text
(active resource package generation, verb, canonical URI)
```

within one invocation chain.

A component accidentally doing:

```text
read(foo://x)
    -> internally calls read(foo://x)
    -> same component
```

must fail deterministically rather than recurse indefinitely.

Map this to canonical `Internal` with useful diagnostics.

Do not add a public recursion-specific error code.

---

# 11. Batch operations

Existing universal batch semantics remain authoritative.

Resource extension routing is logically per target URI.

The kernel may group requests by resolved provider for efficiency only when doing so preserves contract semantics and input/result ordering.

Requirements:

- result order remains aligned to request order;
- independently fallible batch operations remain independently fallible;
- duplicate-URI validation rules such as `write` happen before any mutation;
- resource-level mutation atomicity is preserved;
- same-URI `send` ordering remains serialized as already specified.

Special multi-resource verbs:

## `find`

Resolve each root independently.

The kernel combines provider results according to existing `find` semantics.

## `grep(Resources)`

Resolve each resource source independently.

`grep(Text)` does not perform resource-provider routing; it operates on the supplied `AnchoredText`.

## `poll`

Resolve each target independently.

Cross-provider poll combination remains a kernel responsibility so target indices and `PollCondition` semantics stay global and stable.

---

# 12. Model documentation and seeding

Resource extensions must be self-documenting without requiring hand-written harness changes.

The active resource catalog is generated from the active generations' valid `resource.md` frontmatter.

Seed a compact summary into the model context.

Conceptual output:

```text
Resources:

file://<path>
  Files/directories.
  read write edit find grep delete

file://<path>/symbols/
  Structured symbols defined by a source file.
  read find

file://<path>/symbols/<symbol>/callers
  Callers of a symbol.
  read
  query: limit

repo://<project>/issues/
  Repository issue listing.
  read find
  query: state author label limit

bash://<name>
  Persistent interactive shell resource.
  read poll send abort delete
```

Rules:

- seed only active valid resource packages;
- use `description` and `docs` frontmatter for the compact catalog;
- do not automatically dump full Markdown prose;
- full prose remains inspectable through `resources://`;
- a malformed candidate edit must not erase the previous active generation's docs;
- one malformed package must not erase unrelated packages from the catalog.

Do not require runtime execution of arbitrary resource code merely to generate prompt documentation.

---

# 13. Hot reload and self-modification

Use the same transactional lifecycle as tool components.

A successful write/edit under:

```text
resources://<package>/...
```

marks that package dirty.

Before a dirty package's new generation participates in routing/catalog generation:

1. discover package;
2. parse `resource.md`;
3. compile/load candidate WASM;
4. validate required resource extension WIT;
5. validate all exported universal resource interfaces;
6. resolve and pin typed dependencies;
7. validate declared capabilities;
8. build candidate route/docs metadata;
9. atomically activate the whole generation.

If any step fails:

```text
previous active generation remains active
```

including previous routing behavior, documentation, and dependency graph.

In-flight invocations pin the generation with which they began.

New invocations use the newly active generation after atomic swap.

Activation is per package, never per scheme.

Different active packages advertising the same scheme are normal.

Runtime claim conflicts are detected at dispatch.

---

# 14. Durable state

Do not make long-lived resource identity depend on opaque persistent WASM instance memory.

Compiled components may be cached.

Component instances may remain invocation-scoped.

Durable state belongs in addressable resources or explicitly granted backing capabilities/services.

Examples:

- filesystem resources -> filesystem primitive capability;
- future `bash://` -> PTY/process/session backing capability;
- remote APIs -> network/auth backing capability;
- extension-private durable data -> explicit storage/KV/resource capability.

This preserves hot-swap safety.

---

# 15. Host capabilities and arbitrary power

Resource source is trusted under the same threat model as tool source.

A resource package may import explicitly granted typed capabilities required to implement its resources.

Examples:

```text
WASI
filesystem primitives
network
clock
PTY/process primitives
KV/storage
other package-local WIT contracts
universal Artist resource verbs
```

Do not expose these primitives as a second model-facing semantic tool API.

They are implementation effects beneath resource components.

Do not create generic JSON host calls.

Use typed WIT imports/exports.

Reuse the existing custom component dependency resolution and generation pinning.

---

# 16. Bootstrap boundary

`resources://` itself cannot require an ordinary resource extension in order to load `resources://`.

Keep a tiny trusted bootstrap implementation beneath the extension system for:

```text
URI parsing/canonicalization
resources:// package-file access
component compile/load/link/cache
resource package discovery
active-generation registry
candidate indexing by scheme
claim dispatch
atomic activation/hot swap
invocation context
capability enforcement
recursion protection
cross-provider batch/poll aggregation
```

Do not move arbitrary domain resource semantics into this kernel layer.

Filesystem projections, repository projections, AST semantics, shells, remote APIs, etc. belong above it as resource extensions once migrated.

---

# 17. Compatibility migration

Do not break all native handlers in one commit.

Introduce a compatibility adapter so current native `TypedHandler` implementations can participate in the new resolver while migrations occur.

However:

- first-match registration order must cease to be the architectural routing rule;
- new resource components must use `claim`;
- compatibility handlers must eventually express equivalent `pass` / `handle` / `reserve` behavior.

The end state is resource components, not accumulating more kernel-native domain handlers.

---

# 18. Required proof extension: AST/file projections

After conformance infrastructure works, prove the architecture with an extension of an existing namespace rather than only a new fake scheme.

Use the existing AST/repository projection behavior as the target.

Create an `ast` resource package capable of claiming synthetic resources under ordinary `file://` resources, such as:

```text
file:///src/main.rs/symbols/
file:///src/main.rs/symbols/Foo
file:///src/main.rs/symbols/Foo/callers
file:///src/main.rs/symbols/Foo/callees
file:///src/main.rs/symbols/Foo/impact
```

and the other currently supported AST projections where practical.

Requirements:

- ordinary `file:///src/main.rs` still resolves to filesystem behavior;
- AST children resolve to the AST resource component;
- AST component obtains underlying source through typed resource calls or an explicitly justified primitive capability;
- anchors returned by projections remain stable according to the existing Artist anchor contract;
- unsupported mutations on AST synthetic resources do not fall through to filesystem creation;
- no `file` scheme monopoly is introduced;
- existing observable projection semantics are preserved.

Do not encode `symbols`, `callers`, etc. as query keys merely because query support now exists.

They are child resources because they have further addressable hierarchy.

Queries are reserved for modifiers such as:

```text
?limit=20
?kind=function
?context=10
```

---

# 19. Query/anchor parser conformance tests

Add parser tests proving:

Valid:

```text
file:///a/b
file:///a/b?stat
file:///a/b?state=open
file:///a/b?state=open&label=bug
file:///a/b?label=a&label=b
file:///a/b?state=open#BlueHorse
file:///a/b/symbols/Foo/callers?limit=20#Quartz17
```

Invalid:

```text
file:///a/b#BlueHorse?state=open
```

Also prove:

- path trailing `/` directory semantics survive queries;
- query order survives parse/serialize;
- repeated keys survive;
- anchors are excluded from resource claim input;
- query participates in the selected resource view;
- anchors are scoped to the path+query view.

---

# 20. Routing conformance tests

Use multiple fake WASM resource components.

At minimum prove:

### Same scheme, different paths

```text
component A advertises fake
component B advertises fake

A handles fake://x/a
B handles fake://x/b
```

Both coexist and route correctly.

### Same scheme, different verbs

A handles:

```text
read(fake://x)
```

B handles:

```text
grep(fake://x)
```

Both coexist.

### Pass

A returns `pass`.

B returns `handle`.

B receives the operation.

### Reserve

A returns `reserve`.

No handler receives the operation.

Result is `Unsupported`.

### Ambiguous handle

A returns `handle`.

B returns `handle`.

Result is `Conflict`.

Nothing executes.

### Handle + reserve ambiguity

A returns `handle`.

B returns `reserve`.

Result is `Conflict`.

Nothing executes.

### Unknown scheme

No package advertises `unknown`.

Result is `Unsupported`.

### Known scheme, no claimant

Packages advertise `fake`, all return `pass`.

Result is `NotFound`.

### Claim failure

A candidate claim traps/fails.

Do not silently continue to B.

Return canonical `Internal`.

### Nested composition

Projection component handles:

```text
read(fake://x/view)
```

and internally calls:

```text
read(fake://x)
```

which resolves to another resource component.

Result succeeds.

### Recursion

A handles `read(fake://x)` and recursively calls the exact same resource operation.

Kernel detects the routing cycle and returns `Internal`.

### Hot swap

Generation N handles a URI.

Edit candidate N+1 to invalid source.

Generation N remains active and documented.

Fix candidate.

Generation N+1 activates atomically.

In-flight N stays on N; new calls use N+1.

### Catalog failure isolation

A malformed resource package does not erase unrelated resource documentation or routing.

---

# 21. Typed component conformance tests

Do not prove the design only with native fake handlers.

At least one end-to-end test must:

1. load a real WASM resource extension from `resources://`;
2. invoke its typed `extension.claim`;
3. route a universal typed verb to it;
4. have it call another Artist resource through a typed resource import;
5. return the normal universal typed result.

No JSON semantic boundary is acceptable in this test.

Provider/model JSON adapters may exist above the typed component boundary as they do for tools.

---

# 22. Documentation tests

Prove:

- active `resource.md` docs appear in the compact resource catalog;
- full prose remains readable at `resources://<package>/resource.md`;
- docs can describe path children and query keys independently;
- a failed candidate edit leaves the old docs active;
- malformed unrelated packages do not erase valid docs;
- inactive/broken first-time packages are not seeded as if active.

---

# 23. Things this slice must NOT invent

Do not add:

```text
one provider per scheme
provider registration priority
middleware ordering
next-provider callbacks
generic JSON resource dispatch
closed ResourceKind enum
a second universal verb surface
a second Error algebra
NotLive
AlreadyLive
InvalidState
query-as-child-path hierarchy
automatic AST ?symbols
Brush
bash:// implementation
terminal emulation
```

Do not migrate shell/process semantics in this slice.

Do not change the locked semantics of the existing universal verbs except where URI query parsing is required.

Do not weaken typed WIT boundaries.

Do not let resource documentation become authoritative routing logic.

---

# 24. Architectural end state

Target structure:

```text
                         model / programs
                               |
                               v
                    typed tool WASM components
                          tools://
                               |
                               v
                  universal typed resource verbs
                               |
                               v
                     tiny resource router
                               |
              candidate index + typed claim()
                               |
          +--------------------+--------------------+
          |                    |                    |
          v                    v                    v
 resources://filesystem   resources://ast    resources://github
          |                    |                    |
          +--------- typed nested resource calls ---+
                               |
                               v
                  explicitly granted primitives
```

Core invariant:

> `tools://` makes Artist's actions editable.  
> `resources://` makes Artist's addressable world editable.

A resource extension is not synonymous with a namespace.

A namespace provider is only one special case of a resource extension.

The routing contract is:

```text
(verb, URI) -> claim arbitration -> one typed resource implementation
```

and composition occurs through typed resource-to-resource calls, not provider ordering.

---

# 25. Implementation completion criteria

This slice is complete only when all of the following are true:

- Artist URIs support canonical `path?query#anchor` ordering.
- Query is extension-defined and non-hierarchical.
- Path remains the only child-resource hierarchy.
- `resources://` packages can be authored, compiled, inspected, edited, hot-swapped, and failure-isolated.
- Multiple resource packages can advertise the same URI scheme.
- Routing uses typed `claim(pass|handle|reserve)`, never first registration order.
- Claim conflicts are deterministic `Conflict`.
- Universal resource operations remain typed WIT.
- Resource components can call other resource components through the same typed interfaces.
- Resource components can import custom typed dependencies/host capabilities.
- Resource docs are discovered from `resource.md` and compactly seeded.
- Full docs remain inspectable.
- Broken edits leave the previous generation and docs active.
- At least one real WASM extension augments an existing namespace.
- AST/file projection behavior proves that `file://` can be extended without owning `file://`.
- All existing universal verb conformance tests still pass.
- No Brush/bash implementation has been started as part of this slice.
