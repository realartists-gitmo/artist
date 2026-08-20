# Artist Resource Model and WASM Runtime Design

## Status

This document defines the foundation that must exist before the later model-facing
operation layer, agent loop, session layer, CLI, or provider integration can be
built.

The scope is deliberately limited to:

- the kernel resource model;
- the four native namespace roots;
- provider ownership and URI routing;
- the WASM component runtime;
- component activation, generations, invocation, and retirement;
- the trusted host environment exposed to components.

The model-facing verb set is **not defined here**. Its eventual shape must not
leak backward into this layer.

---

## 1. Core idea

A resource is a stable, URI-addressed, file-shaped thing whose behavior is
owned by a provider.

"File-shaped" describes the interaction boundary, not the underlying data
model. A resource may be:

- an ordinary operating-system file;
- a directory tree;
- a file that appends while its producer is active;
- a generated or computed file;
- a projection of a remote API;
- a phantom directory whose children are discovered lazily;
- a resource backed by internal kernel state;
- an event or observation view.

The kernel does not decide which of these a resource is. It routes an addressed
request to the provider that owns the resource, and the provider implements the
resource's semantics.

The universal read meaning is current bytes at the time of the read. There is
no universal snapshot model and no universal live-stream model.

A provider may internally use snapshots, caches, streams, versions, remote
cursors, or any other mechanism necessary to implement its resource. Those are
provider implementation details, not universal resource concepts.

---

## 2. Native namespace roots

The kernel publishes exactly four native namespace roots:

```text
resources://
tools://
events://
files://
```

These roots are the kernel's bootstrap surface. They are not a closed list of
all future functionality; they are the initial routing domains through which
the rest of the system becomes addressable.

### 2.1 `files://`

`files://` mirrors configured portions of the host operating-system filesystem.

Properties:

- the OS filesystem is the source of truth;
- reads observe current OS bytes;
- directory lookup and listing reflect current OS state;
- ordinary OS permission and I/O failures remain meaningful structured errors;
- no kernel snapshot or version layer is imposed;
- host filesystem behavior is not reimplemented as a second virtual storage
  system.

The native files provider is a bridge between the kernel resource interface and
the OS. Its internal path and handle management are not part of the
cross-component ABI.

### 2.2 `resources://`

`resources://` is the kernel's namespace for resource state and resource
registrations that do not naturally belong to the host filesystem, tools, or
events namespaces.

It provides the addressable home for kernel-owned coordination and for
provider-owned resources that must be registered into the common resource
space. It does not require all resources to be materialized as bytes in kernel
storage.

A resource under this namespace may still be backed by a provider, a component,
an external authority, or computed behavior.

### 2.3 `tools://`

`tools://` exposes the active WASM tool/component catalog as an addressable
namespace.

It is the resource-visible view of:

- loaded component packages;
- active component generations;
- exported tool/resource metadata;
- component-owned tool surfaces once those are activated.

This document does not define the later model schema or model-facing operation
names. `tools://` only makes the active extension surface addressable to the
runtime and other trusted components.

### 2.4 `events://`

`events://` is the native event broker namespace.

It provides addressable event sources, retained event views where appropriate,
and the internal change-notification path used by resource providers and
components.

Events are not the universal resource data model. They are one native namespace
whose resources may naturally represent append-only observations. Ordinary
resources do not become streams merely because the event broker exists.

---

## 3. Resource identity and addressing

### 3.1 URI is the cross-component identity

The URI is the only resource identity that crosses a component boundary.

Components exchange:

- URIs;
- shared typed values;
- structured errors;
- ordinary request and response data.

They do not exchange live kernel handles, Rust trait objects, Wasmtime resource
handles, or pointers to provider state.

Internal inode numbers, file descriptors, component resource handles, and cache
keys may exist, but they are runtime-local implementation details.

### 3.2 URI handling

The kernel owns URI parsing, normalization, and routing. Providers receive a
validated addressed resource rather than reparsing arbitrary model text.

The routing implementation must preserve the following properties:

- schemes are case-insensitive according to URI rules;
- resource paths are normalized consistently;
- equivalent addresses resolve to one canonical identity;
- malformed addresses fail before provider execution;
- provider-specific path semantics remain provider-owned after routing;
- URI content is opaque to components except where the provider contract
  explicitly defines path components.

The kernel may use a standard URI implementation internally. The exact Rust
representation is not an ABI commitment.

### 3.3 Ownership

A provider owns the authority and semantics of the resources it serves.

"Owns" does not mean that the provider must store the bytes itself:

- the files provider delegates authority to the OS;
- a repository provider delegates authority to a remote service;
- a generated provider may compute content on demand;
- a kernel provider may own in-memory or persistent state;
- a WASM provider may delegate to any trusted host service available to it.

The kernel owns routing and lifecycle coordination, not the meaning or storage
of provider data.

---

## 4. Provider boundary

The provider boundary is an internal implementation boundary and a WASM
contract boundary. It is not a model-facing API.

A provider must be able to implement the file-shaped resource behavior needed by
its namespace, including the conceptual operations:

- identify a resource and its kind;
- resolve a child address;
- enumerate children when the resource is a directory;
- report metadata needed by the VFS;
- read current bytes;
- optionally expose provider-specific mutation or observation behavior;
- report structured failures.

The exact Rust trait and WIT function names should be chosen for idiomatic
async implementation. They must not be forced into the later model verb list.

### 4.1 Provider behavior is allowed to vary

Providers may differ in all of the following:

- whether a resource is a file or directory;
- whether children are static or lazily generated;
- whether bytes are local, remote, or computed;
- whether the resource is readable;
- whether the resource is writable;
- whether a resource can report changes;
- whether a lookup performs external work;
- whether a directory listing is expensive;
- whether a resource exists only temporarily.

The kernel must preserve these differences rather than flattening them into a
universal storage model.

### 4.2 No mandatory kernel cache

The kernel does not maintain a universal metadata, directory, or content cache.

A provider may cache internally when that is correct for its authority. Cache
freshness, invalidation, remote consistency, and revalidation belong to that
provider.

Lazy discovery is valid and expected. A `lookup` or directory enumeration may
invoke a WASM component or external integration at the time it is requested.

### 4.3 Resource errors

The kernel provides a small structured error boundary sufficient for routing and
VFS translation, including concepts such as:

- invalid address;
- not found;
- not a directory;
- is a directory;
- unsupported operation;
- unavailable external authority;
- ordinary I/O failure;
- component failure;
- cancellation or timeout.

Provider-specific detail may be carried as structured context, but the kernel
must not require the model-facing layer to understand provider internals.

---

## 5. Kernel architecture

The kernel consists of five conceptual pieces:

1. **URI and namespace router** — maps canonical addresses to the active
   provider/generation;
2. **native namespace adapters** — bootstrap `resources://`, `tools://`,
   `events://`, and `files://`;
3. **provider registry** — tracks claims, ownership, and active registrations;
4. **WASM runtime manager** — loads and activates component generations;
5. **VFS projection** — exposes the resource tree to FUSE, WinFsp, or another
   host adapter.

The VFS is a projection of resource behavior. It is not the source of truth for
all resources and must not force remote or computed resources into a fake local
storage implementation.

### 5.1 Routing

Routing proceeds conceptually as follows:

```text
URI
  -> canonical parse
  -> native-root or active-provider resolution
  -> generation pin
  -> provider invocation
  -> structured result/error
```

Routing must be deterministic. A resource request must never be cloned and
sent wholesale to unrelated providers merely because one item in a larger
request happens to mention a provider's namespace.

Provider registration and replacement are published atomically. A request sees
one complete routing table and one complete component generation, never a
partially activated state.

### 5.2 Provider claims

A provider registration declares the URI space it serves. Claims must be
validated before publication so that ambiguous or conflicting registrations
cannot become active accidentally.

The initial native roots are reserved by the kernel. WASM components may serve
resources beneath those roots through the component contracts, but they do not
silently replace the root routing domains themselves.

The exact claim representation is an internal design choice, provided it
supports deterministic dispatch and atomic activation.

---

## 6. WASM extension model

Except for the native bootstrap needed by the four namespaces, functionality is
implemented as WASM components.

The runtime supports three extension roles:

- **resource/noun components** — provide addressable file-shaped resources;
- **tool/verb components** — provide later operation implementations;
- **event components** — integrate with the native event namespace and broker.

This document defines the runtime needed to host all three roles but does not
define the later model-facing operation surface.

A component may implement more than one role if its package metadata and
exports satisfy the corresponding contracts. The runtime must classify and
validate exports before activation.

### 6.1 Trusted architecture

Artist is maximally trusted at this layer.

There is no capability negotiation or policy/permission theater between the
kernel and an active extension. All host imports defined by the runtime are
available to components under the trusted host contract.

This does not eliminate ordinary operating-system failures or component
lifecycle failures. It means the runtime does not invent an additional policy
layer that changes the semantics of resource access.

### 6.2 URI/value component boundary

WASM components address resources through URIs and exchange shared typed
values. They do not receive live cross-component resource handles.

A component needing another resource asks the trusted host environment to
resolve or operate on its URI. The host performs routing and invokes the owning
provider. This keeps identity stable across component generations and avoids
leaking Wasmtime instance lifetime into resource addresses.

### 6.3 Universal host environment

The runtime links a universal trusted host environment for every active
component. The host environment is not assembled as a per-component capability
set.

It provides the runtime services required by the system, including the ability
to:

- address and access resources through the kernel;
- use the virtual filesystem projection;
- interact with the native namespaces;
- communicate with the event broker;
- perform the ordinary host services required by resource integrations;
- await asynchronous host work;
- receive cancellation and lifecycle signals.

The host interface should be organized into idiomatic WIT worlds/interfaces,
but all active components receive the same universal trusted host surface.

### 6.4 Virtual filesystem access

The WASM host exposes the kernel's virtual filesystem/resource projection rather
than requiring every component to understand host directory paths.

Consequences:

- a component can navigate resources using the same addressable resource space;
- a remote or computed provider can still present a file-shaped tree;
- the component does not need a live Rust or Wasmtime handle from another
  component;
- the kernel remains the routing authority;
- the VFS projection is not confused with the underlying source of truth.

The host may additionally expose ordinary trusted host services where required
by a native integration. Those services are runtime infrastructure, not a new
resource identity mechanism.

---

## 7. Component package and activation

A component is not active merely because its WASM bytes exist on disk. It is
active only after the runtime has prepared and validated the complete component
instance and its registrations.

### 7.1 Package metadata

Each component package must provide enough metadata to identify:

- package/component name;
- version and ABI compatibility;
- exported role(s);
- resource claims or namespace registrations;
- exported contracts;
- human-readable descriptions where needed by later layers;
- dependencies, if any;
- observer/metadata exports required by its role.

Metadata may be carried by the component, a package manifest, or both. The
runtime must combine and validate it before activation rather than discovering
missing requirements during an active request.

### 7.2 Preparation

Preparation includes:

1. locating and reading the package;
2. validating package metadata;
3. compiling/parsing the component;
4. validating required imports and exports;
5. validating WIT/ABI compatibility;
6. constructing the host bindings;
7. preparing role-specific registrations;
8. preparing the component instance/runtime state;
9. validating provider claims and conflicts;
10. producing an immutable activation generation.

No request should discover that it needs to compile a component, derive its
contract, resolve its registration, or construct required runtime state.

### 7.3 Atomic publication

A prepared generation is published as one transaction.

Publication either:

- makes the complete generation and all of its registrations visible; or
- makes none of them visible.

There is no partially active component, partially published namespace, or
half-updated tool/resource catalog.

---

## 8. Generations and retirement

Every active component registration belongs to an immutable generation.

A generation contains the complete prepared state needed to invoke that version,
including:

- validated component metadata;
- component instance/runtime state;
- provider claims;
- role registrations;
- ABI/type information;
- host integration state;
- generation identity.

### 8.1 Invocation pinning

When a request begins, it pins the generation selected by the current routing
snapshot.

The request remains on that generation until it completes, fails, or is
cancelled. A later activation cannot redirect an in-flight request to a newer
generation.

### 8.2 Replacement

Replacement is a publication of a new generation followed by retirement of the
old generation:

```text
prepare N+1
  -> validate N+1
  -> atomically publish N+1
  -> stop accepting new work on N
  -> allow pinned work on N to finish
  -> retire N
```

New requests resolve against `N+1`. Existing requests finish against `N`.

Retirement must clean up component-owned runtime state, event subscriptions,
and background work after pinned operations are complete. The runtime must not
leave an old generation indefinitely reachable merely because it was replaced.

If a resource's external authority remains independently addressable, the new
generation may serve that authority through its own provider registration. This
does not mean that the old component instance remains active.

### 8.3 Resource behavior during retirement

A retired generation does not accept new requests. Resources whose behavior
exists only inside that generation cease to be available after retirement.

Resources backed by an external authority are not required to disappear from
the overall URI space; a new generation may re-register and serve them. The URI
identifies the resource address, while the active generation determines which
provider currently implements it.

---

## 9. Runtime invocation model

The runtime must provide a structured asynchronous invocation boundary suitable
for both VFS/resource access and later operation components.

Required properties:

- one request is associated with one pinned generation;
- request cancellation reaches the component and host work;
- component traps become structured component failures;
- host I/O failures remain distinguishable from component failures;
- a component cannot corrupt the routing registry by mutating published state;
- provider results preserve their type and structure across the WASM boundary;
- independent component invocations may proceed concurrently;
- component-local mutable state is synchronized according to the runtime's
  instance model rather than exposed as shared unsafe state.

The initial implementation may serialize calls through one component instance
when required by Wasmtime store ownership. The runtime abstraction must not
make that serialization part of the resource semantics; it may later use an
instance pool or another internally safe strategy.

### 9.1 Component state

Provider-owned state may live:

- in the component instance;
- in a trusted host-side provider state object;
- in an external authority such as the OS or a remote service;
- in a combination of these.

The runtime must keep that state associated with the generation that owns it.
It must not place provider semantics into a global kernel cache merely to make
WASM invocation convenient.

### 9.2 Background work

Event components and providers may have long-lived background work. Such work
must be registered with the generation lifecycle so that retirement can:

- signal cancellation;
- stop new work;
- detach subscriptions;
- wait for or safely drain tasks;
- release the component runtime state.

A background task must never keep a retired generation accidentally published
or continue mutating a newer generation's registrations.

---

## 10. Events and resource change observation

The event broker is a native kernel service exposed under `events://` and to
trusted WASM components through the universal host environment.

It exists to support resource integrations and change observation. It does not
turn all resources into streams.

A provider may:

- publish that a resource changed;
- expose an append-only event view;
- use an external subscription;
- report that it cannot observe changes;
- implement its own provider-specific observation mechanism.

The kernel event layer owns subscription lifecycle, routing, and generation
cleanup. The provider owns the meaning of its events and the source of truth for
its resource bytes.

Resource reads remain reads of current bytes. Event notifications are separate
coordination signals and must not be mistaken for universal content versions.

---

## 11. What this design intentionally does not define

The following are explicitly deferred:

- the model-facing verb names and schemas;
- how sibling model calls are batched;
- edit/insert/delete/move semantics;
- agent prompting and provider protocol;
- session persistence;
- CLI/TUI behavior;
- model observation formatting;
- AST exposure as tools or resources;
- user approval flows;
- LLM authentication;
- product-level workflows.

Those layers will consume this resource/runtime foundation. They must not be
used to distort it prematurely.

---

## 12. Implementation completion criteria

The resource/runtime foundation is complete when the repository can demonstrate
all of the following without requiring the later agent or CLI layers:

1. The kernel publishes the four native namespace roots.
2. `files://` reads current bytes from a configured OS filesystem mirror.
3. A provider can expose a virtual or remote file-shaped tree with lazy lookup.
4. A provider can expose a computed phantom resource without materializing a
   fake local file.
5. The kernel routes canonical URIs deterministically.
6. WASM components can register resource/tool/event roles through validated
   contracts.
7. Components receive the universal trusted host environment.
8. Components exchange resource identity only through URIs and typed values.
9. Activation is fully prepared and atomically published.
10. In-flight calls remain pinned to their starting generation.
11. Replacement retires old generations after pinned work completes.
12. Event subscriptions and background work are cleaned up on retirement.
13. Native VFS adapters project resource behavior without becoming the source of
    truth for non-filesystem resources.
14. A failing or unavailable provider produces structured errors without
    corrupting unrelated namespaces or generations.

Once these criteria hold, the later resource operations and agent-facing layers
can be built on a stable semantic core rather than defining the core
accidentally.
